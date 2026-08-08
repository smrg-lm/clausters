//! The shared pointer-gesture state machine — one press → drag → release
//! interpreter over the widget tree for **both fronts**.
//!
//! The machine owns the in-progress [`Drag`] and turns pointer/wheel/keyboard
//! input into tree mutations (through the [`interact`] doors and the widget
//! model modules) plus a list of [`GestureEffect`]s for whatever only the front
//! can do: emitting `/gui_event` over its transport, requesting repaints,
//! releasing a pointer grab. The front supplies the per-call [`GestureCtx`]
//! (framebuffer size, modifier keys, the heavy views' lane counts — the one
//! datum that lives in front-side GPU slots) and, at press time, a pointer-grab
//! callback (native pointer lock; a front without one returns `false`).
//!
//! The module is platform-agnostic (no winit, no web-sys): the native windowed
//! front ([`super::gui`]) and the browser front (`super::web`) both drive it,
//! so a selection, a clip drag or a BPF edit behaves identically on either
//! platform by construction.
//!
//! **Module layout.** This file is the machine proper: the [`Drag`] enum (every
//! gesture in flight), the [`Gestures`] state and the pointer entry points the
//! fronts call — `press`, `drag_to`, `release`, `wheel`, `tick`. The three
//! things it leans on are its own children, so the machine reads as press →
//! drag → release without the plumbing in between: [`effects`] is what a
//! gesture *delivers* (the one place the bound-vs-event decision is made),
//! [`nav`] is what it *reads and moves* (hit-testing, a scroll plane, a
//! timeline group's pan/zoom/selection), and [`keys`] is the keyboard half —
//! text editing and the block operations, which share nothing with the pointer
//! but the `Gestures` state they hang off.

use std::collections::HashMap;

use clausters_core::osc::OscType;

use super::interact::{self, Hit, slider_t};
use super::layout::Rect;
use super::signal::Presentation;
use super::widget::{Axis, GestureStep, WidgetKind};
use super::{Host, bpf, controls, patch, piano, pianoroll, scroll};
use crate::viewport::View;

/// What a gesture asks of the front: everything the machine cannot do itself
/// because it owns no transport, no window and no GPU.
#[derive(Debug, PartialEq)]
pub enum GestureEffect {
    /// Emit `/gui_event widget_id <args…>` to the script behind window `def_id`.
    Emit {
        def_id: i32,
        widget_id: i32,
        args: Vec<OscType>,
    },
    /// Repaint the window rooted at this def id (a gesture on one window may
    /// touch linked views in others, so this is not always the pressed window).
    Redraw(i32),
    /// Release the pointer grab a knob/number drag took on window `def_id`.
    ReleasePointer(i32),
}

/// The per-call context the front supplies: which window, its framebuffer size
/// in device pixels, the modifier keys, and the heavy views' lane counts (the
/// channel/lane split lives in the front's GPU slots, so the front snapshots it
/// here; a widget missing from the maps counts as one lane).
pub struct GestureCtx {
    pub def_id: i32,
    pub fb_w: u32,
    pub fb_h: u32,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// Channel count per `waveform` widget id (stacked lanes; `overlay` still
    /// draws one lane and is resolved here, not in the map).
    pub wave_lanes: HashMap<i32, usize>,
    /// Lane (channel STFT) count per `spectrogram` widget id.
    pub spect_lanes: HashMap<i32, usize>,
}

impl GestureCtx {
    /// A bare context (no modifiers, no lane info) — enough for the control
    /// widgets and for tests.
    pub fn new(def_id: i32, fb_w: u32, fb_h: u32) -> Self {
        Self {
            def_id,
            fb_w,
            fb_h,
            shift: false,
            ctrl: false,
            alt: false,
            wave_lanes: HashMap::new(),
            spect_lanes: HashMap::new(),
        }
    }

    /// The lane count a timeline view stacks on screen (overlaid waveform
    /// traces share one lane) — the divisor for lane-relative y gestures.
    fn lanes(&self, id: i32, kind: &WidgetKind) -> usize {
        match kind.signal() {
            // Overlaid traces share one lane, however many channels there are.
            Some(el) if el.display.overlay => 1,
            Some(el) if el.presentation == Presentation::TimeFrequency => {
                self.spect_lanes.get(&id).copied().unwrap_or(1).max(1)
            }
            Some(_) => self.wave_lanes.get(&id).copied().unwrap_or(1).max(1),
            None => 1,
        }
    }
}

/// An in-progress pointer drag, by what it is driving.
#[derive(Clone)]
enum Drag {
    /// A slider: the value follows the cursor within `body` — along x, or along y
    /// when `vertical`.
    Slider { id: i32, body: Rect, vertical: bool },
    /// A knob or number: the value moves incrementally with the vertical drag.
    /// On press the front is asked to grab the pointer; `locked` records which
    /// grab won: when `true` the pointer is locked and motion arrives as
    /// relative deltas ([`Gestures::relative_motion`]); when `false` (confined
    /// or ungrabbed) cursor motion still drives it, and `last_y` re-anchors on
    /// every step so a value pinned at an end has no dead zone — reversing
    /// direction moves it at once instead of sticking and jumping.
    Vertical {
        id: i32,
        last_y: f64,
        body_h: f32,
        locked: bool,
    },
    /// A momentary button held down (emits 0 on release).
    Button { id: i32 },
    /// Panning a timeline view's (waveform/spectrogram) window from a snapshot
    /// (Shift+drag).
    Pan {
        id: i32,
        origin_x: f64,
        start: f64,
        body_w: f64,
    },
    /// Sweeping a selection on a timeline container: `anchor` is the sample
    /// under the press, and the selection spans from it to the cursor's sample.
    /// On an axis that measures a **value** as well (a piano-roll's pitch,
    /// `value` carrying its window and the value under the press) the sweep is
    /// a rectangle: the time span still drives the shared selection every
    /// linked view follows, and the elements inside the rectangle become the
    /// container's own selection.
    Select {
        id: i32,
        body: Rect,
        nav_start: f64,
        nav_len: f64,
        anchor: f64,
        value: Option<(f64, f64, f64)>,
    },
    /// Panning a timeline view's **vertical** display window from a drag on
    /// its y-ruler strip: `y_start` is the window snapshot at the press,
    /// `lane_h` the lane height in device pixels (absolute panning, so a
    /// clamped edge never drifts).
    PanY {
        id: i32,
        origin_y: f64,
        y_start: f64,
        lane_h: f64,
    },
    /// Dragging a `bpf` breakpoint: the point follows the cursor within
    /// `body`, times clamped monotonic between its neighbors.
    BpfPoint { id: i32, index: usize, body: Rect },
    /// Dragging a `bpf` segment vertically: its curvature follows the cursor
    /// (`last_y` re-anchors each step, incremental like a knob drag).
    BpfCurve {
        id: i32,
        segment: usize,
        last_y: f64,
        body_h: f64,
    },
    /// Dragging a multitrack `clip`: the body moves its `offset`, an edge
    /// resizes its `dur`. The cursor maps to a timeline sample through the
    /// lane's `body_x`/`body_w` and the shared `nav_start`/`nav_len`; the
    /// placement follows from a press-time snapshot (`press_sample`,
    /// `orig_offset`, `orig_dur`) so a clamped edge never drifts, snapped to
    /// `grid`.
    Clip {
        id: i32,
        /// The lane the clip sits on — the navigation-group member, which the
        /// clip itself is not; the cursor mapping and the edge scroll reach the
        /// shared axis through it.
        lane: i32,
        part: interact::ClipPart,
        body_x: f64,
        body_w: f64,
        nav_start: f64,
        nav_len: f64,
        press_sample: f64,
        orig_offset: f64,
        orig_dur: f64,
        grid: f64,
    },
    /// A cord being pulled from a patch's port: the widget, the grabbed
    /// port `(box, side, index)` and the widget's area — released over a
    /// compatible port (an outlet↔inlet of matching rate) to draw a cord, over
    /// anything else to cancel. `scale` is the workspace zoom the patch is seen
    /// through, so the pin geometry matches the drawing.
    Wire {
        id: i32,
        port: (usize, super::patch::Side, usize),
        area: Rect,
        scale: f32,
    },
    /// A `patch` box (or the whole selection) being moved on the patch
    /// canvas: the grabbed boxes with their positions at press time (canvas
    /// units), moved together by the cursor delta and emitted as one
    /// `"move"` event per box on release.
    Box {
        id: i32,
        scale: f32,
        origin: (f64, f64),
        grabbed: Vec<(usize, f32, f32)>,
        moved: bool,
    },
    /// The selection marquee on a patch's empty canvas: the selected
    /// set follows the rectangle live; the rectangle itself draws through
    /// [`Gestures::marquee`].
    Marquee {
        id: i32,
        area: Rect,
        scale: f32,
        origin: (f64, f64),
        cursor: (f64, f64),
    },
    /// A break-point of an **automation clip** being dragged in place: the clip
    /// and the point, plus the geometry mapping the cursor back onto the shared
    /// axis and the clip's value range.
    /// Dragging a lane header's level fader: the cursor's x over the fader's
    /// rectangle is the value, so the press itself already sets it.
    LaneLevel { id: i32, rect: Rect },
    ClipPoint {
        id: i32,
        index: usize,
        /// The clip's own axis: the rectangle it was drawn in and the window of
        /// its `[0, dur]` span that rectangle shows. The lane's gutter and the
        /// group's window play no part in a curve edit.
        rect: Rect,
        nav_start: f64,
        nav_len: f64,
    },
    /// Dragging a piano-roll note: the body moves it in time and pitch, an edge
    /// resizes its duration. The cursor maps to a region-relative time through
    /// the grid and the shared `nav`, and to a pitch through the visible window
    /// `[lo, hi]`; a press-time snapshot (`press_time`, `orig_*`) keeps a clamped
    /// edge from drifting, snapped to `grid`.
    Note {
        id: i32,
        index: usize,
        part: pianoroll::NotePart,
        grid: Rect,
        nav_start: f64,
        nav_len: f64,
        lo: f32,
        hi: f32,
        press_time: f64,
        orig_start: f64,
        orig_dur: f64,
        snap: f64,
    },
    /// Dragging a note's velocity bar in the velocity lane: the velocity follows
    /// the cursor's height within `lane`.
    Velocity { id: i32, index: usize, lane: Rect },
    /// Dragging an OSC-event marker along the time axis (its `time` follows the
    /// cursor through the grid's shared `nav`, snapped to `grid`).
    OscMark {
        id: i32,
        index: usize,
        grid: Rect,
        nav_start: f64,
        nav_len: f64,
        snap: f64,
    },
    /// Dragging a **selected** note moves the whole selection rigidly in time
    /// and pitch. `orig` is the `(index, start, pitch)` snapshot at press time
    /// — the grabbed note's entry leads it (the snap anchor) — so a clamped
    /// block never drifts.
    NoteBlock {
        id: i32,
        grid: Rect,
        nav_start: f64,
        nav_len: f64,
        lo: f32,
        hi: f32,
        press_time: f64,
        press_pitch: f32,
        snap: f64,
        orig: Vec<(usize, f64, f32)>,
    },
    /// Dragging the velocity lane over a **selected** note nudges every selected
    /// velocity by the same delta (relative, from the `(index, velocity)` press
    /// snapshot — each note saturates on its own).
    VelocityBlock {
        id: i32,
        lane: Rect,
        /// The lane velocity under the press (the delta's zero).
        press_velocity: i32,
        orig: Vec<(usize, i32)>,
    },
    /// A held `piano` key: crossing into another key glissandos (note-off of
    /// the old, note-on of the new); release sends the note-off. The layout is
    /// snapshotted at press time (the range cannot change under a key drag).
    PianoKey {
        id: i32,
        layout: piano::Layout,
        pitch: i32,
        /// The widget's fixed velocity, or `None` for the press-height map.
        fixed_vel: Option<i32>,
        channel: i32,
    },
    /// Dragging the `piano`'s overview strip pans the visible range relative to
    /// the press snapshot (`min0`/`max0`, the key under the press as `anchor`).
    PianoView {
        id: i32,
        strip: Rect,
        min0: i32,
        max0: i32,
        anchor: i32,
    },
    /// Panning a `scroll` workspace from a press on its empty area: the view
    /// follows the cursor absolutely from the press snapshot (`x0`/`y0`), so a
    /// clamped edge never drifts. `area` is the container's laid-out rect at
    /// press time (the clamp geometry).
    ScrollPan {
        id: i32,
        area: Rect,
        origin_x: f64,
        origin_y: f64,
        x0: f64,
        y0: f64,
    },
    /// Dragging an engraved element up or down a `score`: the vertical
    /// displacement quantizes to whole **diatonic steps** (the page's `step`),
    /// which the widget draws as it moves and the release emits as a
    /// `"transpose"` edit-back. `rect` is the page's laid-out area at press time
    /// (the page→screen fit, which cannot change under a drag) and `origin_y`
    /// the press, so the count is absolute from the snapshot rather than
    /// accumulated.
    ScoreStep {
        id: i32,
        element: String,
        rect: Rect,
        origin_y: f64,
        steps: i32,
    },
    /// Selecting text in an editable `text` field: `anchor` is the caret byte
    /// offset the press landed on; dragging extends the selection from it to the
    /// caret under the cursor. `rect`/`scale` reconstruct the field's layout so
    /// the cursor maps to a caret exactly as the renderer drew it.
    TextSelect {
        id: i32,
        rect: Rect,
        scale: f32,
        anchor: usize,
    },
}

