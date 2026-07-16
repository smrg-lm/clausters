//! Pointer gestures: the press → drag → release state machine over the widget
//! tree, and the wheel navigation of the timeline views. The *pure* halves —
//! hit-testing, value mutation, cursor→sample mapping, clip placement — live in
//! the shared [`interact`](crate::host::interact) module (and the widgets' own
//! model modules), so this file is only the winit-side state and dispatch.

use clausters_core::osc::OscType;
use tracing::debug;
use winit::window::CursorGrabMode;

use crate::host::interact::{self, slider_t};
use crate::host::layout::Rect;
use crate::host::widget::{Ruler, RulerY, WidgetKind};
use crate::host::{bpf, controls, frame, graph, pianoroll, track};
use crate::viewport::View;

use super::app::App;

/// An in-progress pointer drag, by what it is driving. Cloned out of the window
/// state at each motion step, so the host tree can be mutated while it is read.
#[derive(Clone)]
pub(super) enum Drag {
    /// A slider: the value follows the cursor within `body` — along x, or along y
    /// when `vertical`.
    Slider { id: i32, body: Rect, vertical: bool },
    /// A knob or number: the value moves incrementally with the vertical drag.
    /// On press the pointer is grabbed (see [`App::grab_pointer`]) so motion does
    /// not stop over the window's title bar or past its edges, where `CursorMoved`
    /// is otherwise swallowed. `locked` records which grab won: when `true` the
    /// pointer is locked and motion arrives as relative `DeviceEvent::MouseMotion`;
    /// when `false` (confined or ungrabbed) `CursorMoved` still drives it, and
    /// `last_y` re-anchors on every step so a value pinned at an end has no dead
    /// zone — reversing direction moves it at once instead of sticking and jumping.
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

impl App {
    /// Press on a widget: act by kind and possibly start a drag.
    pub(super) fn on_press(&mut self, def_id: i32) {
        let (cx, cy) = self
            .windows
            .get(&def_id)
            .map(|w| w.cursor)
            .unwrap_or((0.0, 0.0));
        let Some((id, rect, kind)) = self.hit(def_id, cx, cy) else {
            return;
        };
        match kind {
            WidgetKind::Slider { range: r, vertical } => {
                let body = controls::body_rect(rect, r.label.is_some());
                let t = slider_t(body, cx, cy, vertical);
                self.set_fraction(def_id, id, t);
                self.emit_value(def_id, id);
                self.set_drag(def_id, Drag::Slider { id, body, vertical });
                self.redraw(def_id);
            }
            WidgetKind::Knob(r) | WidgetKind::Number(r) => {
                let body = controls::body_rect(rect, r.label.is_some());
                let locked = self.grab_pointer(def_id);
                self.set_drag(
                    def_id,
                    Drag::Vertical {
                        id,
                        last_y: cy,
                        body_h: body.h,
                        locked,
                    },
                );
            }
            WidgetKind::Button { .. } => {
                self.deliver(def_id, id, OscType::Int(1));
                self.set_drag(def_id, Drag::Button { id });
                self.redraw(def_id);
            }
            WidgetKind::Toggle { .. } => {
                interact::flip_toggle(&mut self.host, def_id, id);
                self.emit_value(def_id, id);
                self.redraw(def_id);
            }
            WidgetKind::Menu { .. } => {
                interact::cycle_menu(&mut self.host, def_id, id);
                self.emit_value(def_id, id);
                self.redraw(def_id);
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
                let ctrl = self.windows.get(&def_id).is_some_and(|w| w.ctrl);
                let hit_pt = bpf::hit_point(points, body, duration, min, max, exp, cx, cy);
                if ctrl {
                    // Ctrl+click on a point removes it; elsewhere it adds one
                    // at the cursor (which then drags until release).
                    // `None` = nothing changed, `Some(None)` = removed,
                    // `Some(Some(i))` = added at index `i`.
                    let edited: Option<Option<usize>> = match hit_pt {
                        Some(i) => {
                            interact::bpf_edit(&mut self.host, def_id, id, |p, _, _, _, _| {
                                bpf::remove_point(p, i)
                            })
                            .and_then(|removed| removed.then_some(None))
                        }
                        None => interact::bpf_edit(
                            &mut self.host,
                            def_id,
                            id,
                            |p, duration, lo, hi, exp| {
                                bpf::add_point(p, body, duration, lo, hi, exp, cx, cy)
                            },
                        )
                        .map(Some),
                    };
                    if let Some(added) = edited {
                        if let Some(index) = added {
                            self.set_drag(def_id, Drag::BpfPoint { id, index, body });
                        }
                        self.emit_points(def_id, id);
                        self.redraw(def_id);
                    }
                } else if let Some(index) = hit_pt {
                    self.set_drag(def_id, Drag::BpfPoint { id, index, body });
                } else if let Some(segment) = bpf::hit_segment(points, body, duration, cx) {
                    self.set_drag(
                        def_id,
                        Drag::BpfCurve {
                            id,
                            segment,
                            last_y: cy,
                            body_h: body.h.max(1.0) as f64,
                        },
                    );
                }
            }
            WidgetKind::Graph { ref graph, .. } => {
                // A patch's port is the grab point of a rewiring drag; the rest of
                // the patch is display.
                if let Some(port) = graph::port_hit(rect, graph, cx, cy) {
                    self.set_drag(
                        def_id,
                        Drag::Wire {
                            id,
                            port,
                            area: rect,
                        },
                    );
                }
            }
            WidgetKind::Track {
                snap, ref editor, ..
            } => {
                // Shift+drag pans the shared axis (the same gesture the heavy
                // views use), so panning stays available where every plain drag
                // grabs a clip.
                let shift = self.windows.get(&def_id).is_some_and(|w| w.shift);
                if shift {
                    let body = track::lane_body(rect, editor.ruler != Ruler::Off);
                    if let Some((start, _len, _total)) = self.timeline_nav(id) {
                        self.set_drag(
                            def_id,
                            Drag::Pan {
                                id,
                                origin_x: cx,
                                start,
                                body_w: body.w.max(1.0) as f64,
                            },
                        );
                    }
                    return;
                }
                // A press on the lane's **time ruler**, or on empty lane space,
                // *locates* the transport: the multitrack's cursor goes where you
                // point, which is the one gesture a timeline view cannot do
                // without. (Over a clip, the clip's own gestures win.)
                let ruler_on = editor.ruler != Ruler::Off;
                let body = track::lane_body(rect, ruler_on);
                let on_ruler = ruler_on && cy > body.y as f64 + body.h as f64;
                let (fb_w, fb_h) = self.fb(def_id);
                let over_clip =
                    interact::clip_hit(&self.host, def_id, fb_w, fb_h, cx, cy).is_some();
                if on_ruler || (!over_clip && body.contains(cx, cy)) {
                    self.locate_timeline(def_id, id, body, cx);
                    return;
                }
                // A track is the hit target (its clips are placed by the
                // renderer, not the layout engine); find the clip under the
                // cursor and start a move (body) or resize (edge) drag.
                if let Some(h) = interact::clip_hit(&self.host, def_id, fb_w, fb_h, cx, cy) {
                    // An automation clip: a break-point wins over the clip body
                    // (as it wins over a segment in the `bpf` view), and Ctrl+click
                    // adds one - or removes the one under the cursor. The same
                    // gestures, now on a lane.
                    let ctrl = self.windows.get(&def_id).is_some_and(|w| w.ctrl);
                    if h.point.is_some() || (ctrl && self.clip_has_curve(def_id, h.id)) {
                        if ctrl {
                            if interact::clip_point_edit(
                                &mut self.host,
                                def_id,
                                h.id,
                                h.point,
                                h.rect,
                                h.body,
                                &h.nav,
                                h.offset,
                                cx,
                                cy,
                            ) {
                                self.emit_points(def_id, h.id);
                                self.redraw(def_id);
                            }
                        } else if let Some(index) = h.point {
                            self.set_drag(
                                def_id,
                                Drag::ClipPoint {
                                    id: h.id,
                                    index,
                                    rect: h.rect,
                                    body: h.body,
                                    nav_start: h.nav.start,
                                    nav_len: h.nav.len,
                                    offset: h.offset,
                                },
                            );
                        }
                        return;
                    }
                    let press_sample = interact::sample_at(
                        h.nav.start,
                        h.nav.len,
                        h.body.x as f64,
                        h.body.w as f64,
                        cx,
                    );
                    self.set_drag(
                        def_id,
                        Drag::Clip {
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
                        },
                    );
                }
            }
            WidgetKind::PianoRoll { .. } => {
                let (fb_w, fb_h) = self.fb(def_id);
                let Some(h) = interact::pianoroll_hit(&self.host, def_id, fb_w, fb_h, cx, cy)
                else {
                    return;
                };
                let shift = self.windows.get(&def_id).is_some_and(|w| w.shift);
                let ctrl = self.windows.get(&def_id).is_some_and(|w| w.ctrl);
                let alt = self.windows.get(&def_id).is_some_and(|w| w.alt);
                // A press on the keyboard gutter (left of the grid) pans the pitch
                // window — the keyboard is the piano-roll's vertical axis surface,
                // the counterpart of the heavy views' y-ruler strip.
                if cx < h.grid.x as f64 {
                    let y_start = self
                        .host
                        .window_def(def_id)
                        .and_then(|t| t.find(id))
                        .and_then(|w| w.kind.editor())
                        .map_or(0.0, |e| e.y_view().0);
                    self.set_drag(
                        def_id,
                        Drag::PanY {
                            id,
                            origin_y: cy,
                            y_start,
                            lane_h: h.grid.h.max(1.0) as f64,
                        },
                    );
                    return;
                }
                // Shift+drag pans the shared axis (the heavy-view gesture), so
                // panning stays available where a plain drag edits notes/selects.
                if shift {
                    if let Some((start, _len, _total)) = self.timeline_nav(id) {
                        self.set_drag(
                            def_id,
                            Drag::Pan {
                                id,
                                origin_x: cx,
                                start,
                                body_w: h.grid.w.max(1.0) as f64,
                            },
                        );
                    }
                    return;
                }
                self.pianoroll_press(def_id, id, &h, ctrl, alt, cx, cy);
            }
            WidgetKind::Waveform { ref editor, .. }
            | WidgetKind::Spectrogram { ref editor, .. } => {
                let body = frame::timeline_body(rect, editor);
                // A press on the y-ruler strip left of the body starts a
                // vertical pan of the display window (the strip is the y
                // axis' gesture surface; wheel over it zooms).
                if editor.ruler_y != RulerY::Off && cx < body.x as f64 {
                    let lanes = self.timeline_lanes(def_id, id, &kind);
                    self.set_drag(
                        def_id,
                        Drag::PanY {
                            id,
                            origin_y: cy,
                            y_start: editor.y_view().0,
                            lane_h: (body.h as f64 / lanes.max(1) as f64).max(1.0),
                        },
                    );
                    return;
                }
                let shift = self.windows.get(&def_id).is_some_and(|w| w.shift);
                if let Some((start, len, _)) = self.timeline_nav(id) {
                    if shift {
                        // Shift+drag pans the view (the pre-editor gesture).
                        self.set_drag(
                            def_id,
                            Drag::Pan {
                                id,
                                origin_x: cx,
                                start,
                                body_w: body.w.max(1.0) as f64,
                            },
                        );
                    } else {
                        // Plain drag selects (the editor convention). The press
                        // collapses the selection to the sample under it.
                        let anchor =
                            interact::sample_at(start, len, body.x as f64, body.w as f64, cx);
                        self.set_selection(def_id, id, anchor, anchor);
                        self.set_drag(
                            def_id,
                            Drag::Select {
                                id,
                                body_x: body.x as f64,
                                body_w: body.w.max(1.0) as f64,
                                anchor,
                            },
                        );
                        self.redraw(def_id);
                    }
                }
            }
            _ => {}
        }
    }

