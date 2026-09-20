//! What a **press** does: the containers' plans over the hit chain, then the
//! element under the cursor.
//!
//! The press is the phase that decides *which* gesture a pointer-down starts --
//! every [`Drag`] in the machine is opened here, and nowhere else.
//! It runs in two layers, and the split is the reason a widget's own behaviour
//! stays small: the **containers** over the point declare what a modifier means
//! on them ([`GestureStep`]), innermost first, and only when every step has
//! declined does the press reach the **element** -- which is the one arm per
//! widget kind in [`Gestures::element_press`].
//!
//! Both layers are one match apiece rather than a function apiece, deliberately:
//! the arms are short (a hit-test, a snapshot, a `Drag`), and the exhaustive
//! match is what makes a new widget kind impossible to forget here.

use super::super::Host;
use super::super::graphics::signal::trace;
use super::super::interact::{self, Hit};
use super::super::widget::element::PendingEdit;
use super::super::widget::{Claim, GestureStep, WidgetKind};
use super::effects::*;
use super::nav::*;
use super::{Drag, GestureCtx, GestureEffect, Gestures, element, focus};

/// The reasons the two sample-editing arms give, written once because both give
/// them and a reason spelled twice is a reason that will be spelled two ways.
///
/// **What earns a reason, and what earns a decline.** A step that finds nothing
/// to act on where the picture is perfectly good -- the samples are not drawn one
/// by one, the view starts before the take does -- is not this gesture's press to
/// take: it declines and the plan tries its next step, which is what
/// `"sample select"` is composed of. A step that finds the *picture* wrong -- a
/// view holding no samples at all, one that cannot hold an edit in flight -- has
/// hit a fault, and a fault is said out loud and consumed, because falling
/// through there is how a pencil silently becomes a selection tool.
///
/// A refusal that reads as an internal note is still better than the silence it
/// replaced: it tells the reader the gesture arrived and did not work, which is
/// the one thing the silence could not.
const BEFORE_THE_START: &str = "there is no sample here: the view starts before the take does";
const NO_SAMPLES: &str = "these samples are not loaded yet: the view is drawing a summary, and a stroke needs the \
     frames themselves";
const NO_PENDING: &str = "this view cannot hold an edit in flight";

/// **Zoom in until the samples are dots** -- the one refusal with something to
/// aim at, in two readings of one fact, each in the unit that is legible in its
/// own range: far out a pixel holds many samples, and close in a sample holds a
/// fraction of a pixel.
fn zoom_in(per_px: f64, radius: f32) -> String {
    let need = 1.0 / trace::drawable_per_px(radius);
    if per_px > 1.0 {
        format!(
            "zoom in until the samples are dots: one pixel is {per_px:.0} samples, and a dot \
             needs {need:.0} px per sample"
        )
    } else {
        format!(
            "zoom in until the samples are dots: one sample is {:.1} px, and a dot needs \
             {need:.0}",
            1.0 / per_px.max(f64::MIN_POSITIVE)
        )
    }
}

