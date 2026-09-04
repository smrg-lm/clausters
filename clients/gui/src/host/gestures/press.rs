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
use super::super::layers;
use super::super::placement::Placements;
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
    pub fn press(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        cx: f64,
        cy: f64,
    ) -> Vec<GestureEffect> {
        let mut out = Vec::new();
        // **One press per gesture**, which is the single-pointer rule the touch
        // slot already states for fingers and nothing stated for the pointer. A
        // press arriving while a drag is in flight is never a new gesture: it is
        // a second button chorded onto the first, or a stream that repeated the
        // one already in hand -- and a browser's does. Winit turns any
        // `pointermove` carrying a button (`PointerEvent.button != -1`) into a
        // synthesized `MouseInput` whose state is *pressed* while that button is
        // down, so a drag arrives as a fresh press **per frame**; taking it
        // re-runs every press-time decision, and anything anchored at the press
        // -- a bend's origin, a note's press time, a clip's grab sample -- is
        // re-anchored to where the pointer is now. That is the incremental drift
        // the absolute forms were written to end, coming back in through the
        // door beside them.
        //
        // Here rather than in the browser front, because the fronts must hand
        // this machine the same press -> drag -> release and the rule is the
        // machine's, not a platform's. The one thing a front still owes it is a
        // release it can lose (`web::input`, the pointer that comes up outside
        // the window), since without one the drag below would never end.
        if self.dragging() {
            return out;
        }
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
                        self.element_press(host, ctx, &hit, cx, cy, &mut out)
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
                // The element under the sweep answers what its rectangle caught
                // -- a roll's notes -- and a press is that rectangle at no size,
                // which is what lets go of what it held.
                let element = element::At::widget(hit.id, hit.rect, hit.scale, hit.indent);
                sweep_element(host, ctx, element, (cx, cy), (cx, cy));
                self.drag = Some(Drag::Select {
                    id,
                    body: axis.body,
                    nav_start: axis.nav.start,
                    nav_len: axis.nav.len,
                    anchor,
                    origin_x: cx,
                    origin_y: cy,
                    value: value.zip(anchor_v),
                    element: Some(element),
                });
                out.push(GestureEffect::Redraw(def_id));
                true
            }
            // **The marquee**: the objects the rectangle covers, and no span.
            // On a stack of lanes that is the clips it crosses -- the patcher's
            // gesture one level up, and the same `Drag::Marquee` -- while a
            // *time range* over the same lanes is the other selection and is
            // `Select` above.
            (GestureStep::Marquee, interact::Coords::Time(axis)) => {
                if !axis.spans(cx) {
                    return false;
                }
                let lanes = super::nav::MarqueeLanes {
                    id,
                    body: axis.body,
                    nav_start: axis.nav.start,
                    nav_len: axis.nav.len,
                    // The stack this sweep can cross: read at the press, like a
                    // clip drag's, and a lane of its own where there is none.
                    stack: lane_stack(host, ctx, id),
                };
                // A press is the rectangle at no size, so it covers nothing and
                // the hand lets go of whatever it held -- the one rule every
                // view answers a click with.
                marquee_caught(host, ctx, None, Some(&lanes), (cx, cy), (cx, cy));
                self.drag = Some(Drag::Marquee {
                    at: None,
                    lanes: Some(lanes),
                    origin: (cx, cy),
                    cursor: (cx, cy),
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
    fn element_at(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        at: element::At,
        cx: f64,
        cy: f64,
        out: &mut Vec<GestureEffect>,
    ) -> bool {
        let claim = element::press(host, ctx, at, cx, cy).unwrap_or(Claim::Decline);
        let Claim::Take(take) = claim else {
            return false;
        };
        // An element that asked for a **marquee** gets the machine's, not a
        // drag of its own: the press is where its bare canvas is, and
        // everything after it is the one sweep every view shares.
        self.drag = Some(if take.marquee {
            marquee_caught(host, ctx, Some(at), None, (cx, cy), (cx, cy));
            Drag::Marquee {
                at: Some(at),
                lanes: None,
                origin: (cx, cy),
                cursor: (cx, cy),
            }
        } else {
            Drag::Element {
                at,
                edge: take.edge_scroll,
            }
        });
        element::report(host, out, ctx, at.id, take.events);
        out.push(GestureEffect::Redraw(ctx.def_id));
        true
    }

    /// **Which layer a press on a container belongs to, and the press given to
    /// it** — the whole of the interaction rule, in one place.
    ///
    /// The layer is resolved from what is drawn under the pointer
    /// ([`layers::under_pointer`]), made active, and then — and only then —
    /// offered the press. A container that layers editable things has as many
    /// claimants as it has layers plus its own placement, and this is the one
    /// decision between them: everything else in the machine acts on the layer
    /// that came back, so no pass has to know what kinds of thing the container
    /// holds.
    ///
    /// Returns `true` when a **content** layer took the press. The placement
    /// layer's own gesture (the move, the edges) is the caller's, because it is
    /// the container's and not an element's.
    fn clip_layer_press(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        h: &interact::ClipHit,
        cx: f64,
        cy: f64,
        out: &mut Vec<GestureEffect>,
    ) -> bool {
        // The clip's own axis, which is the coordinate system every layer of it
        // is drawn and grabbed against.
        let time = TimeSpace::of(h.local, h.dur);
        let at = element::At {
            id: h.id,
            layer: None,
            rect: h.rect,
            indent: 0.0,
            scale: 1.0,
            time: Some(time),
        };
        // **The placement layer keeps the pixels it draws its own affordance
        // on**, which is the same rule the content layers get from
        // `layers::under_pointer` (the active layer is asked first): a grip is
        // drawn only while the placement is active, and what is lit is what the
        // press takes — a note under the end of a roll clip does not steal the
        // edge from the cursor sitting on the arrow that promised otherwise.
        let placement_active = host
            .window_def(ctx.def_id)
            .and_then(|t| t.find(h.id))
            .is_some_and(|w| layers::active(w) == layers::Layer::Placement);
        if placement_active && h.part != interact::Part::Body {
            return false;
        }
        let Some(layer) = element::layer_under_pointer(host, ctx, at, cx, cy) else {
            return false;
        };
        // Selecting is its own operation and says so when it changed: a script
        // that follows the selection (to show a layer's own inspector, to move
        // a menu) hears the same word it would have sent.
        let announced = host
            .window_def_mut(ctx.def_id)
            .and_then(|t| t.find_mut(h.id))
            .and_then(|w| {
                let mut sel = layers::Selection::of(w)?;
                sel.set(layer).then(|| layer.name(w))
            });
        if let Some(name) = announced {
            deliver_args(
                host,
                out,
                ctx.def_id,
                h.id,
                Some(vec![OscType::String("layer".into()), OscType::String(name)]),
            );
            out.push(GestureEffect::Redraw(ctx.def_id));
        }
        if !matches!(layer, layers::Layer::Content(_)) {
            return false;
        }
        // Delivered on the layer's **own** frame: the container's rectangle
        // and axis when the layer fills it, its own stretch when it names one.
        let at = element::layer_frame(
            host,
            ctx,
            element::At {
                layer: Some(layer),
                time: Some(time),
                ..at
            },
        );
        self.element_at(host, ctx, at, cx, cy, out)
    }

    /// The press the containers handed down: what the widget under the cursor
    /// does with it — a control's value, a note, a break-point, a clip, a piano
    /// key, a cord. Returns whether it was consumed; declining (empty space in
    /// a lane, a patch's bare canvas) hands the press back to the chain.
    fn element_press(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        hit: &Hit,
        cx: f64,
        cy: f64,
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
                    // **Which layer the press belongs to is decided first**,
                    // and everything below this line is the placement layer's
                    // — the move and the edges. A press on a content layer's
                    // own contents (an envelope's break-points, a note) selects
                    // that layer and is that layer's; a press on the clip's
                    // background is on no layer's contents, which is what
                    // leaves it, and the grips with it, to the clip itself.
                    //
                    // The grips need no exception here any more, and that is
                    // the point of the rule: they are drawn only while the
                    // placement is the active layer, so the pixels that light
                    // up and the pixels that resize are the same pixels by
                    // construction rather than by a precedence written down
                    // twice.
                    if self.clip_layer_press(host, ctx, &h, cx, cy, out) {
                        return true;
                    }
                    // **Alt adds or removes one clip**, and consumes the
                    // press: a selection built one box at a time is not a drag,
                    // which is the rule the roll's notes already follow — with
                    // the same key, since Alt is what adds a *note* to a roll's
                    // selection. Which of the two a press means is the layer
                    // question, already answered above: an Alt press that landed
                    // on a body's own contents never reaches here.
                    if ctx.alt && h.part == interact::Part::Body {
                        let held = host
                            .window_def(def_id)
                            .and_then(|t| t.find(h.id))
                            .is_some_and(|w| w.selected);
                        interact::set_clip_selected(host, def_id, h.id, !held);
                        out.push(GestureEffect::Redraw(def_id));
                        return true;
                    }
                    // **Grabbing a selected clip moves the whole selection**;
                    // grabbing an unselected one lets go of it and moves
                    // singly. A trim is always one clip's: two clips of
                    // different lengths have no one edge to pull.
                    let stack = lane_stack(host, ctx, h.lane);
                    let block = clip_block(host, def_id, &stack, h.lane, h.id, h.part);
                    if block.is_empty() && interact::clear_clip_selection(host, def_id) {
                        out.push(GestureEffect::Redraw(def_id));
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
                        press_lane: h.lane,
                        part: h.part,
                        body_x: h.body.x as f64,
                        body_w: h.body.w as f64,
                        nav_start: h.nav.start,
                        nav_len: h.nav.len,
                        press_sample,
                        orig: h.placement,
                        contents: h.contents,
                        grid: snap,
                        block,
                        stack,
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
                    out,
                );
            }
            _ => {}
        }
        // Nothing the element wanted: the press goes back to the chain.
        self.drag.is_some() || out.len() > effects_before
    }
}