    /// Pointer moved while a drag is active: drive the dragged target. The drag
    /// descriptor is cloned out (cheap: geometry plus, for the block gestures, a
    /// small snapshot vec) so the host tree can be mutated under it.
    pub(super) fn on_drag(&mut self, def_id: i32, cx: f64, cy: f64) {
        let Some(drag) = self.windows.get(&def_id).and_then(|w| w.drag.clone()) else {
            return;
        };
        match drag {
            // A held button and a wire-in-flight only act on release; a locked
            // knob drag is driven by relative motion in `device_event`, not by
            // these cursor positions.
            Drag::Button { .. } | Drag::Wire { .. } => {}
            Drag::Vertical { locked: true, .. } => {}
            Drag::Slider { id, body, vertical } => {
                let t = slider_t(body, cx, cy, vertical);
                self.set_fraction(def_id, id, t);
                self.emit_value(def_id, id);
                self.redraw(def_id);
            }
            Drag::Vertical {
                id, last_y, body_h, ..
            } => {
                // Incremental: add this step's delta to the *current* (clamped)
                // fraction and re-anchor `last_y`. A value pinned at an end stays
                // put, but reversing moves it immediately — no snapshot dead zone.
                let cur = self.fraction_of(def_id, id).unwrap_or(0.0);
                let t = (cur + controls::drag_fraction_delta(cy - last_y, body_h)).clamp(0.0, 1.0);
                self.set_fraction(def_id, id, t);
                if let Some(Drag::Vertical { last_y, .. }) =
                    self.windows.get_mut(&def_id).and_then(|w| w.drag.as_mut())
                {
                    *last_y = cy;
                }
                self.emit_value(def_id, id);
                self.redraw(def_id);
            }
            Drag::Pan {
                id,
                origin_x,
                start,
                body_w,
            } => {
                self.pan_timeline(def_id, id, start, (cx - origin_x) / body_w);
            }
            Drag::PanY {
                id,
                origin_y,
                y_start,
                lane_h,
            } => {
                // Dragging down moves the window down with the cursor;
                // absolute from the snapshot, so a clamped edge never drifts.
                let y_len = self
                    .host
                    .window_def(def_id)
                    .and_then(|t| t.find(id))
                    .and_then(|w| w.kind.editor())
                    .map_or(1.0, |e| e.y_view().1);
                let start = y_start + (cy - origin_y) / lane_h * y_len;
                self.set_y_view(def_id, id, start, y_len);
            }
            Drag::BpfPoint { id, index, body } => {
                interact::bpf_edit(&mut self.host, def_id, id, |p, duration, lo, hi, exp| {
                    bpf::move_point(p, index, body, duration, lo, hi, exp, cx, cy);
                });
                self.emit_points(def_id, id);
                self.redraw(def_id);
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
                interact::bpf_edit(&mut self.host, def_id, id, |p, _, _, _, _| {
                    bpf::drag_curve(p, segment, dy_frac);
                });
                if let Some(Drag::BpfCurve { last_y, .. }) =
                    self.windows.get_mut(&def_id).and_then(|w| w.drag.as_mut())
                {
                    *last_y = cy;
                }
                self.emit_points(def_id, id);
                self.redraw(def_id);
            }
            Drag::Select {
                id,
                body_x,
                body_w,
                anchor,
            } => {
                let (start, len) = match self.timeline_nav(id) {
                    Some((start, len, _)) => (start, len),
                    None => return,
                };
                let cur = interact::sample_at(start, len, body_x, body_w, cx);
                self.set_selection(def_id, id, anchor, cur);
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
                interact::clip_set(&mut self.host, def_id, id, Some(new_offset), Some(new_dur));
                // The lane's extent moved with the clip: re-register it, so the
                // shared axis grows when a clip is dragged past the end.
                self.host.sync_track_totals();
                self.emit_clip(def_id, id);
                self.redraw(def_id);
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
                    &mut self.host,
                    def_id,
                    id,
                    index,
                    rect,
                    body,
                    &nav,
                    offset,
                    cx,
                    cy,
                ) {
                    self.emit_points(def_id, id);
                    self.redraw(def_id);
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
                interact::pianoroll_notes_edit(&mut self.host, def_id, id, |notes| match part {
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
                self.host.sync_track_totals();
                self.emit_notes(def_id, id);
                self.redraw(def_id);
            }
            Drag::Velocity { id, index, lane } => {
                let vel = pianoroll::velocity_at(lane, cy);
                interact::pianoroll_notes_edit(&mut self.host, def_id, id, |notes| {
                    pianoroll::set_velocity(notes, index, vel);
                });
                self.emit_notes(def_id, id);
                self.redraw(def_id);
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
                interact::pianoroll_osc_edit(&mut self.host, def_id, id, |osc| {
                    if let Some(m) = osc.get_mut(index) {
                        m.time = interact::snap(time, snap).max(0.0);
                    }
                });
                self.host.sync_track_totals();
                self.emit_osc(def_id, id);
                self.redraw(def_id);
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
                self.set_selection(def_id, id, anchor, cur);
                let pitch = pianoroll::y_to_pitch(cy as f32, lo, hi, grid);
                interact::pianoroll_state_edit(&mut self.host, def_id, id, |notes, sel| {
                    *sel = pianoroll::notes_in_rect(notes, anchor, cur, anchor_pitch, pitch);
                });
                self.redraw(def_id);
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
                interact::pianoroll_notes_edit(&mut self.host, def_id, id, |notes| {
                    pianoroll::move_notes_from(notes, &orig, dt, dp, lo, hi);
                });
                self.host.sync_track_totals();
                self.emit_notes(def_id, id);
                self.redraw(def_id);
            }
            Drag::VelocityBlock {
                id,
                lane,
                press_velocity,
                orig,
            } => {
                let dv = pianoroll::velocity_at(lane, cy) - press_velocity;
                interact::pianoroll_notes_edit(&mut self.host, def_id, id, |notes| {
                    pianoroll::nudge_velocities_from(notes, &orig, dv);
                });
                self.emit_notes(def_id, id);
                self.redraw(def_id);
            }
        }
    }

    /// Release: a held button emits 0; a knob/number drag releases its pointer
    /// grab; any drag ends.
    pub(super) fn on_release(&mut self, def_id: i32) {
        let drag = self.windows.get_mut(&def_id).and_then(|w| w.drag.take());
        match drag {
            Some(Drag::Button { id }) => {
                self.deliver(def_id, id, OscType::Int(0));
                self.redraw(def_id);
            }
            Some(Drag::Vertical { .. }) => self.release_pointer(def_id),
            Some(Drag::Wire { id, port, area }) => {
                // Released over a bus: the control is rewired to it. Over empty
                // space: unwired (the bus is reported empty). Either way the tree
                // is written and the edit leaves as a flat `"wire"` event, so the
                // script updates the logical group and re-renders it.
                let (cx, cy) = self.windows.get(&def_id).map_or((0.0, 0.0), |w| w.cursor);
                if let Some((member, control, bus)) =
                    interact::wire_set(&mut self.host, def_id, id, port, area, cx, cy)
                {
                    self.emit(
                        def_id,
                        id,
                        vec![
                            OscType::String("wire".into()),
                            OscType::Int(member as i32),
                            OscType::String(control),
                            OscType::String(bus),
                        ],
                    );
                    self.redraw(def_id);
                }
            }
            _ => {}
        }
    }

    /// Wheel over a timeline view: zoom the shared time axis anchored at the
    /// cursor, or — over the y-ruler strip / the piano-roll's keyboard gutter —
    /// zoom the vertical display window anchored at the cursor's height.
    pub(super) fn on_wheel(&mut self, def_id: i32, steps: f64) {
        let (cx, cy) = self
            .windows
            .get(&def_id)
            .map(|w| w.cursor)
            .unwrap_or((0.0, 0.0));
        if let Some((id, rect, kind)) = self.hit(def_id, cx, cy)
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
                    self.zoom_timeline_y(def_id, id, factor, 1.0 - rel);
                } else {
                    self.zoom_timeline(def_id, id, r.grid, cx, factor);
                }
                return;
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
                let lanes = self.timeline_lanes(def_id, id, &kind);
                let lane = frame::lane_rect(body, lanes, frame::lane_at(body, lanes, cy));
                let rel = ((cy - lane.y as f64) / lane.h.max(1.0) as f64).clamp(0.0, 1.0);
                self.zoom_timeline_y(def_id, id, factor, 1.0 - rel);
            } else {
                self.zoom_timeline(def_id, id, body, cx, factor);
            }
        }
    }