/// A platform-neutral key for editing a focused `text` field — the fronts
/// (winit native, winit-on-wasm web) translate their key events into this so
/// the editing behavior lives once in the machine. Modifier state rides in the
/// [`GestureCtx`] (`shift` extends a selection, `ctrl` word-jumps and drives
/// cut/copy/paste/select-all on the letter keys).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextKey {
    /// A printable character to insert (already resolved from the layout).
    Char(char),
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    /// Enter: a newline in a multiline field, ignored in a single-line one.
    Enter,
}

/// One window's gesture state: the in-progress drag, if any. The front holds
/// one per window (the browser's single canvas holds one).
#[derive(Default)]
pub struct Gestures {
    drag: Option<Drag>,
    /// The `menu` whose option list is **open**, and where that list was
    /// placed. A popup is the one thing on screen that is not a placement, so
    /// it lives here with the rest of the interaction state and reaches the
    /// renderer through [`FrameInputs`](super::frame::FrameInputs) — the same
    /// road the marquee takes.
    menu: Option<MenuOpen>,
}

/// An open `menu`'s list: which widget opened it and the rectangle it occupies
/// (device pixels), resolved once at the press so the drawing and the click
/// cannot disagree about where the rows are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MenuOpen {
    pub id: i32,
    pub popup: Rect,
}

impl Gestures {
    /// Whether a drag is in progress (the front routes cursor motion here).
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The open `menu`'s list, if one is open — what the renderer draws over
    /// the window and what the next press is tested against first.
    pub fn menu_open(&self) -> Option<MenuOpen> {
        self.menu
    }

    /// Closes an open list, if any; `true` when there was one (the caller
    /// repaints). A def that replaces the tree, a window that closes and a
    /// press outside all end the same way.
    pub fn close_menu(&mut self) -> bool {
        self.menu.take().is_some()
    }

    /// The held momentary button's widget id, if the active drag is one (the
    /// renderer draws it pressed).
    pub fn active_button(&self) -> Option<i32> {
        match &self.drag {
            Some(Drag::Button { id }) => Some(*id),
            _ => None,
        }
    }

    /// The selection marquee in flight, if any: the `patch` widget and the
    /// rectangle between the press and the cursor (device pixels), for the
    /// renderer to draw over the patch.
    pub fn marquee(&self) -> Option<(i32, Rect)> {
        match &self.drag {
            Some(Drag::Marquee {
                id, origin, cursor, ..
            }) => Some((*id, corner_rect(*origin, *cursor))),
            _ => None,
        }
    }

    /// The cord drag in flight, if any: the `patch` widget and the grabbed port
    /// `(box, side, index)` (the renderer draws the cord to the pointer).
    pub fn wiring(&self) -> Option<(i32, (usize, super::patch::Side, usize))> {
        match &self.drag {
            Some(Drag::Wire { id, port, .. }) => Some((*id, *port)),
            _ => None,
        }
    }

    /// Whether the active drag is a *locked* knob/number drag — driven by
    /// relative deltas ([`Self::relative_motion`]), not by cursor positions.
    pub fn locked(&self) -> bool {
        matches!(self.drag, Some(Drag::Vertical { locked: true, .. }))
    }

    /// Whether a clip drag is currently held against a lane's edge, so the
    /// front must keep ticking ([`Self::tick`]) even though the pointer is
    /// standing still — a held cursor produces no events, and the view has to
    /// keep moving under it.
    pub fn edge_scrolling(&self, cx: f64) -> bool {
        self.edge_direction(cx) != 0.0
    }

    /// Which way a clip drag at `cx` pulls the view: `-1` past the left edge,
    /// `+1` past the right, `0` when the cursor is clear of both margins (or no
    /// clip drag is in flight). The margin reaches *outside* the body too, so a
    /// cursor pinned at the window's own edge keeps scrolling.
    fn edge_direction(&self, cx: f64) -> f64 {
        let Some(Drag::Clip { body_x, body_w, .. }) = self.drag else {
            return 0.0;
        };
        if body_w <= 2.0 * EDGE_MARGIN {
            return 0.0;
        }
        if cx < body_x + EDGE_MARGIN {
            -1.0
        } else if cx > body_x + body_w - EDGE_MARGIN {
            1.0
        } else {
            0.0
        }
    }

