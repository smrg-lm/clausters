//! `curve` -- a drawable break-point function, standing on its own or filling a
//! clip's automation body.
//!
//! **One element, two placements**, which is the whole reason it is the leaf
//! the clip bodies were designed against. On its own it draws a framed field over its own
//! `[0, duration]` domain; as a clip's [`Curve`](BodyRole::Curve) body it is
//! handed the container's axis ([`Ctx::time`]) and draws bare against it, over
//! the clip's span. The mapping is one object either way ([`bpf::Axes`]), so a
//! breakpoint is grabbed by the pixels it was drawn on and the edit that leaves
//! is the same `"points"` payload -- a script consumes it without caring which
//! view drew it.
//!
//! The edit is expressed in the **owner's terms**: the whole breakpoint list in
//! the envelope's own units, never a pixel delta, because whoever owns the data
//! applies it and sends back a fresh drawing.

use serde_json::{Map, Value};

use clausters_core::osc::OscType;

use crate::host::graphics::bpf::{self, Axes};
use crate::host::graphics::controls;
use crate::host::layout::Rect;
use crate::host::paint::Draw;
use crate::host::structures::points::{self, BpfPoint};
use crate::host::widget::element::{
    BodyRole, Claim, Ctx, Element, Events, Input, OnAxis, Take, TimeSpace,
};
use crate::host::widget::parse::{label, number, number_f64, set_f, set_f64, set_label, truthy};
use crate::host::widget::{EditorProps, Ruler, RulerY};
use crate::host::{font, metrics::Metrics, ruler};
use crate::viewport::View;

/// A break-point function over `[min, max]`, using the server's own envelope
/// shape numbers -- what it draws is what an `EnvGen` plays.
#[derive(Debug, Clone)]
pub struct Curve {
    points: Vec<BpfPoint>,
    min: f32,
    max: f32,
    /// The time domain when the element spans its own (0 = fit the last point);
    /// a body spans its container's instead.
    duration: f64,
    exp: bool,
    label: Option<String>,
    /// The grab in flight -- the state that used to be two `Drag` variants,
    /// because the widget could not hold it.
    grab: Option<Grab>,
    /// **The selected segment**, by the point that starts it: the one a press
    /// on a segment chose, which an editor's controls act on -- its shape,
    /// say. View state, set back by the `segment` prop and reported by the
    /// `"segment"` event (`-1` for none).
    selected: Option<usize>,
    /// Whether a hand may edit this curve. See `notes::Notes::editable`: the
    /// picture must not follow a hand that cannot edit, so the refusal happens
    /// at the press rather than when an owner declines the edit afterwards.
    editable: bool,
    /// The axis chrome of a curve **standing on its own**: its rulers, and the
    /// navigation group it shares them with.
    ///
    /// Both units default **off**, unlike every other timeline view, because a
    /// break-point function is drawn far more often as a bare envelope than as
    /// a measured figure -- and because a curve that grew a ruler nobody asked
    /// for would move the picture in every window that already draws one. A
    /// **body** carries [`EditorProps::body`], which is no chrome at all: a
    /// clip's automation is drawn against the clip's axes and rules nothing.
    editor: EditorProps,
    /// Whether this curve is a **container's body** rather than a view of its
    /// own -- the one thing the placement cannot be asked, now that a standalone
    /// curve is a navigation-group member too and so is handed a
    /// [`TimeSpace`] exactly like a body is.
    ///
    /// The distinction is what the two doors mean: a body fills the rectangle
    /// its clip drew and rules nothing; a view draws its own field, its label
    /// and its strips, and shares only the *axis* with the group it is on.
    body: bool,
}

/// The parts of a standalone curve's rectangle ([`Curve::regions`]).
struct Regions {
    /// Where the curve itself is drawn.
    field: Rect,
    /// The time strip under the field, when one is asked for.
    time: Option<Rect>,
    /// The left edge of the value strip, when one is asked for.
    value: Option<f32>,
}

/// What a held press is moving: a breakpoint, or a segment's curvature.
#[derive(Debug, Clone, Copy)]
enum Grab {
    Point(usize),
    /// A segment bent by a vertical drag, measured **from the press**: the y it
    /// started at and the curvature it had. Absolute, not incremental -- see
    /// [`points::bend_curve`] for what the incremental form got wrong.
    Segment {
        index: usize,
        press_y: f64,
        from: f32,
    },
}

pub(super) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(from_props(props)))
}

/// The **body** flavor: the same element, drawn inside a rectangle a container
/// already decided and over the axis that container hands it.
///
/// It is the same parse as a standalone curve's -- one product of one set of
/// props, which is what keeps the two placements from drifting -- with the
/// chrome dropped and the placement told. What a multitrack builds through this
/// is both of its curves: a track automation's row and a box's envelope layer
/// are the same element in two places, and the only difference between them is
/// the rectangle and the span they are handed.
pub(crate) fn body(props: &Map<String, Value>) -> Curve {
    Curve {
        editor: EditorProps::body(),
        body: true,
        ..from_props(props)
    }
}