impl Gestures {
    /// Press: run the **containers' gesture plans** over the hit, innermost
    /// first, until one of their steps consumes it.
    ///
    /// The order is the containers', not the widget's. Each container over the
    /// point declares what a modifier does on it ([`GestureMap`](super::super::widget::GestureMap)) -- pan its axis,
    /// sweep a selection, locate the transport, or hand the press to the
    /// element under the cursor -- and a step that declines passes the press on,
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
        // **Which press in a run this is**, counted before anything else can
        // return early: a double click is two presses close in time and place,
        // and the run has to be tracked whether or not either press ends up
        // reaching an element.
        self.count_press(ctx, cx, cy);
        self.click = None;
        // An element that **declared** an overlay is modal: it is over
        // everything, so it is tested before the tree and it swallows the press
        // either way -- on its own area it acts, anywhere else it closes, the
        // way a menu everywhere else behaves. It is asked for the point and
        // answers for both cases, since only it knows where its area is.
        if let Some((id, rect, scale)) = element::overlay_owner(host, ctx) {
            out.push(GestureEffect::Redraw(ctx.def_id));
            // An overlay stands over the window, on nobody's axis.
            let at = element::At {
                clicks: self.clicks(),
                ..element::At::widget(id, rect, scale, 0.0)
            };
            // Not through `element::press`: that door filters the point against
            // the element's declared shape, and an overlay is offered the press
            // **because it is outside** as often as because it is inside -- a
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
        // **The status bar**, which is chrome and not a widget: it is under no
        // part of the tree (the layout never got its pixels), so it is tested
        // here and it consumes the press. A click opens it into the window's
        // log area and the next one closes it again.
        if let Some(band) = host.status_bar_rect(ctx.def_id, ctx.fb_w, ctx.fb_h)
            && band.contains(cx, cy)
        {
            let open = !host.status_open(ctx.def_id);
            host.set_status_open(ctx.def_id, open);
            out.push(GestureEffect::Redraw(ctx.def_id));
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
                row_h: y.row_h,
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
        // **The click is the machine's, not a step's**, and what it places is
        // the **position cursor** -- so where the press landed on the axis is
        // read here, once, whatever the plans below do with it (see
        // [`Gestures::release`]).
        //
        // **A click places the mark where it landed on nothing.** The cursor
        // says where the reader is -- where a playback starts and where a paste
        // lands -- so it is never a side effect of pointing *at* something: a
        // press that an element takes (a box, a note, a curve, a header
        // control) is that thing's, and the mark stays where it was. What is
        // left is the ruler and the slack -- the space between boxes, a lane's
        // empty tail, a grid nothing is drawn on -- and clicking there is the
        // ordinary way to say "here", without reaching for the strip at the top
        // of the window every time. The press below clears this again if an
        // element claims it. Beside the axis -- a lane's header -- there is no
        // position at all.
        self.click = hit.chain.iter().rev().find_map(|f| match (f.id, f.coords) {
            (Some(id), interact::Coords::Time(axis)) if axis.spans(cx) => Some(super::Click {
                id,
                body: axis.body,
                ruler: f
                    .ruler
                    .then(|| crate::host::frame::ruler_strip(f.rect, axis.body)),
                origin_x: cx,
            }),
            _ => None,
        });
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
                    // **The steps that already answered for this pixel.** A
                    // locate has put the cursor there itself, and the three
                    // editing steps wrote something *at* the press: neither is a
                    // click looking for a place, so the release adds none.
                    //
                    // And an **element** that took the press had something under
                    // the pointer -- a box, a note, a curve, a control -- so the
                    // click is that thing's and not a place. What is left for
                    // the mark is the ruler and the slack.
                    if matches!(
                        step,
                        GestureStep::Locate
                            | GestureStep::Marker
                            | GestureStep::Sample
                            | GestureStep::Draw
                            | GestureStep::Element
                    ) {
                        self.click = None;
                    }
                    return out;
                }
            }
        }
        // Nobody took it.
        self.pan_sole_axis(host, ctx, cx);
        out
    }

    /// Shift+drag means "pan the axis" wherever it starts, so in a window with
    /// **one** navigation group it means that off the lanes too -- the gap
    /// between them, the slack under the last one, a container's margin, the
    /// window's own edge. Returns whether it grabbed.
    fn pan_sole_axis(&mut self, host: &Host, ctx: &GestureCtx, cx: f64) -> bool {
        if !ctx.shift {
            return false;
        }
        let Some(sole) =
            interact::sole_time_axis(host, ctx.def_id, ctx.fb_w, ctx.fb_h, &|id, kind| {
                ctx.rows(id, kind)
            })
        else {
            return false;
        };
        self.drag = Some(Drag::Pan {
            id: sole.id,
            origin_x: cx,
            start: sole.axis.nav.start,
            body: sole.axis.body,
        });
        true
    }

    /// One container-level step of a press: the gestures that belong to the
    /// coordinate system rather than to what is drawn in it. Each reads the
    /// frame the chain resolved -- the axis' own body, window and view state --
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
                    body: axis.body,
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
            // **The marquee**: the objects the rectangle covers, and no span --
            // the patcher's gesture and the multitrack's, one `Drag::Marquee`,
            // while a *time range* over the same view is the other selection
            // and is `Select` above.
            (GestureStep::Marquee, interact::Coords::Time(axis)) => {
                if !axis.spans(cx) {
                    return false;
                }
                // The element under it is what answers: a rectangle asks
                // whoever holds the contents -- a roll's notes, a patcher's
                // boxes, a multitrack's clips.
                let element = element::At::widget(hit.id, hit.rect, hit.scale, hit.indent);
                // A press is the rectangle at no size, so it covers nothing and
                // the hand lets go of whatever it held -- the one rule every
                // view answers a click with.
                marquee_caught(host, ctx, Some(element), (cx, cy), (cx, cy));
                self.drag = Some(Drag::Marquee {
                    at: Some(element),
                    origin: (cx, cy),
                    cursor: (cx, cy),
                });
                out.push(GestureEffect::Redraw(def_id));
                true
            }
            // **The ruler's one edit.** A press adds a marker at the time it
            // points at, or takes away the one it landed on -- the same
            // add-or-remove Ctrl already means over a roll's notes and a
            // curve's break-points. It is not a drag: a marker is a moment,
            // and the press has already said which.
            (GestureStep::Marker, interact::Coords::Time(axis)) => {
                if !axis.spans(cx) {
                    return false;
                }
                let strip = crate::host::frame::ruler_strip(frame.rect, axis.body);
                if strip.h <= 0.0 || !strip.contains(cx, cy) {
                    return false;
                }
                let Some(mut markers) = host
                    .widget_kind(def_id, id)
                    .and_then(|k| k.editor().map(|e| e.markers.clone()))
                else {
                    return false;
                };
                match super::nav::marker_under(host, def_id, id, strip, &markers, cx) {
                    Some(i) => {
                        markers.remove(i);
                    }
                    None => {
                        let time = interact::sample_at(
                            axis.nav.start,
                            axis.nav.len,
                            axis.body.x as f64,
                            axis.body.w as f64,
                            cx,
                        )
                        .max(0.0);
                        // **Numbered, not blank.** The label is what a client
                        // is handed with the time, so a marker with none says
                        // nothing to the reader or to the owner; the count is
                        // the one name the host can give without asking.
                        let label = (markers.len() + 1).to_string();
                        markers.push(super::super::widget::Marker {
                            time,
                            label,
                            color: None,
                        });
                        markers.sort_by(|a, b| a.time.total_cmp(&b.time));
                    }
                }
                super::nav::set_markers(host, out, def_id, id, markers);
                true
            }
            (GestureStep::Sample, interact::Coords::Time(axis)) => {
                // **Outside the surface this arm acts on: not mine.** These two
                // are the only declines left in the arm -- past them the press
                // is this gesture's, and every way it can fail is refused out
                // loud (`effects::refuse`) rather than handed to a sweep.
                if !axis.spans(cx) {
                    return false;
                }
                let Some(value) =
                    value_axis(host, ctx, frame, hit).filter(|v| v.body.contains(cx, cy))
                else {
                    return false;
                };
                // A sample is grabbable exactly where it is **drawn**: the
                // trace marks each one with a disc only when they are far
                // enough apart to be told apart, and the same question decides
                // whether there is anything here to take hold of. Read from the
                // drawing's own rule rather than restated, so the two can never
                // drift into offering a grab on a picture that shows no points.
                //
                // **And this one declines rather than refusing**, which is the
                // opposite of what the pencil's identical gate does one arm
                // below. Both are specified, one line apart, and the difference
                // is the point: there is nothing here to *grab*, so the plan
                // falls through and `"sample select"` edits where the samples
                // are visible and sweeps where they are not -- while a stroke
                // that fell through would turn a refused edit into a selection,
                // which is what a pencil must never do.
                let radius = host.metrics_for(ctx.def_id).point_radius;
                let per_px = axis.nav.len / axis.body.w.max(1.0) as f64;
                if !crate::host::graphics::signal::trace::samples_are_drawn(per_px, radius) {
                    return false;
                }
                let frames = interact::sample_at(
                    axis.nav.start,
                    axis.nav.len,
                    axis.body.x as f64,
                    axis.body.w as f64,
                    cx,
                );
                // Before the take begins: still "not a thing on screen", so it
                // declines with the gate above rather than refusing.
                if frames < 0.0 {
                    return false;
                }
                let index = frames.round().max(0.0) as usize;
                let channel = crate::host::frame::channel_at(value.body, value.rows.max(1), cy);
                // What it is now, so the intent that leaves on release is
                // absolute *and* carries its own inverse.
                let Some(previous) = host
                    .window_def(ctx.def_id)
                    .and_then(|t| t.find(id))
                    .and_then(|w| w.kind.sample_value(channel, index))
                else {
                    return refuse(host, out, def_id, id, "sample", NO_SAMPLES.into());
                };
                let held =
                    PendingEdit::one(channel, index, value.value_in(channel, cy) as f32, previous);
                if !set_pending(host, def_id, id, Some(held)) {
                    return refuse(host, out, def_id, id, "sample", NO_PENDING.into());
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
                // **Outside the surface this arm acts on: not mine.** As in the
                // `Sample` arm above -- past these two the press is the
                // pencil's, and every way it can fail is said out loud.
                if !axis.spans(cx) {
                    return false;
                }
                let Some(value) =
                    value_axis(host, ctx, frame, hit).filter(|v| v.body.contains(cx, cy))
                else {
                    return false;
                };
                // **Refused until the picture draws its samples one by one**,
                // and said out loud: a stroke over a summarized trace writes
                // values the reader cannot see, and a pencil that silently does
                // nothing teaches that it sometimes does not work.
                //
                // The threshold is the *drawing's*, asked rather than restated
                // (`graphics::signal::trace::samples_are_drawn`). It used to be
                // one pixel per sample, which is a different number from the one
                // that puts the dots on the trace: between the two a stroke was
                // allowed over a picture with no dots in it, and a drag across
                // the body wrote hundreds of samples the hand could not aim at.
                // The example's own instructions had the rule right all along --
                // *zoom in until the samples are discs, then draw*.
                let radius = host.metrics_for(def_id).point_radius;
                let per_px = axis.nav.len / axis.body.w.max(1.0) as f64;
                if !trace::samples_are_drawn(per_px, radius) {
                    return refuse(host, out, def_id, id, "draw", zoom_in(per_px, radius));
                }
                let frames = interact::sample_at(
                    axis.nav.start,
                    axis.nav.len,
                    axis.body.x as f64,
                    axis.body.w as f64,
                    cx,
                );
                if frames < 0.0 {
                    return refuse(host, out, def_id, id, "draw", BEFORE_THE_START.into());
                }
                let index = frames.round().max(0.0) as usize;
                let channel = crate::host::frame::channel_at(value.body, value.rows.max(1), cy);
                let Some(previous) = host
                    .window_def(ctx.def_id)
                    .and_then(|t| t.find(id))
                    .and_then(|w| w.kind.sample_value(channel, index))
                else {
                    return refuse(host, out, def_id, id, "draw", NO_SAMPLES.into());
                };
                let v = value.value_in(channel, cy) as f32;
                if !set_pending(
                    host,
                    def_id,
                    id,
                    Some(PendingEdit::one(channel, index, v, previous)),
                ) {
                    return refuse(host, out, def_id, id, "draw", NO_PENDING.into());
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
                locate_timeline(host, out, ctx, id, axis.body, cx);
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
    /// the address ([`element::At`]) -- everything the machine does with the
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
            marquee_caught(host, ctx, Some(at), (cx, cy), (cx, cy));
            Drag::Marquee {
                at: Some(at),
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

    /// The press the containers handed down: what the widget under the cursor
    /// does with it -- a control's value, a note, a break-point, a clip, a piano
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
        let effects_before = out.len();
        // A registered element gets the press on the live widget (the `kind` the
        // hit carries is a copy), and answers the way it answers anywhere: it
        // consumed it, or it declines and the press goes back up the chain. The
        // claim is taken before anything is delivered, so the element's borrow
        // of the tree is over by the time the event leaves.
        if matches!(hit.kind, WidgetKind::Custom(_)) {
            return self.element_at(
                host,
                ctx,
                element::At {
                    clicks: self.clicks(),
                    ..element::At::widget(id, rect, scale, indent)
                },
                cx,
                cy,
                out,
            );
        }
        // Nothing the element wanted: the press goes back to the chain.
        self.drag.is_some() || out.len() > effects_before
    }
}
