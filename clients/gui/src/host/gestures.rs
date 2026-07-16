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
//! front ([`super::gui`]) and the browser front ([`super::web`]) both drive it,
//! so a selection, a clip drag or a BPF edit behaves identically on either
//! platform by construction.

use std::collections::HashMap;

use clausters_core::osc::OscType;

use super::interact::{self, slider_t, value_of};
use super::layout::Rect;
use super::widget::{Ruler, RulerY, Widget, WidgetKind};
use super::{Host, bpf, controls, frame, graph, pianoroll, track};
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
    /// A wire being pulled from a `graph` patch's port: the widget, the port
    /// (member, control) and the widget's area — released over a bus to rewire
    /// it, over empty space to unwire.
    Wire {
        id: i32,
        port: (usize, usize),
        area: Rect,
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

    /// The rewiring drag in flight, if any: the `graph` widget and the grabbed
    /// port (the renderer draws the wire to the pointer).
    pub fn wiring(&self) -> Option<(i32, (usize, usize))> {
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
        let Some((id, rect, kind)) = hit(host, ctx, cx, cy) else {
            return out;
        };
        let def_id = ctx.def_id;
        match kind {
            WidgetKind::Slider { range: r, vertical } => {
                let body = controls::body_rect(rect, r.label.is_some());
                let t = slider_t(body, cx, cy, vertical);
                interact::set_fraction(host, def_id, id, t);
                emit_value(host, &mut out, def_id, id);
                self.drag = Some(Drag::Slider { id, body, vertical });
                out.push(GestureEffect::Redraw(def_id));
            }
            WidgetKind::Knob(r) | WidgetKind::Number(r) => {
                let body = controls::body_rect(rect, r.label.is_some());
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
            WidgetKind::Bpf {
                ref points,
                min,
                max,
                duration,
                exp,
                ref label,
                ..
            } => {
                let body = bpf::body(rect, label.is_some());
                let hit_pt = bpf::hit_point(points, body, duration, min, max, exp, cx, cy);
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
            WidgetKind::Graph { ref graph, .. } => {
                // A patch's port is the grab point of a rewiring drag; the rest of
                // the patch is display.
                if let Some(port) = graph::port_hit(rect, graph, cx, cy) {
                    self.drag = Some(Drag::Wire {
                        id,
                        port,
                        area: rect,
                    });
                }
            }
            WidgetKind::Track {
                snap, ref editor, ..
            } => {
                // Shift+drag pans the shared axis (the same gesture the heavy
                // views use), so panning stays available where every plain drag
                // grabs a clip.
                if ctx.shift {
                    let body = track::lane_body(rect, editor.ruler != Ruler::Off);
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
                // A press on the lane's **time ruler**, or on empty lane space,
                // *locates* the transport: the multitrack's cursor goes where you
                // point, which is the one gesture a timeline view cannot do
                // without. (Over a clip, the clip's own gestures win.)
                let ruler_on = editor.ruler != Ruler::Off;
                let body = track::lane_body(rect, ruler_on);
                let on_ruler = ruler_on && cy > body.y as f64 + body.h as f64;
                let over_clip =
                    interact::clip_hit(host, def_id, ctx.fb_w, ctx.fb_h, cx, cy).is_some();
                if on_ruler || (!over_clip && body.contains(cx, cy)) {
                    locate_timeline(host, &mut out, def_id, id, body, cx);
                    return out;
                }
                // A track is the hit target (its clips are placed by the
                // renderer, not the layout engine); find the clip under the
                // cursor and start a move (body) or resize (edge) drag.
                if let Some(h) = interact::clip_hit(host, def_id, ctx.fb_w, ctx.fb_h, cx, cy) {
                    // An automation clip: a break-point wins over the clip body
                    // (as it wins over a segment in the `bpf` view), and Ctrl+click
                    // adds one - or removes the one under the cursor. The same
                    // gestures, now on a lane.
                    if h.point.is_some() || (ctx.ctrl && clip_has_curve(host, def_id, h.id)) {
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
            WidgetKind::PianoRoll { .. } => {
                let Some(h) = interact::pianoroll_hit(host, def_id, ctx.fb_w, ctx.fb_h, cx, cy)
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
                let body = frame::timeline_body(rect, editor);
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
                // Map the cursor to a timeline sample; the placement math
                // (move/resize against the press snapshot, snapped and clamped)
                // is the shared `clip_drag_placement`.
                let sample = interact::sample_at(nav_start, nav_len, body_x, body_w, cx);
                let (new_offset, new_dur) = interact::clip_drag_placement(
                    part,
                    sample,
                    press_sample,
                    orig_offset,
                    orig_dur,
                    grid,
                );
                interact::clip_set(host, def_id, id, Some(new_offset), Some(new_dur));
                // The lane's extent moved with the clip: re-register it, so the
                // shared axis grows when a clip is dragged past the end.
                host.sync_track_totals();
                emit_clip(host, &mut out, def_id, id);
                out.push(GestureEffect::Redraw(def_id));
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
            Some(Drag::Wire { id, port, area }) => {
                // Released over a bus: the control is rewired to it. Over empty
                // space: unwired (the bus is reported empty). Either way the tree
                // is written and the edit leaves as a flat `"wire"` event, so the
                // script updates the logical group and re-renders it.
                if let Some((member, control, bus)) =
                    interact::wire_set(host, def_id, id, port, area, cx, cy)
                {
                    out.push(GestureEffect::Emit {
                        def_id,
                        widget_id: id,
                        args: vec![
                            OscType::String("wire".into()),
                            OscType::Int(member as i32),
                            OscType::String(control),
                            OscType::String(bus),
                        ],
                    });
                    out.push(GestureEffect::Redraw(def_id));
                }
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
        if let Some((id, rect, kind)) = hit(host, ctx, cx, cy)
            && let Some(editor) = kind.editor()
        {
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
                let r =
                    pianoroll::regions(rect, editor.ruler != Ruler::Off, *osc_lane, *velocity_lane);
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
                WidgetKind::Track { .. } => track::lane_body(rect, editor.ruler != Ruler::Off),
                _ => frame::timeline_body(rect, editor),
            };
            if editor.ruler_y != RulerY::Off && cx < body.x as f64 {
                // Wheel over the y-ruler strip zooms the vertical
                // display window, anchored at the cursor's height
                // within the lane under it.
                let lanes = ctx.lanes(id, &kind);
                let lane = frame::lane_rect(body, lanes, frame::lane_at(body, lanes, cy));
                let rel = ((cy - lane.y as f64) / lane.h.max(1.0) as f64).clamp(0.0, 1.0);
                zoom_timeline_y(host, &mut out, def_id, id, factor, 1.0 - rel);
            } else {
                zoom_timeline(host, &mut out, def_id, id, body, cx, factor);
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
        let Some((id, _rect, WidgetKind::PianoRoll { snap, .. })) = hit(host, ctx, cx, cy) else {
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
        let Some((id, _rect, WidgetKind::PianoRoll { .. })) = hit(host, ctx, cx, cy) else {
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
        let Some(h) = interact::pianoroll_hit(host, def_id, ctx.fb_w, ctx.fb_h, cx, cy) else {
            return out;
        };
        let Some((id, _rect, WidgetKind::PianoRoll { .. })) = hit(host, ctx, cx, cy) else {
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
        let Some((id, _rect, WidgetKind::PianoRoll { .. })) = hit(host, ctx, cx, cy) else {
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

/// The deepest widget under `(x, y)`: its id, rect and a clone of its kind.
fn hit(host: &Host, ctx: &GestureCtx, x: f64, y: f64) -> Option<(i32, Rect, WidgetKind)> {
    interact::hit(host, ctx.def_id, ctx.fb_w, ctx.fb_h, x, y)
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

/// Whether clip `id` carries a break-point curve (an automation clip).
fn clip_has_curve(host: &Host, def_id: i32, id: i32) -> bool {
    host.window_def(def_id)
        .and_then(|t| t.find(id))
        .and_then(track::clip_draw)
        .is_some_and(|clip| !clip.points.is_empty())
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
    let (start, len) = crate::viewport::clamp_span(start, len);
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
    let (start, len) = crate::viewport::zoom_span(y0, ylen, factor, anchor);
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

    use super::super::{ClientId, GUI_DEF};
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

    #[test]
    fn slider_press_and_drag_set_the_value_and_emit() {
        let mut host = host_from(
            r#"{"type":"window","children":[
                {"id":10,"type":"slider","min":0.0,"max":10.0,"value":2.5}]}"#,
        );
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 400, 100);
        let effects = g.press(&mut host, &ctx, 200.0, 50.0, &mut || false);
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
        g.drag_to(&mut host, &ctx, 399.0, 50.0);
        assert_eq!(slider_value(&host, 10), 10.0);
        assert!(g.release(&mut host, &ctx, 399.0, 50.0).is_empty());
        assert!(!g.dragging());
    }

    #[test]
    fn button_press_emits_one_and_release_emits_zero() {
        let mut host = host_from(r#"{"type":"window","children":[{"id":20,"type":"button"}]}"#);
        let mut g = Gestures::default();
        let ctx = GestureCtx::new(1, 200, 100);
        let effects = g.press(&mut host, &ctx, 100.0, 50.0, &mut || false);
        assert_eq!(g.active_button(), Some(20));
        assert!(effects.iter().any(|e| matches!(
            e,
            GestureEffect::Emit { widget_id: 20, args, .. } if args == &[OscType::Int(1)]
        )));
        let effects = g.release(&mut host, &ctx, 100.0, 50.0);
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
        let effects = g.press(&mut host, &ctx, 100.0, 50.0, &mut || false);
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
        g.press(&mut host, &ctx, 100.0, 100.0, &mut || true);
        assert!(g.locked());
        // Locked: cursor motion is ignored (relative deltas drive it instead).
        let effects = g.drag_to(&mut host, &ctx, 100.0, 150.0);
        assert!(effects.is_empty());
        let effects = g.relative_motion(&mut host, &ctx, -20.0);
        assert!(effects.contains(&GestureEffect::Redraw(1)));
        // Release asks the front to drop the pointer grab.
        let effects = g.release(&mut host, &ctx, 100.0, 150.0);
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
        // The selection landed in the editor props with a positive length.
        let editor = host
            .window_def(1)
            .unwrap()
            .find(50)
            .unwrap()
            .kind
            .editor()
            .cloned()
            .unwrap();
        assert!(editor.sel_len > 0.0);
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
}