fn from_props(props: &Map<String, Value>) -> Curve {
    let min = number(props, "min", 0.0);
    let max = number(props, "max", 1.0);
    let (lo, hi) = (min.min(max), min.max(max));
    Curve {
        points: props
            .get("points")
            .and_then(|v| points::parse_points(v, lo, hi))
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| points::default_points(lo)),
        min: lo,
        max: hi,
        duration: number_f64(props, "duration", 0.0),
        exp: props.get("exp").and_then(truthy).unwrap_or(false),
        label: label(props),
        grab: None,
        selected: None,
        editable: props.get("editable").and_then(truthy).unwrap_or(true),
        editor: standalone_chrome(props),
        body: false,
    }
}

/// The chrome of a curve standing on its own, with **both rulers off** unless
/// the props ask for one: an envelope is a picture before it is a measurement,
/// and the views that already draw one must not move because the element
/// learned to rule itself.
fn standalone_chrome(props: &Map<String, Value>) -> EditorProps {
    let mut editor = EditorProps::parse(props, RulerY::Off);
    editor.ruler = Ruler::parse_with(props, Ruler::Off);
    editor
}

impl Curve {
    /// The display mapping for this placement: **which rectangle** the picture
    /// is drawn in, and **which window** of time it is drawn over.
    ///
    /// The two questions are independent, which is the whole of what "the same
    /// element in two places" costs. A **body** fills the rectangle its clip
    /// drew; a **view** draws inside its own field, with the strips and the
    /// label outside it. Either one follows the group's window when it is on a
    /// navigation group ([`Ctx::time`]) and its own domain when it is on none --
    /// so a curve stacked with a ruler shows the ruler's time, and a curve
    /// standing alone shows all of itself.
    fn axes(
        &self,
        rect: Rect,
        indent: f32,
        m: &Metrics,
        time: Option<crate::host::widget::element::TimeSpace>,
    ) -> Axes {
        let field = if self.body {
            rect
        } else {
            self.regions(rect, indent, m).field
        };
        match time {
            Some(t) => Axes {
                body: field,
                view: t.view,
                dom: t.span,
                lo: self.min,
                hi: self.max,
                exp: self.exp,
            },
            None => Axes::spanning(
                field,
                points::domain(&self.points, self.duration),
                self.min,
                self.max,
                self.exp,
            ),
        }
    }

    /// **The three rectangles a curve standing on its own is made of**: the
    /// field its picture is drawn in, the time strip under it and the value
    /// strip left of it.
    ///
    /// The height is settled before the width, as in every ruled view: how
    /// tall the field ends up is what decides how finely the value axis steps,
    /// and therefore how wide the labels the left strip must hold are.
    /// `indent` is the **group's** gutter rather than this curve's own wish, so
    /// a ruler stacked with it starts its ticks at the same pixel.
    fn regions(&self, rect: Rect, indent: f32, m: &Metrics) -> Regions {
        let outer = controls::body_rect(rect, self.label.is_some(), m);
        let ruled = self.editor.ruler != Ruler::Off && outer.h > m.ruler_h * 2.0;
        let strip_h = if ruled { m.ruler_h } else { 0.0 };
        let gutter = if self.editor.ruler_y == RulerY::Off {
            0.0
        } else {
            indent.min((outer.w * 0.5).max(0.0))
        };
        let field = Rect::new(
            outer.x + gutter,
            outer.y,
            (outer.w - gutter).max(0.0),
            (outer.h - strip_h).max(0.0),
        );
        Regions {
            field,
            time: ruled.then(|| Rect::new(field.x, field.y + field.h, field.w, strip_h)),
            value: (gutter > 0.0).then_some(outer.x),
        }
    }

    /// The window this curve draws: its group's when it is on one, else the
    /// whole of its own domain.
    fn view(&self, time: Option<TimeSpace>) -> View {
        match time {
            Some(t) => t.view,
            None => {
                let dom = points::domain(&self.points, self.duration);
                View {
                    start: 0.0,
                    len: dom.max(1e-9),
                }
            }
        }
    }

    /// Whether a **whole-field** gesture (bending a segment) is this curve's to
    /// take right now.
    ///
    /// A body only owns the rectangle while it is the container's active edit
    /// layer -- inactive, the rectangle means the clip's own drag. A **view**
    /// owns its field always: sharing an axis with a ruler is not being layered
    /// under anything, and a group hands out no active layer for a member to be.
    fn active(&self, time: Option<TimeSpace>) -> bool {
        !self.body || time.is_none_or(|t| t.active)
    }

    /// The sample rate its time ruler labels with: its own, else the world's.
    fn rate(&self, world_rate: f64) -> f64 {
        if self.editor.sample_rate > 0.0 {
            self.editor.sample_rate
        } else {
            world_rate
        }
    }

