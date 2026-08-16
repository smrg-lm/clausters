//! What a **press** does: the containers' plans over the hit chain, then the
//! element under the cursor.
//!
//! The press is the phase that decides *which* gesture a pointer-down starts —
//! every [`Drag`] in the machine is opened here, and nowhere else.
//! It runs in two layers, and the split is the reason a widget's own behaviour
//! stays small: the **containers** over the point declare what a modifier means
//! on them ([`GestureStep`]), innermost first, and only when every step has
//! declined does the press reach the **element** — which is the one arm per
//! widget kind in [`Gestures::element_press`].
//!
//! Both layers are one match apiece rather than a function apiece, deliberately:
//! the arms are short (a hit-test, a snapshot, a `Drag`), and the exhaustive
//! match is what makes a new widget kind impossible to forget here.

use super::super::Host;
use super::super::interact::{self, Hit};
use super::super::widget::element::{PendingEdit, TimeSpace};
use super::super::widget::{Claim, GestureStep, WidgetKind};
use super::effects::*;
use super::nav::*;
use super::{Drag, GestureCtx, GestureEffect, Gestures, element, focus};
use clausters_core::osc::OscType;

impl Gestures {
    /// Press: run the **containers' gesture plans** over the hit, innermost
    /// first, until one of their steps consumes it.
    ///
    /// The order is the containers', not the widget's. Each container over the
    /// point declares what a modifier does on it ([`GestureMap`](super::super::widget::GestureMap)) — pan its axis,
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
        // An element that **declared** an overlay is modal: it is over
        // everything, so it is tested before the tree and it swallows the press
        // either way — on its own area it acts, anywhere else it closes, the
        // way a menu everywhere else behaves. It is asked for the point and
        // answers for both cases, since only it knows where its area is.
        if let Some((id, rect, scale)) = element::overlay_owner(host, ctx) {
            out.push(GestureEffect::Redraw(ctx.def_id));
            // An overlay stands over the window, on nobody's axis.
            let at = element::At::widget(id, rect, scale, 0.0);
            // Not through `element::press`: that door filters the point against
            // the element's declared shape, and an overlay is offered the press
            // **because it is outside** as often as because it is inside — a
            // click on the window closes the list. The shape filter answers
            // "is this widget's drawing under the pointer", which is the tree's
            // question, not a modal's.
            let claim = element::with(host, ctx, at, |el, input| el.press((cx, cy), input))
                .unwrap_or(Claim::Decline);
            if let Claim::Take(take) = claim {
                element::report(host, &mut out, ctx, id, take.events);
            }
            return out;
        }
        let Some(hit) = hit(host, ctx, cx, cy) else {
            // A press on empty space drops the focus (a caret disappears).
            focus::on_press(host, &mut out, ctx, None);
            self.pan_sole_axis(host, ctx, cx);
            return out;
        };
        // The focus follows the press: onto a widget that takes one, off
        // whatever held it otherwise. It is asked of the widget rather than
        // matched on its kind, so a registered element is a stop like any other.
        focus::on_press(host, &mut out, ctx, Some((hit.id, &hit.kind)));
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
        // A spectrum's **frequency** axis is grabbed on the axis itself, before
        // any modifier and before the chain: the element is nobody's container,
        // so there is no coordinate system over it to offer a pan, and dragging
        // its curve sideways can mean nothing else.
        if let Some(axis) = freq_axis(host, ctx, &hit)
            && axis.surface.contains(cx, cy)
        {
            self.drag = Some(Drag::PanX {
                id: hit.id,
                origin_x: cx,
                x_start: axis.start,
                body_w: axis.body.w.max(1.0) as f64,
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
                    action => {
                        self.container_press(host, ctx, frame, &hit, action, cx, cy, &mut out)
                    }
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
        hit: &interact::Hit,
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
            (
                step @ (GestureStep::Select | GestureStep::SelectBox),
                interact::Coords::Time(axis),
            ) => {
                if !axis.spans(cx) {
                    return false;
                }
                // The second axis, where the plan asked for it *and* the view
                // under it measures one. A `select_box` over a picture with one
                // measured axis declines rather than degrading, so a plan can
                // name both steps and get a rectangle where there is one to
                // draw and the plain span where there is not.
                let value = (step == GestureStep::SelectBox)
                    .then(|| value_axis(host, ctx, frame, hit).filter(|v| v.body.contains(cx, cy)))
                    .flatten();
                if step == GestureStep::SelectBox && value.is_none() {
                    return false;
                }
                // The press collapses the shared selection to the sample under
                // it; the drag sweeps from there. An element that sweeps a
                // *rectangle* over that span -- a roll picking the notes inside
                // it -- claims the press itself and asks for the selection
                // (`Events::and_select`), so the container's plan is the sweep
                // and never what is drawn in it.
                let anchor = interact::sample_at(
                    axis.nav.start,
                    axis.nav.len,
                    axis.body.x as f64,
                    axis.body.w as f64,
                    cx,
                );
                let anchor_v = value.map(|v| v.value_at(cy));
                set_selection(host, out, def_id, id, anchor, anchor, None);
                self.drag = Some(Drag::Select {
                    id,
                    body: axis.body,
                    nav_start: axis.nav.start,
                    nav_len: axis.nav.len,
                    anchor,
                    value: value.zip(anchor_v),
                });
                out.push(GestureEffect::Redraw(def_id));
                true
            }
            (GestureStep::Sample, interact::Coords::Time(axis)) => {
                if !axis.spans(cx) {
                    return false;
                }
                // A sample is grabbable exactly where it is **drawn**: the
                // trace marks each one with a disc only when they are far
                // enough apart to be told apart, and the same question decides
                // whether there is anything here to take hold of. Read from the
                // drawing's own rule rather than restated, so the two can never
                // drift into offering a grab on a picture that shows no points.
                let spacing = (axis.body.w as f64 / axis.nav.len.max(1e-9)) as f32;
                let radius = host.metrics_for(ctx.def_id).point_radius;
                if !crate::host::graphics::signal::trace::dots_fit(spacing, radius) {
                    return false;
                }
                let Some(value) =
                    value_axis(host, ctx, frame, hit).filter(|v| v.body.contains(cx, cy))
                else {
                    return false;
                };
                let frames = interact::sample_at(
                    axis.nav.start,
                    axis.nav.len,
                    axis.body.x as f64,
                    axis.body.w as f64,
                    cx,
                );
                if frames < 0.0 {
                    return false;
                }
                let index = frames.round().max(0.0) as usize;
                let channel = crate::host::frame::lane_at(value.body, value.lanes.max(1), cy);
                // What it is now, so the intent that leaves on release is
                // absolute *and* carries its own inverse.
                let Some(previous) = host
                    .window_def(ctx.def_id)
                    .and_then(|t| t.find(id))
                    .and_then(|w| w.kind.sample_value(channel, index))
                else {
                    return false; // an overview with no samples has none to grab
                };
                let held =
                    PendingEdit::one(channel, index, value.value_in(channel, cy) as f32, previous);
                if !set_pending(host, def_id, id, Some(held)) {
                    return false;
                }
                self.drag = Some(Drag::Sample {
                    id,
                    axis: value,
                    channel,
                    frame: index,
                    previous,
                });
                out.push(GestureEffect::Redraw(def_id));
                true
            }
            (GestureStep::Draw, interact::Coords::Time(axis)) => {
                if !axis.spans(cx) {
                    return false;
                }
                let Some(value) =
                    value_axis(host, ctx, frame, hit).filter(|v| v.body.contains(cx, cy))
                else {
                    return false;
                };
                // **Refused where a pixel is more than one sample**, and said
                // out loud: a stroke there would write values the reader cannot
                // see, and a pencil that silently does nothing teaches that it
                // sometimes does not work.
                let per_px = axis.nav.len / axis.body.w.max(1.0) as f64;
                if per_px > 1.0 {
                    emit(
                        host,
                        out,
                        def_id,
                        id,
                        vec![
                            OscType::String("refused".into()),
                            OscType::String("draw".into()),
                            OscType::String(format!(
                                "zoom in to draw: one pixel is {per_px:.0} samples"
                            )),
                        ],
                    );
                    return true; // consumed: the plan must not fall through to a sweep
                }
                let frames = interact::sample_at(
                    axis.nav.start,
                    axis.nav.len,
                    axis.body.x as f64,
                    axis.body.w as f64,
                    cx,
                );
                if frames < 0.0 {
                    return false;
                }
                let index = frames.round().max(0.0) as usize;
                let channel = crate::host::frame::lane_at(value.body, value.lanes.max(1), cy);
                let Some(previous) = host
                    .window_def(ctx.def_id)
                    .and_then(|t| t.find(id))
                    .and_then(|w| w.kind.sample_value(channel, index))
                else {
                    return false;
                };
                let v = value.value_in(channel, cy) as f32;
                if !set_pending(
                    host,
                    def_id,
                    id,
                    Some(PendingEdit::one(channel, index, v, previous)),
                ) {
                    return false;
                }
                self.drag = Some(Drag::Draw {
                    id,
                    axis: value,
                    body: axis.body,
                    nav_start: axis.nav.start,
                    nav_len: axis.nav.len,
                    channel,
                    last_frame: index,
                    last_value: v,
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

    /// Offers a press to the element `at` addresses: it takes it (and holds it
    /// from here, the drag carrying no geometry of its own because what the
    /// drag *means* is the element's) or declines and the press walks on.
    ///
    /// One function, because a widget and a container's **body** differ only in
    /// the address ([`element::At`]) — everything the machine does with the
    /// claim is the same, and a second copy of it is how the two would drift.
    #[allow(clippy::too_many_arguments)] // an address, the context, a cursor, the grab
    fn element_at(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        at: element::At,
        cx: f64,
        cy: f64,
        grab: &mut dyn FnMut() -> bool,
        out: &mut Vec<GestureEffect>,
    ) -> bool {
        let claim = element::press(host, ctx, at, cx, cy).unwrap_or(Claim::Decline);
        let Claim::Take(take) = claim else {
            return false;
        };
        let grab = take.grab && grab();
        self.drag = Some(Drag::Element {
            at,
            grab,
            edge: take.edge_scroll,
        });
        element::report(host, out, ctx, at.id, take.events);
        out.push(GestureEffect::Redraw(ctx.def_id));
        true
    }

    /// Offers a press to a clip's **element bodies**, topmost first.
    ///
    /// The container resolves the address and the coordinate system — a body
    /// fills the clip's rectangle and reads the clip's own axis, which is what
    /// the layout placed it on — and learns nothing else about what it holds.
    #[allow(clippy::too_many_arguments)] // a hit, the context, a cursor, the grab
    fn clip_body_press(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        h: &interact::ClipHit,
        cx: f64,
        cy: f64,
        grab: &mut dyn FnMut() -> bool,
        out: &mut Vec<GestureEffect>,
    ) -> bool {
        let Some(widget) = host.window_def(ctx.def_id).and_then(|t| t.find(h.id)) else {
            return false;
        };
        // Collected before the offer, because the press mutates the tree the
        // roles were read out of.
        let roles: Vec<_> = widget
            .children
            .iter()
            .rev()
            .filter(|c| matches!(c.kind, WidgetKind::Custom(_)))
            .filter_map(|c| c.kind.body_role())
            .collect();
        for body in roles {
            let at = element::At {
                id: h.id,
                body: Some(body),
                rect: h.rect,
                indent: 0.0,
                scale: 1.0,
                time: Some(TimeSpace::of(h.local, h.dur)),
            };
            if self.element_at(host, ctx, at, cx, cy, grab, out) {
                return true;
            }
        }
        false
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
            id,
            rect,
            scale,
            indent,
            ..
        } = *hit;
        let (chain, kind) = (&hit.chain, hit.kind.clone());
        let def_id = ctx.def_id;
        let effects_before = out.len();
        match kind {
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
                let snap = match host.widget_kind(def_id, lane.0) {
                    Some(WidgetKind::Track { snap, .. }) => *snap,
                    _ => 0.0,
                };
                // The clip's own axis, resolved by the layout and carried down
                // the hit chain — not re-derived from the lane's window here.
                let Some(local) = interact::local_time_of(chain) else {
                    return false;
                };
                if let Some(h) = interact::clip_hit(host, def_id, lane, local, cx) {
                    // The clip's **bodies** get the press first, topmost first:
                    // an element drawn on the clip's axis (an envelope's
                    // break-points) is what the pointer is on, and the clip's
                    // own move is what is under it. A body that declines hands
                    // it straight back, exactly as an element declining
                    // anywhere else does.
                    //
                    // **Except on a grip.** The strip at a clip's end is lit
                    // under the pointer precisely to say the press there
                    // resizes the clip, and an affordance that is drawn has to
                    // be the one that acts: offered to the bodies first, those
                    // dozen pixels went to whatever the body found under them
                    // — a note at the end of a roll clip moved instead of the
                    // clip's edge, from a cursor sitting on the arrow that
                    // promised otherwise. The body keeps everything that is not
                    // a grip, which is the whole clip whenever its ends are off
                    // screen or it is too narrow to carry one.
                    let on_grip = h.part != interact::ClipPart::Body;
                    if !on_grip && self.clip_body_press(host, ctx, &h, cx, cy, grab, out) {
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
            // A registered element gets the press on the live widget (the `kind`
            // matched above is the hit's copy), and answers the same way a
            // built-in arm does by hand: it consumed it, or it declines and the
            // press goes back up the chain. The claim is taken before anything
            // is delivered, so the element's borrow of the tree is over by the
            // time the event leaves.
            WidgetKind::Custom(_) => {
                return self.element_at(
                    host,
                    ctx,
                    element::At::widget(id, rect, scale, indent),
                    cx,
                    cy,
                    grab,
                    out,
                );
            }
            _ => {}
        }
        // Nothing the element wanted: the press goes back to the chain.
        self.drag.is_some() || out.len() > effects_before
    }
}