    /// Advances an edge-held clip drag by `dt` seconds: pans the group's window
    /// in the held direction and re-applies the drag at the standing cursor, so
    /// the clip travels with the view.
    ///
    /// This is what lets a clip be moved further than one window's worth. The
    /// drag itself maps the cursor through the *current* window, so panning is
    /// the whole mechanism — nothing here touches the placement math.
    pub fn tick(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        cx: f64,
        dt: f64,
    ) -> Vec<GestureEffect> {
        let mut out = Vec::new();
        let dir = self.edge_direction(cx);
        let Some(Drag::Clip {
            id,
            lane,
            part,
            body_x,
            body_w,
            nav_start,
            nav_len,
            press_sample,
            orig_offset,
            orig_dur,
            grid,
        }) = self.drag
        else {
            return out;
        };
        if dir == 0.0 || dt <= 0.0 {
            return out;
        }
        let Some((start, len, _)) = group_view(host, lane) else {
            return out;
        };
        // Pan first, then re-apply the drag against the window it left behind.
        // `pan_timeline` clamps to the group's span (the multitrack headroom),
        // and the span itself grows as the dragged clip extends the content —
        // so the view keeps making room instead of stopping at today's end.
        let step = dir * len * EDGE_SCROLL_PER_SEC * dt;
        let roots = host.pan_timeline(lane, start + step);
        for root in roots {
            out.push(GestureEffect::Redraw(root));
        }
        apply_clip_drag(
            host,
            &mut out,
            ctx.def_id,
            ClipDrag {
                id,
                lane,
                part,
                body_x,
                body_w,
                nav_start,
                nav_len,
                press_sample,
                orig_offset,
                orig_dur,
                grid,
            },
            cx,
        );
        emit_view(host, &mut out, ctx.def_id, lane);
        out
    }

    /// Press: run the **containers' gesture plans** over the hit, innermost
    /// first, until one of their steps consumes it.
    ///
    /// The order is the containers', not the widget's. Each container over the
    /// point declares what a modifier does on it ([`super::widget::GestureMap`]) — pan its axis,
    /// sweep a selection, locate the transport, or hand the press to the
    /// element under the cursor — and a step that declines passes the press on,
    /// outward through the chain. That is why Shift+drag pans the same way over
    /// a waveform, a lane and a piano-roll (their axis claims it before any of
    /// them sees it), and why Shift on a patcher's empty canvas still pans the
    /// workspace *around* the patcher: the canvas declines and the plane
    /// outside it takes over.
    ///
    /// `grab` is the front's pointer-grab attempt for a knob/number drag
    /// (returns whether the pointer was *locked*); a front without pointer lock
    /// returns `false`.
    pub fn press(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        cx: f64,
        cy: f64,
        grab: &mut dyn FnMut() -> bool,
    ) -> Vec<GestureEffect> {
        let mut out = Vec::new();
        // An **open list is modal**: it is over everything, so it is tested
        // before the tree, and it swallows the press either way — on a row it
        // picks that option, anywhere else it just closes, the way a menu
        // everywhere else behaves.
        if let Some(open) = self.menu.take() {
            out.push(GestureEffect::Redraw(ctx.def_id));
            if let Some(row) = self.menu_row(host, ctx, open, cx, cy) {
                interact::set_menu_index(host, ctx.def_id, open.id, row);
                emit_value(host, &mut out, ctx.def_id, open.id);
            }
            return out;
        }
        let Some(hit) = hit(host, ctx, cx, cy) else {
            // A press on empty space drops the text focus (the caret disappears).
            if let Some(old) = host.clear_text_focus() {
                out.push(GestureEffect::Redraw(old));
            }
            self.pan_sole_axis(host, ctx, cx);
            return out;
        };
        // A press on anything other than the focused text field defocuses it.
        if !matches!(hit.kind, WidgetKind::Text { .. })
            && let Some(old) = host.clear_text_focus()
        {
            out.push(GestureEffect::Redraw(old));
        }
        // The vertical axis is grabbed on its own strip, before any modifier: a
        // press on a y-ruler or a piano-roll's keyboard gutter means *that*
        // axis, whatever the container maps the drag to elsewhere.
        if let Some((id, axis)) = interact::time_of(&hit.chain)
            && let Some(y) = axis.y
            && y.strip.contains(cx, cy)
        {
            self.drag = Some(Drag::PanY {
                id,
                origin_y: cy,
                y_start: y.start,
                lane_h: y.lane_h,
            });
            return out;
        }
        let mut element_ran = false;
        for frame in hit.chain.iter().rev() {
            for step in frame.map.plan(ctx.shift, ctx.ctrl, ctx.alt).steps() {
                let consumed = match step {
                    // The element gets exactly one turn, wherever the first
                    // container that offers it sits.
                    GestureStep::Element if !element_ran => {
                        element_ran = true;
                        self.element_press(host, ctx, &hit, cx, cy, grab, &mut out)
                    }
                    GestureStep::Element => false,
                    action => self.container_press(host, ctx, frame, action, cx, cy, &mut out),
                };
                if consumed {
                    return out;
                }
            }
        }
        // Nobody took it.
        self.pan_sole_axis(host, ctx, cx);
        out
    }

    /// Shift+drag means "pan the axis" wherever it starts, so in a window with
    /// **one** navigation group it means that off the lanes too — the gap
    /// between them, the slack under the last one, a container's margin, the
    /// window's own edge. Returns whether it grabbed.
    fn pan_sole_axis(&mut self, host: &Host, ctx: &GestureCtx, cx: f64) -> bool {
        if !ctx.shift {
            return false;
        }
        let Some(sole) =
            interact::sole_time_axis(host, ctx.def_id, ctx.fb_w, ctx.fb_h, &|id, kind| {
                ctx.lanes(id, kind)
            })
        else {
            return false;
        };
        self.drag = Some(Drag::Pan {
            id: sole.id,
            origin_x: cx,
            start: sole.axis.nav.start,
            body_w: sole.axis.body.w.max(1.0) as f64,
        });
        true
    }

    /// The option row an open list has under `(cx, cy)` — `None` when the press
    /// landed outside the list (which closes it and picks nothing). The option
    /// count comes from the tree, so a list left open over a widget that is
    /// gone resolves to nothing rather than to a stale row.
    fn menu_row(
        &self,
        host: &Host,
        ctx: &GestureCtx,
        open: MenuOpen,
        cx: f64,
        cy: f64,
    ) -> Option<usize> {
        let options = host
            .window_def(ctx.def_id)
            .and_then(|tree| tree.find(open.id))
            .and_then(|w| match &w.kind {
                WidgetKind::Menu { options, .. } => Some(options.len()),
                _ => None,
            })?;
        controls::menu_row_at(open.popup, options, cx, cy)
    }

    /// One container-level step of a press: the gestures that belong to the
    /// coordinate system rather than to what is drawn in it. Each reads the
    /// frame the chain resolved — the axis' own body, window and view state —
    /// so a pan is one implementation for the five timeline views and a plane
    /// pan is one for every workspace. Returns whether the step consumed the
    /// press; a step that has nothing to act on (a locate outside the axis'
    /// body, a selection on a canvas with no marquee) declines, and the plan
    /// goes on.
    #[allow(clippy::too_many_arguments)] // one press: a container, a step, a cursor
    fn container_press(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        frame: &interact::Frame,
        step: GestureStep,
        cx: f64,
        cy: f64,
        out: &mut Vec<GestureEffect>,
    ) -> bool {
        let def_id = ctx.def_id;
        let Some(id) = frame.id else {
            return false; // an unaddressable container navigates nothing
        };
        match (step, frame.coords) {
            (GestureStep::Pan, interact::Coords::Time(axis)) => {
                self.drag = Some(Drag::Pan {
                    id,
                    origin_x: cx,
                    start: axis.nav.start,
                    body_w: axis.body.w.max(1.0) as f64,
                });
                true
            }
            (GestureStep::Pan, interact::Coords::Plane(view)) => {
                // A plane with nowhere to go **declines**, the way its wheel
                // does: the slack under a short stack is not a surface with a
                // gesture of its own, and eating the press there is what left
                // Shift+drag dead everywhere except over a lane.
                if !interact::plane_can_pan(host, def_id, id, frame.rect, view) {
                    return false;
                }
                self.drag = Some(Drag::ScrollPan {
                    id,
                    area: frame.rect,
                    origin_x: cx,
                    origin_y: cy,
                    x0: view.view_x,
                    y0: view.view_y,
                });
                true
            }
            (GestureStep::Select, interact::Coords::Time(axis)) => {
                if !axis.spans(cx) {
                    return false;
                }
                // The press collapses the shared selection to the sample under
                // it; the drag sweeps from there. On an axis that measures a
                // value too (a roll's pitch), the sweep is a rectangle and the
                // container's own elements inside it become its selection --
                // but only when the press is on the body, since the strips
                // under it (a velocity lane, a ruler) read the time axis alone.
                let anchor = interact::sample_at(
                    axis.nav.start,
                    axis.nav.len,
                    axis.body.x as f64,
                    axis.body.w as f64,
                    cx,
                );
                let window = axis
                    .y
                    .filter(|_| axis.body.contains(cx, cy))
                    .and_then(|y| y.window);
                if window.is_some() {
                    // A fresh sweep drops the set the previous one left.
                    interact::clear_element_selection(host, def_id, id);
                }
                let value =
                    window.map(|(lo, hi)| (lo, hi, interact::value_at(axis.body, lo, hi, cy)));
                set_selection(host, out, def_id, id, anchor, anchor);
                self.drag = Some(Drag::Select {
                    id,
                    body: axis.body,
                    nav_start: axis.nav.start,
                    nav_len: axis.nav.len,
                    anchor,
                    value,
                });
                out.push(GestureEffect::Redraw(def_id));
                true
            }
            (GestureStep::Select, interact::Coords::Canvas) => {
                interact::graph_select(host, def_id, id, Vec::new());
                self.drag = Some(Drag::Marquee {
                    id,
                    area: frame.rect,
                    scale: frame.scale,
                    origin: (cx, cy),
                    cursor: (cx, cy),
                });
                out.push(GestureEffect::Redraw(def_id));
                true
            }
            (GestureStep::Locate, interact::Coords::Time(axis)) => {
                if !axis.spans(cx) {
                    return false; // beside the axis (a lane's header): no position
                }
                locate_timeline(host, out, def_id, id, axis.body, cx);
                true
            }
            _ => false,
        }
    }