    /// **The two strips of a curve standing on its own**, drawn only where the
    /// props asked for them.
    ///
    /// The horizontal one is the ordinary time ruler, so a curve laid over a
    /// multitrack can be read in seconds, samples or `bar:beat` -- through the axis'
    /// tempo map where it has one, which is what lets an envelope of tempo be
    /// read against the beats it produces. The vertical one is the plain value
    /// axis over `[min, max]`, the same one a plot draws: a break-point
    /// function's values are its own parameter's, so the amplitude ladders say
    /// nothing about them.
    fn draw_rulers(&self, d: &mut Draw, ctx: &Ctx) {
        let r = self.regions(ctx.rect, ctx.indent, ctx.metrics);
        if let Some(strip) = r.time {
            let nav = self.view(ctx.time);
            let ticks = ruler::time_ticks(
                nav.start,
                nav.len,
                strip.w as f64,
                self.rate(ctx.world.sample_rate),
                crate::host::frame::time_unit(&self.editor),
                ctx.metrics,
            );
            ruler::draw_ticks_h(d, strip, &ticks, self.editor.dir);
        }
        if let Some(strip_x) = r.value {
            let ticks = ruler::value_ticks(
                self.min as f64,
                self.max as f64,
                r.field.h as f64,
                ctx.metrics,
            );
            ruler::draw_ticks_v(d, r.field.x, strip_x, r.field, &ticks);
        }
    }

    /// The break-points as they now stand -- what a container holding this as
    /// a layer reports them *with its own identity* in front of, since there
    /// the payload is the whole multitrack's curves and not this one's.
    pub(crate) fn points(&self) -> &[BpfPoint] {
        &self.points
    }

    /// **What a hover over this curve reads**, drawn into `rect` against
    /// `time`: a break-point's own value when the pointer is on one (and
    /// `true`), else the curve's value at the pointer's time -- anywhere over
    /// the rectangle when `anywhere`, as a lane's row is the curve's own, and
    /// only on the line otherwise, as a layer's field is its note's. `None`
    /// when the pointer is on none of it.
    pub(crate) fn hover_value(
        &self,
        at: (f64, f64),
        rect: Rect,
        time: TimeSpace,
        m: &Metrics,
        anywhere: bool,
    ) -> Option<(f32, bool)> {
        let ax = self.axes(rect, 0.0, m, Some(time));
        if !ax.body.contains(at.0, at.1) {
            return None;
        }
        if let Some(i) = ax.hit_point(&self.points, at.0, at.1, m) {
            return self.points.get(i).map(|p| (p.value, true));
        }
        (anywhere || ax.on_line(&self.points, at.0, at.1, m))
            .then(|| (points::value_at(&self.points, ax.t(at.0)), false))
    }

    /// **What the pointer reads over a curve standing on its own**: the point
    /// under it -- its number, its time and its value -- or the curve's value
    /// at the pointer's time, each against the range the field is drawn over,
    /// which is the rule an editor keeps the curve in; and the shape of the
    /// segment it starts or is over, with the curvature of a custom one.
    /// `None` off the field and off every point.
    fn readout(&self, at: (f64, f64), ax: &Axes, rate: f64, m: &Metrics) -> Option<String> {
        // A point on the field's edge is still under the pointer that grabs
        // it, so a point answers before the field is asked.
        let point = ax.hit_point(&self.points, at.0, at.1, m);
        if point.is_none() && !ax.body.contains(at.0, at.1) {
            return None;
        }
        let per_px = ax.view.len / rate / f64::from(ax.body.w.max(1.0));
        let (head, time, value, segment) = match point {
            Some(i) => {
                let p = self.points.get(i)?;
                let next = (i + 1 < self.points.len()).then_some(i);
                (format!("point {}  ", i + 1), p.time, p.value, next)
            }
            None => {
                let t = ax.t(at.0);
                (
                    "".into(),
                    t,
                    points::value_at(&self.points, t),
                    ax.hit_segment(&self.points, at.0),
                )
            }
        };
        let (lo, hi) = (f64::from(self.min), f64::from(self.max));
        let read = |v: f64| bpf::readout_value(v, lo, hi);
        let shape = segment
            .and_then(|i| self.points.get(i))
            .map(|p| match p.shape {
                clausters_core::envshape::SHAPE_CURVE => format!("  curve {:+.2}", p.curve),
                shape => format!("  {}", clausters_core::envshape::shape_name(shape)),
            });
        Some(format!(
            "{head}{}  {} [{}, {}]{}",
            ruler::readout_time(time, rate, per_px),
            read(f64::from(value)),
            read(lo),
            read(hi),
            shape.unwrap_or_default(),
        ))
    }

