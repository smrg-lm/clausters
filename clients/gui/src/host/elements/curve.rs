//! `curve` — a drawable break-point function, standing on its own or filling a
//! clip's automation body.
//!
//! **One element, two placements**, which is the whole reason it is the leaf
//! K5 was designed against. On its own it draws a framed field over its own
//! `[0, duration]` domain; as a clip's [`Curve`](BodyRole::Curve) body it is
//! handed the container's axis ([`Ctx::time`]) and draws bare against it, over
//! the clip's span. The mapping is one object either way ([`bpf::Axes`]), so a
//! breakpoint is grabbed by the pixels it was drawn on and the edit that leaves
//! is the same `"points"` payload — a script consumes it without caring which
//! view drew it.
//!
//! The edit is expressed in the **owner's terms**: the whole breakpoint list in
//! the envelope's own units, never a pixel delta, because whoever owns the data
//! applies it and sends back a fresh drawing.

use serde_json::{Map, Value};

use clausters_core::osc::OscType;

use crate::host::bpf::{self, Axes, BpfPoint};
use crate::host::controls;
use crate::host::layout::Rect;
use crate::host::paint::Draw;
use crate::host::widget::element::{BodyRole, Claim, Ctx, Element, Events, Input};
use crate::host::widget::parse::{label, number, number_f64, set_f, set_f64, set_label, truthy};
use crate::host::{font, metrics::Metrics};

/// A break-point function over `[min, max]`, using the server's own envelope
/// shape numbers — what it draws is what an `EnvGen` plays.
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
    /// The grab in flight — the state that used to be two `Drag` variants,
    /// because the widget could not hold it.
    grab: Option<Grab>,
}

/// What a held press is moving: a breakpoint, or a segment's curvature.
#[derive(Debug, Clone, Copy)]
enum Grab {
    Point(usize),
    /// A segment bent by a vertical drag, `last_y` re-anchored every step
    /// (incremental, like a knob) so a bend has no dead zone.
    Segment {
        index: usize,
        last_y: f64,
    },
}

pub(super) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(from_props(props)))
}

/// The **body** flavor: the same element over its own value range, with no
/// domain of its own (a clip's body spans the clip) and nothing to name it.
/// `None` when the props carry no curve at all, which is a clip without one.
pub(crate) fn body(props: &Map<String, Value>) -> Option<Curve> {
    // A layered clip's bodies do not share an axis — an envelope's units are
    // not the pitches under it — so the curve reads its own range first.
    let min = number(props, "points_min", number(props, "min", -1.0));
    let max = number(props, "points_max", number(props, "max", 1.0));
    let points = props
        .get("points")
        .and_then(|v| bpf::parse_points(v, min, max))
        .filter(|p| !p.is_empty())?;
    Some(Curve {
        points,
        min,
        max,
        duration: 0.0,
        exp: props.get("exp").and_then(truthy).unwrap_or(false),
        label: None,
        grab: None,
    })
}

/// An **empty** body, for a clip growing a curve it was not built with.
pub(crate) fn empty_body() -> Curve {
    Curve {
        points: Vec::new(),
        min: -1.0,
        max: 1.0,
        duration: 0.0,
        exp: false,
        label: None,
        grab: None,
    }
}

fn from_props(props: &Map<String, Value>) -> Curve {
    let min = number(props, "min", 0.0);
    let max = number(props, "max", 1.0);
    let (lo, hi) = (min.min(max), min.max(max));
    Curve {
        points: props
            .get("points")
            .and_then(|v| bpf::parse_points(v, lo, hi))
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| bpf::default_points(lo)),
        min: lo,
        max: hi,
        duration: number_f64(props, "duration", 0.0),
        exp: props.get("exp").and_then(truthy).unwrap_or(false),
        label: label(props),
        grab: None,
    }
}

impl Curve {
    /// The display mapping for this placement: the container's axis when it was
    /// given one, else its own domain across its own field.
    ///
    /// This is the whole of what "the same element in two places" costs — one
    /// branch, in the one function every draw and every hit goes through.
    fn axes(
        &self,
        rect: Rect,
        m: &Metrics,
        time: Option<crate::host::widget::element::TimeSpace>,
    ) -> Axes {
        match time {
            Some(t) => Axes {
                body: rect,
                view: t.view,
                dom: t.span,
                lo: self.min,
                hi: self.max,
                exp: self.exp,
            },
            None => Axes::spanning(
                controls::body_rect(rect, self.label.is_some(), m),
                bpf::domain(&self.points, self.duration),
                self.min,
                self.max,
                self.exp,
            ),
        }
    }