    /// The press the containers handed down: what the widget under the cursor
    /// does with it — a control's value, a note, a break-point, a clip, a piano
    /// key, a cord. Returns whether it was consumed; declining (empty space in
    /// a lane, a patch's bare canvas) hands the press back to the chain.
    #[allow(clippy::too_many_arguments)] // one press: a hit, the context, a cursor
    fn element_press(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        hit: &Hit,
        cx: f64,
        cy: f64,
        grab: &mut dyn FnMut() -> bool,
        out: &mut Vec<GestureEffect>,
    ) -> bool {
        let Hit {
            id, rect, scale, ..
        } = *hit;
        let (chain, kind) = (&hit.chain, hit.kind.clone());
        let def_id = ctx.def_id;
        let effects_before = out.len();
        match kind {
            WidgetKind::Slider { range: r, vertical } => {
                // The track area, not the whole body: the grab has to agree with
                // the groove the renderer drew (`controls::slider_track`) — at
                // the placement's own table, which is what the renderer used.
                let body = controls::slider_track(
                    rect,
                    r.label.is_some(),
                    r.text_size * scale,
                    &host.metrics_for(def_id).at(scale),
                );
                let t = slider_t(body, cx, cy, vertical);
                interact::set_fraction(host, def_id, id, t);
                emit_value(host, out, def_id, id);
                self.drag = Some(Drag::Slider { id, body, vertical });
                out.push(GestureEffect::Redraw(def_id));
            }
            WidgetKind::Knob(r) | WidgetKind::Number(r) => {
                let body = controls::body_rect_at(
                    rect,
                    r.label.is_some(),
                    r.text_size * scale,
                    &host.metrics_for(def_id).at(scale),
                );
                let locked = grab();
                self.drag = Some(Drag::Vertical {
                    id,
                    last_y: cy,
                    body_h: body.h,
                    locked,
                });
            }
            WidgetKind::Button { .. } => {
                deliver(host, out, def_id, id, OscType::Int(1));
                self.drag = Some(Drag::Button { id });
                out.push(GestureEffect::Redraw(def_id));
            }
            WidgetKind::Toggle { .. } => {
                interact::flip_toggle(host, def_id, id);
                emit_value(host, out, def_id, id);
                out.push(GestureEffect::Redraw(def_id));
            }
            WidgetKind::Menu {
                ref options,
                ref label,
                text_size,
                ..
            } => {
                // The list hangs off the menu's **body** (the field the chosen
                // option is drawn in), not off the whole cell, so it lines up
                // with what it is replacing rather than with the label over it.
                let m = host.metrics_for(def_id).at(scale);
                let size = text_size * scale;
                let body = controls::body_rect_at(rect, label.is_some(), size, &m);
                self.menu = Some(MenuOpen {
                    id,
                    popup: controls::menu_popup(body, options.len(), size, ctx.fb_h as f32, &m),
                });
                out.push(GestureEffect::Redraw(def_id));
            }
            WidgetKind::Text { .. } => {
                // Focus the field and drop the caret where the press landed; a
                // drag from here extends a selection.
                host.focus_text(def_id, id);
                let pos =
                    interact::text_caret_at(host, def_id, id, rect, scale, cx, cy).unwrap_or(0);
                interact::text_edit(host, def_id, id, |value, caret, _| {
                    super::textedit::clamp(value, caret); // guard a stale caret
                    caret.pos = pos;
                    caret.anchor = None;
                });
                self.drag = Some(Drag::TextSelect {
                    id,
                    rect,
                    scale,
                    anchor: pos,
                });
                out.push(GestureEffect::Redraw(def_id));
            }
            WidgetKind::Bpf {
                ref points,
                min,
                max,
                duration,
                exp,
                ref label,
                ..
            } => {
                let body = bpf::body(rect, label.is_some(), host.metrics_for(def_id));
                let hit_pt = bpf::hit_point(
                    points,
                    body,
                    duration,
                    min,
                    max,
                    exp,
                    cx,
                    cy,
                    host.metrics_for(def_id),
                );
                if ctx.ctrl {
                    // Ctrl+click on a point removes it; elsewhere it adds one
                    // at the cursor (which then drags until release).
                    // `None` = nothing changed, `Some(None)` = removed,
                    // `Some(Some(i))` = added at index `i`.
                    let edited: Option<Option<usize>> = match hit_pt {
                        Some(i) => interact::bpf_edit(host, def_id, id, |p, _, _, _, _| {
                            bpf::remove_point(p, i)
                        })
                        .and_then(|removed| removed.then_some(None)),
                        None => interact::bpf_edit(host, def_id, id, |p, duration, lo, hi, exp| {
                            bpf::add_point(p, body, duration, lo, hi, exp, cx, cy)
                        })
                        .map(Some),
                    };
                    if let Some(added) = edited {
                        if let Some(index) = added {
                            self.drag = Some(Drag::BpfPoint { id, index, body });
                        }
                        emit_points(host, out, def_id, id);
                        out.push(GestureEffect::Redraw(def_id));
                    }
                } else if let Some(index) = hit_pt {
                    self.drag = Some(Drag::BpfPoint { id, index, body });
                } else if let Some(segment) = bpf::hit_segment(points, body, duration, cx) {
                    self.drag = Some(Drag::BpfCurve {
                        id,
                        segment,
                        last_y: cy,
                        body_h: body.h.max(1.0) as f64,
                    });
                }
            }
            WidgetKind::Patch {
                ref patch,
                ref selected,
                ..
            } => {
                // A port wins: the cord drag. Then a box: select it and start a
                // move (a press on an already-selected box keeps the set, so the
                // drag moves the whole selection). The empty canvas is not the
                // element's: the press goes back to the canvas' own plan, which
                // sweeps the marquee on a plain drag and leaves Shift to the
                // workspace outside it.
                if let Some(port) = patch::port_hit(rect, patch, cx, cy, scale) {
                    self.drag = Some(Drag::Wire {
                        id,
                        port,
                        area: rect,
                        scale,
                    });
                } else if let Some(hit_box) = patch::box_hit(rect, patch, cx, cy, scale) {
                    let set = if selected.contains(&hit_box) {
                        selected.clone()
                    } else {
                        vec![hit_box]
                    };
                    let grabbed = set
                        .iter()
                        .map(|&i| {
                            let (x, y) = patch::box_pos(rect, patch, i, scale);
                            (i, x, y)
                        })
                        .collect();
                    interact::graph_select(host, def_id, id, set);
                    self.drag = Some(Drag::Box {
                        id,
                        scale,
                        origin: (cx, cy),
                        grabbed,
                        moved: false,
                    });
                    out.push(GestureEffect::Redraw(def_id));
                }
            }
            // A lane's **header** is the element: the band beside the axis
            // carries the controls, so a press there is a mute, a solo or a
            // fader rather than a position. A press on the band's empty space
            // still means nothing (it names no sample), which is what it has
            // meant since the axis stopped locating from the header.
            WidgetKind::Track { .. } => {
                let Some((_, axis)) = interact::time_of(chain) else {
                    return false;
                };
                let Some(h) = interact::header_hit(host, def_id, id, rect, axis.body.x, cx, cy)
                else {
                    return false;
                };
                interact::header_set(host, def_id, id, h.part, h.fader.map(|r| (r, cx)));
                if let Some(r) = h.fader.filter(|_| h.part == interact::HeaderPart::Fader) {
                    self.drag = Some(Drag::LaneLevel { id, rect: r });
                }
                emit_lane(host, out, def_id, id, h.part);
                out.push(GestureEffect::Redraw(def_id));
            }
            // A **clip** is the element now: the layout places it on its lane's
            // axis, so the hit lands on it directly and the press reads the
            // rectangle that was drawn. Empty lane space and the ruler strip
            // are not a clip at all — the press falls back to the chain, where
            // the lane's plan locates the transport.
            WidgetKind::Clip { .. } => {
                let Some(lane) = interact::time_of(chain) else {
                    return false;
                };
                // The lane's own grid, from the container the axis came from.
                let snap = match host.window_def(def_id).and_then(|t| t.find(lane.0)) {
                    Some(w) => match w.kind {
                        WidgetKind::Track { snap, .. } => snap,
                        _ => 0.0,
                    },
                    None => 0.0,
                };
                // The clip's own axis, resolved by the layout and carried down
                // the hit chain — not re-derived from the lane's window here.
                let Some(local) = interact::local_time_of(chain) else {
                    return false;
                };
                if let Some(h) = interact::clip_hit(host, def_id, lane, local, cx, cy) {
                    // An automation clip: a break-point wins over the clip body
                    // (as it wins over a segment in the `bpf` view), and Ctrl+click
                    // adds one - or removes the one under the cursor. The same
                    // gestures, now on a lane.
                    if h.point.is_some() || (ctx.ctrl && h.has_curve) {
                        if ctx.ctrl {
                            if interact::clip_point_edit(
                                host, def_id, h.id, h.point, h.rect, &h.local, cx, cy,
                            ) {
                                emit_points(host, out, def_id, h.id);
                                out.push(GestureEffect::Redraw(def_id));
                            }
                        } else if let Some(index) = h.point {
                            self.drag = Some(Drag::ClipPoint {
                                id: h.id,
                                index,
                                rect: h.rect,
                                nav_start: h.local.start,
                                nav_len: h.local.len,
                            });
                        }
                        return true;
                    }
                    let press_sample = interact::sample_at(
                        h.nav.start,
                        h.nav.len,
                        h.body.x as f64,
                        h.body.w as f64,
                        cx,
                    );
                    self.drag = Some(Drag::Clip {
                        id: h.id,
                        lane: h.lane,
                        part: h.part,
                        body_x: h.body.x as f64,
                        body_w: h.body.w as f64,
                        nav_start: h.nav.start,
                        nav_len: h.nav.len,
                        press_sample,
                        orig_offset: h.offset,
                        orig_dur: h.dur,
                        grid: snap,
                    });
                }
            }
            WidgetKind::Piano {
                min,
                max,
                active_min,
                active_max,
                pan,
                overview,
                velocity,
                channel,
                ref label,
                ..
            } => {
                let l = piano::layout(
                    rect,
                    min,
                    max,
                    overview,
                    label.is_some(),
                    host.metrics_for(def_id),
                );
                // A press on the overview strip grabs the visible window: the
                // drag pans it (relative, from the press snapshot). Gated by
                // `pan` — a fixed-range piano ignores the strip.
                if let Some(strip) = l.overview
                    && strip.contains(cx, cy)
                {
                    if pan {
                        self.drag = Some(Drag::PianoView {
                            id,
                            strip,
                            min0: l.min,
                            max0: l.max,
                            anchor: piano::overview_hit(strip, cx as f32),
                        });
                    }
                    return true;
                }
                // A press on a key plays it — inert outside the active range.
                if let Some(p) = piano::hit(&l, cx as f32, cy as f32) {
                    if !(active_min..=active_max).contains(&p) {
                        return true;
                    }
                    let vel = velocity.unwrap_or_else(|| piano::velocity_at(&l, p, cy as f32));
                    piano_note(host, out, def_id, id, p, vel, 1, channel);
                    self.drag = Some(Drag::PianoKey {
                        id,
                        layout: l,
                        pitch: p,
                        fixed_vel: velocity,
                        channel,
                    });
                    out.push(GestureEffect::Redraw(def_id));
                }
            }
            WidgetKind::Score(ref data) => {
                // A press names the engraved element under it by its MEI id —
                // the same id the client engraved from, so a driver resolves it
                // in its own score. Pressing blank paper clears the selection.
                let picked = data.hit(rect, cx as f32, cy as f32).map(str::to_string);
                if interact::score_select(host, def_id, id, picked.as_deref()) {
                    out.push(GestureEffect::Emit {
                        def_id,
                        widget_id: id,
                        args: interact::score_element_args(picked.as_deref()),
                    });
                    out.push(GestureEffect::Redraw(def_id));
                }
                // ...and, on an editable score, holding it drags the element's
                // pitch. A press that does not move stays a plain selection: the
                // release emits nothing more. A read-only page (the default)
                // still selects and reports the element above, but a drag does
                // nothing — the host holds no score, so an edit the client will
                // not apply is a gesture it cannot fulfil.
                if data.editable
                    && let Some(element) = picked
                {
                    self.drag = Some(Drag::ScoreStep {
                        id,
                        element,
                        rect,
                        origin_y: cy,
                        steps: 0,
                    });
                }
            }
            WidgetKind::PianoRoll { .. } => {
                let Some((_, axis)) = interact::time_of(chain) else {
                    return false;
                };
                let Some(h) = interact::pianoroll_hit(host, def_id, (id, rect, axis), cx, cy)
                else {
                    return false;
                };
                self.pianoroll_press(host, out, ctx, id, &h, cx, cy);
            }
            _ => {}
        }
        // Nothing the element wanted: the press goes back to the chain.
        self.drag.is_some() || out.len() > effects_before
    }