    /// Draws [`Self::readout`] in the field's top-right corner, right-aligned
    /// inside the field and dropping its tail first where the field is
    /// narrow, as a roll draws its own in its bottom-right one.
    ///
    /// **Hung as the roll's is sat**: the roll's sits two pixels over the
    /// bottom edge and its ink hangs a [`font::descent`] below that; this one
    /// hangs two pixels under the top edge with the [`font::ascent`] its ink
    /// may reach above the body box cleared too, less the same descent, so the
    /// gap between the ink and the edge is the roll's mirrored.
    fn draw_readout(&self, d: &mut Draw, ctx: &Ctx, ax: &Axes) {
        // On hover and while editing alike: a point being dragged is under the
        // pointer that drags it, and a segment being bent under the one that
        // bends it, so the readout follows the edit as it happens.
        let Some(at) = ctx.world.cursor else {
            return;
        };
        let rate = self.rate(ctx.world.sample_rate);
        let Some(text) = self.readout(at, ax, rate, ctx.metrics) else {
            return;
        };
        let m = ctx.metrics;
        let room = ax.body.w - 2.0 * m.pad;
        let w = font::width(&text, m.caption_scale).min(room);
        let scale = m.caption_scale;
        let (x, y) = (
            ax.body.x + ax.body.w - w - m.pad,
            ax.body.y + 2.0 + font::ascent(scale) - font::descent(scale),
        );
        let color = d.parts().2.ruler_text;
        crate::host::graphics::plate_text(d, &text, x, y, room, m.caption_scale, color);
    }

    /// The edit-back payload: the `"points"` tag plus the flat `t v shape curve`
    /// list -- the envelope's own units, which is what its owner applies.
    fn points_event(&self) -> Events {
        let mut args = vec![OscType::String("points".into())];
        args.extend(points::points_args(&self.points));
        Events::message(args)
    }

    /// **Selects segment `index`** (or none) on a curve standing on its own,
    /// and the `"segment"` report of it when it changed -- `-1` for none. A
    /// body selects nothing: a clip's curve is its container's to address.
    fn select_segment(&mut self, index: Option<usize>) -> Option<Vec<OscType>> {
        if self.body || self.selected == index {
            return None;
        }
        self.selected = index;
        let at = index.map_or(-1, |i| i as i32);
        Some(vec![OscType::String("segment".into()), OscType::Int(at)])
    }
}

