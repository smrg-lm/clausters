//! What the **wheel** does: zoom and scroll, over whatever is under the cursor.
//!
//! The odd phase out — it opens no [`Drag`](super::Drag) and ends none, so it
//! reads the same chain a press does and acts immediately. Which axis it moves
//! is the container's, not the widget's: a navigable view zooms its time window
//! (its value window with the modifier), a scroll plane zooms or scrolls its
//! own, and a plain container passes the wheel outward.

use clausters_core::osc::OscType;

use super::super::interact::{self, Hit};
use super::super::signal::Presentation;
use super::super::widget::{Axis, WidgetKind};
use super::super::{Host, piano, scroll};
use super::effects::*;
use super::nav::*;
use super::{GestureCtx, GestureEffect, Gestures};

impl Gestures {
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
        let Some(found) = hit(host, ctx, cx, cy) else {
            return out;
        };
        // A spectrum zooms its **frequency** axis, anchored at the cursor: the
        // one navigable axis in the host that is not the window's time, and the
        // one that needs no history behind it — every bin is there every frame.
        if let Some(axis) = freq_axis(host, ctx, &found)
            && axis.surface.contains(cx, cy)
        {
            let factor = 0.85f64.powf(steps);
            zoom_freq(host, &mut out, def_id, found.id, axis, cx, factor);
            return out;
        }
        let Hit {
            id,
            rect,
            kind,
            chain,
            ..
        } = found;
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
        // Nothing under the pointer claimed the wheel. The fall-through is for
        // pixels with **nothing drawn on them** — a gap between lanes, the
        // slack under the last one, a container's margin — where in a window
        // with one axis those pixels *are* that axis, so the wheel means there
        // what it means over a lane: Ctrl the lanes' thickness, otherwise the
        // time zoom, anchored at the cursor.
        //
        // Over an element that draws a picture of its own and simply has no
        // wheel, they are not empty: the reader pointed at that element. The
        // press path shares the mechanism and means it differently — Shift+drag
        // pans the axis from anywhere at all, over any element — so the
        // question is asked here and not there.
        if !kind.is_bare_surface() {
            return out;
        }
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
}