    /// Pointer moved while a drag is active: drive the dragged target. The drag
    /// descriptor is cloned out (cheap: geometry plus, for the block gestures, a
    /// small snapshot vec) so the host tree can be mutated under it.
    pub fn drag_to(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        cx: f64,
        cy: f64,
    ) -> Vec<GestureEffect> {
        let mut out = Vec::new();
        let def_id = ctx.def_id;
        let Some(drag) = self.drag.clone() else {
            return out;
        };
        match drag {
            // A held button and a wire-in-flight only act on release; a locked
            // knob drag is driven by relative motion (`relative_motion`), not by
            // these cursor positions.
            Drag::Button { .. } | Drag::Wire { .. } => {}
            Drag::TextSelect {
                id,
                rect,
                scale,
                anchor,
            } => {
                // Extend the selection from the press anchor to the caret under
                // the cursor. No emit: the string did not change, only the view
                // state (the selection).
                if let Some(pos) = interact::text_caret_at(host, def_id, id, rect, scale, cx, cy) {
                    interact::text_edit(host, def_id, id, |_, caret, _| {
                        caret.pos = pos;
                        caret.anchor = Some(anchor);
                    });
                    out.push(GestureEffect::Redraw(def_id));
                }
            }
            Drag::Box {
                id,
                scale,
                origin,
                ref grabbed,
                ..
            } => {
                // The whole grabbed set moves by the cursor delta, in canvas
                // units (the screen delta divided by the workspace zoom).
                let dx = ((cx - origin.0) / scale as f64) as f32;
                let dy = ((cy - origin.1) / scale as f64) as f32;
                let moves: Vec<_> = grabbed
                    .iter()
                    .map(|&(i, x0, y0)| (i, x0 + dx, y0 + dy))
                    .collect();
                interact::graph_move(host, def_id, id, &moves);
                if let Some(Drag::Box { moved, .. }) = self.drag.as_mut() {
                    *moved = true;
                }
                out.push(GestureEffect::Redraw(def_id));
            }
            Drag::Marquee {
                id,
                area,
                scale,
                origin,
                ..
            } => {
                interact::graph_marquee(host, def_id, id, area, origin, (cx, cy), scale);
                if let Some(Drag::Marquee { cursor, .. }) = self.drag.as_mut() {
                    *cursor = (cx, cy);
                }
                out.push(GestureEffect::Redraw(def_id));
            }
            Drag::Vertical { locked: true, .. } => {}
            Drag::Slider { id, body, vertical } => {
                let t = slider_t(body, cx, cy, vertical);
                interact::set_fraction(host, def_id, id, t);
                emit_value(host, &mut out, def_id, id);
                out.push(GestureEffect::Redraw(def_id));
            }
            Drag::Vertical {
                id, last_y, body_h, ..
            } => {
                // Incremental: add this step's delta to the *current* (clamped)
                // fraction and re-anchor `last_y`. A value pinned at an end stays
                // put, but reversing moves it immediately — no snapshot dead zone.
                let cur = interact::fraction_of(host, def_id, id).unwrap_or(0.0);
                let t = (cur + controls::drag_fraction_delta(cy - last_y, body_h)).clamp(0.0, 1.0);
                interact::set_fraction(host, def_id, id, t);
                if let Some(Drag::Vertical { last_y, .. }) = self.drag.as_mut() {
                    *last_y = cy;
                }
                emit_value(host, &mut out, def_id, id);
                out.push(GestureEffect::Redraw(def_id));
            }
            Drag::Pan {
                id,
                origin_x,
                start,
                body_w,
            } => {
                pan_timeline(host, &mut out, def_id, id, start, (cx - origin_x) / body_w);
            }
            Drag::PanY {
                id,
                origin_y,
                y_start,
                lane_h,
            } => {
                // Dragging down moves the window down with the cursor;
                // absolute from the snapshot, so a clamped edge never drifts.
                let y_len = host
                    .window_def(def_id)
                    .and_then(|t| t.find(id))
                    .and_then(|w| w.kind.editor())
                    .map_or(1.0, |e| e.y_view().1);
                let start = y_start + (cy - origin_y) / lane_h * y_len;
                set_y_view(host, &mut out, def_id, id, start, y_len);
            }
            Drag::PianoKey {
                id,
                layout,
                pitch,
                fixed_vel,
                channel,
            } => {
                // Glissando: crossing into another (active) key releases the
                // held one and presses the new; leaving the keyboard keeps the
                // note held until release.
                if let Some(p) = piano::hit(&layout, cx as f32, cy as f32)
                    && p != pitch
                    && interact::piano_key_active(host, def_id, id, p)
                {
                    let vel =
                        fixed_vel.unwrap_or_else(|| piano::velocity_at(&layout, p, cy as f32));
                    piano_note(host, &mut out, def_id, id, pitch, 0, 0, channel);
                    piano_note(host, &mut out, def_id, id, p, vel, 1, channel);
                    if let Some(Drag::PianoKey { pitch, .. }) = self.drag.as_mut() {
                        *pitch = p;
                    }
                    out.push(GestureEffect::Redraw(def_id));
                }
            }
            Drag::ScoreStep {
                id,
                ref element,
                rect,
                origin_y,
                steps,
            } => {
                // Absolute from the press, quantized to whole steps: the page
                // is redrawn only when the drag crosses one, so the pixels
                // between two pitches cost nothing.
                let Some(n) = score_steps(host, def_id, id, rect, cy - origin_y) else {
                    return out;
                };
                if n != steps && interact::score_drag(host, def_id, id, element, n) {
                    if let Some(Drag::ScoreStep { steps, .. }) = self.drag.as_mut() {
                        *steps = n;
                    }
                    out.push(GestureEffect::Redraw(def_id));
                }
            }
            Drag::PianoView {
                id,
                strip,
                min0,
                max0,
                anchor,
            } => {
                let cur = piano::overview_hit(strip, cx as f32);
                let (nmin, nmax) = piano::pan_range(min0, max0, cur - anchor);
                set_piano_range(host, &mut out, def_id, id, nmin, nmax);
            }
            Drag::ScrollPan {
                id,
                area,
                origin_x,
                origin_y,
                x0,
                y0,
            } => {
                // Dragging the plane moves the content with the cursor: the
                // view offsets run against the drag, in content units (the
                // zoom divides the pixel displacement), gated by the axis.
                let Some(view) = scroll_view(host, def_id, id) else {
                    return out;
                };
                let zoom = view.zoom(host.metrics_for(def_id));
                let nx = match view.axis {
                    Axis::Y => x0,
                    _ => x0 - (cx - origin_x) / zoom,
                };
                let ny = match view.axis {
                    Axis::X => y0,
                    _ => y0 - (cy - origin_y) / zoom,
                };
                set_scroll_view(host, &mut out, def_id, id, area, (nx, ny, zoom));
            }
            Drag::BpfPoint { id, index, body } => {
                interact::bpf_edit(host, def_id, id, |p, duration, lo, hi, exp| {
                    bpf::move_point(p, index, body, duration, lo, hi, exp, cx, cy);
                });
                emit_points(host, &mut out, def_id, id);
                out.push(GestureEffect::Redraw(def_id));
            }
            Drag::BpfCurve {
                id,
                segment,
                last_y,
                body_h,
            } => {
                // Incremental like a knob: the upward step bends the curve so
                // the segment's middle follows the cursor.
                let dy_frac = (last_y - cy) / body_h;
                interact::bpf_edit(host, def_id, id, |p, _, _, _, _| {
                    bpf::drag_curve(p, segment, dy_frac);
                });
                if let Some(Drag::BpfCurve { last_y, .. }) = self.drag.as_mut() {
                    *last_y = cy;
                }
                emit_points(host, &mut out, def_id, id);
                out.push(GestureEffect::Redraw(def_id));
            }
            Drag::Select {
                id,
                body,
                nav_start,
                nav_len,
                anchor,
                value,
            } => {
                // Against the group's **current** window (the press-time one is
                // the fallback for a view that is in no group): the axis may
                // have moved under the sweep, and the anchor is already a
                // timeline coordinate.
                let (start, len) =
                    group_view(host, id).map_or((nav_start, nav_len), |(s, l, _)| (s, l));
                let cur = interact::sample_at(start, len, body.x as f64, body.w as f64, cx);
                set_selection(host, &mut out, def_id, id, anchor, cur);
                if let Some((lo, hi, anchor_value)) = value {
                    let v = interact::value_at(body, lo, hi, cy);
                    interact::select_elements_in_rect(
                        host,
                        def_id,
                        id,
                        (anchor, cur),
                        (anchor_value, v),
                    );
                    out.push(GestureEffect::Redraw(def_id));
                }
            }
            Drag::Clip {
                id,
                lane,
                part,
                body_x,
                body_w,
                nav_start,
                nav_len,
                press_sample,
                orig_offset,
                orig_dur,
                grid,
            } => {
                apply_clip_drag(
                    host,
                    &mut out,
                    def_id,
                    ClipDrag {
                        id,
                        lane,
                        part,
                        body_x,
                        body_w,
                        nav_start,
                        nav_len,
                        press_sample,
                        orig_offset,
                        orig_dur,
                        grid,
                    },
                    cx,
                );
            }
            Drag::LaneLevel { id, rect } => {
                let part = interact::HeaderPart::Fader;
                interact::header_set(host, def_id, id, part, Some((rect, cx)));
                emit_lane(host, &mut out, def_id, id, part);
                out.push(GestureEffect::Redraw(def_id));
            }
            Drag::ClipPoint {
                id,
                index,
                rect,
                nav_start,
                nav_len,
            } => {
                // The curve of an automation clip, edited in place: the cursor maps
                // back through the clip's own axis (time) and its value range,
                // then the point moves with the `bpf` model's own semantics.
                let local = View {
                    start: nav_start,
                    len: nav_len,
                };
                if interact::clip_point_move(host, def_id, id, index, rect, &local, cx, cy) {
                    emit_points(host, &mut out, def_id, id);
                    out.push(GestureEffect::Redraw(def_id));
                }
            }
            Drag::Note {
                id,
                index,
                part,
                grid,
                nav_start,
                nav_len,
                lo,
                hi,
                press_time,
                orig_start,
                orig_dur,
                snap,
            } => {
                // Map the cursor to a region-relative time and (for a body move)
                // a pitch; a press-time snapshot keeps a clamped edge from
                // drifting, snapped to the note grid.
                let time =
                    interact::sample_at(nav_start, nav_len, grid.x as f64, grid.w as f64, cx);
                interact::pianoroll_notes_edit(host, def_id, id, |notes| match part {
                    pianoroll::NotePart::Body => {
                        let delta = time - press_time;
                        let new_start = interact::snap(orig_start + delta, snap);
                        let pitch = pianoroll::y_to_pitch(cy as f32, lo, hi, grid);
                        pianoroll::move_note(notes, index, new_start, pitch, lo, hi);
                        // The duration is preserved by move_note; re-assert it in
                        // case a prior edit changed it under a running drag.
                        if let Some(n) = notes.get_mut(index) {
                            n.dur = orig_dur;
                        }
                    }
                    other => {
                        pianoroll::resize_note(
                            notes,
                            index,
                            other,
                            interact::snap(time, snap),
                            1.0,
                        );
                    }
                });
                host.sync_track_totals();
                emit_notes(host, &mut out, def_id, id);
                out.push(GestureEffect::Redraw(def_id));
            }
            Drag::Velocity { id, index, lane } => {
                let vel = pianoroll::velocity_at(lane, cy);
                interact::pianoroll_notes_edit(host, def_id, id, |notes| {
                    pianoroll::set_velocity(notes, index, vel);
                });
                emit_notes(host, &mut out, def_id, id);
                out.push(GestureEffect::Redraw(def_id));
            }
            Drag::OscMark {
                id,
                index,
                grid,
                nav_start,
                nav_len,
                snap,
            } => {
                let time =
                    interact::sample_at(nav_start, nav_len, grid.x as f64, grid.w as f64, cx);
                interact::pianoroll_osc_edit(host, def_id, id, |osc| {
                    if let Some(m) = osc.get_mut(index) {
                        m.time = interact::snap(time, snap).max(0.0);
                    }
                });
                host.sync_track_totals();
                emit_osc(host, &mut out, def_id, id);
                out.push(GestureEffect::Redraw(def_id));
            }
            Drag::NoteBlock {
                id,
                grid,
                nav_start,
                nav_len,
                lo,
                hi,
                press_time,
                press_pitch,
                snap,
                orig,
            } => {
                // The block move: the grabbed note (the leading snapshot entry)
                // snaps to the note grid, and the whole selection moves rigidly
                // by that delta — the core clamps it as one.
                let time =
                    interact::sample_at(nav_start, nav_len, grid.x as f64, grid.w as f64, cx);
                let dt = match orig.first() {
                    Some((_, s0, _)) => interact::snap(s0 + (time - press_time), snap) - s0,
                    None => 0.0,
                };
                let dp = pianoroll::y_to_pitch(cy as f32, lo, hi, grid) - press_pitch;
                interact::pianoroll_notes_edit(host, def_id, id, |notes| {
                    pianoroll::move_notes_from(notes, &orig, dt, dp, lo, hi);
                });
                host.sync_track_totals();
                emit_notes(host, &mut out, def_id, id);
                out.push(GestureEffect::Redraw(def_id));
            }
            Drag::VelocityBlock {
                id,
                lane,
                press_velocity,
                orig,
            } => {
                let dv = pianoroll::velocity_at(lane, cy) - press_velocity;
                interact::pianoroll_notes_edit(host, def_id, id, |notes| {
                    pianoroll::nudge_velocities_from(notes, &orig, dv);
                });
                emit_notes(host, &mut out, def_id, id);
                out.push(GestureEffect::Redraw(def_id));
            }
        }
        out
    }