/// **The block a clip press takes hold of**: per lane of the stack, the
/// press-time `(index, offset, row)` of every selected clip on it — the
/// snapshot shape `placement::move_block` moves, and the same one a roll builds
/// for a block of notes. The grabbed clip's lane leads, and the grabbed clip
/// leads inside it, because it is the snap anchor the rest keep their distance
/// from.
///
/// **A selection is not one lane's**, so neither is the block: a marquee down
/// the stack takes clips of several lanes and grabbing any one of them moves
/// all of them, which is the rule the patcher already states for a set of boxes
/// on a plane. A lane of the stack holding nothing selected contributes
/// nothing.
///
/// Empty when the grabbed clip is not selected (the press moves it alone) or
/// when an **edge** was grabbed: a trim is one clip's, since two clips of
/// different lengths have no one edge to pull.
fn clip_block(
    host: &mut Host,
    def_id: i32,
    stack: &LaneStack,
    lane_id: i32,
    clip_id: i32,
    part: interact::Part,
) -> super::nav::ClipBlock {
    if part != interact::Part::Body {
        return Vec::new();
    }
    // The grabbed lane first, then the rest of the stack in order; a stack that
    // was never read is the grabbed lane alone.
    let lanes = std::iter::once(lane_id)
        .chain(stack.ids.iter().copied().filter(|id| *id != lane_id))
        .collect::<Vec<i32>>();
    let mut out: super::nav::ClipBlock = Vec::new();
    for lane in lanes {
        let Some(w) = host.window_def_mut(def_id).and_then(|t| t.find_mut(lane)) else {
            continue;
        };
        let clips = crate::host::graphics::track::LaneClips::of(w, 0.0);
        // The grabbed clip leads its own lane; the others are in drawing order.
        let grabbed = (lane == lane_id)
            .then(|| clips.index_of(clip_id).filter(|&i| clips.is_selected(i)))
            .flatten();
        if lane == lane_id && grabbed.is_none() {
            // The press moves an unselected clip alone, whatever else is held.
            return Vec::new();
        }
        let held: super::nav::HeldClips = grabbed
            .into_iter()
            .chain((0..clips.len()).filter(|&i| Some(i) != grabbed && clips.is_selected(i)))
            .map(|i| (i, clips.placement(i).offset, Placements::row(&clips, i)))
            .collect();
        if !held.is_empty() {
            out.push((lane, held));
        }
    }
    out
}