impl Element for Curve {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "editable" => {
                let Some(on) = truthy(v) else { return false };
                self.editable = on;
                true
            }
            // The full breakpoint list replaces in one set -- the flat
            // `[t, v, shape, curve, ...]` array, or that array as a JSON string
            // (the `/gui_set` scalar carrier).
            "points" => match points::parse_points(v, self.min, self.max) {
                Some(p) if !p.is_empty() => {
                    self.points = p;
                    true
                }
                _ => false,
            },
            "min" => set_f(&mut self.min, v),
            "max" => set_f(&mut self.max, v),
            "duration" => set_f64(&mut self.duration, v),
            "exp" => truthy(v).map(|b| self.exp = b).is_some(),
            "label" => set_label(&mut self.label, v),
            // The selected segment, by the point that starts it; a negative
            // index, or one past the last segment, is none.
            "segment" => {
                let Some(i) = v.as_f64() else { return false };
                self.selected =
                    (i >= 0.0 && (i as usize) + 1 < self.points.len()).then_some(i as usize);
                true
            }
            _ => false,
        }
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        let ax = self.axes(ctx.rect, ctx.indent, ctx.metrics, ctx.time);
        // The field and whatever names it are the *view's*: a body is drawn
        // against the container's axes, inside a rectangle the container drew.
        if !self.body {
            let (mesh, m, theme) = d.parts();
            if let Some(text) = &self.label {
                font::text(
                    mesh,
                    text,
                    ctx.rect.x + m.pad,
                    ctx.rect.y + m.pad,
                    m.text_scale,
                    theme.text,
                );
            }
            if ax.body.w <= 0.0 || ax.body.h <= 0.0 {
                return;
            }
            mesh.rect(ax.body, theme.field);
            mesh.border(ax.body, m.divider_w, theme.accent);
            self.draw_rulers(d, ctx);
        }
        // What a bend would take: the segment being held, or the one under the
        // pointer when nothing is. Only where the curve can be edited at all --
        // an affordance over a read-only body would announce a gesture that is
        // about to be refused -- and only while this curve is the **active edit
        // layer** of whatever placed it. That is the defect this closes: inside
        // a clip the segment used to light up whether or not the curve was the
        // layer in hand, and a press there moved the clip, which is exactly the
        // promise a lit affordance must not make. Active, the bend *is* the
        // gesture, so the mark and the press agree again.
        let lit = if !self.editable || !self.active(ctx.time) {
            None
        } else {
            match self.grab {
                Some(Grab::Segment { index, .. }) => Some(index),
                Some(Grab::Point(_)) => None,
                None => ctx
                    .world
                    .cursor
                    .filter(|(x, y)| ax.body.contains(*x, *y))
                    .and_then(|(x, _)| ax.hit_segment(&self.points, x)),
            }
        };
        bpf::draw_with(d, &ax, &self.points, lit, self.selected);
        if !self.body {
            self.draw_readout(d, ctx, &ax);
        }
    }

    /// A curve standing on its own reads what is under the pointer.
    fn hover_readout(&self) -> bool {
        !self.body
    }

    fn info(&self) -> Vec<(String, Value)> {
        // The list as the JSON string `/gui_set points` already accepts: a
        // query gives back exactly what a set would take.
        vec![
            (
                "points".into(),
                Value::from(points::points_json(&self.points).to_string()),
            ),
            (
                "segment".into(),
                Value::from(self.selected.map_or(-1, |i| i as i64)),
            ),
        ]
    }

    fn body_role(&self) -> Option<BodyRole> {
        Some(BodyRole::Curve)
    }

    /// A clip's body: the line over the clip's own axis, with none of the
    /// chrome [`draw`](Self::draw) paints when it stands on its own -- no label,
    /// no field, no border, because the clip drew those.
    ///
    /// **This existing is the whole of it.** `Element::draw_body` defaults to
    /// drawing nothing, and a curve went without it: a clip carrying `points`
    /// built its body, placed it and collected it for the pass, and the pass
    /// called a method that did nothing -- so an automation lane was an empty
    /// rectangle in every client, native and browser alike. What hid it is that
    /// the tests drove `draw` with a `TimeSpace`, which is the *standalone*
    /// door taking the body-shaped branch, and the two doors are only the same
    /// when something connects them. `a_clip_body_draws_the_line` is the test
    /// that goes through this one.
    ///
    /// The lit segment is not drawn here: what a bend would take is read off
    /// the cursor, and a body is handed no world to read it from.
    fn draw_body(&self, d: &mut Draw, rect: Rect, time: &TimeSpace) {
        let ax = {
            let (_mesh, m, _theme) = d.parts();
            self.axes(rect, 0.0, m, Some(*time))
        };
        bpf::draw_with(d, &ax, &self.points, None, None);
    }

    /// **A curve's own contents are its break-points and the segments between
    /// them** -- the line, not the field it is drawn over. Everything else in
    /// the rectangle is the container's, which is how an automation drawn
    /// across a whole clip still leaves the clip draggable.
    ///
    /// A read-only curve answers `false`, the same as empty space: the press
    /// falls through to the clip instead of being consumed by a refusal.
    fn layer_hit(&self, at: (f64, f64), input: &Input) -> bool {
        if !self.editable {
            return false;
        }
        let ax = self.axes(input.rect, input.indent, input.metrics, input.time);
        ax.body.contains(at.0, at.1)
            && (ax
                .hit_point(&self.points, at.0, at.1, input.metrics)
                .is_some()
                || ax.on_line(&self.points, at.0, at.1, input.metrics))
    }

    fn press(&mut self, at: (f64, f64), input: &Input) -> Claim {
        if !self.editable {
            // Consumed and said out loud, like the roll's: a curve drawn from
            // samples this editor cannot write is not a dead widget.
            return Claim::Take(Take {
                events: Events::refused("points", "this curve is read-only here"),
                ..Take::default()
            });
        }
        let ax = self.axes(input.rect, input.indent, input.metrics, input.time);
        let hit = ax.hit_point(&self.points, at.0, at.1, input.metrics);
        // Ctrl+click on a point removes it; elsewhere it adds one at the cursor
        // (which then drags until release).
        if input.mods.ctrl {
            match hit {
                Some(i) if points::remove_point(&mut self.points, i) => {}
                Some(_) => return Claim::Decline,
                None => self.grab = Some(Grab::Point(ax.add_point(&mut self.points, at.0, at.1))),
            }
            // A point added or removed renumbers the segments after it, so
            // the one selected is no longer the one it was.
            let events = self.points_event();
            return Claim::events(match self.select_segment(None) {
                Some(cleared) => events.and(cleared),
                None => events,
            });
        }
        if let Some(i) = hit {
            self.grab = Some(Grab::Point(i));
            return match self.select_segment(None) {
                Some(cleared) => Claim::events(Events::message(cleared)),
                None => Claim::take(),
            };
        }
        // Bending a **segment** is the gesture of whoever holds the layer.
        // Standing on its own the whole field is the element's; inside a
        // container it is the element's while this curve is the **active**
        // layer -- which is what selecting the curve means, and what the lit
        // segment says is on offer. Inactive, the rectangle means the
        // container's own drag (moving a clip, resizing it), so the press goes
        // back to it.
        if self.active(input.time)
            && let Some(index) = ax.hit_segment(&self.points, at.0)
        {
            self.grab = Some(Grab::Segment {
                index,
                press_y: at.1,
                from: self.points.get(index).map_or(0.0, |p| p.curve),
            });
            // **A press on a segment selects it**, and the bend is the same
            // press held and moved.
            return match self.select_segment(Some(index)) {
                Some(chosen) => Claim::events(Events::message(chosen)),
                None => Claim::take(),
            };
        }
        // Nothing of this element's: the press goes back to the chain, where a
        // container's own plan (a clip's move, a plane's pan) is waiting.
        Claim::Decline
    }

    /// The curve follows the hand; **the edit leaves on release**.
    ///
    /// One gesture is one edit -- the rule `Drag::Draw` and `Drag::Sample`
    /// already state at their own release, and the one this never had. A value
    /// per frame is a document edit per frame: a hundred entries in the history
    /// for one bend, and a hundred round trips whose acknowledgements the next
    /// frame outruns, so every frame after the first names a version its owner
    /// has moved past and comes back refused. The picture is then snapped to the
    /// answer of the first frame, over and over -- a curve trembling under the
    /// hand editing it.
    fn drag(&mut self, at: (f64, f64), input: &Input) -> Events {
        let ax = self.axes(input.rect, input.indent, input.metrics, input.time);
        match &mut self.grab {
            Some(Grab::Point(i)) => ax.move_point(&mut self.points, *i, at.0, at.1),
            Some(Grab::Segment {
                index,
                press_y,
                from,
            }) => {
                let dy_frac = (*press_y - at.1) / ax.body.h.max(1.0) as f64;
                points::bend_curve(&mut self.points, *index, dy_frac, *from);
            }
            None => return Events::none(),
        }
        Events::none()
    }

    fn release(&mut self, _at: (f64, f64), _inside: bool, _input: &Input) -> Events {
        // What the drag amounts to, once -- see `drag`.
        let held = self.grab.take().is_some();
        if held {
            self.points_event()
        } else {
            Events::none()
        }
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }

    fn on_axis(&self) -> Option<&dyn OnAxis> {
        Some(self)
    }

    fn on_axis_mut(&mut self) -> Option<&mut dyn OnAxis> {
        Some(self)
    }
}