    /// Release: a held button emits 0; a knob/number drag releases its pointer
    /// grab; a pulled wire lands (rewire over a bus, unwire elsewhere); any
    /// drag ends.
    pub fn release(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        cx: f64,
        cy: f64,
    ) -> Vec<GestureEffect> {
        let mut out = Vec::new();
        let def_id = ctx.def_id;
        match self.drag.take() {
            Some(Drag::Button { id }) => {
                deliver(host, &mut out, def_id, id, OscType::Int(0));
                out.push(GestureEffect::Redraw(def_id));
            }
            Some(Drag::Vertical { .. }) => out.push(GestureEffect::ReleasePointer(def_id)),
            Some(Drag::PianoKey {
                id, pitch, channel, ..
            }) => {
                piano_note(host, &mut out, def_id, id, pitch, 0, 0, channel);
                out.push(GestureEffect::Redraw(def_id));
            }
            Some(Drag::Wire {
                id,
                port,
                area,
                scale,
            }) => {
                // Released over a compatible port: a directed cord is drawn
                // (outlet -> inlet, matching rate) and the edit leaves as the
                // flat directed `"wire" src_box outlet dst_box inlet` event, so
                // the driver adds the cord and re-renders. Anything else cancels.
                if let Some((from, outlet, to, inlet)) =
                    interact::graph_cord(host, def_id, id, port, area, cx, cy, scale)
                {
                    out.push(GestureEffect::Emit {
                        def_id,
                        widget_id: id,
                        args: vec![
                            OscType::String("wire".into()),
                            OscType::Int(from as i32),
                            OscType::String(outlet),
                            OscType::Int(to as i32),
                            OscType::String(inlet),
                        ],
                    });
                    out.push(GestureEffect::Redraw(def_id));
                }
            }
            Some(Drag::Box {
                id,
                scale,
                origin,
                grabbed,
                moved,
                ..
            }) => {
                // The boxes were moved live along the drag; the release emits
                // the round trip — one `"move" index x y` per box, so the driver
                // owns the geometry (the clip pattern).
                if moved {
                    let dx = ((cx - origin.0) / scale as f64) as f32;
                    let dy = ((cy - origin.1) / scale as f64) as f32;
                    for (index, x0, y0) in grabbed {
                        out.push(GestureEffect::Emit {
                            def_id,
                            widget_id: id,
                            args: vec![
                                OscType::String("move".into()),
                                OscType::Int(index as i32),
                                OscType::Float(x0 + dx),
                                OscType::Float(y0 + dy),
                            ],
                        });
                    }
                    out.push(GestureEffect::Redraw(def_id));
                }
            }
            Some(Drag::Marquee { .. }) => {
                // The selection followed the rectangle live; the release just
                // drops the marquee chrome.
                out.push(GestureEffect::Redraw(def_id));
            }
            Some(Drag::ScoreStep { id, element, .. }) => {
                // The element was displaced live; the release asks the client
                // to make it true — the host holds no score, so the pitch edit
                // is the driver's to apply and re-engrave (the clip pattern).
                if let Some(steps) = interact::score_drag_end(host, def_id, id) {
                    out.push(GestureEffect::Emit {
                        def_id,
                        widget_id: id,
                        args: interact::score_transpose_args(&element, steps),
                    });
                }
                out.push(GestureEffect::Redraw(def_id));
            }
            _ => {}
        }
        out
    }

