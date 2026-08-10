//! What a **drag** does while it is held, and what its **release** delivers.
//!
//! One arm per [`Drag`] variant, twice: [`Gestures::drag_to`]
//! moves the thing under the cursor, [`Gestures::release`] ends the gesture and
//! emits whatever the edit owes its owner (an edit-back payload, a final value,
//! a note-off). Keeping the two matches side by side is what keeps them in
//! step — a variant that grows a drag behaviour and forgets its release is
//! visible here rather than three hundred lines away.
//!
//! [`Gestures::tick`] belongs to the same phase: it is the frame step of a drag
//! held against a lane's edge, which pans the view and carries the drag with
//! it, and it exists for no other reason.

use clausters_core::osc::OscType;

use super::super::interact::{self};
use super::super::widget::{Axis, WidgetKind};
use super::super::{Host, piano, pianoroll};
use super::effects::*;
use super::nav::*;
use super::{Drag, GestureCtx, GestureEffect, Gestures, element};

impl Gestures {
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
            // A wire in flight only acts on release.
            Drag::Wire { .. } => {}
            // A grabbed element is driven by `relative_motion` for the same
            // reason: the cursor is not travelling, so these positions are not
            // the gesture.
            Drag::Element { grab: true, .. } => {}
            Drag::Element { at, .. } => {
                let events = element::with(host, ctx, at, |el, input| el.drag((cx, cy), input));
                if let Some(events) = events {
                    // **A held drag always repaints**, whether or not it
                    // reported: an element was given the drag because it
                    // changes something, and what it changes is not always
                    // deliverable — a text selection extending, a score's
                    // element crossing a diatonic step. Reporting already asks
                    // for the repaint, so this is the other case; only the
                    // element that is *gone* (a widget freed under the drag)
                    // skips both.
                    let silent = events.is_empty();
                    element::report(host, &mut out, ctx, at.id, events);
                    if silent {
                        out.push(GestureEffect::Redraw(def_id));
                    }
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
                    .widget_kind(def_id, id)
                    .and_then(WidgetKind::editor)
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
            Drag::PanX {
                id,
                origin_x,
                x_start,
                body_w,
            } => {
                // Dragging right moves the axis right with the cursor: the
                // frequency grabbed stays under it. Over the window on the
                // screen, which is what the hand is on — where the floor has
                // opened the axis, a pixel is worth more hertz than the
                // request would say.
                let x_len = freq_window(host, def_id, id, ctx.sample_rate).map_or(1.0, |w| w.1);
                let start = x_start - (cx - origin_x) / body_w * x_len;
                pan_x_view(host, &mut out, def_id, id, start, ctx.sample_rate);
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
            Some(Drag::Element { at, grab }) => {
                // What the drag *delivers*, as against what it showed along the
                // way. The grab is the front's to undo, whatever came back.
                let events = element::with(host, ctx, at, |el, input| el.release((cx, cy), input));
                if let Some(events) = events {
                    element::report(host, &mut out, ctx, at.id, events);
                }
                if grab {
                    out.push(GestureEffect::ReleasePointer(def_id));
                }
                out.push(GestureEffect::Redraw(def_id));
            }
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
                if let Some((from, outlet, to, inlet)) = interact::graph_cord(
                    host,
                    def_id,
                    id,
                    port,
                    interact::CanvasAt {
                        area,
                        scale,
                        cx,
                        cy,
                    },
                ) {
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
        if let Some(Drag::Element { at, grab: true }) = self.drag {
            let events = element::with(host, ctx, at, |el, input| {
                el.drag_relative((0.0, dy), input)
            });
            if let Some(events) = events {
                let silent = events.is_empty();
                element::report(host, &mut out, ctx, at.id, events);
                if silent {
                    out.push(GestureEffect::Redraw(ctx.def_id));
                }
            }
            return out;
        }
        out
    }
}