    pub(super) fn set_drag(&mut self, def_id: i32, drag: Drag) {
        if let Some(ws) = self.windows.get_mut(&def_id) {
            ws.drag = Some(drag);
        }
    }

    /// Grabs the pointer for a knob/number drag so motion keeps arriving even
    /// over the window decorations or past its edges, where `CursorMoved`
    /// otherwise stops (the title-bar/out-of-surface gap). Tries `Locked` first —
    /// the cursor stays put and motion comes as relative `DeviceEvent::MouseMotion`
    /// (the canonical knob feel, unbounded range) — and falls back to `Confined`,
    /// which keeps the cursor inside the client area (so it cannot reach the title
    /// bar) and is still driven by `CursorMoved`. Returns whether the pointer was
    /// *locked* (which motion source the drag should read).
    fn grab_pointer(&self, def_id: i32) -> bool {
        let Some(ws) = self.windows.get(&def_id) else {
            return false;
        };
        let window = &ws.gpu.window;
        if window.set_cursor_grab(CursorGrabMode::Locked).is_ok() {
            window.set_cursor_visible(false);
            return true;
        }
        if let Err(e) = window.set_cursor_grab(CursorGrabMode::Confined) {
            debug!("gui_def {def_id}: no pointer grab for the drag ({e})");
        }
        false
    }

