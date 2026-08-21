//! What the **wheel** does: zoom and scroll, over whatever is under the cursor.
//!
//! The odd phase out — it opens no [`Drag`](super::Drag) and ends none, so it
//! reads the same chain a press does and acts immediately. Which axis it moves
//! is the container's, not the widget's: a navigable view zooms its time window
//! (its value window with the modifier), a scroll plane zooms or scrolls its
//! own, and a plain container passes the wheel outward.

use clausters_core::osc::OscType;

use super::super::interact::{self, Hit};
use super::super::widget::{Axis, WidgetKind};
use super::super::{Host, scroll};
use super::effects::*;
use super::nav::*;
use super::{GestureCtx, GestureEffect, Gestures, element};

/// One wheel event as the shell reported it, in the shell's own terms — the
/// two shapes every source has, and the only two this crate has to know.
///
/// It exists so the arithmetic below can live here, where the wheel's meaning
/// is, rather than in each shell: the module is platform-agnostic by rule
/// (no winit, no web-sys), so a shell translates its own event into this and
/// the conversion to *steps* is written once.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WheelDelta {
    /// Scroll in **lines** — an X11 wheel button, a browser's
    /// `DOM_DELTA_LINE`.
    Lines(f64),
    /// Scroll in **physical pixels** — a trackpad, a browser's
    /// `DOM_DELTA_PIXEL` (which winit has already multiplied by the window's
    /// scale factor before it reaches us).
    Pixels(f64),
}

/// What a **line** report means on a shell — the half of the calibration that
/// is not a number everywhere.
#[derive(Debug, Clone, Copy)]
pub enum Lines {
    /// One notch is this many lines, in a unit the shell shares with us. X11
    /// and Wayland send exactly one line per notch, so the count divides.
    Per(f64),
    /// The magnitude is the **viewer's own scroll preference** — how far a
    /// document moves per notch — so it is not ours to divide by. Measured at
    /// **6** in Firefox 153 on X11, uniform across every event, and it is a
    /// setting rather than a browser constant: the next machine may say 3.
    ///
    /// Nothing there is about zoom. No browser or OS carries a preference for
    /// how far a notch should *zoom*, and the browsers' own `ctrl`+wheel moves
    /// one step per notch whatever the scroll setting says. So the event is
    /// counted as the one notch it is and only its direction is read, which is
    /// also what makes it agree with `Per(1.0)` natively without either being
    /// tuned to the other.
    Notch,
}

/// What one wheel notch reports **on this shell**, and so what turns a scroll
/// report into zoom steps.
///
/// **A notch is a count, not a distance**, and that is the whole of why this
/// type exists. The same two lines were copied into all three shells —
/// `LineDelta(_, y) => y`, `PixelDelta(p) => p.y / 50.0` — and the divisor was
/// calibrated for the native trackpad, so a wheel click was one step in a
/// window and several in a page. The input was never normalized *per shell*,
/// and one divisor cannot be right for two sources.
///
/// **The pixel figure is logical**, which is the second half. winit hands a
/// browser's `deltaY` over already converted from CSS pixels to physical ones,
/// so the same notch on a 2x display reports twice the pixels — and dividing
/// that by a constant makes the zoom rate a property of the *display*. A
/// trackpad reports physical pixels natively for the same reason. So the scale
/// comes off before the divisor does.
///
/// **What the host does with a step is its own quantum** — `0.85^steps` for a
/// zoom, [`super::super::scroll::WHEEL_PAN_PX`] for a pan, `1.1^steps` for a
/// lane's height. None of them is a distance the platform has an opinion about,
/// which is why honouring a raw delta was never respecting a preference; it was
/// letting one leak into a place it means nothing.
#[derive(Debug, Clone, Copy)]
pub struct Wheel {
    /// What a line report means here.
    pub lines: Lines,
    /// **Logical** pixels one notch reports, for a report that is continuous
    /// and cannot be counted (a trackpad, a high-resolution wheel).
    pub pixels_per_step: f64,
}

impl Wheel {
    /// A window on a desktop: one line per notch, and the trackpad figure the
    /// original divisor was calibrated against.
    pub const NATIVE: Wheel = Wheel {
        lines: Lines::Per(1.0),
        pixels_per_step: 50.0,
    };

    /// A canvas in a page, where both halves vary with the browser: Firefox
    /// reports lines, Chrome reports pixels, and the same wheel gives both.
    ///
    /// The pixel figure is the browser's own step at `DOM_DELTA_PIXEL`,
    /// **measured at 120 CSS pixels** per notch in Chrome 151 on X11, identical
    /// across every event. It goes in as read: `deltaY` is already in CSS
    /// pixels, winit converts it to physical ones with the window's scale
    /// factor, and [`Self::steps`] divides that back out — so the ratio cancels
    /// and the constant stays in the browser's own unit.
    pub const BROWSER: Wheel = Wheel {
        lines: Lines::Notch,
        pixels_per_step: 120.0,
    };