impl OnAxis for Curve {
    fn editor(&self) -> Option<&EditorProps> {
        Some(&self.editor)
    }

    fn editor_mut(&mut self) -> Option<&mut EditorProps> {
        Some(&mut self.editor)
    }

    /// How far this curve's own content reaches, so a ruler stacked with it
    /// rules the span the picture actually covers. It is what makes the axis
    /// **shared**: without it a window holding only a curve has no extent at
    /// all, and a `timeruler` over it would label one sample.
    fn content_span(&self) -> Option<f64> {
        Some(points::domain(&self.points, self.duration))
    }

    /// The wish, from the props alone: the ruler role's own width. What the
    /// band actually needs depends on the labels, which depend on how tall the
    /// curve ended up -- that is [`OnAxis::measured_gutter`], one pass later.
    fn gutter(&self, m: &Metrics) -> f32 {
        if self.editor.ruler_y == RulerY::Off {
            0.0
        } else {
            m.ruler_w
        }
    }

    /// The value strip measured against its **own** labels, once the placement
    /// is known: a BPM axis over `[30, 90]` formats three characters where an
    /// amplitude axis formats six, and the step it labels at follows the
    /// height. `None` while the role-sized wish already covers it, so the
    /// second layout pass is taken only when one is owed.
    fn measured_gutter(&self, rect: Rect, m: &Metrics) -> Option<f32> {
        if self.editor.ruler_y == RulerY::Off {
            return None;
        }
        let field = self.regions(rect, 0.0, m).field;
        let want = ruler::value_strip_w(self.min as f64, self.max as f64, field.h, m);
        (want > m.ruler_w).then_some(want)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::metrics::Metrics;

    use crate::host::widget::element::{Mods, TimeSpace};

    use crate::viewport::View;

    fn props(json: &str) -> Map<String, Value> {
        serde_json::from_str(json).unwrap()
    }

    fn input<'a>(m: &'a Metrics, rect: Rect, time: Option<TimeSpace>) -> Input<'a> {
        Input {
            metrics: m,
            indent: 0.0,
            rect,
            scale: 1.0,
            mods: Mods::default(),
            viewport: (400.0, 300.0),
            clicks: 1,
            time,
        }
    }

    /// A ramp with no label, so its field is the whole rect inset by the pad --
    /// the geometry both placements are compared against.
    fn ramp() -> Curve {
        from_props(&props(
            r#"{"min":0.0,"max":1.0,"duration":100.0,
                "points":[0.0,0.0,1,0.0,100.0,1.0,1,0.0]}"#,
        ))
    }

    /// **The readout names the point under the pointer**, its value against
    /// the range the field is drawn over and the shape of the segment it
    /// starts; between points it reads the curve where the pointer is.
    #[test]
    fn the_readout_reads_a_point_against_its_range_and_its_segment() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 400.0, 200.0);
        let c = from_props(&props(
            r#"{"min":0.0,"max":1.0,"duration":2.0,"sample_rate":1.0,
                "points":[0.0,0.0,2,0.0, 1.0,0.5,5,-4.0, 2.0,0.0,1,0.0]}"#,
        ));
        let ax = c.axes(rect, 0.0, &m, None);
        let on = |t: f64, v: f32| (f64::from(ax.x(t)), f64::from(ax.y(v)));
        let point = c
            .readout(on(1.0, 0.5), &ax, 1.0, &m)
            .expect("over the field");
        assert!(point.starts_with("point 2  "), "{point}");
        assert!(point.contains("0.50 [0.00, 1.00]"), "{point}");
        assert!(point.ends_with("curve -4.00"), "{point}");
        let between = c
            .readout(on(0.5, 0.9), &ax, 1.0, &m)
            .expect("over the field");
        assert!(!between.starts_with("point"), "{between}");
        assert!(
            between.ends_with("  exp"),
            "the segment under it: {between}"
        );
        let last = c
            .readout(on(2.0, 0.0), &ax, 1.0, &m)
            .expect("over the field");
        assert!(
            last.ends_with("[0.00, 1.00]"),
            "the last point starts no segment: {last}"
        );
        assert_eq!(c.readout((-5.0, -5.0), &ax, 1.0, &m), None);
    }

    #[test]
    fn props_parse_and_default() {
        let c = ramp();
        assert_eq!((c.min, c.max, c.duration), (0.0, 1.0, 100.0));
        assert_eq!(c.points.len(), 2);

        // No points at all is the default two-point envelope, so the widget is
        // editable from the moment it exists.
        let c = from_props(&props("{}"));
        assert_eq!(c.points.len(), 2);
        // An inverted range is read as a range, not as a mistake.
        let c = from_props(&props(r#"{"min":1.0,"max":-1.0}"#));
        assert_eq!((c.min, c.max), (-1.0, 1.0));
    }

    /// The whole point of the port: **one** element, mapped through whichever
    /// axis it was given. The same press lands on the same breakpoint standing
    /// alone and as a clip's body, because both go through `axes`.
    #[test]
    fn a_press_grabs_the_same_point_on_its_own_axis_and_on_a_container_s() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 100.0 + 2.0 * m.pad, 100.0 + 2.0 * m.pad);
        // Standing alone: the field is the rect inset by the pad, and the
        // domain spans it, so t=100 is the field's right edge.
        let mut c = ramp();
        let field = controls::body_rect(rect, false, &m);
        let at = (field.x as f64 + field.w as f64, field.y as f64);
        assert!(matches!(
            c.press(at, &input(&m, rect, None)),
            Claim::Take(_)
        ));
        assert!(matches!(c.grab, Some(Grab::Point(1))));

        // As a body: the container's rectangle *is* the field, and the window
        // it hands down is what maps time to pixels -- here the second half of
        // the clip, so the same last point sits at the same right edge.
        let mut c = ramp();
        let time = Some(TimeSpace::of(
            View {
                start: 50.0,
                len: 50.0,
            },
            100.0,
        ));
        let at = (rect.w as f64, rect.y as f64);
        assert!(matches!(
            c.press(at, &input(&m, rect, time)),
            Claim::Take(_)
        ));
        assert!(matches!(c.grab, Some(Grab::Point(1))));
    }

    /// A drag reports the whole list in the envelope's own units -- the owner's
    /// terms, never a pixel delta -- and the release adds nothing to it.
    #[test]
    fn a_point_drag_reports_the_edited_list() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 120.0, 120.0);
        let mut c = ramp();
        let field = controls::body_rect(rect, false, &m);
        c.press(
            (field.x as f64, field.y as f64 + field.h as f64),
            &input(&m, rect, None),
        );
        assert!(matches!(c.grab, Some(Grab::Point(0))));

        // Dragging point 0 to the top of the field takes it to the range top;
        // its time cannot pass its neighbour. The curve follows the hand and
        // says nothing: one gesture is one edit.
        let events = c.drag(
            (field.x as f64 + field.w as f64, field.y as f64),
            &input(&m, rect, None),
        );
        assert_eq!(c.points[0].value, 1.0);
        assert_eq!(c.points[0].time, 100.0, "clamped monotonic to point 1");
        assert!(events.is_empty(), "a frame of a drag is not an edit");

        // The release is the edit, and it carries the whole list.
        let msgs = c
            .release((0.0, 0.0), true, &input(&m, rect, None))
            .into_messages();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0][0], OscType::String("points".into()));
        assert_eq!(msgs[0].len(), 1 + 4 * 2, "the tag plus a quad per point");
        assert!(c.grab.is_none());
    }

    /// Ctrl adds a point where there is none and removes the one under the
    /// cursor, reporting the list either way.
    #[test]
    fn ctrl_adds_and_removes_a_point() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 120.0, 120.0);
        let field = controls::body_rect(rect, false, &m);
        let mid = (
            field.x as f64 + field.w as f64 * 0.5,
            field.y as f64 + field.h as f64 * 0.5,
        );

        let mut c = ramp();

        let mut ctrl = input(&m, rect, None);
        ctrl.mods = Mods {
            ctrl: true,
            ..Mods::default()
        };
        assert!(matches!(c.press(mid, &ctrl), Claim::Take(_)));
        assert_eq!(c.points.len(), 3, "added under the cursor");
        assert!(matches!(c.grab, Some(Grab::Point(1))));

        // ...and Ctrl on the point it just added takes it away again.
        c.grab = None;
        let on_point = (c.points[1].time, 0.0);
        let ax = c.axes(rect, 0.0, &m, None);
        assert!(matches!(
            c.press(
                (ax.x(on_point.0) as f64, ax.y(c.points[1].value) as f64),
                &ctrl
            ),
            Claim::Take(_)
        ));
        assert_eq!(c.points.len(), 2);
    }

    /// A segment drag bends it, measured from the press.
    #[test]
    fn a_segment_drag_bends_it() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 120.0, 120.0);
        let mut c = ramp();
        let field = controls::body_rect(rect, false, &m);
        let x = field.x as f64 + field.w as f64 * 0.5;
        assert!(matches!(
            c.press(
                (x, field.y as f64 + field.h as f64 * 0.5),
                &input(&m, rect, None)
            ),
            Claim::Take(_)
        ));
        assert!(matches!(c.grab, Some(Grab::Segment { index: 0, .. })));
        let before = points::value_at(&c.points, 50.0);
        c.drag((x, field.y as f64), &input(&m, rect, None));
        assert!(points::value_at(&c.points, 50.0) > before, "bent upward");
    }

    /// **A press on a segment selects it**, reported as `"segment"`; a press
    /// on a point lets it go, and the prop sets it back. While the press is
    /// held the readout is not drawn: the hand is editing.
    #[test]
    fn a_press_selects_a_segment_and_a_point_lets_it_go() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 120.0, 120.0);
        let mut c = ramp();
        let field = controls::body_rect(rect, false, &m);
        let mid = (
            field.x as f64 + field.w as f64 * 0.5,
            field.y as f64 + field.h as f64 * 0.5,
        );
        let Claim::Take(take) = c.press(mid, &input(&m, rect, None)) else {
            panic!("the segment's press");
        };
        let msgs = take.events.into_messages();
        assert_eq!(
            msgs[0],
            vec![OscType::String("segment".into()), OscType::Int(0)]
        );
        assert_eq!(c.selected, Some(0));
        c.release(mid, true, &input(&m, rect, None));
        let corner = (field.x as f64, field.y as f64 + field.h as f64);
        let Claim::Take(take) = c.press(corner, &input(&m, rect, None)) else {
            panic!("the point's press");
        };
        assert_eq!(
            take.events.into_messages()[0],
            vec![OscType::String("segment".into()), OscType::Int(-1)]
        );
        assert_eq!(c.selected, None);
        assert!(c.set("segment", &serde_json::json!(0)));
        assert_eq!(c.selected, Some(0));
        assert!(c.set("segment", &serde_json::json!(1)));
        assert_eq!(c.selected, None, "past the last segment is none");
    }

    /// **A bend is where the pointer is, not where it has been.** The
    /// incremental form -- each step measured from the last -- drifts twice
    /// over: the clamp eats the motion spent past the limit, so coming back
    /// leaves the bend short by however far it went, and a pointer that has
    /// left the element keeps accumulating whatever motion still arrives, so
    /// the shape falls out of phase with the hand. Both were reported as one
    /// symptom: "al salir el cursor del area sigue sumando, se pierde el offset
    /// original y se desfasa el cursor del movimiento".
    #[test]
    fn a_bend_is_absolute_against_the_press() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 120.0, 120.0);
        let field = controls::body_rect(rect, false, &m);
        let x = field.x as f64 + field.w as f64 * 0.5;
        let mid = field.y as f64 + field.h as f64 * 0.5;
        let at = |dy: f64| (x, mid - dy);

        let mut c = ramp();
        c.press(at(0.0), &input(&m, rect, None));
        assert!(matches!(c.grab, Some(Grab::Segment { index: 0, .. })));
        c.drag(at(20.0), &input(&m, rect, None));
        let twenty = c.points[0].curve;

        // Far past the field -- and past the clamp -- and back to the same
        // twenty pixels: the same bend, because the same cursor position has
        // one answer.
        for dy in [400.0, 4000.0, -4000.0, 20.0] {
            c.drag(at(dy), &input(&m, rect, None));
        }
        assert_eq!(
            c.points[0].curve, twenty,
            "the excursion left no offset behind"
        );

        // And back to the press: no bend at all, which is what "in phase with
        // the hand" means at the one position where it can be checked exactly.
        c.drag(at(0.0), &input(&m, rect, None));
        assert_eq!(c.points[0].curve, 0.0);
    }
}