    /// Releases the pointer grab a knob/number drag took and restores the cursor.
    fn release_pointer(&self, def_id: i32) {
        if let Some(ws) = self.windows.get(&def_id) {
            let _ = ws.gpu.window.set_cursor_grab(CursorGrabMode::None);
            ws.gpu.window.set_cursor_visible(true);
        }
    }

    /// Locates the transport: the timeline position under the cursor becomes the
    /// group's static cursor (drawn at once on every lane, so the click lands
    /// where you see it) and leaves as `/gui_event <id> "locate" <position>` — the
    /// script seeks its playhead there, which is what actually moves the music.
    fn locate_timeline(&mut self, def_id: i32, id: i32, body: Rect, cx: f64) {
        let Some((start, len, _total)) = self.timeline_nav(id) else {
            return;
        };
        let pos = interact::sample_at(start, len, body.x as f64, body.w as f64, cx).max(0.0);
        let roots = self.host.set_timeline_cursor(id, pos);
        self.emit(
            def_id,
            id,
            vec![OscType::String("locate".into()), OscType::Float(pos as f32)],
        );
        self.redraw_all(&roots);
        self.redraw(def_id);
    }

    /// Whether clip `id` carries a break-point curve (an automation clip).
    fn clip_has_curve(&self, def_id: i32, id: i32) -> bool {
        self.host
            .window_def(def_id)
            .and_then(|t| t.find(id))
            .and_then(track::clip_draw)
            .is_some_and(|clip| !clip.points.is_empty())
    }