    /// Relative pointer motion while a **locked** knob/number drag is active
    /// (native pointer lock: the cursor stays put, motion arrives as deltas).
    /// A no-op for any other drag state.
    pub fn relative_motion(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        dy: f64,
    ) -> Vec<GestureEffect> {
        let mut out = Vec::new();
        let Some(Drag::Vertical {
            id,
            body_h,
            locked: true,
            ..
        }) = self.drag
        else {
            return out;
        };
        let def_id = ctx.def_id;
        let cur = interact::fraction_of(host, def_id, id).unwrap_or(0.0);
        let t = (cur + controls::drag_fraction_delta(dy, body_h)).clamp(0.0, 1.0);
        interact::set_fraction(host, def_id, id, t);
        emit_value(host, &mut out, def_id, id);
        out.push(GestureEffect::Redraw(def_id));
        out
    }

    /// Wheel over a timeline view: zoom the shared time axis anchored at the
    /// cursor, or — over the y-ruler strip / the piano-roll's keyboard gutter —
    /// zoom the vertical display window anchored at the cursor's height.
    pub fn wheel(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        cx: f64,
        cy: f64,
        steps: f64,
    ) -> Vec<GestureEffect> {
        let mut out = Vec::new();
        let def_id = ctx.def_id;
        let Some(Hit {
            id,
            rect,
            kind,
            chain,
            ..
        }) = hit(host, ctx, cx, cy)
        else {
            return out;
        };
        // The piano navigates its own MIDI range, not a timeline group: wheel
        // over the overview strip zooms the range (anchored at the cursor's
        // key), over the keys it pans by whole white keys. Both gated by `pan`.
        if let WidgetKind::Piano {
            min,
            max,
            pan,
            overview,
            ref label,
            ..
        } = kind
        {
            if pan {
                let l = piano::layout(
                    rect,
                    min,
                    max,
                    overview,
                    label.is_some(),
                    host.metrics_for(def_id),
                );
                let (nmin, nmax) = match l.overview.filter(|s| s.contains(cx, cy)) {
                    Some(strip) => {
                        let anchor = piano::overview_hit(strip, cx as f32) as f64;
                        piano::zoom_range(l.min, l.max, 0.85f64.powf(steps), anchor)
                    }
                    None => piano::pan_white(l.min, l.max, steps.round() as i32),
                };
                set_piano_range(host, &mut out, def_id, id, nmin, nmax);
            }
            return out;
        }
        // **Ctrl+wheel over a lane is the other axis of the view**: not time,
        // which the bare wheel already zooms, but how thick the lane is. The
        // stack it lives in cannot do it — a plane's zoom is uniform over both
        // axes and would stretch the time axis out from under the ruler — and a
        // lane's thickness is a number on the wire, so this is an edit of the
        // document like a clip's placement: applied here and emitted as
        // `"height" h` for whoever owns the tree to mirror (a driver usually
        // gives every lane the same thickness, which is its call, not ours).
        if ctx.ctrl
            && let Some((tid, _)) = interact::time_of(&chain)
            && let Some(frame) = chain.iter().rev().find(|f| f.id == Some(tid))
        {
            // The wire's lengths are logical, the rectangle is physical: a lane
            // with no `h` of its own is measured off the pixels it was drawn at
            // and given one, so the first turn of the wheel does not jump.
            let ui = host.metrics_for(def_id).ui_scale.max(f32::EPSILON);
            let drawn = frame.rect.h / ui;
            if let Some(h) =
                interact::lane_resize(host, def_id, tid, drawn, 1.1f32.powf(steps as f32))
            {
                emit(
                    &mut out,
                    def_id,
                    tid,
                    vec![OscType::String("height".into()), OscType::Float(h)],
                );
                out.push(GestureEffect::Redraw(def_id));
                return out;
            }
        }
        // A timeline view's wheel is its **axis'**, and the axis is on the
        // chain: over the vertical strip it zooms the display window, anywhere
        // else the shared time axis, both anchored at the cursor.
        if let Some((tid, axis)) = interact::time_of(&chain) {
            let factor = 0.85f64.powf(steps);
            match axis.y.filter(|y| y.strip.contains(cx, cy)) {
                // The vertical anchor depends on what the axis *measures*,
                // because one window is shared by every channel lane:
                //
                // - **Amplitude** (the waveform): the window keeps its own
                //   centre, so zero stays at the centre of *every* lane and
                //   the trace grows and shrinks inside its lane. An anchor
                //   taken from the cursor's height would be meaningless for
                //   the other lanes, and any off-centre window pushes the
                //   wave out of the lane and clips it.
                // - **Frequency** (the spectrogram) and **pitch** (the roll):
                //   the cursor's height, which is the value under it. There the
                //   shared window says the same thing in every lane -- all of
                //   them show that band -- so anchoring at the cursor is both
                //   meaningful and what the reader wants.
                Some(y) => {
                    let anchor = match kind.signal() {
                        Some(el) if el.presentation == Presentation::Signal => 0.5,
                        _ => {
                            let lane_top = axis.body.y as f64
                                + ((cy - axis.body.y as f64) / y.lane_h).floor() * y.lane_h;
                            1.0 - ((cy - lane_top) / y.lane_h).clamp(0.0, 1.0)
                        }
                    };
                    zoom_timeline_y(host, &mut out, def_id, tid, factor, anchor);
                }
                None => zoom_timeline(host, &mut out, def_id, tid, axis.body, cx, factor),
            }
            return out;
        }
        // The 2D workspace: wheel zooms the plane anchored at the cursor;
        // with zoom disabled it pans along the axis instead (Shift pans x in
        // a two-axis workspace) — the plain scroll view's wheel. A widget
        // with its own wheel (a timeline view, a piano) won above.
        if let Some((id, area, view)) = interact::plane_of(&chain) {
            let zoom = view.zoom(host.metrics_for(def_id));
            let next = if view.zoom_enabled {
                let factor = 0.85f64.powf(-steps); // wheel up zooms in
                scroll::zoom_at((view.view_x, view.view_y, zoom), area, (cx, cy), factor)
            } else {
                let d = steps * scroll::WHEEL_PAN_PX / zoom;
                match view.axis {
                    Axis::X => (view.view_x - d, view.view_y, zoom),
                    _ if ctx.shift => (view.view_x - d, view.view_y, zoom),
                    _ => (view.view_x, view.view_y - d, zoom),
                }
            };
            // A plane that **cannot** move passes the wheel on rather than
            // eating it: the slack under a short stack is not a surface with a
            // gesture of its own.
            if set_scroll_view(host, &mut out, def_id, id, area, next) {
                return out;
            }
        }
        // Nothing under the pointer claimed the wheel — a gap between lanes,
        // the slack under the last one, a container's margin. In a window with
        // **one** axis those pixels are that axis with nothing drawn on them,
        // so the wheel means there what it means over a lane: Ctrl the lanes'
        // thickness, otherwise the time zoom, anchored at the cursor.
        if let Some(sole) =
            interact::sole_time_axis(host, def_id, ctx.fb_w, ctx.fb_h, &|id, kind| {
                ctx.lanes(id, kind)
            })
        {
            if ctx.ctrl {
                let factor = 1.1f32.powf(steps as f32);
                let ui = host.metrics_for(def_id).ui_scale.max(f32::EPSILON);
                for lane in sole.lanes {
                    let drawn = sole.axis.body.h / ui;
                    if let Some(h) = interact::lane_resize(host, def_id, lane, drawn, factor) {
                        emit(
                            &mut out,
                            def_id,
                            lane,
                            vec![OscType::String("height".into()), OscType::Float(h)],
                        );
                    }
                }
                out.push(GestureEffect::Redraw(def_id));
            } else {
                let factor = 0.85f64.powf(steps);
                zoom_timeline(host, &mut out, def_id, sole.id, sole.axis.body, cx, factor);
            }
        }
        out
    }