    /// The zoom steps one event means: positive up, and one notch is one step
    /// on every shell, every browser and every display.
    ///
    /// `ui_scale` is the window's own (the page's device-pixel ratio, the
    /// desktop's scale factor), and it is what makes a pixel report comparable
    /// with the constant it is divided by. A line report never sees it: a line
    /// is not a length on screen.
    pub fn steps(&self, delta: WheelDelta, ui_scale: f64) -> f64 {
        match delta {
            WheelDelta::Lines(y) => match self.lines {
                Lines::Per(n) => y / n.max(f64::EPSILON),
                // `f64::signum` answers 1.0 for a zero, which would turn a
                // report of nothing into a zoom.
                Lines::Notch if y == 0.0 => 0.0,
                Lines::Notch => y.signum(),
            },
            WheelDelta::Pixels(y) => {
                let logical = y / ui_scale.max(f64::EPSILON);
                logical / self.pixels_per_step.max(f64::EPSILON)
            }
        }
    }
}

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
            indent,
            scale: found_scale,
        } = found;
        // **An element with a wheel of its own wins over the container it sits
        // in**: a keyboard's range, and whatever a registered element navigates
        // — a picture it owns is what the reader pointed at. `None` back means
        // it has none, and the container gets its turn below.
        if let WidgetKind::Custom(_) = kind {
            let at = element::At::widget(id, rect, found_scale, indent);
            // **On the shape it draws, like the press.** An element states one
            // hit shape and every pointer question reads it: the wheel over the
            // corner of a knob's cell used to reach the element rather than the
            // axis drawn behind it, which is the same mistake the press made
            // before it was filtered — and worse to meet, since a wheel is not
            // even aimed, it is where the hand happened to leave the pointer.
            let reported = element::with(host, ctx, at, |el, input| {
                el.hit_area(input)
                    .hit(cx, cy, input.metrics.hit_slop)
                    .then(|| el.wheel((cx, cy), (0.0, steps), input))
                    .flatten()
            })
            .flatten();
            if let Some(events) = reported {
                element::report(host, &mut out, ctx, id, events);
                return out;
            }
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
                    host,
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
                // The vertical anchor depends on what the axis *measures*, and
                // the widget is what knows ([`WidgetKind::centres_y_zoom`]):
                // an amplitude axis holds its own centre, so zero stays at the
                // centre of every lane, and an axis of values (frequency,
                // pitch) holds the value under the cursor. Only the second is
                // arithmetic the machine can do, since it is the lane geometry
                // it already has.
                Some(y) => {
                    let anchor = if kind.centres_y_zoom() {
                        0.5
                    } else {
                        let lane_top = axis.body.y as f64
                            + ((cy - axis.body.y as f64) / y.lane_h).floor() * y.lane_h;
                        1.0 - ((cy - lane_top) / y.lane_h).clamp(0.0, 1.0)
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
        // wheel, they are not empty: the reader pointed at that element (it was
        // asked above and declined). The press path shares the mechanism and
        // means it differently — Shift+drag pans the axis from anywhere at all,
        // over any element — so the question is asked here and not there.
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
                            host,
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

#[cfg(test)]
mod wheel_tests {
    use super::{Lines, Wheel, WheelDelta};

    /// **One notch is one step**, on either shell, in either browser and on any
    /// display. The defect was felt as a page zooming faster than a window, so
    /// the assertion is the two agreeing rather than either one's arithmetic.
    #[test]
    fn a_notch_is_a_step_on_every_shell_and_every_display() {
        // A desktop wheel reports one line per notch.
        let native = Wheel::NATIVE.steps(WheelDelta::Lines(1.0), 1.0);
        assert!((native - 1.0).abs() < 1e-9, "a wheel click is one step");

        // Firefox reports lines too, but the count is the viewer's scroll
        // preference: 6 on the machine this was measured on, and a setting
        // rather than a constant. The preference is not read, so a machine set
        // to 3 -- or to 16 -- zooms exactly as far.
        for preference in [1.0, 3.0, 6.0, 16.0] {
            let page = Wheel::BROWSER.steps(WheelDelta::Lines(preference), 1.0);
            assert!(
                (page - native).abs() < 1e-9,
                "a page at {preference} lines a notch zoomed {page} where a window zoomed {native}"
            );
        }
        // Direction survives the counting, and nothing is not a notch.
        assert!(Wheel::BROWSER.steps(WheelDelta::Lines(-6.0), 1.0) < 0.0);
        assert_eq!(Wheel::BROWSER.steps(WheelDelta::Lines(0.0), 1.0), 0.0);

        // Chrome reports pixels, where the magnitude *is* the information: a
        // trackpad's stream cannot be counted, so that half stays a divisor.
        // 120 CSS pixels a notch, measured. winit hands them over multiplied by
        // the window's scale factor, which is what the second argument undoes —
        // so the notch is a step on every display rather than on one.
        for ratio in [1.0, 1.5, 2.0, 2.5] {
            let pixels = Wheel::BROWSER.steps(WheelDelta::Pixels(120.0 * ratio), ratio);
            assert!(
                (pixels - native).abs() < 1e-9,
                "a page at devicePixelRatio {ratio} zoomed {pixels} where a window zoomed {native}"
            );
        }

        // A fraction of a notch is a fraction of a step: a trackpad's fine
        // scroll is not quantized here.
        assert!(Wheel::NATIVE.steps(WheelDelta::Lines(-1.0), 1.0) < 0.0);
        let half = Wheel::NATIVE.steps(WheelDelta::Pixels(25.0), 1.0);
        assert!((half - 0.5).abs() < 1e-9, "a half notch is half a step");
    }

    /// A scale of zero (a shell that has not reported one yet) must not divide
    /// the zoom to infinity, and neither must a calibration of zero: the guards
    /// are in the arithmetic, not at the call sites, since there are three of
    /// those.
    #[test]
    fn an_unreported_scale_does_not_run_away() {
        assert!(
            Wheel::BROWSER
                .steps(WheelDelta::Pixels(100.0), 0.0)
                .is_finite()
        );
        let zeroed = Wheel {
            lines: Lines::Per(0.0),
            pixels_per_step: 0.0,
        };
        assert!(zeroed.steps(WheelDelta::Lines(1.0), 1.0).is_finite());
        assert!(zeroed.steps(WheelDelta::Pixels(1.0), 1.0).is_finite());
    }
}