    /// Writes the selection spanning samples `a..b` (any order, clamped to the
    /// timeline) into view `id`'s navigation group — every member follows —
    /// and emits **one** `"selection" start len` event, carrying the
    /// interacted member's id.
    pub(super) fn set_selection(&mut self, def_id: i32, id: i32, a: f64, b: f64) {
        let Some((start, len, roots)) = self.host.select_timeline(id, a, b) else {
            return;
        };
        self.redraw_all(&roots);
        self.emit(
            def_id,
            id,
            vec![
                OscType::String("selection".into()),
                OscType::Float(start as f32),
                OscType::Float(len as f32),
            ],
        );
    }

    fn pan_timeline(&mut self, def_id: i32, id: i32, start: f64, dx_fraction: f64) {
        let Some((_, len, _)) = self.timeline_nav(id) else {
            return;
        };
        let roots = self.host.pan_timeline(id, start - dx_fraction * len);
        self.emit_view(def_id, id);
        self.redraw_all(&roots);
    }

    /// Emits a timeline view's visible range as a `/gui_event id "view" start len`
    /// — once per gesture step, carrying the interacted member's id (linked
    /// members repaint but do not re-emit).
    pub(super) fn emit_view(&self, def_id: i32, id: i32) {
        if let Some((start, len, _)) = self.timeline_nav(id) {
            self.emit(
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

    /// The lane count timeline view `id` stacks on screen (overlaid waveform
    /// traces share one lane) — the divisor for lane-relative y gestures.
    fn timeline_lanes(&self, def_id: i32, id: i32, kind: &WidgetKind) -> usize {
        let Some(ws) = self.windows.get(&def_id) else {
            return 1;
        };
        match kind {
            WidgetKind::Waveform { overlay: true, .. } => 1,
            WidgetKind::Waveform { .. } => ws
                .waveforms
                .get(&id)
                .map_or(1, |s| s.view.num_channels().max(1)),
            WidgetKind::Spectrogram { .. } => {
                ws.spectrograms.get(&id).map_or(1, |s| s.views.len().max(1))
            }
            _ => 1,
        }
    }

    /// Writes timeline view `id`'s vertical display window (clamped) into its
    /// editor props and emits the `"view_y" y_start y_len` event — the
    /// vertical sibling of [`Self::emit_view`]'s range.
    pub(super) fn set_y_view(&mut self, def_id: i32, id: i32, start: f64, len: f64) {
        let (start, len) = crate::viewport::clamp_span(start, len);
        if let Some(editor) = self
            .host
            .window_def_mut(def_id)
            .and_then(|t| t.find_mut(id))
            .and_then(|w| w.kind.editor_mut())
        {
            (editor.y_start, editor.y_len) = (start, len);
        }
        self.emit(
            def_id,
            id,
            vec![
                OscType::String("view_y".into()),
                OscType::Float(start as f32),
                OscType::Float(len as f32),
            ],
        );
        self.redraw(def_id);
    }

    /// Anchor-preserving vertical zoom of timeline view `id`: `anchor` in
    /// display coordinates (0 = lane bottom, 1 = lane top).
    fn zoom_timeline_y(&mut self, def_id: i32, id: i32, factor: f64, anchor: f64) {
        let Some((y0, ylen)) = self
            .host
            .window_def(def_id)
            .and_then(|t| t.find(id))
            .and_then(|w| w.kind.editor())
            .map(|e| e.y_view())
        else {
            return;
        };
        let (start, len) = crate::viewport::zoom_span(y0, ylen, factor, anchor);
        self.set_y_view(def_id, id, start, len);
    }

    fn zoom_timeline(&mut self, def_id: i32, id: i32, body: Rect, cx: f64, factor: f64) {
        let anchor = ((cx - body.x as f64) / body.w.max(1.0) as f64).clamp(0.0, 1.0);
        let roots = self.host.zoom_timeline(id, factor, anchor);
        self.emit_view(def_id, id);
        self.redraw_all(&roots);
    }

    /// `R` over a window: reset every timeline view's navigation (the whole
    /// group, linked members in other windows too) and its vertical axis.
    pub(super) fn reset_timelines(&mut self, def_id: i32) {
        let mut ids: Vec<i32> = Vec::new();
        if let Some(ws) = self.windows.get(&def_id) {
            ids.extend(ws.waveforms.keys().copied());
            ids.extend(ws.spectrograms.keys().copied());
        }
        for id in ids {
            // The whole group resets (linked members in other windows too).
            let roots = self.host.reset_timeline(id);
            self.redraw_all(&roots);
            self.emit_view(def_id, id);
            // The reset also restores the full vertical axis (and reports it).
            self.set_y_view(def_id, id, 0.0, 1.0);
        }
        self.redraw(def_id);
    }
}