    /// Handles a plain (non-Shift) press on a `pianoroll`: start a note
    /// move/resize (a **selected** note moves the whole selection), a velocity
    /// drag (over a selected note, the whole selection's) or an OSC-marker
    /// drag; Ctrl+click adds or removes a note/marker; Alt+click toggles a note
    /// in/out of the multi-note selection; a press on empty grid drags the
    /// marquee — the shared time selection restricted in pitch, which fills the
    /// selected set.
    #[allow(clippy::too_many_arguments)] // one press: a hit, the context, a cursor
    fn pianoroll_press(
        &mut self,
        host: &mut Host,
        out: &mut Vec<GestureEffect>,
        ctx: &GestureCtx,
        id: i32,
        h: &interact::PianoRollHit,
        cx: f64,
        cy: f64,
    ) {
        let def_id = ctx.def_id;
        let nav = View {
            start: h.nav.start,
            len: h.nav.len,
        };
        match h.region {
            interact::PrRegion::Grid => {
                // Alt+click toggles a note in/out of the multi-note selection
                // (a non-rectangular selection, one note at a time).
                if ctx.alt {
                    if let Some(nh) = h.note {
                        interact::pianoroll_state_edit(host, def_id, id, |_, sel| {
                            pianoroll::toggle_selected(sel, nh.index);
                        });
                        out.push(GestureEffect::Redraw(def_id));
                    }
                    return;
                }
                if ctx.ctrl {
                    match h.note {
                        // Ctrl+click on a note removes it (the selection's
                        // indices shift down past it).
                        Some(nh) => {
                            interact::pianoroll_state_edit(host, def_id, id, |notes, sel| {
                                pianoroll::remove_note(notes, nh.index);
                                *sel = pianoroll::selection_after_removal(sel, nh.index);
                            });
                        }
                        // Ctrl+click on empty grid adds a note there, then drags
                        // its end to set the length until release.
                        None => {
                            let time = interact::snap(
                                pianoroll::time_at(h.grid, &nav, 0.0, cx as f32),
                                h.snap,
                            )
                            .max(0.0);
                            let pitch = pianoroll::y_to_pitch(cy as f32, h.lo, h.hi, h.grid)
                                .round()
                                .clamp(h.lo, h.hi);
                            let dur = if h.snap > 0.0 {
                                h.snap
                            } else {
                                (h.nav.len * 0.05).max(1.0)
                            };
                            let index = interact::pianoroll_notes_edit(host, def_id, id, |notes| {
                                pianoroll::insert_note(
                                    notes,
                                    pianoroll::Note::new(time, dur, pitch),
                                )
                            });
                            if let Some(index) = index {
                                self.drag = Some(Drag::Note {
                                    id,
                                    index,
                                    part: pianoroll::NotePart::End,
                                    grid: h.grid,
                                    nav_start: h.nav.start,
                                    nav_len: h.nav.len,
                                    lo: h.lo,
                                    hi: h.hi,
                                    press_time: time,
                                    orig_start: time,
                                    orig_dur: dur,
                                    snap: h.snap,
                                });
                            }
                        }
                    }
                    host.sync_track_totals();
                    emit_notes(host, out, def_id, id);
                    out.push(GestureEffect::Redraw(def_id));
                    return;
                }
                // Move (body) or resize (edge) the note under the cursor.
                // Grabbing the body of a **selected** note moves the whole
                // selection rigidly; grabbing an unselected one drops the
                // selection first (the single-note gesture, as before). Empty
                // grid is nothing of the element's: the press goes back to the
                // roll's own plan, whose plain drag sweeps the selection --
                // the shared time span, restricted in pitch.
                if let Some(nh) = h.note {
                    let press_time = pianoroll::time_at(h.grid, &nav, 0.0, cx as f32);
                    if nh.part == pianoroll::NotePart::Body {
                        let orig =
                            interact::pianoroll_state_edit(host, def_id, id, |notes, sel| {
                                if !sel.contains(&nh.index) {
                                    sel.clear();
                                    return Vec::new();
                                }
                                // The grabbed note's snapshot leads (the
                                // snap anchor).
                                let mut idx = sel.clone();
                                idx.retain(|&i| i != nh.index);
                                idx.insert(0, nh.index);
                                idx.iter()
                                    .filter_map(|&i| notes.get(i).map(|n| (i, n.start, n.pitch)))
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        if !orig.is_empty() {
                            let press_pitch = pianoroll::y_to_pitch(cy as f32, h.lo, h.hi, h.grid);
                            self.drag = Some(Drag::NoteBlock {
                                id,
                                grid: h.grid,
                                nav_start: h.nav.start,
                                nav_len: h.nav.len,
                                lo: h.lo,
                                hi: h.hi,
                                press_time,
                                press_pitch,
                                snap: h.snap,
                                orig,
                            });
                            return;
                        }
                    }
                    let (orig_start, orig_dur) =
                        note_at(host, def_id, id, nh.index).unwrap_or((0.0, 0.0));
                    self.drag = Some(Drag::Note {
                        id,
                        index: nh.index,
                        part: nh.part,
                        grid: h.grid,
                        nav_start: h.nav.start,
                        nav_len: h.nav.len,
                        lo: h.lo,
                        hi: h.hi,
                        press_time,
                        orig_start,
                        orig_dur,
                        snap: h.snap,
                    });
                }
            }
            interact::PrRegion::Velocity => {
                if let Some(nh) = h.note {
                    // Over a **selected** note the whole selection's velocities
                    // nudge together (relative, from a press snapshot); over an
                    // unselected one the single bar follows the cursor.
                    let orig = interact::pianoroll_state_edit(host, def_id, id, |notes, sel| {
                        if !sel.contains(&nh.index) {
                            return Vec::new();
                        }
                        sel.iter()
                            .filter_map(|&i| notes.get(i).map(|n| (i, n.velocity)))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                    if !orig.is_empty() {
                        let lane = h.region_rect;
                        self.drag = Some(Drag::VelocityBlock {
                            id,
                            lane,
                            press_velocity: pianoroll::velocity_at(lane, cy),
                            orig,
                        });
                        return;
                    }
                    self.drag = Some(Drag::Velocity {
                        id,
                        index: nh.index,
                        lane: h.region_rect,
                    });
                }
            }
            interact::PrRegion::Osc => {
                if ctx.ctrl {
                    match h.osc_index {
                        Some(index) => {
                            interact::pianoroll_osc_edit(host, def_id, id, |osc| {
                                if index < osc.len() {
                                    osc.remove(index);
                                }
                            });
                        }
                        None => {
                            let time = interact::snap(
                                pianoroll::time_at(h.grid, &nav, 0.0, cx as f32),
                                h.snap,
                            )
                            .max(0.0);
                            interact::pianoroll_osc_edit(host, def_id, id, |osc| {
                                osc.push(pianoroll::OscMark { time, label: None });
                            });
                        }
                    }
                    host.sync_track_totals();
                    emit_osc(host, out, def_id, id);
                    out.push(GestureEffect::Redraw(def_id));
                } else if let Some(index) = h.osc_index {
                    self.drag = Some(Drag::OscMark {
                        id,
                        index,
                        grid: h.grid,
                        nav_start: h.nav.start,
                        nav_len: h.nav.len,
                        snap: h.snap,
                    });
                }
            }
        }
    }
}

mod effects;
mod keys;
mod nav;

use effects::*;
use nav::*;

pub(crate) use nav::corner_rect;

#[cfg(test)]
mod tests;
