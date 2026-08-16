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

use super::super::Host;
use super::super::interact::{self};
use super::super::timeline;
use super::super::widget::{Axis, WidgetKind};
use super::effects::*;
use super::nav::*;
use super::{Drag, GestureCtx, GestureEffect, Gestures, element};
use clausters_core::osc::OscType;

impl Gestures {
    /// Advances an edge-held drag by `dt` seconds: pans the group's window in
    /// the held direction and re-applies the drag at the standing cursor, so
    /// what is being dragged travels with the view.
    ///
    /// This is what lets a clip — or a note — be moved further than one
    /// window's worth. The drag itself maps the cursor through the *current*
    /// window, so panning is the whole mechanism: nothing here touches the
    /// placement math, and an element that asked for this reads its axis from
    /// [`Input::time`](crate::host::widget::element::Input::time) each step for
    /// exactly the same reason.
    pub fn tick(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        cx: f64,
        cy: f64,
        dt: f64,
    ) -> Vec<GestureEffect> {
        let mut out = Vec::new();
        let dir = self.edge_direction(cx);
        if dir == 0.0 || dt <= 0.0 {
            return out;
        }
        // An element's edge scroll is the pan plus an ordinary drag step: the
        // element mutates itself against the window it is left with, which is
        // the same mechanism the clip's arm below spells out by hand because a
        // clip is a container and keeps its own drag.
        if let Some(Drag::Element { at, edge: true, .. }) = self.drag {
            let Some((start, len, _)) = group_view(host, at.id) else {
                return out;
            };
            let roots = host.pan_timeline(at.id, start + dir * len * EDGE_SCROLL_PER_SEC * dt);
            redraw_all(&mut out, &roots);
            emit_view(host, &mut out, ctx.def_id, at.id);
            let events = element::with(host, ctx, at, |el, input| el.drag((cx, cy), input));
            if let Some(events) = events {
                element::report(host, &mut out, ctx, at.id, events);
            }
            return out;
        }
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
                // The second axis, where the view has one. A sweep that never
                // left its height names one value twice, which `value_span`
                // reads as the empty range it is: a horizontal drag restricts
                // nothing, and only a rectangle reports a band.
                let range = value.and_then(|(axis, from)| {
                    let (min, max) = timeline::value_span(
                        from,
                        axis.value_at(cy),
                        (axis.domain.0 as f64, axis.domain.1 as f64),
                    );
                    (max > min && !axis.is_whole(min, max)).then_some((min, max))
                });
                set_selection(host, &mut out, def_id, id, anchor, cur, range);
            }
            Drag::Sample {
                id,
                axis,
                channel,
                frame,
                previous,
            } => {
                // Only the value follows the pointer: which sample is held was
                // decided at the press, and a view scrolling under the drag must
                // not hand the hand a different one.
                let held = crate::host::widget::element::PendingEdit::one(
                    channel,
                    frame,
                    axis.value_at(cy) as f32,
                    previous,
                );
                set_pending(host, def_id, id, Some(held));
                out.push(GestureEffect::Redraw(def_id));
            }
            Drag::Draw {
                id,
                axis,
                body,
                nav_start,
                nav_len,
                channel,
                last_frame,
                last_value,
            } => {
                let (start, len) =
                    group_view(host, id).map_or((nav_start, nav_len), |(s, l, _)| (s, l));
                // **The stroke stops at the edge of the view**, because that is
                // the same rule the pencil is refused under: it writes what the
                // reader can see. A hand that slides off the picture — or out
                // of the window, where the pointer keeps reporting — would
                // otherwise go on rewriting samples nobody is looking at, and
                // the damage is only discovered by scrolling there. Clamped
                // rather than stopped, so the last visible column still follows
                // the hand and the stroke ends where the eye does.
                let cx = cx.clamp(body.x as f64, (body.x + body.w) as f64);
                let frames =
                    interact::sample_at(start, len, body.x as f64, body.w as f64, cx).max(0.0);
                // ...and inside what exists: the right edge of a view showing
                // the whole material maps to one *past* its last sample, and a
                // stroke carrying that frame is refused whole by the owner.
                let last = host
                    .material_frames(def_id, id)
                    .unwrap_or(0)
                    .saturating_sub(1);
                let now = (
                    (frames.round() as u64).min(last) as usize,
                    axis.value_at(cy) as f32,
                );
                extend_stroke(host, def_id, id, channel, (last_frame, last_value), now);
                self.drag = Some(Drag::Draw {
                    id,
                    axis,
                    body,
                    nav_start,
                    nav_len,
                    channel,
                    last_frame: now.0,
                    last_value: now.1,
                });
                out.push(GestureEffect::Redraw(def_id));
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
        // **One arm**, which is what the port left behind: every other drag the
        // machine holds is a container's navigation, and a plan acts along the
        // way rather than at the end.
        // A stroke leaves as **one intent**, whatever it passed over: one
        // gesture is one edit, and what the hand did on the way is the pending
        // drawing's business rather than the owner's.
        if let Some(Drag::Draw { id, channel, .. }) = self.drag.clone() {
            self.drag = None;
            let held = host
                .window_def(def_id)
                .and_then(|t| t.find(id))
                .and_then(|w| w.kind.pending_edit())
                .cloned();
            if let Some(held) = held {
                // The samples ride as **blobs**, the convention every bulk
                // payload in this system already follows: a stroke over a few
                // thousand samples as typed arguments is the encode this rule
                // exists to avoid. Both runs go, so the intent carries its own
                // inverse exactly as a single dragged sample does.
                emit(
                    host,
                    &mut out,
                    def_id,
                    id,
                    vec![
                        OscType::String("draw".into()),
                        OscType::Int(channel as i32),
                        OscType::Long(held.start as i64),
                        OscType::Blob(samples_blob(&held.values)),
                        OscType::Blob(samples_blob(&held.previous)),
                    ],
                );
            }
            out.push(GestureEffect::Redraw(def_id));
            return out;
        }
        // A dragged sample leaves as **one intent at the end**, not a stream of
        // them: one gesture is one edit, and what the hand did on the way is
        // the pending drawing's business rather than the owner's.
        if let Some(Drag::Sample {
            id,
            axis,
            channel,
            frame,
            previous,
        }) = self.drag.clone()
        {
            self.drag = None;
            let value = axis.value_at(cy) as f32;
            // The pending stays until the owner answers: dropping it here would
            // snap the picture back to the material for as long as the round
            // trip takes, which reads as the edit having been refused.
            set_pending(
                host,
                def_id,
                id,
                Some(crate::host::widget::element::PendingEdit::one(
                    channel, frame, value, previous,
                )),
            );
            emit(
                host,
                &mut out,
                def_id,
                id,
                vec![
                    OscType::String("sample".into()),
                    OscType::Int(channel as i32),
                    // A sample index is exact or it is the wrong sample: a
                    // float runs out of integers at 16.7 million, which is six
                    // minutes of audio.
                    OscType::Long(frame as i64),
                    OscType::Float(value),
                    OscType::Float(previous),
                ],
            );
            out.push(GestureEffect::Redraw(def_id));
            return out;
        }
        if let Some(Drag::Element { at, grab, .. }) = self.drag.take() {
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
        if let Some(Drag::Element { at, grab: true, .. }) = self.drag {
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

/// Little-endian `f32` bytes — the one bulk payload convention this system has,
/// shared with `/buffer_setRange`, `/buffer_getRange.reply` and the clipboard.
fn samples_blob(values: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}