    /// The edit-back payload: the `"points"` tag plus the flat `t v shape curve`
    /// list — the envelope's own units, which is what its owner applies.
    fn points_event(&self) -> Events {
        let mut args = vec![OscType::String("points".into())];
        args.extend(bpf::points_args(&self.points));
        Events::message(args)
    }
}

impl Element for Curve {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            // The full breakpoint list replaces in one set — the flat
            // `[t, v, shape, curve, …]` array, or that array as a JSON string
            // (the `/gui_set` scalar carrier).
            "points" => match bpf::parse_points(v, self.min, self.max) {
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
            _ => false,
        }
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        let ax = self.axes(ctx.rect, ctx.metrics, ctx.time);
        // The field and whatever names it are the *view's*: a body is drawn
        // against the container's axes, inside a rectangle the container drew.
        if ctx.time.is_none() {
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
        }
        bpf::draw(d, &ax, &self.points);
    }

    fn info(&self) -> Vec<(String, Value)> {
        // The list as the JSON string `/gui_set points` already accepts: a
        // query gives back exactly what a set would take.
        vec![(
            "points".into(),
            Value::from(bpf::points_json(&self.points).to_string()),
        )]
    }

    fn body_role(&self) -> Option<BodyRole> {
        Some(BodyRole::Curve)
    }

    fn press(&mut self, at: (f64, f64), input: &Input) -> Claim {
        let ax = self.axes(input.rect, input.metrics, input.time);
        let hit = ax.hit_point(&self.points, at.0, at.1, input.metrics);
        // Ctrl+click on a point removes it; elsewhere it adds one at the cursor
        // (which then drags until release).
        if input.mods.ctrl {
            match hit {
                Some(i) if bpf::remove_point(&mut self.points, i) => {}
                Some(_) => return Claim::Decline,
                None => self.grab = Some(Grab::Point(ax.add_point(&mut self.points, at.0, at.1))),
            }
            return Claim::events(self.points_event());
        }
        if let Some(i) = hit {
            self.grab = Some(Grab::Point(i));
            return Claim::take();
        }
        // Bending a **segment** is the view's gesture and not a body's: a body
        // shares its rectangle with its container, whose own drag is what the
        // rest of that rectangle means (moving a clip, resizing it), so a body
        // claims its break-points and nothing else. Standing on its own the
        // whole field is the element's, and there a segment bends.
        if input.time.is_none()
            && let Some(index) = ax.hit_segment(&self.points, at.0)
        {
            self.grab = Some(Grab::Segment {
                index,
                last_y: at.1,
            });
            return Claim::take();
        }
        // Nothing of this element's: the press goes back to the chain, where a
        // container's own plan (a clip's move, a plane's pan) is waiting.
        Claim::Decline
    }

    fn drag(&mut self, at: (f64, f64), input: &Input) -> Events {
        let ax = self.axes(input.rect, input.metrics, input.time);
        match &mut self.grab {
            Some(Grab::Point(i)) => ax.move_point(&mut self.points, *i, at.0, at.1),
            Some(Grab::Segment { index, last_y }) => {
                let dy_frac = (*last_y - at.1) / ax.body.h.max(1.0) as f64;
                *last_y = at.1;
                bpf::drag_curve(&mut self.points, *index, dy_frac);
            }
            None => return Events::none(),
        }
        self.points_event()
    }

    fn release(&mut self, _at: (f64, f64), _input: &Input) -> Events {
        self.grab = None;
        Events::none()
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::metrics::Metrics;
    use crate::host::paint::Mesh;
    use crate::host::theme::Theme;
    use crate::host::widget::element::{Mods, TimeSpace};
    use crate::host::world::World;
    use crate::viewport::View;

    fn props(json: &str) -> Map<String, Value> {
        serde_json::from_str(json).unwrap()
    }

    fn input<'a>(m: &'a Metrics, rect: Rect, time: Option<TimeSpace>) -> Input<'a> {
        Input {
            metrics: m,
            rect,
            scale: 1.0,
            mods: Mods::default(),
            viewport: (400.0, 300.0),
            time,
        }
    }

    /// A ramp with no label, so its field is the whole rect inset by the pad —
    /// the geometry both placements are compared against.
    fn ramp() -> Curve {
        from_props(&props(
            r#"{"min":0.0,"max":1.0,"duration":100.0,
                "points":[0.0,0.0,1,0.0,100.0,1.0,1,0.0]}"#,
        ))
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

    /// The element declares the role, which is how the clip recognizes it.
    #[test]
    fn it_fills_the_curve_body_role() {
        assert_eq!(ramp().body_role(), Some(BodyRole::Curve));
        assert_eq!(empty_body().body_role(), Some(BodyRole::Curve));
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
        // it hands down is what maps time to pixels — here the second half of
        // the clip, so the same last point sits at the same right edge.
        let mut c = ramp();
        let time = Some(TimeSpace {
            view: View {
                start: 50.0,
                len: 50.0,
            },
            span: 100.0,
        });
        let at = (rect.w as f64, rect.y as f64);
        assert!(matches!(
            c.press(at, &input(&m, rect, time)),
            Claim::Take(_)
        ));
        assert!(matches!(c.grab, Some(Grab::Point(1))));
    }

    /// A drag reports the whole list in the envelope's own units — the owner's
    /// terms, never a pixel delta — and the release adds nothing to it.
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
        // its time cannot pass its neighbour.
        let events = c.drag(
            (field.x as f64 + field.w as f64, field.y as f64),
            &input(&m, rect, None),
        );
        assert_eq!(c.points[0].value, 1.0);
        assert_eq!(c.points[0].time, 100.0, "clamped monotonic to point 1");
        let msgs = events.clone().into_messages();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0][0], OscType::String("points".into()));
        assert_eq!(msgs[0].len(), 1 + 4 * 2, "the tag plus a quad per point");

        assert!(c.release((0.0, 0.0), &input(&m, rect, None)).is_empty());
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
        let ax = c.axes(rect, &m, None);
        assert!(matches!(
            c.press(
                (ax.x(on_point.0) as f64, ax.y(c.points[1].value) as f64),
                &ctrl
            ),
            Claim::Take(_)
        ));
        assert_eq!(c.points.len(), 2);
    }

    /// A segment drag bends it incrementally, re-anchored every step, which is
    /// what keeps a bend from having a dead zone.
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
        let before = bpf::value_at(&c.points, 50.0);
        c.drag((x, field.y as f64), &input(&m, rect, None));
        assert!(bpf::value_at(&c.points, 50.0) > before, "bent upward");
    }

    /// A body draws no chrome of its own: the same points, in the same
    /// rectangle, put less geometry in the mesh than the framed view does.
    #[test]
    fn a_body_draws_without_the_view_s_chrome() {
        let m = Metrics::default();
        let theme = Theme::default();
        let rect = Rect::new(0.0, 0.0, 120.0, 80.0);
        let c = from_props(&props(
            r#"{"min":0.0,"max":1.0,"duration":100.0,"label":"env",
                "points":[0.0,0.0,1,0.0,100.0,1.0,1,0.0]}"#,
        ));

        let mut alone = Mesh::new();
        c.draw(
            &mut Draw::new(&mut alone, &m, &theme),
            &Ctx {
                world: &World::default(),
                metrics: &m,
                rect,
                scale: 1.0,
                time: None,
            },
        );
        let mut body = Mesh::new();
        c.draw(
            &mut Draw::new(&mut body, &m, &theme),
            &Ctx {
                world: &World::default(),
                metrics: &m,
                rect,
                scale: 1.0,
                time: Some(TimeSpace {
                    view: View::full(100),
                    span: 100.0,
                }),
            },
        );
        assert!(!alone.is_empty() && !body.is_empty());
        assert!(
            body.vertex_count() < alone.vertex_count(),
            "no label, no field, no border: {} vs {}",
            body.vertex_count(),
            alone.vertex_count()
        );
    }
}
