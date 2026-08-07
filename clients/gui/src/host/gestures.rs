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

use std::collections::HashMap;

use clausters_core::osc::OscType;

use super::interact::{self, Hit, slider_t, value_of};
use super::layout::Rect;
use super::widget::{Axis, Ruler, RulerY, ScrollView, Widget, WidgetKind};
use super::{Host, bpf, controls, frame, patch, piano, pianoroll, scroll, track};
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
        match kind {
            WidgetKind::Waveform { overlay: true, .. } => 1,
            WidgetKind::Waveform { .. } => self.wave_lanes.get(&id).copied().unwrap_or(1).max(1),
            WidgetKind::Spectrogram { .. } => {
                self.spect_lanes.get(&id).copied().unwrap_or(1).max(1)
            }
            _ => 1,
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
    /// Dragging a selection on a timeline view: `anchor` is the sample under
    /// the press; the selection spans from it to the cursor's sample.
    Select {
        id: i32,
        body_x: f64,
        body_w: f64,
        anchor: f64,
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
    ClipPoint {
        id: i32,
        index: usize,
        rect: Rect,
        body: Rect,
        nav_start: f64,
        nav_len: f64,
        offset: f64,
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
    /// A marquee on the piano-roll's empty grid: the time span keeps driving the
    /// **shared time selection** (linked views follow it, exactly as
    /// [`Drag::Select`] does), and the notes inside the time × pitch rectangle
    /// become the widget's multi-note selection.
    SelectNotes {
        id: i32,
        grid: Rect,
        nav_start: f64,
        nav_len: f64,
        lo: f32,
        hi: f32,
        /// The absolute sample under the press.
        anchor: f64,
        /// The (fractional) pitch under the press.
        anchor_pitch: f32,
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
}

impl Gestures {
    /// Whether a drag is in progress (the front routes cursor motion here).
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
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
        let Some((start, len, _)) = nav(host, lane) else {
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

    /// Press on a widget: act by kind and possibly start a drag. `grab` is the
    /// front's pointer-grab attempt for a knob/number drag (returns whether the
    /// pointer was *locked*); a front without pointer lock returns `false`.
    pub fn press(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        cx: f64,
        cy: f64,
        grab: &mut dyn FnMut() -> bool,
    ) -> Vec<GestureEffect> {
        let mut out = Vec::new();
        let Some(Hit {
            id,
            rect,
            scale,
            kind,
            chain,
        }) = hit(host, ctx, cx, cy)
        else {
            // A press on empty space drops the text focus (the caret disappears).
            if let Some(old) = host.clear_text_focus() {
                out.push(GestureEffect::Redraw(old));
            }
            return out;
        };
        let def_id = ctx.def_id;
        // A press on anything other than the focused text field defocuses it.
        if !matches!(kind, WidgetKind::Text { .. })
            && let Some(old) = host.clear_text_focus()
        {
            out.push(GestureEffect::Redraw(old));
        }
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
                emit_value(host, &mut out, def_id, id);
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
                deliver(host, &mut out, def_id, id, OscType::Int(1));
                self.drag = Some(Drag::Button { id });
                out.push(GestureEffect::Redraw(def_id));
            }
            WidgetKind::Toggle { .. } => {
                interact::flip_toggle(host, def_id, id);
                emit_value(host, &mut out, def_id, id);
                out.push(GestureEffect::Redraw(def_id));
            }
            WidgetKind::Menu { .. } => {
                interact::cycle_menu(host, def_id, id);
                emit_value(host, &mut out, def_id, id);
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
                        emit_points(host, &mut out, def_id, id);
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
                // drag moves the whole selection). On the empty canvas a plain
                // drag sweeps the **marquee** selection; **Shift+drag pans** the
                // enclosing `scroll` workspace — the same convention the heavy
                // views use (Shift pans where a plain drag does the local thing).
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
                } else if !ctx.shift {
                    interact::graph_select(host, def_id, id, Vec::new());
                    self.drag = Some(Drag::Marquee {
                        id,
                        area: rect,
                        scale,
                        origin: (cx, cy),
                        cursor: (cx, cy),
                    });
                    out.push(GestureEffect::Redraw(def_id));
                }
                // else (Shift+empty): leave it unconsumed so the `scroll` pan
                // fallback below grabs the workspace.
            }
            // The free-standing ruler behaves like a lane's ruler strip: a
            // press locates the transport, Shift+drag pans the shared axis.
            // It is the DAW gesture -- you scrub on the ruler.
            WidgetKind::TimeRuler { .. } => {
                let body = frame::ruler_strip_body(rect, host.metrics_for(def_id));
                if ctx.shift {
                    if let Some((start, _len, _total)) = nav(host, id) {
                        self.drag = Some(Drag::Pan {
                            id,
                            origin_x: cx,
                            start,
                            body_w: body.w.max(1.0) as f64,
                        });
                    }
                    return out;
                }
                locate_timeline(host, &mut out, def_id, id, body, cx);
                return out;
            }
            WidgetKind::Track {
                snap, ref editor, ..
            } => {
                // The lane *is* the time axis: its body and its window come off
                // the hit's own chain, so the locate, the pan and the clip grab
                // below all measure against the one the renderer drew.
                let Some((_, axis)) = interact::time_of(&chain) else {
                    return out;
                };
                let (body, lane_nav) = (axis.body, axis.nav);
                let lane = (id, axis);
                // Shift+drag pans the shared axis (the same gesture the heavy
                // views use), so panning stays available where every plain drag
                // grabs a clip.
                if ctx.shift {
                    self.drag = Some(Drag::Pan {
                        id,
                        origin_x: cx,
                        start: lane_nav.start,
                        body_w: body.w.max(1.0) as f64,
                    });
                    return out;
                }
                // A press on the lane's **time ruler**, or on empty lane space,
                // *locates* the transport: the multitrack's cursor goes where you
                // point, which is the one gesture a timeline view cannot do
                // without. (Over a clip, the clip's own gestures win.)
                let ruler_on = editor.ruler != Ruler::Off;
                let on_ruler = ruler_on && cy > body.y as f64 + body.h as f64;
                let over_clip = interact::clip_hit(host, def_id, lane, cx, cy).is_some();
                if on_ruler || (!over_clip && body.contains(cx, cy)) {
                    locate_timeline(host, &mut out, def_id, id, body, cx);
                    return out;
                }
                // A track is the hit target (its clips are placed by the
                // renderer, not the layout engine); find the clip under the
                // cursor and start a move (body) or resize (edge) drag.
                if let Some(h) = interact::clip_hit(host, def_id, lane, cx, cy) {
                    // An automation clip: a break-point wins over the clip body
                    // (as it wins over a segment in the `bpf` view), and Ctrl+click
                    // adds one - or removes the one under the cursor. The same
                    // gestures, now on a lane.
                    if h.point.is_some() || (ctx.ctrl && h.has_curve) {
                        if ctx.ctrl {
                            if interact::clip_point_edit(
                                host, def_id, h.id, h.point, h.rect, h.body, &h.nav, h.offset, cx,
                                cy,
                            ) {
                                emit_points(host, &mut out, def_id, h.id);
                                out.push(GestureEffect::Redraw(def_id));
                            }
                        } else if let Some(index) = h.point {
                            self.drag = Some(Drag::ClipPoint {
                                id: h.id,
                                index,
                                rect: h.rect,
                                body: h.body,
                                nav_start: h.nav.start,
                                nav_len: h.nav.len,
                                offset: h.offset,
                            });
                        }
                        return out;
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
                    return out;
                }
                // A press on a key plays it — inert outside the active range.
                if let Some(p) = piano::hit(&l, cx as f32, cy as f32) {
                    if !(active_min..=active_max).contains(&p) {
                        return out;
                    }
                    let vel = velocity.unwrap_or_else(|| piano::velocity_at(&l, p, cy as f32));
                    piano_note(host, &mut out, def_id, id, p, vel, 1, channel);
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
                let Some((_, axis)) = interact::time_of(&chain) else {
                    return out;
                };
                let Some(h) = interact::pianoroll_hit(host, def_id, (id, rect, axis), cx, cy)
                else {
                    return out;
                };
                // A press on the keyboard gutter (left of the grid) pans the pitch
                // window — the keyboard is the piano-roll's vertical axis surface,
                // the counterpart of the heavy views' y-ruler strip.
                if cx < h.grid.x as f64 {
                    let y_start = host
                        .window_def(def_id)
                        .and_then(|t| t.find(id))
                        .and_then(|w| w.kind.editor())
                        .map_or(0.0, |e| e.y_view().0);
                    self.drag = Some(Drag::PanY {
                        id,
                        origin_y: cy,
                        y_start,
                        lane_h: h.grid.h.max(1.0) as f64,
                    });
                    return out;
                }
                // Shift+drag pans the shared axis (the heavy-view gesture), so
                // panning stays available where a plain drag edits notes/selects.
                if ctx.shift {
                    if let Some((start, _len, _total)) = nav(host, id) {
                        self.drag = Some(Drag::Pan {
                            id,
                            origin_x: cx,
                            start,
                            body_w: h.grid.w.max(1.0) as f64,
                        });
                    }
                    return out;
                }
                self.pianoroll_press(host, &mut out, ctx, id, &h, cx, cy);
            }
            WidgetKind::Waveform { ref editor, .. }
            | WidgetKind::Spectrogram { ref editor, .. } => {
                let body = frame::timeline_body(rect, editor, host.metrics_for(def_id));
                // A press on the y-ruler strip left of the body starts a
                // vertical pan of the display window (the strip is the y
                // axis' gesture surface; wheel over it zooms).
                if editor.ruler_y != RulerY::Off && cx < body.x as f64 {
                    let lanes = ctx.lanes(id, &kind);
                    self.drag = Some(Drag::PanY {
                        id,
                        origin_y: cy,
                        y_start: editor.y_view().0,
                        lane_h: (body.h as f64 / lanes.max(1) as f64).max(1.0),
                    });
                    return out;
                }
                if let Some((start, len, _)) = nav(host, id) {
                    if ctx.shift {
                        // Shift+drag pans the view (the pre-editor gesture).
                        self.drag = Some(Drag::Pan {
                            id,
                            origin_x: cx,
                            start,
                            body_w: body.w.max(1.0) as f64,
                        });
                    } else {
                        // Plain drag selects (the editor convention). The press
                        // collapses the selection to the sample under it.
                        let anchor =
                            interact::sample_at(start, len, body.x as f64, body.w as f64, cx);
                        set_selection(host, &mut out, def_id, id, anchor, anchor);
                        self.drag = Some(Drag::Select {
                            id,
                            body_x: body.x as f64,
                            body_w: body.w.max(1.0) as f64,
                            anchor,
                        });
                        out.push(GestureEffect::Redraw(def_id));
                    }
                }
            }
            _ => {}
        }
        // Nothing consumed the press: inside a `scroll` workspace, grab the
        // plane and pan it (a press on the container's empty area hits the
        // `scroll` itself; one on a non-interactive child falls through here).
        if self.drag.is_none()
            && out.is_empty()
            && let Some((sid, area, view)) = interact::plane_of(&chain)
        {
            self.drag = Some(Drag::ScrollPan {
                id: sid,
                area,
                origin_x: cx,
                origin_y: cy,
                x0: view.view_x,
                y0: view.view_y,
            });
        }
        out
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
                body_x,
                body_w,
                anchor,
            } => {
                let Some((start, len, _)) = nav(host, id) else {
                    return out;
                };
                let cur = interact::sample_at(start, len, body_x, body_w, cx);
                set_selection(host, &mut out, def_id, id, anchor, cur);
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
            Drag::ClipPoint {
                id,
                index,
                rect,
                body,
                nav_start,
                nav_len,
                offset,
            } => {
                // The curve of an automation clip, edited in place: the cursor maps
                // back through the shared axis (time) and the clip's value range,
                // then the point moves with the `bpf` model's own semantics.
                let nav = View {
                    start: nav_start,
                    len: nav_len,
                };
                if interact::clip_point_move(
                    host, def_id, id, index, rect, body, &nav, offset, cx, cy,
                ) {
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
            Drag::SelectNotes {
                id,
                grid,
                nav_start,
                nav_len,
                lo,
                hi,
                anchor,
                anchor_pitch,
            } => {
                // The marquee: the time span keeps driving the shared selection
                // (linked views follow it), and the time × pitch rectangle
                // fills the widget's multi-note selection.
                let cur = interact::sample_at(nav_start, nav_len, grid.x as f64, grid.w as f64, cx);
                set_selection(host, &mut out, def_id, id, anchor, cur);
                let pitch = pianoroll::y_to_pitch(cy as f32, lo, hi, grid);
                interact::pianoroll_state_edit(host, def_id, id, |notes, sel| {
                    *sel = pianoroll::notes_in_rect(notes, anchor, cur, anchor_pitch, pitch);
                });
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
        if let Some(editor) = kind.editor() {
            let factor = 0.85f64.powf(steps);
            // The piano-roll's vertical axis is the keyboard gutter, not a
            // y-ruler strip: wheel over it zooms the pitch window, wheel
            // over the grid zooms the shared time axis.
            if let WidgetKind::PianoRoll {
                osc_lane,
                velocity_lane,
                ..
            } = &kind
            {
                let r = pianoroll::regions(
                    rect,
                    editor.ruler != Ruler::Off,
                    *osc_lane,
                    *velocity_lane,
                    host.metrics_for(def_id),
                );
                if cx < r.grid.x as f64 {
                    let rel = ((cy - r.grid.y as f64) / r.grid.h.max(1.0) as f64).clamp(0.0, 1.0);
                    zoom_timeline_y(host, &mut out, def_id, id, factor, 1.0 - rel);
                } else {
                    zoom_timeline(host, &mut out, def_id, id, r.grid, cx, factor);
                }
                return out;
            }
            // A lane's body is the strip right of its header (and above
            // its ruler); a heavy view's is its rect minus its rulers.
            let body = match kind {
                WidgetKind::Track { .. } => {
                    track::lane_body(rect, editor.ruler != Ruler::Off, host.metrics_for(def_id))
                }
                // The free-standing ruler is all strip: no body to subtract.
                WidgetKind::TimeRuler { .. } => {
                    frame::ruler_strip_body(rect, host.metrics_for(def_id))
                }
                _ => frame::timeline_body(rect, editor, host.metrics_for(def_id)),
            };
            if editor.ruler_y != RulerY::Off && cx < body.x as f64 {
                // Wheel over the y-ruler strip zooms the vertical display
                // window. The anchor depends on what the axis measures,
                // because one window is shared by every channel lane:
                //
                // - **Amplitude** (the waveform): the window keeps its own
                //   centre, so zero stays at the centre of *every* lane and
                //   the trace grows and shrinks inside its lane. An anchor
                //   taken from the cursor's height would be meaningless for
                //   the other lanes, and any off-centre window pushes the
                //   wave out of the lane and clips it.
                // - **Frequency** (the spectrogram): the cursor's height,
                //   which is the frequency under it. There the shared window
                //   says the same thing in every lane — all of them show that
                //   band — so anchoring at the cursor is both meaningful and
                //   what the reader wants.
                let anchor = match kind {
                    WidgetKind::Waveform { .. } => 0.5,
                    _ => {
                        let lanes = ctx.lanes(id, &kind);
                        let lane = frame::lane_rect(body, lanes, frame::lane_at(body, lanes, cy));
                        let rel = ((cy - lane.y as f64) / lane.h.max(1.0) as f64).clamp(0.0, 1.0);
                        1.0 - rel
                    }
                };
                zoom_timeline_y(host, &mut out, def_id, id, factor, anchor);
            } else {
                zoom_timeline(host, &mut out, def_id, id, body, cx, factor);
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
            set_scroll_view(host, &mut out, def_id, id, area, next);
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
                match h.note {
                    // Move (body) or resize (edge) the note under the cursor.
                    // Grabbing the body of a **selected** note moves the whole
                    // selection rigidly; grabbing an unselected one drops the
                    // selection first (the single-note gesture, as before).
                    Some(nh) => {
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
                                        .filter_map(|&i| {
                                            notes.get(i).map(|n| (i, n.start, n.pitch))
                                        })
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default();
                            if !orig.is_empty() {
                                let press_pitch =
                                    pianoroll::y_to_pitch(cy as f32, h.lo, h.hi, h.grid);
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
                    // Empty grid: plain drag selects (the heavy-view
                    // convention), and the marquee doubles as the note
                    // selection — the time span restricted in pitch.
                    None => {
                        if let Some((start, len, _)) = nav_of(host, id) {
                            let anchor = interact::sample_at(
                                start,
                                len,
                                h.grid.x as f64,
                                h.grid.w as f64,
                                cx,
                            );
                            set_selection(host, out, def_id, id, anchor, anchor);
                            let anchor_pitch = pianoroll::y_to_pitch(cy as f32, h.lo, h.hi, h.grid);
                            // The marquee restarts: the previous set drops.
                            interact::pianoroll_state_edit(host, def_id, id, |_, sel| sel.clear());
                            self.drag = Some(Drag::SelectNotes {
                                id,
                                grid: h.grid,
                                nav_start: start,
                                nav_len: len,
                                lo: h.lo,
                                hi: h.hi,
                                anchor,
                                anchor_pitch,
                            });
                            out.push(GestureEffect::Redraw(def_id));
                        }
                    }
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

    // ---- keyboard block operations ----

    /// A key while a `text` field is focused: edit it and, on any content
    /// change, deliver its new string exactly as a numeric control delivers on a
    /// drag — bound → straight to the audio server, else a `/gui_event`, on
    /// **every** keystroke (never gated on Enter). Modifiers ride in `ctx`
    /// (`shift` extends a selection, `ctrl` word-jumps and drives
    /// cut/copy/paste/select-all). Clipboard cut/copy/paste use the host-wide
    /// `clipboard` (the native internal clipboard; the browser front swaps in the
    /// OS clipboard around this call).
    ///
    /// Returns `Some(effects)` when the key was consumed by the focused field
    /// (the front then skips its global editor shortcuts), or `None` when no
    /// text field is focused in this window (the front runs its shortcuts).
    pub fn text_key(
        &self,
        host: &mut Host,
        ctx: &GestureCtx,
        key: TextKey,
        clipboard: &mut String,
    ) -> Option<Vec<GestureEffect>> {
        let def_id = ctx.def_id;
        // Only when a field in *this* window holds the focus.
        let (fdef, id) = host.focused_text()?;
        if fdef != def_id {
            return None;
        }
        let mut out = Vec::new();
        let mut changed = false;
        let edit = |host: &mut Host, f: &mut dyn FnMut(&mut String, &mut super::textedit::Caret, bool) -> bool| {
            interact::text_edit(host, def_id, id, |v, c, ml| f(v, c, ml)).unwrap_or(false)
        };

        match key {
            TextKey::Char(c) if ctx.ctrl => match c.to_ascii_lowercase() {
                'c' => {
                    if let Some(Some(s)) = interact::text_edit(host, def_id, id, |v, c, _| {
                        super::textedit::selected(v, c).map(str::to_string)
                    }) {
                        *clipboard = s;
                    }
                }
                'x' => {
                    let cut = &mut *clipboard;
                    changed = edit(host, &mut |v, c, _| {
                        if let Some(s) = super::textedit::selected(v, c) {
                            *cut = s.to_string();
                            super::textedit::delete_selection(v, c)
                        } else {
                            false
                        }
                    });
                }
                'v' => {
                    let paste = clipboard.clone();
                    if !paste.is_empty() {
                        changed = edit(host, &mut |v, c, ml| {
                            let text = if ml {
                                paste.clone()
                            } else {
                                paste.replace('\n', " ")
                            };
                            super::textedit::insert(v, c, &text)
                        });
                    }
                }
                'a' => {
                    edit(host, &mut |v, c, _| {
                        super::textedit::select_all(v, c);
                        false
                    });
                }
                _ => {} // another Ctrl combo: consumed but inert
            },
            // A plain (or Alt-less) printable char inserts; Alt combos are inert.
            TextKey::Char(c) if !ctx.alt => {
                changed = edit(host, &mut |v, cc, _| {
                    super::textedit::insert(v, cc, c.encode_utf8(&mut [0; 4]))
                });
            }
            TextKey::Char(_) => {}
            TextKey::Backspace => {
                changed = edit(host, &mut |v, c, _| super::textedit::backspace(v, c))
            }
            TextKey::Delete => changed = edit(host, &mut |v, c, _| super::textedit::delete(v, c)),
            TextKey::Left => {
                let word = ctx.ctrl;
                let sel = ctx.shift;
                edit(host, &mut |v, c, _| {
                    if word {
                        super::textedit::move_word_left(v, c, sel);
                    } else {
                        super::textedit::move_left(v, c, sel);
                    }
                    false
                });
            }
            TextKey::Right => {
                let word = ctx.ctrl;
                let sel = ctx.shift;
                edit(host, &mut |v, c, _| {
                    if word {
                        super::textedit::move_word_right(v, c, sel);
                    } else {
                        super::textedit::move_right(v, c, sel);
                    }
                    false
                });
            }
            TextKey::Up => {
                let sel = ctx.shift;
                edit(host, &mut |v, c, _| {
                    super::textedit::move_up(v, c, sel);
                    false
                });
            }
            TextKey::Down => {
                let sel = ctx.shift;
                edit(host, &mut |v, c, _| {
                    super::textedit::move_down(v, c, sel);
                    false
                });
            }
            TextKey::Home => {
                let sel = ctx.shift;
                edit(host, &mut |v, c, _| {
                    super::textedit::move_home(v, c, sel);
                    false
                });
            }
            TextKey::End => {
                let sel = ctx.shift;
                edit(host, &mut |v, c, _| {
                    super::textedit::move_end(v, c, sel);
                    false
                });
            }
            TextKey::Enter => {
                changed = edit(host, &mut |v, c, ml| {
                    if ml {
                        super::textedit::insert(v, c, "\n")
                    } else {
                        false // a single-line field ignores Enter (no send-on-Enter)
                    }
                });
            }
        }

        // The focused field always repaints (the caret/selection moved); a
        // content change also delivers the new value, ungated.
        out.push(GestureEffect::Redraw(def_id));
        if changed {
            emit_value(host, &mut out, def_id, id);
        }
        Some(out)
    }

    /// `q` over a piano-roll: quantize the selected notes' onsets (all of them
    /// when nothing is selected) to the widget's `snap` grid — the same grid a
    /// drag snaps to. Durations are kept; a roll with no grid is left alone.
    /// (The client-side counterpart, in beats over the model, is the Python
    /// `Timeline.quantize` — the standalone host cannot reach it, hence both.)
    pub fn quantize(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        cx: f64,
        cy: f64,
    ) -> Vec<GestureEffect> {
        let mut out = Vec::new();
        let def_id = ctx.def_id;
        let Some(Hit {
            id,
            kind: WidgetKind::PianoRoll { snap, .. },
            ..
        }) = hit(host, ctx, cx, cy)
        else {
            return out;
        };
        let moved = interact::pianoroll_state_edit(host, def_id, id, |notes, sel| {
            pianoroll::quantize_notes(notes, sel, snap)
        })
        .unwrap_or(false);
        if moved {
            host.sync_track_totals();
            emit_notes(host, &mut out, def_id, id);
            out.push(GestureEffect::Redraw(def_id));
        }
        out
    }

    /// Ctrl+C / Ctrl+X over a piano-roll: copy the selected notes to the
    /// host-wide `clipboard`, normalized to the block's first onset (a cut also
    /// removes them) — host-wide so a block travels between rolls and windows.
    /// A no-op when the cursor is elsewhere or nothing is selected.
    pub fn copy_selected(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        cx: f64,
        cy: f64,
        cut: bool,
        clipboard: &mut Vec<pianoroll::Note>,
    ) -> Vec<GestureEffect> {
        let mut out = Vec::new();
        let def_id = ctx.def_id;
        let Some(Hit {
            id,
            kind: WidgetKind::PianoRoll { .. },
            ..
        }) = hit(host, ctx, cx, cy)
        else {
            return out;
        };
        let copied = interact::pianoroll_state_edit(host, def_id, id, |notes, sel| {
            let clip = pianoroll::copy_notes(notes, sel);
            if cut && !clip.is_empty() {
                pianoroll::remove_notes(notes, sel);
                sel.clear();
            }
            clip
        })
        .unwrap_or_default();
        if copied.is_empty() {
            return out;
        }
        *clipboard = copied;
        if cut {
            host.sync_track_totals();
            emit_notes(host, &mut out, def_id, id);
            out.push(GestureEffect::Redraw(def_id));
        }
        out
    }

    /// Ctrl+V over a piano-roll: paste the clipboard with its first onset at
    /// the cursor's time (snapped to the note grid), original pitches and
    /// spread kept. The pasted block becomes the new selection, ready to drag
    /// into place.
    pub fn paste_at_cursor(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        cx: f64,
        cy: f64,
        clipboard: &[pianoroll::Note],
    ) -> Vec<GestureEffect> {
        let mut out = Vec::new();
        let def_id = ctx.def_id;
        if clipboard.is_empty() {
            return out;
        }
        let Some(Hit {
            id,
            rect,
            kind: WidgetKind::PianoRoll { .. },
            chain,
            ..
        }) = hit(host, ctx, cx, cy)
        else {
            return out;
        };
        let Some((_, axis)) = interact::time_of(&chain) else {
            return out;
        };
        let Some(h) = interact::pianoroll_hit(host, def_id, (id, rect, axis), cx, cy) else {
            return out;
        };
        let nav = View {
            start: h.nav.start,
            len: h.nav.len,
        };
        let at = interact::snap(pianoroll::time_at(h.grid, &nav, 0.0, cx as f32), h.snap);
        interact::pianoroll_state_edit(host, def_id, id, |notes, sel| {
            *sel = pianoroll::paste_notes(notes, clipboard, at);
        });
        host.sync_track_totals();
        emit_notes(host, &mut out, def_id, id);
        out.push(GestureEffect::Redraw(def_id));
        out
    }

    /// Delete/Backspace: remove every selected note of the piano-roll under the
    /// cursor — the block delete (Ctrl+click removes one). A no-op when the
    /// cursor is elsewhere or nothing is selected.
    pub fn delete_selected(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        cx: f64,
        cy: f64,
    ) -> Vec<GestureEffect> {
        let mut out = Vec::new();
        let def_id = ctx.def_id;
        let Some(Hit {
            id,
            kind: WidgetKind::PianoRoll { .. },
            ..
        }) = hit(host, ctx, cx, cy)
        else {
            return out;
        };
        let removed = interact::pianoroll_state_edit(host, def_id, id, |notes, sel| {
            if sel.is_empty() {
                return false;
            }
            pianoroll::remove_notes(notes, sel);
            sel.clear();
            true
        })
        .unwrap_or(false);
        if removed {
            host.sync_track_totals();
            emit_notes(host, &mut out, def_id, id);
            out.push(GestureEffect::Redraw(def_id));
        }
        out
    }

    /// `R` over a window: reset every timeline view's navigation (the whole
    /// group, linked members in other windows too) and its vertical axis. The
    /// views are found by walking the window's tree (waveform/spectrogram
    /// widgets), so no front slot list is needed.
    pub fn reset_timelines(&mut self, host: &mut Host, ctx: &GestureCtx) -> Vec<GestureEffect> {
        let mut out = Vec::new();
        let def_id = ctx.def_id;
        let mut ids: Vec<i32> = Vec::new();
        if let Some(tree) = host.window_def(def_id) {
            collect_timeline_ids(tree, &mut ids);
        }
        for id in ids {
            // The whole group resets (linked members in other windows too).
            let roots = host.reset_timeline(id);
            redraw_all(&mut out, &roots);
            emit_view(host, &mut out, def_id, id);
            // The reset also restores the full vertical axis (and reports it).
            set_y_view(host, &mut out, def_id, id, 0.0, 1.0);
        }
        out.push(GestureEffect::Redraw(def_id));
        out
    }
}

// ---- shared helpers (tree queries, delivery, timeline navigation) ----

/// The rectangle spanned by two corner points, whatever their order.
pub(crate) fn corner_rect(a: (f64, f64), b: (f64, f64)) -> Rect {
    let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
    let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
    Rect::new(x0 as f32, y0 as f32, (x1 - x0) as f32, (y1 - y0) as f32)
}

/// The deepest widget under `(x, y)` and the containers over it — the lane
/// counts the vertical axes are panned through coming off the front's context.
fn hit(host: &Host, ctx: &GestureCtx, x: f64, y: f64) -> Option<Hit> {
    interact::hit(host, ctx.def_id, ctx.fb_w, ctx.fb_h, x, y, &|id, kind| {
        ctx.lanes(id, kind)
    })
}

/// A `scroll` widget's **current** view state and configuration. A drag reads
/// it every step: the plane it is panning moves under it, so the chain's
/// press-time snapshot would be one frame stale by the second step.
fn scroll_view(host: &Host, def_id: i32, id: i32) -> Option<ScrollView> {
    match &host.window_def(def_id)?.find(id)?.kind {
        WidgetKind::Scroll { view, .. } => Some(*view),
        _ => None,
    }
}

/// Applies a `scroll` view change (clamped through the shared door) and, when
/// it actually moved, emits the `"view" x y zoom` payload and repaints. Always
/// an event, never a bound forward: the view is view state, exactly as the
/// timeline views' `"view"` and the piano's `"range"`.
fn set_scroll_view(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    id: i32,
    area: Rect,
    next: (f64, f64, f64),
) {
    if let Some((x, y, zoom)) = interact::scroll_set_view(host, def_id, id, area, next) {
        emit(
            out,
            def_id,
            id,
            vec![
                OscType::String("view".into()),
                OscType::Float(x as f32),
                OscType::Float(y as f32),
                OscType::Float(zoom as f32),
            ],
        );
        out.push(GestureEffect::Redraw(def_id));
    }
}

/// The navigation window of timeline view `id`'s group:
/// `(start, len, total)` in timeline samples.
fn nav(host: &Host, id: i32) -> Option<(f64, f64, usize)> {
    host.timeline_nav(id)
        .map(|(nav, total)| (nav.start, nav.len, total))
}

/// Alias of [`nav`] where the local name `nav` is already a `View`.
fn nav_of(host: &Host, id: i32) -> Option<(f64, f64, usize)> {
    nav(host, id)
}

/// A piano-roll note's current `(start, dur)` in the host tree.
fn note_at(host: &Host, def_id: i32, id: i32, index: usize) -> Option<(f64, f64)> {
    match &host.window_def(def_id)?.find(id)?.kind {
        WidgetKind::PianoRoll { notes, .. } => notes.get(index).map(|n| (n.start, n.dur)),
        _ => None,
    }
}

/// The diatonic steps a vertical drag of `dy` pixels means on score `id`, whose
/// page is fitted into `rect`.
fn score_steps(host: &Host, def_id: i32, id: i32, rect: Rect, dy: f64) -> Option<i32> {
    match &host.window_def(def_id)?.find(id)?.kind {
        WidgetKind::Score(data) => Some(data.steps_for(rect, dy as f32)),
        _ => None,
    }
}

/// Appends every timeline (waveform/spectrogram) widget id in the tree.
fn collect_timeline_ids(widget: &Widget, out: &mut Vec<i32>) {
    if let (WidgetKind::Waveform { .. } | WidgetKind::Spectrogram { .. }, Some(id)) =
        (&widget.kind, widget.id)
    {
        out.push(id);
    }
    for child in &widget.children {
        collect_timeline_ids(child, out);
    }
}

/// Emits `/gui_event widget_id <args…>` (as an effect for the front to send).
fn emit(out: &mut Vec<GestureEffect>, def_id: i32, widget_id: i32, args: Vec<OscType>) {
    out.push(GestureEffect::Emit {
        def_id,
        widget_id,
        args,
    });
}

/// Routes a widget's new `value` to the audio server when it is bound
/// (`/gui_bind`, the low-latency path that bypasses the script), or to the
/// script as a `/gui_event` otherwise. Every interaction that produces a
/// value goes through here, so a single binding check covers them all.
fn deliver(host: &Host, out: &mut Vec<GestureEffect>, def_id: i32, widget_id: i32, value: OscType) {
    if host.forward(widget_id, value.clone()) {
        return; // bound: the value went straight to the audio server
    }
    emit(out, def_id, widget_id, vec![value]);
}

/// Delivers a control's current value: straight to the audio server when the
/// widget is bound, otherwise as a `/gui_event` to the script.
fn emit_value(host: &Host, out: &mut Vec<GestureEffect>, def_id: i32, widget_id: i32) {
    if let Some(value) = host.window_def(def_id).and_then(|t| value_of(t, widget_id)) {
        deliver(host, out, def_id, widget_id, value);
    }
}

/// Delivers an edited flat structure — the edit-back pattern: a **bound**
/// widget forwards `args[1..]` (without the leading tag, which names the event
/// payload, not a server argument) straight to the audio server; an unbound one
/// emits the whole tagged list as a `/gui_event`.
fn deliver_args(
    host: &Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    widget_id: i32,
    args: Option<Vec<OscType>>,
) {
    let Some(args) = args else {
        return;
    };
    if host.is_bound(widget_id) {
        host.forward_args(widget_id, args[1..].to_vec());
        return;
    }
    emit(out, def_id, widget_id, args);
}

/// Delivers a `bpf`/automation-clip widget's edited breakpoint list.
fn emit_points(host: &Host, out: &mut Vec<GestureEffect>, def_id: i32, widget_id: i32) {
    let args = host
        .window_def(def_id)
        .and_then(|t| interact::bpf_event_args(t, widget_id));
    deliver_args(host, out, def_id, widget_id, args);
}

/// Delivers a `clip`'s edited placement (`"clip" offset dur`).
fn emit_clip(host: &Host, out: &mut Vec<GestureEffect>, def_id: i32, widget_id: i32) {
    let args = host
        .window_def(def_id)
        .and_then(|t| interact::clip_event_args(t, widget_id));
    deliver_args(host, out, def_id, widget_id, args);
}

/// Plays or releases one `piano` key: updates the held-key view state, drives
/// the host-managed voice when the widget is in voice mode, and delivers the
/// MIDI-shaped `"note" pitch velocity state channel` payload — to the audio
/// server when the piano is bound, to the script as a `/gui_event` otherwise.
#[allow(clippy::too_many_arguments)] // one note event, all scalars
fn piano_note(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    widget_id: i32,
    pitch: i32,
    velocity: i32,
    state: i32,
    channel: i32,
) {
    if state != 0 {
        interact::piano_press_key(host, def_id, widget_id, pitch);
        host.piano_voice_on(def_id, widget_id, pitch, velocity);
    } else {
        interact::piano_release_key(host, def_id, widget_id, pitch);
        host.piano_voice_off(widget_id, pitch);
    }
    deliver_args(
        host,
        out,
        def_id,
        widget_id,
        Some(interact::piano_note_args(pitch, velocity, state, channel)),
    );
}

/// Applies a `piano` range change (pan/zoom) and, when it actually moved,
/// emits the `"range" min max` event and repaints — the `"view"` posture on
/// the keyboard's own MIDI axis.
fn set_piano_range(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    id: i32,
    min: i32,
    max: i32,
) {
    if let Some((min, max)) = interact::piano_set_range(host, def_id, id, min, max) {
        // Always an event, never a bound forward: a binding carries the note
        // payload, the range is view state (the timeline views' "view" posture).
        emit(
            out,
            def_id,
            id,
            vec![
                OscType::String("range".into()),
                OscType::Int(min),
                OscType::Int(max),
            ],
        );
        out.push(GestureEffect::Redraw(def_id));
    }
}

/// Delivers a piano-roll's edited notes (`"notes" start dur pitch vel ch …`).
fn emit_notes(host: &Host, out: &mut Vec<GestureEffect>, def_id: i32, widget_id: i32) {
    let args = host
        .window_def(def_id)
        .and_then(|t| interact::notes_event_args(t, widget_id));
    deliver_args(host, out, def_id, widget_id, args);
}

/// Delivers a piano-roll's edited OSC events (`"osc" time label …`).
fn emit_osc(host: &Host, out: &mut Vec<GestureEffect>, def_id: i32, widget_id: i32) {
    let args = host
        .window_def(def_id)
        .and_then(|t| interact::osc_event_args(t, widget_id));
    deliver_args(host, out, def_id, widget_id, args);
}

/// Repaints every window in `roots` (the windows a group mutation touched).
fn redraw_all(out: &mut Vec<GestureEffect>, roots: &[i32]) {
    for root in roots {
        out.push(GestureEffect::Redraw(*root));
    }
}

/// Writes the selection spanning samples `a..b` (any order, clamped to the
/// timeline) into view `id`'s navigation group — every member follows — and
/// emits **one** `"selection" start len` event, carrying the interacted
/// member's id.
fn set_selection(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    id: i32,
    a: f64,
    b: f64,
) {
    let Some((start, len, roots)) = host.select_timeline(id, a, b) else {
        return;
    };
    redraw_all(out, &roots);
    emit(
        out,
        def_id,
        id,
        vec![
            OscType::String("selection".into()),
            OscType::Float(start as f32),
            OscType::Float(len as f32),
        ],
    );
}

/// Locates the transport: the timeline position under the cursor becomes the
/// group's static cursor (drawn at once on every lane, so the click lands
/// where you see it) and leaves as `/gui_event <id> "locate" <position>` — the
/// script seeks its playhead there, which is what actually moves the music.
fn locate_timeline(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    id: i32,
    body: Rect,
    cx: f64,
) {
    let Some((start, len, _total)) = nav(host, id) else {
        return;
    };
    let pos = interact::sample_at(start, len, body.x as f64, body.w as f64, cx).max(0.0);
    let roots = host.set_timeline_cursor(id, pos);
    emit(
        out,
        def_id,
        id,
        vec![OscType::String("locate".into()), OscType::Float(pos as f32)],
    );
    redraw_all(out, &roots);
    out.push(GestureEffect::Redraw(def_id));
}

fn pan_timeline(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    id: i32,
    start: f64,
    dx_fraction: f64,
) {
    let Some((_, len, _)) = nav(host, id) else {
        return;
    };
    let roots = host.pan_timeline(id, start - dx_fraction * len);
    emit_view(host, out, def_id, id);
    redraw_all(out, &roots);
}

/// Emits a timeline view's visible range as a `/gui_event id "view" start len`
/// — once per gesture step, carrying the interacted member's id (linked
/// members repaint but do not re-emit).
fn emit_view(host: &Host, out: &mut Vec<GestureEffect>, def_id: i32, id: i32) {
    if let Some((start, len, _)) = nav(host, id) {
        emit(
            out,
            def_id,
            id,
            vec![
                OscType::String("view".into()),
                OscType::Float(start as f32),
                OscType::Float(len as f32),
            ],
        );
    }
}

/// One in-flight clip drag, as the placement math needs it: the press-time
/// snapshot plus the lane geometry the cursor maps through.
#[derive(Clone, Copy)]
struct ClipDrag {
    id: i32,
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
}

/// Applies a clip drag at cursor `cx`: maps the cursor to a timeline sample,
/// runs the shared placement math (move/resize against the press snapshot,
/// snapped and clamped), writes it and reports it.
///
/// The cursor maps through the group's **current** window, not the press-time
/// one — that is what lets the edge auto-scroll ([`Gestures::tick`]) carry the
/// clip: panning the view under a held cursor moves the sample beneath it, and
/// the clip follows. `press_sample` is already a timeline coordinate, so it
/// stays fixed while the window moves.
fn apply_clip_drag(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    d: ClipDrag,
    cx: f64,
) {
    let (nav_start, nav_len) = nav(host, d.lane)
        .map(|(start, len, _)| (start, len))
        .unwrap_or((d.nav_start, d.nav_len));
    let sample = interact::sample_at(nav_start, nav_len, d.body_x, d.body_w, cx);
    let (new_offset, new_dur) = interact::clip_drag_placement(
        d.part,
        sample,
        d.press_sample,
        d.orig_offset,
        d.orig_dur,
        d.grid,
    );
    interact::clip_set(host, def_id, d.id, Some(new_offset), Some(new_dur));
    // The lane's extent moved with the clip: re-register it, so the shared axis
    // grows when a clip is dragged past the end — keeping the window's length,
    // so the axis *scrolls* under the drag rather than zooming out from under
    // the cursor (a DAW scrolls at constant zoom; the refit is for content that
    // changes under a still view).
    host.sync_track_totals_keeping_view();
    emit_clip(host, out, def_id, d.id);
    out.push(GestureEffect::Redraw(def_id));
}

/// How near a lane body's edge (device pixels) a held clip drag starts pulling
/// the view along with it.
const EDGE_MARGIN: f64 = 28.0;

/// How much of the visible window one second pinned against the edge scrolls.
/// Deliberately a *fraction of the window* rather than a pixel rate: zoomed in,
/// a clip must still travel at a usable speed, and zoomed out the same gesture
/// must not fly off the composition.
const EDGE_SCROLL_PER_SEC: f64 = 0.9;

/// Writes timeline view `id`'s vertical display window (clamped) into its
/// editor props and emits the `"view_y" y_start y_len` event — the vertical
/// sibling of [`emit_view`]'s range.
fn set_y_view(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    id: i32,
    start: f64,
    len: f64,
) {
    let mut axis = crate::viewport::Axis::normalized(crate::viewport::Unit::Norm);
    axis.set_span(start, len);
    let (start, len) = axis.span();
    if let Some(editor) = host
        .window_def_mut(def_id)
        .and_then(|t| t.find_mut(id))
        .and_then(|w| w.kind.editor_mut())
    {
        (editor.y_start, editor.y_len) = (start, len);
    }
    emit(
        out,
        def_id,
        id,
        vec![
            OscType::String("view_y".into()),
            OscType::Float(start as f32),
            OscType::Float(len as f32),
        ],
    );
    out.push(GestureEffect::Redraw(def_id));
}

/// Anchor-preserving vertical zoom of timeline view `id`: `anchor` in display
/// coordinates (0 = lane bottom, 1 = lane top).
fn zoom_timeline_y(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    id: i32,
    factor: f64,
    anchor: f64,
) {
    let Some((y0, ylen)) = host
        .window_def(def_id)
        .and_then(|t| t.find(id))
        .and_then(|w| w.kind.editor())
        .map(|e| e.y_view())
    else {
        return;
    };
    let mut axis = crate::viewport::Axis::normalized(crate::viewport::Unit::Norm);
    axis.set_span(y0, ylen);
    axis.zoom(factor, anchor);
    let (start, len) = axis.span();
    set_y_view(host, out, def_id, id, start, len);
}

fn zoom_timeline(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    id: i32,
    body: Rect,
    cx: f64,
    factor: f64,
) {
    let anchor = ((cx - body.x as f64) / body.w.max(1.0) as f64).clamp(0.0, 1.0);
    let roots = host.zoom_timeline(id, factor, anchor);
    emit_view(host, out, def_id, id);
    redraw_all(out, &roots);
}

#[cfg(test)]
mod tests {
    use clausters_core::osc::{OscMessage, OscPacket};

    use super::super::metrics::Metrics;
    use super::super::{ClientId, GUI_DEF, GUI_SET};
    use super::*;

    fn from() -> ClientId {
        ClientId::Udp(std::net::SocketAddr::from((
            std::net::Ipv4Addr::LOCALHOST,
            9000,
        )))
    }

    fn host_from(json: &str) -> Host {
        let mut host = Host::new();
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: GUI_DEF.into(),
                args: vec![OscType::Int(1), OscType::String(json.into())],
            }),
            from(),
        );
        host
    }

    /// A live `/gui_set` of one string-valued prop, as a script would send it.
    fn set_prop(host: &mut Host, id: i32, key: &str, value: &str) {
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: GUI_SET.into(),
                args: vec![
                    OscType::Int(id),
                    OscType::String(key.into()),
                    OscType::String(value.into()),
                ],
            }),
            from(),
        );
    }

    fn has_emit_tag(effects: &[GestureEffect], id: i32, tag: &str) -> bool {
        effects.iter().any(|e| match e {
            GestureEffect::Emit {
                widget_id, args, ..
            } => *widget_id == id && args.first() == Some(&OscType::String(tag.into())),
            _ => false,
        })
    }

    fn slider_value(host: &Host, id: i32) -> f32 {
        match &host.window_def(1).unwrap().find(id).unwrap().kind {
            WidgetKind::Slider { range, .. } => range.value,
            other => panic!("not a slider: {other:?}"),
        }
    }

    /// A `scroll` workspace's live view state.
    fn view_of(host: &Host, id: i32) -> ScrollView {
        match &host.window_def(1).unwrap().find(id).unwrap().kind {
            WidgetKind::Scroll { view, .. } => *view,
            other => panic!("not a scroll: {other:?}"),
        }
    }

    /// A window holding one 2D workspace with a 2000x2000 content area.
    fn workspace(extra: &str) -> Host {
        host_from(&format!(
            r#"{{"type":"window","margin":0,"children":[
                {{"id":20,"type":"scroll","margin":0,
                  "content_w":2000,"content_h":2000{extra},
                  "children":[{{"id":21,"type":"label","text":"a",
                                "x":100,"y":100,"w":80,"h":40}}]}}]}}"#
        ))
    }

    /// A window holding one full-area directed patch: `tone` (an outlet)
    /// and `dac` (an inlet and an outlet), a cord tone.out → dac.in.
    fn patch_host() -> Host {
        host_from(
            r#"{"type":"window","margin":0,"children":[
                {"id":7,"type":"patch",
                 "boxes":[{"def":"tone","outlets":["out"]},
                          {"def":"dac","inlets":["in"],"outlets":["out"]}],
                 "cords":[0,0,1,0]}]}"#,
        )
    }

    fn patch_of(host: &Host) -> super::super::patch::PatchDraw {
        match &host.window_def(1).unwrap().find(7).unwrap().kind {
            WidgetKind::Patch { patch, .. } => patch.clone(),
            other => panic!("not a patch: {other:?}"),
        }
    }

    fn selection_of(host: &Host) -> Vec<usize> {
        match &host.window_def(1).unwrap().find(7).unwrap().kind {
            WidgetKind::Patch { selected, .. } => selected.clone(),
            other => panic!("not a patch: {other:?}"),
        }
    }

    #[test]
    fn dragging_a_box_selects_it_moves_it_and_emits_the_move() {
        let mut host = patch_host();
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 600, 400);
        let area = Rect::new(0.0, 0.0, 600.0, 400.0);
        let before = patch_of(&host);
        let b0 = patch::obj_rect(area, &before, 0, 1.0);
        // Grab the box body, clear of the outlet pin at the bottom-centre.
        let (px, py) = ((b0.x + 12.0) as f64, (b0.y + 8.0) as f64);
        let mut grab = || false;
        g.press(&mut host, &ctx, px, py, &mut grab);
        assert_eq!(selection_of(&host), vec![0]);
        g.drag_to(&mut host, &ctx, px + 150.0, py + 80.0);
        let effects = g.release(&mut host, &ctx, px + 150.0, py + 80.0);
        assert!(has_emit_tag(&effects, 7, "move"), "the round trip leaves");
        let after = patch_of(&host);
        // The first drag makes the auto placement explicit, moved by the delta.
        let (x0, y0) = (b0.x - area.x, b0.y - area.y);
        assert_eq!(after.boxes[0].x, Some(x0 + 150.0));
        assert_eq!(after.boxes[0].y, Some(y0 + 80.0));
        // The untouched box keeps its auto placement.
        assert_eq!(after.boxes[1].x, None);
    }

    #[test]
    fn a_plain_drag_marquees_and_shift_pans_leaving_the_selection() {
        let mut host = patch_host();
        let mut g = Gestures::default();
        let plain = GestureCtx::new(1, 600, 400);
        let mut shift = GestureCtx::new(1, 600, 400);
        shift.shift = true;
        let area = Rect::new(0.0, 0.0, 600.0, 400.0);
        let before = patch_of(&host);
        // A plain drag from the empty middle-bottom over the two stacked boxes.
        let b1 = patch::obj_rect(area, &before, 1, 1.0);
        g.press(&mut host, &plain, 300.0, 390.0, &mut || false);
        g.drag_to(&mut host, &plain, (b1.x - 2.0) as f64, 2.0);
        assert_eq!(
            selection_of(&host),
            vec![0, 1],
            "the marquee spans both boxes"
        );
        assert!(g.marquee().is_some(), "the rectangle draws while dragging");
        g.release(&mut host, &plain, (b1.x - 2.0) as f64, 2.0);
        assert!(g.marquee().is_none());
        // Shift+drag on empty canvas pans (the heavy-view convention): it starts
        // no marquee and leaves the selection untouched.
        g.press(&mut host, &shift, 300.0, 390.0, &mut || false);
        g.drag_to(&mut host, &shift, 330.0, 360.0);
        assert!(g.marquee().is_none(), "Shift pans, it does not marquee");
        g.release(&mut host, &shift, 330.0, 360.0);
        assert_eq!(selection_of(&host), vec![0, 1], "Shift+drag does not clear");
        // A plain click on empty canvas (a zero-size marquee) clears the set.
        g.press(&mut host, &plain, 300.0, 390.0, &mut || false);
        g.release(&mut host, &plain, 300.0, 390.0);
        assert!(selection_of(&host).is_empty());
    }

    #[test]
    fn a_cord_drag_from_an_outlet_lands_on_an_inlet() {
        let mut host = patch_host();
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 600, 400);
        let area = Rect::new(0.0, 0.0, 600.0, 400.0);
        let before = patch_of(&host);
        // Grab dac's outlet, drop on... first detach: grab tone's outlet.
        let (px, py) = patch::port_pin(area, &before, 0, patch::Side::Out, 0, 1.0);
        g.press(&mut host, &ctx, px as f64, py as f64, &mut || false);
        assert!(g.wiring().is_some(), "a press on a port starts the cord");
        // Released over dac's inlet: the cord lands, no move is emitted.
        let (ix, iy) = patch::port_pin(area, &before, 1, patch::Side::In, 0, 1.0);
        let effects = g.release(&mut host, &ctx, ix as f64, iy as f64);
        assert!(has_emit_tag(&effects, 7, "wire"));
        assert!(!has_emit_tag(&effects, 7, "move"));
    }

    #[test]
    fn wheel_zooms_the_workspace_anchored_at_the_cursor() {
        let mut host = workspace("");
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 600, 400);
        let (cx, cy) = (300.0, 200.0);
        let before = view_of(&host, 20);
        let m = Metrics::default();
        let content_under_cursor =
            |v: ScrollView| (v.view_x + cx / v.zoom(&m), v.view_y + cy / v.zoom(&m));
        let effects = g.wheel(&mut host, &ctx, cx, cy, 1.0);
        let after = view_of(&host, 20);
        assert!(after.zoom(&m) > before.zoom(&m), "wheel up zooms in");
        let (bx, by) = content_under_cursor(before);
        let (ax, ay) = content_under_cursor(after);
        assert!((bx - ax).abs() < 1e-6 && (by - ay).abs() < 1e-6);
        // View state: always an event (never a bound forward), plus a repaint.
        assert!(has_emit_tag(&effects, 20, "view"));
        assert!(effects.contains(&GestureEffect::Redraw(1)));
    }

    #[test]
    fn dragging_the_empty_plane_pans_both_axes() {
        let mut host = workspace("");
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 600, 400);
        // A press on the container's empty area (away from the child) grabs it.
        g.press(&mut host, &ctx, 500.0, 350.0, &mut || false);
        assert!(g.dragging());
        let effects = g.drag_to(&mut host, &ctx, 450.0, 300.0);
        let v = view_of(&host, 20);
        // The content follows the cursor: dragging left/up moves the view right/down.
        assert_eq!((v.view_x, v.view_y), (50.0, 50.0));
        assert!(has_emit_tag(&effects, 20, "view"));
        g.release(&mut host, &ctx, 450.0, 300.0);
        assert!(!g.dragging());
    }

    #[test]
    fn the_plane_pans_every_direction_from_its_origin() {
        // The regression this fixes: a plane sitting at the content's top-left
        // corner (its default) used to be clamped dead against down/right
        // drags — half the gestures did nothing and it read as broken. The
        // free plane overscrolls, so every direction moves it.
        let mut host = workspace("");
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 600, 400);
        assert_eq!(
            (view_of(&host, 20).view_x, view_of(&host, 20).view_y),
            (0.0, 0.0)
        );
        g.press(&mut host, &ctx, 500.0, 350.0, &mut || false);
        let effects = g.drag_to(&mut host, &ctx, 560.0, 390.0);
        let v = view_of(&host, 20);
        assert_eq!((v.view_x, v.view_y), (-60.0, -40.0), "down/right moves it");
        assert!(has_emit_tag(&effects, 20, "view"));
        // And it stops at half a viewport out, so the content is never lost.
        g.drag_to(&mut host, &ctx, 5000.0, 5000.0);
        let v = view_of(&host, 20);
        assert_eq!((v.view_x, v.view_y), (-300.0, -200.0));
    }

    /// The same anchor, on a plane whose **content follows the zoom**: a graph
    /// sizes its plane to itself-but-never-below-the-viewport, so the visible
    /// content shrinks as the zoom grows. Clamping the new pan against the
    /// content of the *old* zoom slid the plane out from under the cursor —
    /// invisible on a plane with an explicit `content_w`, which is why the test
    /// above did not catch it.
    #[test]
    fn wheel_zoom_over_a_graph_sized_plane_holds_the_cursor_too() {
        let mut host = host_from(
            r#"{"type":"window","margin":0,"children":[
                {"id":20,"type":"scroll","margin":0,"children":[
                  {"id":7,"type":"patch","boxes":[
                    {"def":"tone","outlets":["out"]},
                    {"def":"dac","inlets":["in"],"outlets":["out"]}],
                   "cords":[0,0,1,0]}]}]}"#,
        );
        let m = Metrics::default();
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 600, 400);
        let (cx, cy) = (420.0, 260.0);
        let area = crate::host::layout::Rect::new(0.0, 0.0, 600.0, 400.0);
        // Where the graph itself sits on screen — the thing an eye tracks, and
        // the thing the content extent moves when it follows the zoom.
        let graph = |host: &Host| {
            crate::host::layout::layout(area, host.window_def(1).unwrap(), &m)
                .into_iter()
                .find(|p| p.widget.id == Some(7))
                .map(|p| (p.rect.x + p.rect.w * 0.5, p.rect.y + p.rect.h * 0.5))
                .expect("the patch is placed")
        };
        let before = view_of(&host, 20);
        let (bx, by) = graph(&host);
        g.wheel(&mut host, &ctx, cx, cy, 1.0);
        let after = view_of(&host, 20);
        let factor = (after.zoom(&m) / before.zoom(&m)) as f32;
        assert!(factor > 1.0, "wheel up zooms in");
        let (ax, ay) = graph(&host);
        // A zoom about the cursor maps every pixel p to cursor + (p - cursor) * f.
        let (wx, wy) = (
            cx as f32 + (bx - cx as f32) * factor,
            cy as f32 + (by - cy as f32) * factor,
        );
        assert!(
            (ax - wx).abs() < 0.5 && (ay - wy).abs() < 0.5,
            "the graph slid under the cursor: expected {wx},{wy}, got {ax},{ay}"
        );
    }

    #[test]
    fn a_vertical_scroll_view_is_the_workspace_constrained_by_configuration() {
        // `axis: "y"` with `zoom: 0` *is* a plain vertical scroll view: the
        // wheel scrolls, x never moves, the zoom stays put.
        let mut host = workspace(r#","axis":"y","zoom":0"#);
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 600, 400);
        g.wheel(&mut host, &ctx, 300.0, 200.0, -1.0);
        let v = view_of(&host, 20);
        assert_eq!(
            v.zoom(&Metrics::default()),
            1.0,
            "zoom disabled: the wheel does not scale"
        );
        assert_eq!(v.view_x, 0.0, "the x axis is not pannable");
        assert_eq!(v.view_y, scroll::WHEEL_PAN_PX, "the wheel scrolls down");
        // A drag on the plane likewise moves only y.
        g.press(&mut host, &ctx, 500.0, 350.0, &mut || false);
        g.drag_to(&mut host, &ctx, 400.0, 300.0);
        let v = view_of(&host, 20);
        assert_eq!(v.view_x, 0.0);
        assert_eq!(v.view_y, scroll::WHEEL_PAN_PX + 50.0);
    }

    #[test]
    fn a_horizontal_strip_pans_only_x_and_clamps_to_the_content() {
        let mut host = workspace(r#","axis":"x","zoom":0"#);
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 600, 400);
        // The wheel drives the single axis; far past the end it clamps to
        // content - visible = 2000 - 600.
        for _ in 0..100 {
            g.wheel(&mut host, &ctx, 300.0, 200.0, -1.0);
        }
        let v = view_of(&host, 20);
        assert_eq!(v.view_x, 1400.0);
        assert_eq!(v.view_y, 0.0);
    }

    #[test]
    fn a_widget_inside_the_workspace_still_takes_the_press() {
        let mut host = host_from(
            r#"{"type":"window","margin":0,"children":[
                {"id":20,"type":"scroll","margin":0,"content_w":2000,"content_h":2000,
                 "children":[{"id":21,"type":"toggle","value":0,
                              "x":0,"y":0,"w":100,"h":50}]}]}"#,
        );
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 600, 400);
        // Over the toggle: the widget wins, no pan drag starts.
        let effects = g.press(&mut host, &ctx, 50.0, 25.0, &mut || false);
        assert!(!g.dragging(), "the toggle consumed the press");
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, GestureEffect::Emit { widget_id: 21, .. }))
        );
        // Scrolled out of view, the same widget is no longer hit: the press
        // falls through to the plane.
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: super::super::GUI_SET.into(),
                args: vec![
                    OscType::Int(20),
                    OscType::String("view_x".into()),
                    OscType::Float(500.0),
                ],
            }),
            from(),
        );
        g.press(&mut host, &ctx, 50.0, 25.0, &mut || false);
        assert!(g.dragging(), "the scrolled-away widget is not hit");
    }

    #[test]
    fn slider_press_and_drag_set_the_value_and_emit() {
        let mut host = host_from(
            r#"{"type":"window","children":[
                {"id":10,"type":"slider","min":0.0,"max":10.0,"value":2.5}]}"#,
        );
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 400, 100);
        // The slider is natural-thick: a strip under the window's margin, not
        // the whole pane, so the press aims inside it.
        let effects = g.press(&mut host, &ctx, 200.0, 25.0, &mut || false);
        assert!(g.dragging());
        let after_press = slider_value(&host, 10);
        assert!(after_press > 2.5, "press near the middle raises 2.5");
        // Unbound: the new value leaves as a /gui_event carrying one float.
        assert!(effects.iter().any(|e| matches!(
            e,
            GestureEffect::Emit { widget_id: 10, args, .. } if args.len() == 1
        )));
        assert!(effects.contains(&GestureEffect::Redraw(1)));
        // Dragging to the far right pins the value at max.
        g.drag_to(&mut host, &ctx, 399.0, 25.0);
        assert_eq!(slider_value(&host, 10), 10.0);
        assert!(g.release(&mut host, &ctx, 399.0, 25.0).is_empty());
        assert!(!g.dragging());
    }

    #[test]
    fn button_press_emits_one_and_release_emits_zero() {
        let mut host = host_from(r#"{"type":"window","children":[{"id":20,"type":"button"}]}"#);
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 200, 100);
        // A button is one control line tall (its natural height).
        let effects = g.press(&mut host, &ctx, 100.0, 16.0, &mut || false);
        assert_eq!(g.active_button(), Some(20));
        assert!(effects.iter().any(|e| matches!(
            e,
            GestureEffect::Emit { widget_id: 20, args, .. } if args == &[OscType::Int(1)]
        )));
        let effects = g.release(&mut host, &ctx, 100.0, 16.0);
        assert!(effects.iter().any(|e| matches!(
            e,
            GestureEffect::Emit { widget_id: 20, args, .. } if args == &[OscType::Int(0)]
        )));
        assert_eq!(g.active_button(), None);
    }

    #[test]
    fn toggle_press_flips_the_state() {
        let mut host =
            host_from(r#"{"type":"window","children":[{"id":30,"type":"toggle","value":0}]}"#);
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 200, 100);
        let effects = g.press(&mut host, &ctx, 100.0, 16.0, &mut || false);
        match &host.window_def(1).unwrap().find(30).unwrap().kind {
            WidgetKind::Toggle { value, .. } => assert!(*value),
            other => panic!("not a toggle: {other:?}"),
        }
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, GestureEffect::Emit { widget_id: 30, .. }))
        );
        assert!(!g.dragging()); // a toggle is a click, not a drag
    }

    #[test]
    fn knob_press_records_the_grab_result_and_locked_ignores_cursor_motion() {
        let mut host = host_from(
            r#"{"type":"window","children":[
                {"id":40,"type":"knob","min":0.0,"max":1.0,"value":0.5}]}"#,
        );
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 200, 200);
        // A knob is as tall as its disc plus its read-out (its natural height),
        // so it is a strip at the top of the window: the press aims inside it.
        g.press(&mut host, &ctx, 22.0, 30.0, &mut || true);
        assert!(g.locked());
        // Locked: cursor motion is ignored (relative deltas drive it instead).
        let effects = g.drag_to(&mut host, &ctx, 22.0, 80.0);
        assert!(effects.is_empty());
        let effects = g.relative_motion(&mut host, &ctx, -20.0);
        assert!(effects.contains(&GestureEffect::Redraw(1)));
        // Release asks the front to drop the pointer grab.
        let effects = g.release(&mut host, &ctx, 22.0, 80.0);
        assert!(effects.contains(&GestureEffect::ReleasePointer(1)));
    }

    #[test]
    fn waveform_press_and_drag_select_a_range() {
        let mut host = host_from(
            r#"{"type":"window","children":[
                {"id":50,"type":"waveform","data":[0.0,0.5,-0.5,1.0],"base_bucket":2}]}"#,
        );
        host.set_timeline_total(50, 1000);
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 800, 300);
        // Press inside the view body (right of the y-ruler strip), then drag.
        let effects = g.press(&mut host, &ctx, 400.0, 150.0, &mut || false);
        assert!(has_emit_tag(&effects, 50, "selection"));
        let effects = g.drag_to(&mut host, &ctx, 600.0, 150.0);
        assert!(has_emit_tag(&effects, 50, "selection"));
        // The selection landed in the widget's navigation group — where every
        // reader of it looks — with a positive length.
        let key = host.timeline_key(50).unwrap();
        assert!(host.timelines().state(key).unwrap().sel_len > 0.0);
    }

    #[test]
    fn wheel_zooms_the_time_axis_and_emits_the_view() {
        let mut host = host_from(
            r#"{"type":"window","children":[
                {"id":60,"type":"waveform","data":[0.0,0.5,-0.5,1.0],"base_bucket":2}]}"#,
        );
        host.set_timeline_total(60, 1000);
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 800, 300);
        let before = host.timeline_nav(60).unwrap().0.len;
        let effects = g.wheel(&mut host, &ctx, 400.0, 150.0, 1.0);
        let after = host.timeline_nav(60).unwrap().0.len;
        assert!(after < before, "wheel-in shrinks the visible window");
        assert!(has_emit_tag(&effects, 60, "view"));
    }

    /// The amplitude axis zooms symmetrically: whatever lane the cursor is
    /// over, the window keeps its centre — so every channel's zero line stays
    /// at its lane's centre instead of sliding out of the lane. The regression
    /// this fixes: the anchor used to be the cursor's height within its lane,
    /// which is meaningless for the *other* lanes of one shared window — a
    /// wheel near the top of channel 2 pushed every channel's wave to the
    /// bottom of its lane, clipped.
    #[test]
    fn the_amplitude_axis_zooms_about_its_centre_whatever_lane_is_under_the_cursor() {
        let mut host = host_from(
            r#"{"type":"window","children":[
                {"id":61,"type":"waveform","data":[0.0,0.5,-0.5,1.0],"base_bucket":2}]}"#,
        );
        host.set_timeline_total(61, 1000);
        let mut g = Gestures::default();
        let mut ctx = GestureCtx::new(1, 800, 300);
        // Four channels: the body splits into four lanes.
        ctx.wave_lanes.insert(61, 4);
        // Wheel over the y-ruler strip (left of the body), high inside the
        // *last* lane — the worst case for a cursor-derived anchor.
        let effects = g.wheel(&mut host, &ctx, 10.0, 212.0, 4.0);
        assert!(has_emit_tag(&effects, 61, "view_y"));
        let (start, len) = host
            .window_def(1)
            .unwrap()
            .find(61)
            .unwrap()
            .kind
            .editor()
            .unwrap()
            .y_view();
        assert!(len < 1.0, "wheel-in shrinks the amplitude window");
        // Zero (display 0.5) sits at the centre of the window, so it lands at
        // the centre of every lane.
        assert!(
            (start + len / 2.0 - 0.5).abs() < 1e-9,
            "the window stays centred on zero: got ({start}, {len})"
        );
    }

    // --- multitrack lanes: the edge auto-scroll ---

    /// One lane, one short clip, on a long axis — so zooming in leaves most of
    /// the timeline off screen, which is the case the edge scroll exists for.
    fn lane_host() -> Host {
        host_from(
            r#"{"type":"window","margin":0,"children":[
                {"id":70,"type":"track","label":"lane","children":[
                    {"id":71,"type":"clip","offset":0.0,"dur":1000.0},
                    {"id":72,"type":"clip","offset":9000.0,"dur":1000.0}
                ]}]}"#,
        )
    }

    fn clip_offset(host: &Host, id: i32) -> f64 {
        match &host.window_def(1).unwrap().find(id).unwrap().kind {
            WidgetKind::Clip { offset, .. } => *offset,
            other => panic!("not a clip: {other:?}"),
        }
    }

    /// A clip dragged against the lane's edge pulls the view along, so it can
    /// travel further than the visible window. The regression this fixes: the
    /// drag mapped the cursor through the *press-time* window and nothing
    /// scrolled, so a clip could never move more than one window's worth —
    /// zoomed in, that was a sliver, and holding at the edge did nothing at all.
    #[test]
    fn a_clip_dragged_to_the_edge_pulls_the_view_along() {
        let mut host = lane_host();
        host.sync_track_totals();
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 800, 200);

        // Zoom in hard, anchored at the left: the window shows a fraction of
        // the timeline, and the first clip sits at its left end.
        for _ in 0..6 {
            host.zoom_timeline(70, 0.6, 0.0);
        }
        let (before, _) = host.timeline_nav(70).unwrap();
        assert!(before.len < 2000.0, "zoomed in: {}", before.len);

        // Grab the clip and drag it hard against the right edge.
        // (past the lane's 96 px header strip, so the press lands on the clip)
        g.press(&mut host, &ctx, 300.0, 100.0, &mut || false);
        assert!(g.dragging(), "the press grabbed the clip");
        g.drag_to(&mut host, &ctx, 790.0, 100.0);
        let parked = clip_offset(&host, 71);
        assert!(parked > 0.0, "the drag moved it: {parked}");
        assert!(g.edge_scrolling(790.0), "and it is pinned at the edge");
        let (at_edge, _) = host.timeline_nav(70).unwrap();
        assert_eq!(at_edge.start, before.start, "the drag alone does not pan");

        // Now hold there: every tick pans the view and carries the clip.
        let mut effects = Vec::new();
        for _ in 0..10 {
            effects = g.tick(&mut host, &ctx, 790.0, 1.0 / 30.0);
        }
        let (after, _) = host.timeline_nav(70).unwrap();
        assert!(
            after.start > at_edge.start,
            "the window followed the drag: {} -> {}",
            at_edge.start,
            after.start
        );
        assert!(
            clip_offset(&host, 71) > parked,
            "and the clip travelled with it: {parked} -> {}",
            clip_offset(&host, 71)
        );
        // The move keeps reporting itself, and the view move is reported too.
        assert!(has_emit_tag(&effects, 71, "clip"));
        // The view move is the *lane's* — the group member, not the clip.
        assert!(has_emit_tag(&effects, 70, "view"));

        // A cursor clear of the margins scrolls nothing.
        let (held, _) = host.timeline_nav(70).unwrap();
        let idle = g.tick(&mut host, &ctx, 400.0, 1.0 / 30.0);
        assert_eq!(host.timeline_nav(70).unwrap().0.start, held.start);
        assert!(idle.is_empty());

        // And the scroll stops with the drag.
        g.release(&mut host, &ctx, 790.0, 100.0);
        let (dropped, _) = host.timeline_nav(70).unwrap();
        g.tick(&mut host, &ctx, 790.0, 1.0 / 30.0);
        assert_eq!(host.timeline_nav(70).unwrap().0.start, dropped.start);
    }

    /// Dragging keeps the zoom and scrolls, **from the untouched view too**.
    /// The regression this fixes: a window showing exactly the whole timeline
    /// was refitted to the new total on every drag step, so extending the
    /// content zoomed the axis out from under the cursor instead of scrolling —
    /// and the edge scroll's pan was overwritten as fast as it was applied. It
    /// only appeared to work once the zoom had been changed at least once,
    /// which is what took the window off the exact-full case.
    #[test]
    fn dragging_from_the_full_view_scrolls_instead_of_zooming_out() {
        let mut host = lane_host();
        host.sync_track_totals();
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 800, 200);
        // Untouched: the window shows the whole timeline, exactly.
        let (before, total) = host.timeline_nav(70).unwrap();
        assert_eq!(before.len, total as f64, "showing it all, never zoomed");

        // Drag the far clip past the end and hold at the edge. It spans
        // 9000..10000 of a 10000-sample axis drawn over the body (96..800), so
        // it occupies roughly the last 70 px.
        g.press(&mut host, &ctx, 760.0, 100.0, &mut || false);
        assert!(g.dragging(), "the press grabbed the far clip");
        g.drag_to(&mut host, &ctx, 790.0, 100.0);
        for _ in 0..20 {
            g.tick(&mut host, &ctx, 790.0, 1.0 / 30.0);
        }
        let (after, grown) = host.timeline_nav(70).unwrap();
        assert!(grown > total, "the content grew with the clip");
        assert!(
            (after.len - before.len).abs() < 1.0,
            "the zoom held: {} -> {}",
            before.len,
            after.len
        );
        assert!(
            after.start > before.start,
            "and the axis scrolled: {} -> {}",
            before.start,
            after.start
        );
        g.release(&mut host, &ctx, 790.0, 100.0);
    }

    /// The left edge scrolls the other way, and never past the axis origin.
    #[test]
    fn the_left_edge_scrolls_back_and_stops_at_the_origin() {
        let mut host = lane_host();
        host.sync_track_totals();
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 800, 200);
        for _ in 0..6 {
            host.zoom_timeline(70, 0.6, 0.0);
        }
        // Start from a window well into the timeline, holding the far clip.
        host.pan_timeline(70, 9000.0);
        let (before, _) = host.timeline_nav(70).unwrap();
        assert!(before.start > 0.0);
        g.press(&mut host, &ctx, 400.0, 100.0, &mut || false);
        g.drag_to(&mut host, &ctx, 10.0, 100.0);
        for _ in 0..10 {
            g.tick(&mut host, &ctx, 10.0, 1.0 / 30.0);
        }
        let (after, _) = host.timeline_nav(70).unwrap();
        assert!(after.start < before.start, "the window walked back");
        // Keep holding: it parks at the origin instead of running negative.
        for _ in 0..2000 {
            g.tick(&mut host, &ctx, 10.0, 1.0 / 30.0);
        }
        assert_eq!(host.timeline_nav(70).unwrap().0.start, 0.0);
        assert!(
            clip_offset(&host, 72) >= 0.0,
            "and the clip never goes negative"
        );
    }

    // --- score ---

    /// A one-score window, the page fitted 1:1 into the child rect: a window of
    /// 1012x412 gives the child (6,6,1000,400), matching the 1000x400 viewBox.
    fn score_host() -> Host {
        // Editable, so the drag tests exercise the transpose gesture; the
        // read-only default is covered by its own test below.
        host_from(
            r#"{"type":"window","children":[
                {"id":80,"type":"score","vb":[1000,400],"editable":true,
                 "glyphs":{"E0A4":"M0 -39c0 68 73 172 200 172c66 0 114 -37 114 -95c0 -84 -106 -171 -218 -171c-58 0 -96 34 -96 93Z"},
                 "prims":[{"k":"line","pts":[[0,200],[1000,200]],"w":13,"id":"staff"},
                          {"k":"glyph","cp":"E0A4","xf":[500,200,0.72,-0.72],"id":"n1"}]}]}"#,
        )
    }

    fn score_selected(host: &Host) -> Option<String> {
        match &host.window_def(1).unwrap().find(80).unwrap().kind {
            WidgetKind::Score(data) => data.selected.clone(),
            other => panic!("not a score: {other:?}"),
        }
    }

    fn element_emits(effects: &[GestureEffect]) -> Vec<String> {
        effects
            .iter()
            .filter_map(|e| match e {
                GestureEffect::Emit { args, .. }
                    if args.first() == Some(&OscType::String("element".into())) =>
                {
                    match &args[1..] {
                        [OscType::String(s)] => Some(s.clone()),
                        _ => panic!("malformed element payload: {args:?}"),
                    }
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_press_on_the_score_selects_the_element_and_emits_its_id() {
        let mut host = score_host();
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 1012, 412);
        // the notehead sits at page (500, 200) -> child rect origin (6, 6)
        let effects = g.press(&mut host, &ctx, 556.0, 196.0, &mut || false);
        assert_eq!(element_emits(&effects), vec!["n1".to_string()]);
        assert_eq!(score_selected(&host).as_deref(), Some("n1"));
        // pressing the same element again changes nothing: no event, no repaint
        let again = g.press(&mut host, &ctx, 556.0, 196.0, &mut || false);
        assert!(again.is_empty(), "a re-press on the selection is inert");
        // blank paper clears it, reported as an empty id
        let cleared = g.press(&mut host, &ctx, 106.0, 386.0, &mut || false);
        assert_eq!(element_emits(&cleared), vec![String::new()]);
        assert_eq!(score_selected(&host), None);
    }

    fn score_drag_preview(host: &Host) -> Option<(String, i32)> {
        match &host.window_def(1).unwrap().find(80).unwrap().kind {
            WidgetKind::Score(data) => data.drag.as_ref().map(|d| (d.id.clone(), d.steps)),
            other => panic!("not a score: {other:?}"),
        }
    }

    fn transpose_emits(effects: &[GestureEffect]) -> Vec<(String, i32)> {
        effects
            .iter()
            .filter_map(|e| match e {
                GestureEffect::Emit { args, .. }
                    if args.first() == Some(&OscType::String("transpose".into())) =>
                {
                    match &args[1..] {
                        [OscType::String(s), OscType::Int(n)] => Some((s.clone(), *n)),
                        _ => panic!("malformed transpose payload: {args:?}"),
                    }
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn dragging_a_note_up_the_staff_transposes_it_in_diatonic_steps() {
        let mut host = score_host();
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 1012, 412);
        // grab the notehead at page (500, 200); the page is fitted 1:1, so a
        // diatonic step is the default 90 page units = 90 px
        g.press(&mut host, &ctx, 556.0, 196.0, &mut || false);
        // short of a step the page does not move
        g.drag_to(&mut host, &ctx, 556.0, 156.0);
        assert_eq!(score_drag_preview(&host), None);
        // two steps up: drawn displaced while the drag lasts
        g.drag_to(&mut host, &ctx, 556.0, 16.0);
        assert_eq!(score_drag_preview(&host), Some(("n1".into(), 2)));
        // the release asks the client for the edit, in steps
        let effects = g.release(&mut host, &ctx, 556.0, 16.0);
        assert_eq!(transpose_emits(&effects), vec![("n1".to_string(), 2)]);
        // and the displacement stands until the re-engraved page arrives
        assert_eq!(score_drag_preview(&host), Some(("n1".into(), 2)));
        set_prop(&mut host, 80, "display_list", r#"{"vb":[1000,400]}"#);
        assert_eq!(score_drag_preview(&host), None);
    }

    #[test]
    fn a_press_that_does_not_move_the_note_stays_a_selection() {
        let mut host = score_host();
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 1012, 412);
        g.press(&mut host, &ctx, 556.0, 196.0, &mut || false);
        // wandering back and forth within one step is not an edit
        g.drag_to(&mut host, &ctx, 556.0, 240.0);
        let effects = g.release(&mut host, &ctx, 556.0, 240.0);
        assert!(transpose_emits(&effects).is_empty(), "no step, no edit");
        assert_eq!(score_drag_preview(&host), None);
        assert_eq!(score_selected(&host).as_deref(), Some("n1"));
    }

    #[test]
    fn a_read_only_score_selects_but_a_drag_does_not_transpose() {
        // The same host without `editable`: a press still selects and reports
        // the element (inspecting a page is not editing it), but dragging it a
        // full step neither previews nor emits a transpose.
        let mut host = host_from(
            r#"{"type":"window","children":[
                {"id":80,"type":"score","vb":[1000,400],
                 "glyphs":{"E0A4":"M0 -39c0 68 73 172 200 172c66 0 114 -37 114 -95c0 -84 -106 -171 -218 -171c-58 0 -96 34 -96 93Z"},
                 "prims":[{"k":"line","pts":[[0,200],[1000,200]],"w":13,"id":"staff"},
                          {"k":"glyph","cp":"E0A4","xf":[500,200,0.72,-0.72],"id":"n1"}]}]}"#,
        );
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 1012, 412);
        let picked = g.press(&mut host, &ctx, 556.0, 196.0, &mut || false);
        assert_eq!(element_emits(&picked), vec!["n1".to_string()]);
        assert_eq!(score_selected(&host).as_deref(), Some("n1"));
        // two full steps up: no preview while dragging, no transpose on release
        g.drag_to(&mut host, &ctx, 556.0, 16.0);
        assert_eq!(score_drag_preview(&host), None);
        let effects = g.release(&mut host, &ctx, 556.0, 16.0);
        assert!(transpose_emits(&effects).is_empty(), "read-only: no edit");
    }

    // --- piano ---

    /// A one-piano window (no overview, no label: the keys fill the widget
    /// rect), plus the layout the gestures see. The window is 712x132 so the
    /// child rect is (6,6,700,120): one octave C4..C5 = 8 white keys.
    fn piano_host(extra: &str) -> (Host, piano::Layout) {
        let json = format!(
            r#"{{"type":"window","children":[
                {{"id":70,"type":"piano","min":60,"max":72,"overview":0{extra}}}]}}"#
        );
        let host = host_from(&json);
        let l = piano::layout(
            Rect::new(6.0, 6.0, 700.0, 120.0),
            60,
            72,
            false,
            false,
            &Metrics::default(),
        );
        (host, l)
    }

    fn note_emits(effects: &[GestureEffect]) -> Vec<(i32, i32, i32, i32)> {
        effects
            .iter()
            .filter_map(|e| match e {
                GestureEffect::Emit { args, .. }
                    if args.first() == Some(&OscType::String("note".into())) =>
                {
                    match args[1..] {
                        [
                            OscType::Int(p),
                            OscType::Int(v),
                            OscType::Int(s),
                            OscType::Int(c),
                        ] => Some((p, v, s, c)),
                        _ => panic!("malformed note payload: {args:?}"),
                    }
                }
                _ => None,
            })
            .collect()
    }

    fn piano_pressed(host: &Host) -> Vec<i32> {
        match &host.window_def(1).unwrap().find(70).unwrap().kind {
            WidgetKind::Piano { pressed, .. } => pressed.clone(),
            other => panic!("not a piano: {other:?}"),
        }
    }

    #[test]
    fn piano_press_glissando_and_release_emit_midi_shaped_notes() {
        let (mut host, l) = piano_host(r#","channel":2"#);
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 712, 132);
        // Press the front of C4: note-on, high velocity, channel carried.
        let c = piano::key_rect(&l, 60).unwrap();
        let effects = g.press(
            &mut host,
            &ctx,
            (c.x + c.w * 0.5) as f64,
            (c.y + c.h - 1.0) as f64,
            &mut || false,
        );
        let notes = note_emits(&effects);
        assert_eq!(notes.len(), 1);
        let (p, v, s, ch) = notes[0];
        assert_eq!((p, s, ch), (60, 1, 2));
        assert!(v > 120, "front-of-key press is loud, got {v}");
        assert_eq!(piano_pressed(&host), vec![60]);
        // Glissando onto D4: off 60, on 62 (the new key's own velocity).
        let d = piano::key_rect(&l, 62).unwrap();
        let effects = g.drag_to(
            &mut host,
            &ctx,
            (d.x + d.w * 0.5) as f64,
            (d.y + d.h * 0.5) as f64,
        );
        let notes = note_emits(&effects);
        assert_eq!(notes.len(), 2);
        assert_eq!((notes[0].0, notes[0].2), (60, 0));
        assert_eq!((notes[1].0, notes[1].2), (62, 1));
        assert_eq!(piano_pressed(&host), vec![62]);
        // Release: note-off of the held key.
        let effects = g.release(&mut host, &ctx, d.x as f64, d.y as f64);
        let notes = note_emits(&effects);
        assert_eq!(notes.len(), 1);
        assert_eq!((notes[0].0, notes[0].2), (62, 0));
        assert!(piano_pressed(&host).is_empty());
    }

    #[test]
    fn piano_glissando_across_two_keys_releases_each_left_key() {
        // A glissando spanning more than one crossing: every key left behind
        // must get its note-off, and the final release offs the last key only.
        let (mut host, l) = piano_host("");
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 712, 132);
        let c = piano::key_rect(&l, 60).unwrap();
        g.press(
            &mut host,
            &ctx,
            (c.x + c.w * 0.5) as f64,
            (c.y + c.h - 1.0) as f64,
            &mut || false,
        );
        let d = piano::key_rect(&l, 62).unwrap();
        g.drag_to(
            &mut host,
            &ctx,
            (d.x + d.w * 0.5) as f64,
            (d.y + d.h * 0.5) as f64,
        );
        let e = piano::key_rect(&l, 64).unwrap();
        let effects = g.drag_to(
            &mut host,
            &ctx,
            (e.x + e.w * 0.5) as f64,
            (e.y + e.h * 0.5) as f64,
        );
        let notes = note_emits(&effects);
        assert_eq!(notes.len(), 2, "second crossing: one off, one on");
        assert_eq!((notes[0].0, notes[0].2), (62, 0), "the key left is 62");
        assert_eq!((notes[1].0, notes[1].2), (64, 1));
        assert_eq!(piano_pressed(&host), vec![64]);
        let effects = g.release(&mut host, &ctx, e.x as f64, e.y as f64);
        let notes = note_emits(&effects);
        assert_eq!(notes.len(), 1);
        assert_eq!((notes[0].0, notes[0].2), (64, 0));
        assert!(piano_pressed(&host).is_empty());
    }

    #[test]
    fn piano_fixed_velocity_and_grayed_keys() {
        // A fixed velocity overrides the press-height map.
        let (mut host, l) = piano_host(r#","velocity":90"#);
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 712, 132);
        let c = piano::key_rect(&l, 60).unwrap();
        let effects = g.press(
            &mut host,
            &ctx,
            (c.x + 2.0) as f64,
            (c.y + c.h - 1.0) as f64,
            &mut || false,
        );
        assert_eq!(note_emits(&effects)[0].1, 90);
        g.release(&mut host, &ctx, c.x as f64, c.y as f64);
        // A press outside the active range is inert: no event, no held key.
        let (mut host, _) = piano_host(r#","active_min":64,"active_max":72"#);
        let mut g = Gestures::default();
        let effects = g.press(
            &mut host,
            &ctx,
            (c.x + 2.0) as f64,
            (c.y + c.h - 1.0) as f64,
            &mut || false,
        );
        assert!(note_emits(&effects).is_empty());
        assert!(!g.dragging());
        assert!(piano_pressed(&host).is_empty());
    }

    #[test]
    fn piano_wheel_pans_the_range_and_pan_zero_freezes_it() {
        let (mut host, l) = piano_host("");
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 712, 132);
        let c = piano::key_rect(&l, 60).unwrap();
        let (cx, cy) = ((c.x + c.w * 0.5) as f64, (c.y + c.h - 1.0) as f64);
        let effects = g.wheel(&mut host, &ctx, cx, cy, 1.0);
        assert!(has_emit_tag(&effects, 70, "range"));
        match &host.window_def(1).unwrap().find(70).unwrap().kind {
            WidgetKind::Piano { min, max, .. } => assert_eq!((*min, *max), (62, 74)),
            other => panic!("not a piano: {other:?}"),
        }
        // `pan: 0` silences every range gesture.
        let (mut host, _) = piano_host(r#","pan":0"#);
        let effects = g.wheel(&mut host, &ctx, cx, cy, 1.0);
        assert!(effects.is_empty());
        match &host.window_def(1).unwrap().find(70).unwrap().kind {
            WidgetKind::Piano { min, max, .. } => assert_eq!((*min, *max), (60, 72)),
            other => panic!("not a piano: {other:?}"),
        }
    }

    #[test]
    fn piano_overview_drag_pans_and_wheel_zooms() {
        // With the overview on, the strip sits at the top of the widget rect.
        let host_json = r#"{"type":"window","children":[
            {"id":70,"type":"piano","min":60,"max":72}]}"#;
        let mut host = host_from(host_json);
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 712, 132);
        let l = piano::layout(
            Rect::new(6.0, 6.0, 700.0, 120.0),
            60,
            72,
            true,
            false,
            &Metrics::default(),
        );
        let strip = l.overview.unwrap();
        let sy = (strip.y + strip.h * 0.5) as f64;
        // Drag along the strip: the window pans with the cursor.
        let x0 = piano::overview_key_x(strip, 66) as f64;
        let x1 = piano::overview_key_x(strip, 78) as f64;
        let effects = g.press(&mut host, &ctx, x0, sy, &mut || false);
        assert!(note_emits(&effects).is_empty(), "the strip plays no note");
        let effects = g.drag_to(&mut host, &ctx, x1, sy);
        assert!(has_emit_tag(&effects, 70, "range"));
        match &host.window_def(1).unwrap().find(70).unwrap().kind {
            WidgetKind::Piano { min, max, .. } => {
                assert_eq!(max - min, 12, "pan keeps the span");
                assert!(*min > 60, "the window moved right");
            }
            other => panic!("not a piano: {other:?}"),
        }
        g.release(&mut host, &ctx, x1, sy);
        // Wheel over the strip zooms out (steps < 0 widens the span).
        let effects = g.wheel(&mut host, &ctx, x1, sy, -2.0);
        assert!(has_emit_tag(&effects, 70, "range"));
        match &host.window_def(1).unwrap().find(70).unwrap().kind {
            WidgetKind::Piano { min, max, .. } => assert!(max - min > 12),
            other => panic!("not a piano: {other:?}"),
        }
    }

    #[test]
    fn piano_voice_mode_tracks_one_node_per_held_pitch() {
        let (mut host, l) = piano_host(r#","voice":"pv","voice_args":["pan",0.5]"#);
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 712, 132);
        let c = piano::key_rect(&l, 60).unwrap();
        let (cx, cy) = ((c.x + 2.0) as f64, (c.y + c.h - 1.0) as f64);
        g.press(&mut host, &ctx, cx, cy, &mut || false);
        let voices = host.piano_voices(70).to_vec();
        assert_eq!(voices.len(), 1);
        assert_eq!(voices[0].0, 60);
        // Glissando: the old voice is released, a new node sounds the new key.
        let d = piano::key_rect(&l, 62).unwrap();
        g.drag_to(
            &mut host,
            &ctx,
            (d.x + d.w * 0.5) as f64,
            (d.y + 1.0) as f64,
        );
        let after = host.piano_voices(70).to_vec();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].0, 62);
        assert_ne!(after[0].1, voices[0].1, "a fresh node id per voice");
        // Release clears the bookkeeping.
        g.release(&mut host, &ctx, cx, cy);
        assert!(host.piano_voices(70).is_empty());
        // A freed widget releases whatever is still held.
        g.press(&mut host, &ctx, cx, cy, &mut || false);
        assert!(!host.piano_voices(70).is_empty());
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: super::super::GUI_FREE.into(),
                args: vec![OscType::Int(1)],
            }),
            from(),
        );
        assert!(host.piano_voices(70).is_empty());
    }

    // --- editable text field ------------------------------------------------

    /// A window with one editable `text` field (id 5) filling it.
    /// A window holding one single-line field. It is **natural-sized**, so it
    /// is a control-high strip at the top of the window rather than the whole
    /// pane — every press below aims inside that strip.
    fn text_host() -> Host {
        host_from(r#"{"type":"window","margin":0,"children":[{"id":5,"type":"text"}]}"#)
    }

    fn text_value(host: &Host, id: i32) -> String {
        match &host.window_def(1).unwrap().find(id).unwrap().kind {
            WidgetKind::Text { value, .. } => value.clone(),
            other => panic!("not a text field: {other:?}"),
        }
    }

    /// The string of the single `Emit` in `effects`, if any.
    fn emitted_string(effects: &[GestureEffect]) -> Option<String> {
        effects.iter().find_map(|e| match e {
            GestureEffect::Emit { args, .. } => match args.first() {
                Some(OscType::String(s)) => Some(s.clone()),
                _ => None,
            },
            _ => None,
        })
    }

    #[test]
    fn a_press_focuses_the_field_and_typing_emits_on_every_keystroke() {
        let mut host = text_host();
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 600, 400);
        // A press focuses the field (no emit yet — a click is not an edit).
        let e = g.press(&mut host, &ctx, 30.0, 15.0, &mut || false);
        assert_eq!(host.focused_text(), Some((1, 5)));
        assert!(emitted_string(&e).is_none());
        // Each character is delivered as the field's whole string, ungated.
        for (ch, expect) in [('h', "h"), ('i', "hi")] {
            let e = g
                .text_key(&mut host, &ctx, TextKey::Char(ch), &mut String::new())
                .expect("the focused field consumes the key");
            assert_eq!(emitted_string(&e).as_deref(), Some(expect));
            assert_eq!(text_value(&host, 5), expect);
        }
        // Backspace edits and re-emits.
        let e = g
            .text_key(&mut host, &ctx, TextKey::Backspace, &mut String::new())
            .unwrap();
        assert_eq!(emitted_string(&e).as_deref(), Some("h"));
    }

    #[test]
    fn keys_are_ignored_when_no_field_is_focused() {
        let mut host = text_host();
        let g = Gestures::default();
        let ctx = GestureCtx::new(1, 600, 400);
        // Nothing focused: the machine declines the key (the front runs its
        // global shortcuts instead).
        assert!(
            g.text_key(&mut host, &ctx, TextKey::Char('x'), &mut String::new())
                .is_none()
        );
        assert_eq!(text_value(&host, 5), "");
    }

    #[test]
    fn a_press_elsewhere_defocuses_the_field() {
        // Two fields; focusing one then pressing the other moves the focus.
        let mut host = host_from(
            r#"{"type":"window","margin":0,"layout":"row","children":[
                {"id":5,"type":"text"},{"id":6,"type":"text"}]}"#,
        );
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 600, 400);
        g.press(&mut host, &ctx, 30.0, 30.0, &mut || false);
        assert_eq!(host.focused_text(), Some((1, 5)));
        g.press(&mut host, &ctx, 330.0, 30.0, &mut || false);
        assert_eq!(host.focused_text(), Some((1, 6)));
    }

    #[test]
    fn enter_inserts_a_newline_only_in_a_multiline_field() {
        let ctx = GestureCtx::new(1, 600, 400);
        // Single-line: Enter is inert (no send-on-Enter).
        let mut host = text_host();
        let mut g = Gestures::default();
        g.press(&mut host, &ctx, 30.0, 15.0, &mut || false);
        let e = g
            .text_key(&mut host, &ctx, TextKey::Enter, &mut String::new())
            .unwrap();
        assert!(emitted_string(&e).is_none());
        assert_eq!(text_value(&host, 5), "");
        // Multiline: Enter inserts a newline and emits.
        let mut host = host_from(
            r#"{"type":"window","margin":0,"children":[{"id":5,"type":"text","multiline":true}]}"#,
        );
        let mut g = Gestures::default();
        g.press(&mut host, &ctx, 30.0, 30.0, &mut || false);
        g.text_key(&mut host, &ctx, TextKey::Char('a'), &mut String::new());
        let e = g
            .text_key(&mut host, &ctx, TextKey::Enter, &mut String::new())
            .unwrap();
        assert_eq!(emitted_string(&e).as_deref(), Some("a\n"));
    }

    #[test]
    fn cut_and_paste_move_text_through_the_clipboard() {
        let mut host = text_host();
        let mut g = Gestures::default();
        let mut ctx = GestureCtx::new(1, 600, 400);
        let mut clip = String::new();
        g.press(&mut host, &ctx, 30.0, 15.0, &mut || false);
        for ch in "abc".chars() {
            g.text_key(&mut host, &ctx, TextKey::Char(ch), &mut clip);
        }
        // Select all, then cut to the clipboard.
        ctx.ctrl = true;
        g.text_key(&mut host, &ctx, TextKey::Char('a'), &mut clip);
        g.text_key(&mut host, &ctx, TextKey::Char('x'), &mut clip);
        assert_eq!(clip, "abc");
        assert_eq!(text_value(&host, 5), "");
        // Paste it back twice.
        g.text_key(&mut host, &ctx, TextKey::Char('v'), &mut clip);
        g.text_key(&mut host, &ctx, TextKey::Char('v'), &mut clip);
        assert_eq!(text_value(&host, 5), "abcabc");
    }
}
