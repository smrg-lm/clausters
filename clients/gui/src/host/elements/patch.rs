//! `patch` — the directed, typed patcher: boxes with inlets on their top edge
//! and outlets on their bottom, and a cord per `outlet → inlet` connection.
//!
//! **The leaf that proves the drag shape is general rather than control-shaped**,
//! which is why it is the last of the port. Nothing here is a value in a groove:
//! one drag pulls a cord from a port and only means something where it is let
//! go, another moves a whole selection of boxes by a delta, and a third sweeps a
//! marquee that selects nothing but *itself*. All three are the element's own
//! state, measured against the rect and the workspace zoom that arrive with
//! every step.
//!
//! Its plane is its own: a box's `x`/`y` are **canvas units** relative to the
//! widget origin, seen through the enclosing workspace's zoom, so the element
//! needs no container's axis — the one leaf of the group-aware three that asks
//! for nothing at all. What it does drive is the workspace's **content extent**
//! ([`Element::content_size`]): the graph is laid out by the host, so how far it
//! reaches is a fact only the element holds.
//!
//! The edit is expressed in the owner's terms, twice over: a cord leaves as the
//! flat directed `"wire" src_box outlet dst_box inlet` with the port *names*,
//! and a move leaves as one `"move" index x y` per box in canvas units — the
//! driver adds the cord or the position and sends back a fresh drawing.

use clausters_core::osc::OscType;
use serde_json::{Map, Value};

use crate::host::graphics::patch::{self, PatchDraw, Port, Side};
use crate::host::layout::Rect;
use crate::host::paint::Draw;
use crate::host::widget::element::{Claim, Ctx, Element, Events, HitArea, Input};
use crate::host::widget::parse::{label, set_label};

/// A patcher over its own canvas. `selected` and `drag` are native view state —
/// the gestures build them and no `/gui_set` writes them.
#[derive(Debug, Clone)]
pub struct Patch {
    patch: PatchDraw,
    /// The multi-box selection (box indices). It clears when a script replaces
    /// `boxes`, since the indices would dangle over the new list.
    selected: Vec<usize>,
    label: Option<String>,
    drag: Option<Drag>,
}

/// What a held press on a patcher is doing. Each carries its **press-time data**
/// and no geometry: the rect and the zoom arrive with every step.
#[derive(Debug, Clone)]
enum Drag {
    /// A cord being pulled from a port: the grabbed `(box, side, index)` and the
    /// cursor it is drawn to. It acts only on release — over a compatible port
    /// (an outlet↔inlet of matching rate) it draws a cord, anywhere else it
    /// cancels.
    Wire {
        port: (usize, Side, usize),
        cursor: (f64, f64),
    },
    /// The selection moving as one: the grabbed boxes with their canvas
    /// positions at press time, moved together by the cursor delta and emitted
    /// as one `"move"` per box on release.
    Move {
        origin: (f64, f64),
        grabbed: Vec<(usize, f32, f32)>,
        moved: bool,
    },
    /// The selection marquee on the bare canvas: the selected set follows the
    /// rectangle live, and the rectangle itself is drawn by this element.
    Marquee {
        origin: (f64, f64),
        cursor: (f64, f64),
    },
}

pub(crate) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(from_props(props)))
}

/// The props a patcher node carries, read once — shared by the constructor and
/// by the tests beside it.
fn from_props(props: &Map<String, Value>) -> Patch {
    Patch {
        patch: parse_patch(props),
        selected: Vec::new(),
        label: label(props),
        drag: None,
    }
}

impl Patch {
    #[cfg(test)]
    /// The graph as it stands, for the crate's own gesture suite — which drives
    /// a real host and has no other way to see what a drag wrote.
    pub(crate) fn draw_state(&self) -> &PatchDraw {
        &self.patch
    }

    #[cfg(test)]
    /// The multi-box selection, for the same suite: it is view state, so no
    /// `/gui_query` reports it.
    pub(crate) fn selected(&self) -> &[usize] {
        &self.selected
    }

    /// The boxes the rectangle between `a` and `b` (device pixels) touches —
    /// the marquee's write, recomputed on every step because the set *is* the
    /// rectangle rather than a thing accumulated along it.
    fn boxes_in(&self, rect: Rect, a: (f64, f64), b: (f64, f64), scale: f32) -> Vec<usize> {
        let sel = crate::host::gestures::corner_rect(a, b);
        let overlaps = |r: Rect| {
            r.x < sel.x + sel.w && sel.x < r.x + r.w && r.y < sel.y + sel.h && sel.y < r.y + r.h
        };
        (0..self.patch.boxes.len())
            .filter(|&i| overlaps(patch::obj_rect(rect, &self.patch, i, scale)))
            .collect()
    }
}

impl Element for Patch {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            // The whole patch at once (its parts are arrays, and a `/gui_set`
            // value is a scalar — so they ride as their JSON, like `points`).
            "boxes" | "cords" => {
                let value = match v {
                    Value::String(s) => match serde_json::from_str::<Value>(s) {
                        Ok(parsed) => parsed,
                        Err(_) => return false,
                    },
                    other => other.clone(),
                };
                let props = std::iter::once((key.to_string(), value)).collect();
                let parsed = parse_patch(&props);
                match key {
                    "boxes" if !parsed.boxes.is_empty() => self.patch.boxes = parsed.boxes,
                    "cords" => self.patch.cords = parsed.cords,
                    _ => return false,
                }
                // The box selection would dangle over a replaced `boxes` list.
                if key == "boxes" {
                    self.selected.clear();
                }
                true
            }
            "label" => set_label(&mut self.label, v),
            _ => false,
        }
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        // Flat geometry in the window's one mesh, like the other static views.
        // The canvas scales with the enclosing workspace's zoom, so boxes,
        // cords and text zoom together.
        let (live, marquee) = match &self.drag {
            Some(Drag::Wire { port, cursor }) => {
                (Some((*port, (cursor.0 as f32, cursor.1 as f32))), None)
            }
            Some(Drag::Marquee { origin, cursor }) => (
                None,
                Some(crate::host::gestures::corner_rect(*origin, *cursor)),
            ),
            _ => (None, None),
        };
        patch::draw(
            d,
            ctx.rect,
            &self.patch,
            self.label.as_deref(),
            &patch::CanvasState {
                live,
                selected: &self.selected,
                marquee,
                scale: ctx.scale,
            },
        );
    }

    /// The graph the host laid out, in canvas units: what a workspace sizes its
    /// content to, since only the element knows how far its boxes reach.
    fn content_size(&self) -> Option<(f32, f32)> {
        Some(patch::natural_size(&self.patch))
    }

    /// **The canvas is the panel, and the paper around it is the workspace's.**
    /// The frame the renderer draws hugs the laid-out boxes, while the widget's
    /// rect is whatever the scroll view gave it — never smaller than the window.
    /// Claiming that whole rect made a drag on the empty paper *beside* the
    /// graph sweep a marquee, where nothing is drawn and the workspace should
    /// simply pan. The two now agree, the same way a knob's disc and a
    /// checkbox's box do: what can be grabbed is exactly what is drawn.
    ///
    /// A [`HitArea::Region`] and not a rect, so the machine adds no slop: this
    /// edge is a **boundary** — canvas on one side, workspace on the other —
    /// and a few pixels of air around it start a selection outside the frame
    /// the reader is looking at.
    fn hit_area(&self, input: &Input) -> HitArea {
        HitArea::Region(patch::content_rect(input.rect, &self.patch, input.scale))
    }

    fn press(&mut self, at: (f64, f64), input: &Input) -> Claim {
        let (rect, scale) = (input.rect, input.scale);
        // A port wins: the cord drag. Then a box: select it and start a move (a
        // press on an already-selected box keeps the set, so the drag moves the
        // whole selection). The bare canvas sweeps the marquee — the element's
        // own, since what it selects is the element's — and leaves the modifier
        // that is the *container's*: Shift+drag pans the workspace under it.
        if let Some(port) = patch::port_hit(rect, &self.patch, at.0, at.1, scale) {
            self.drag = Some(Drag::Wire { port, cursor: at });
            return Claim::take();
        }
        if let Some(hit) = patch::box_hit(rect, &self.patch, at.0, at.1, scale) {
            if !self.selected.contains(&hit) {
                self.selected = vec![hit];
            }
            self.drag = Some(Drag::Move {
                origin: at,
                grabbed: self
                    .selected
                    .iter()
                    .map(|&i| {
                        let (x, y) = patch::box_pos(rect, &self.patch, i, scale);
                        (i, x, y)
                    })
                    .collect(),
                moved: false,
            });
            return Claim::take();
        }
        if input.mods.shift {
            return Claim::Decline;
        }
        self.selected.clear();
        self.drag = Some(Drag::Marquee {
            origin: at,
            cursor: at,
        });
        Claim::take()
    }

    fn drag(&mut self, at: (f64, f64), input: &Input) -> Events {
        let (rect, scale) = (input.rect, input.scale);
        match self.drag.take() {
            Some(Drag::Wire { port, .. }) => {
                self.drag = Some(Drag::Wire { port, cursor: at });
            }
            Some(Drag::Move {
                origin,
                grabbed,
                moved: _,
            }) => {
                // The whole grabbed set moves by the cursor delta, in canvas
                // units (the screen delta divided by the workspace zoom). The
                // position becomes explicit from a box's first drag.
                let (dx, dy) = delta(origin, at, scale);
                for &(i, x0, y0) in &grabbed {
                    if let Some(o) = self.patch.boxes.get_mut(i) {
                        (o.x, o.y) = (Some(x0 + dx), Some(y0 + dy));
                    }
                }
                self.drag = Some(Drag::Move {
                    origin,
                    grabbed,
                    moved: true,
                });
            }
            Some(Drag::Marquee { origin, .. }) => {
                self.selected = self.boxes_in(rect, origin, at, scale);
                self.drag = Some(Drag::Marquee { origin, cursor: at });
            }
            None => {}
        }
        // Nothing leaves along the way: a cord means something only where it is
        // let go, and the moved boxes are reported as one round trip on release.
        Events::none()
    }

    fn release(&mut self, at: (f64, f64), _inside: bool, input: &Input) -> Events {
        let (rect, scale) = (input.rect, input.scale);
        match self.drag.take() {
            // Released over a compatible port: the cord is added (deduped) and
            // the edit leaves as the flat directed `"wire"` event, so the driver
            // adds it and re-renders. Anything else cancels.
            Some(Drag::Wire { port, .. }) => {
                let Some(drop) = patch::port_hit(rect, &self.patch, at.0, at.1, scale) else {
                    return Events::none();
                };
                let Some(cord) = patch::cord_between(&self.patch, port, drop) else {
                    return Events::none();
                };
                let names = self
                    .patch
                    .boxes
                    .get(cord.from)
                    .and_then(|b| b.outlets.get(cord.from_out))
                    .map(|p| p.name.clone())
                    .zip(
                        self.patch
                            .boxes
                            .get(cord.to)
                            .and_then(|b| b.inlets.get(cord.to_in))
                            .map(|p| p.name.clone()),
                    );
                let Some((outlet, inlet)) = names else {
                    return Events::none();
                };
                if !self.patch.cords.contains(&cord) {
                    self.patch.cords.push(cord);
                }
                Events::message(vec![
                    OscType::String("wire".into()),
                    OscType::Int(cord.from as i32),
                    OscType::String(outlet),
                    OscType::Int(cord.to as i32),
                    OscType::String(inlet),
                ])
            }
            // The boxes moved live along the drag; the release emits the round
            // trip — one `"move" index x y` per box, so the driver owns the
            // geometry (the clip pattern).
            Some(Drag::Move {
                origin,
                grabbed,
                moved: true,
            }) => {
                let (dx, dy) = delta(origin, at, scale);
                grabbed.into_iter().fold(Events::none(), |ev, (i, x0, y0)| {
                    ev.and(vec![
                        OscType::String("move".into()),
                        OscType::Int(i as i32),
                        OscType::Float(x0 + dx),
                        OscType::Float(y0 + dy),
                    ])
                })
            }
            // The selection followed the marquee live; the release just drops
            // the chrome (the redraw a silent release already asks for).
            _ => Events::none(),
        }
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }
}

/// A cursor delta in **canvas units**: the screen delta divided by the
/// workspace zoom the boxes are seen through.
fn delta(origin: (f64, f64), at: (f64, f64), scale: f32) -> (f32, f32) {
    (
        ((at.0 - origin.0) / scale as f64) as f32,
        ((at.1 - origin.1) / scale as f64) as f32,
    )
}

/// Parses a `patch` widget's patch: `members` (each a `name` plus its wired
/// control `ports`), `buses` (names, `OUT` among them) and `wires` (flat triples
/// `[member, control, bus]`). A malformed entry is skipped, so a partial patch
/// still draws.
fn parse_patch(props: &serde_json::Map<String, Value>) -> PatchDraw {
    use crate::host::graphics::patch::{BoxRole, Cord, Obj};

    let boxes = props
        .get("boxes")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|b| {
                    let role = match b.get("role").and_then(Value::as_str) {
                        Some("source") => BoxRole::Source,
                        Some("const") => BoxRole::Const,
                        _ => BoxRole::Object,
                    };
                    Some(Obj {
                        def: b.get("def")?.as_str()?.to_string(),
                        inlets: parse_ports(b.get("inlets")),
                        outlets: parse_ports(b.get("outlets")),
                        x: b.get("x").and_then(Value::as_f64).map(|n| n as f32),
                        y: b.get("y").and_then(Value::as_f64).map(|n| n as f32),
                        role,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    // A cord is a flat `from_box from_outlet to_box to_inlet` quadruple.
    let cords = props
        .get("cords")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .as_chunks::<4>()
                .0
                .iter()
                .filter_map(|c| {
                    Some(Cord {
                        from: c[0].as_u64()? as usize,
                        from_out: c[1].as_u64()? as usize,
                        to: c[2].as_u64()? as usize,
                        to_in: c[3].as_u64()? as usize,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    PatchDraw { boxes, cords }
}

/// Parses a box's port array: each entry a plain name string (audio, the
/// default) or an object `{"name": …, "rate": "audio"|"control"|"init"}`.
fn parse_ports(v: Option<&Value>) -> Vec<Port> {
    v.and_then(Value::as_array)
        .map(|ps| {
            ps.iter()
                .filter_map(|p| match p {
                    Value::String(name) => Some(Port::audio(name.clone())),
                    Value::Object(o) => {
                        let name = o.get("name")?.as_str()?.to_string();
                        Some(match o.get("rate").and_then(Value::as_str) {
                            Some("control") => Port::control(name),
                            Some("init") => Port::init(name),
                            _ => Port::audio(name),
                        })
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::metrics::Metrics;
    use crate::host::widget::element::Mods;

    fn props(json: &str) -> Map<String, Value> {
        serde_json::from_str(json).unwrap()
    }

    /// `tone → dac`, both boxes placed explicitly so every hit below is
    /// arithmetic on the props rather than on the auto layout.
    fn graph() -> Patch {
        from_props(&props(
            r#"{"boxes":[{"def":"tone","x":0.0,"y":0.0,"outlets":["out"]},
                         {"def":"dac","x":0.0,"y":120.0,"inlets":["in"]}]}"#,
        ))
    }

    fn input<'a>(m: &'a Metrics, rect: Rect, mods: Mods) -> Input<'a> {
        Input {
            metrics: m,
            indent: 0.0,
            rect,
            scale: 1.0,
            mods,
            viewport: (400.0, 300.0),
            time: None,
        }
    }

    fn rect() -> Rect {
        Rect::new(0.0, 0.0, 400.0, 300.0)
    }

    /// The centre of box `i`, in device pixels.
    fn centre(p: &Patch, i: usize) -> (f64, f64) {
        let r = patch::obj_rect(rect(), &p.patch, i, 1.0);
        ((r.x + r.w / 2.0) as f64, (r.y + r.h / 2.0) as f64)
    }

    #[test]
    fn props_parse_and_default() {
        let p = graph();
        assert_eq!(p.patch.boxes.len(), 2);
        assert!(p.patch.cords.is_empty());
        assert!(from_props(&props("{}")).patch.boxes.is_empty());
    }

    /// A press on an outlet pulls a cord that means nothing until it is let go;
    /// released on a compatible inlet it adds the cord and reports the edit in
    /// the owner's terms — the port *names*, not the pixels it was drawn over.
    #[test]
    fn a_cord_is_drawn_by_grabbing_a_port_and_dropping_it_on_another() {
        let (m, r) = (Metrics::default(), rect());
        let mut p = graph();
        let out = patch::port_pin(r, &p.patch, 0, Side::Out, 0, 1.0);
        let inl = patch::port_pin(r, &p.patch, 1, Side::In, 0, 1.0);

        let at = (out.0 as f64, out.1 as f64);
        assert!(matches!(
            p.press(at, &input(&m, r, Mods::default())),
            Claim::Take(_)
        ));
        assert!(matches!(p.drag, Some(Drag::Wire { .. })));
        // Along the way it only follows the pointer.
        assert!(
            p.drag((200.0, 60.0), &input(&m, r, Mods::default()))
                .is_empty()
        );
        assert!(p.patch.cords.is_empty());

        let ev = p.release(
            (inl.0 as f64, inl.1 as f64),
            true,
            &input(&m, r, Mods::default()),
        );
        assert_eq!(p.patch.cords.len(), 1);
        let msgs = ev.into_messages();
        assert_eq!(
            msgs[0],
            vec![
                OscType::String("wire".into()),
                OscType::Int(0),
                OscType::String("out".into()),
                OscType::Int(1),
                OscType::String("in".into()),
            ]
        );

        // A release on nothing cancels, leaving the patch as it was.
        let mut p = graph();
        p.press((out.0 as f64, out.1 as f64), &input(&m, r, Mods::default()));
        assert!(
            p.release((380.0, 290.0), true, &input(&m, r, Mods::default()))
                .is_empty()
        );
        assert!(p.patch.cords.is_empty());
    }

    /// A box drag moves the whole selection and reports one `"move"` per box, in
    /// canvas units — the driver owns the geometry.
    #[test]
    fn a_box_drag_moves_the_selection_and_reports_one_move_each() {
        let (m, r) = (Metrics::default(), rect());
        let mut p = graph();
        p.selected = vec![0, 1];
        let at = centre(&p, 0);
        assert!(matches!(
            p.press(at, &input(&m, r, Mods::default())),
            Claim::Take(_)
        ));
        assert_eq!(p.selected, vec![0, 1], "a selected box keeps the set");

        let moved = (at.0 + 30.0, at.1 + 10.0);
        assert!(p.drag(moved, &input(&m, r, Mods::default())).is_empty());
        assert_eq!(p.patch.boxes[0].x, Some(30.0));
        assert_eq!(p.patch.boxes[1].y, Some(130.0), "the whole set moves");

        let msgs = p
            .release(moved, true, &input(&m, r, Mods::default()))
            .into_messages();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0][0], OscType::String("move".into()));
        assert_eq!(msgs[1][3], OscType::Float(130.0));

        // A press that never moved reports nothing (it was a selection click).
        let mut p = graph();
        p.press(centre(&p, 1), &input(&m, r, Mods::default()));
        assert_eq!(p.selected, vec![1], "an unselected box takes the set");
        assert!(
            p.release(centre(&p, 1), true, &input(&m, r, Mods::default()))
                .is_empty()
        );
    }

    /// The marquee is the **element's**: it sweeps its own boxes and asks the
    /// machine for nothing, which is what makes the canvas need no coordinate
    /// system of its own. Shift is the container's, so the press walks on.
    #[test]
    fn the_marquee_selects_the_boxes_it_sweeps_and_shift_leaves_the_press() {
        let (m, r) = (Metrics::default(), rect());
        let mut p = graph();
        p.selected = vec![0];
        let empty = (350.0, 250.0);
        assert!(matches!(
            p.press(empty, &input(&m, r, Mods::default())),
            Claim::Take(_)
        ));
        assert!(p.selected.is_empty(), "the press drops the last sweep");

        p.drag((0.0, 0.0), &input(&m, r, Mods::default()));
        assert_eq!(p.selected, vec![0, 1], "the rectangle spans both boxes");
        assert!(
            p.release((0.0, 0.0), true, &input(&m, r, Mods::default()))
                .is_empty()
        );
        assert!(p.drag.is_none());

        // Shift+drag on the bare canvas is the workspace's pan, not a marquee.
        let mut p = graph();
        let shift = Mods {
            shift: true,
            ..Mods::default()
        };
        assert_eq!(p.press(empty, &input(&m, r, shift)), Claim::Decline);
        assert!(p.drag.is_none());
        // A box still takes a Shift press: what falls through is the bare canvas.
        let at = centre(&p, 0);
        assert!(matches!(p.press(at, &input(&m, r, shift)), Claim::Take(_)));
    }

    /// **The paper beside the graph is the workspace's.** The frame the renderer
    /// draws hugs the boxes; the widget's rect is whatever the scroll view gave
    /// it, never smaller than the window. Claiming the whole rect made a drag on
    /// the empty paper at the sides sweep a marquee over nothing — the same
    /// defect a knob's cell corners and a checkbox's air had, and the same fix:
    /// the hit area is what is drawn.
    #[test]
    fn the_canvas_is_the_drawn_panel_and_not_the_rect_it_was_given() {
        let (m, r) = (Metrics::default(), rect());
        let p = graph();
        let HitArea::Region(area) = p.hit_area(&input(&m, r, Mods::default())) else {
            panic!("a patcher's canvas is a region, taken at its edge");
        };
        // No slop: the edge is a boundary, and a selection that starts a few
        // pixels outside the frame is the whole of what the reader sees wrong.
        let m_slop = Metrics::default().hit_slop;
        assert!(m_slop > 0.0, "the metric exists, and this area declines it");
        assert!(
            !p.hit_area(&input(&m, r, Mods::default())).hit(
                (area.x + area.w + 2.0) as f64,
                (area.y + area.h / 2.0) as f64,
                m_slop,
            ),
            "two pixels outside the panel is outside the canvas"
        );
        assert!(
            area.w < r.w && area.h < r.h,
            "the panel is smaller than the paper it sits on: {area:?} in {r:?}"
        );
        let inside = |x: f64, y: f64| {
            x >= area.x as f64
                && y >= area.y as f64
                && x <= (area.x + area.w) as f64
                && y <= (area.y + area.h) as f64
        };
        // Every box is inside it, and the sides are not.
        for i in 0..p.patch.boxes.len() {
            let (x, y) = centre(&p, i);
            assert!(inside(x, y), "box {i} at ({x}, {y}) is outside the panel");
        }
        assert!(
            !inside((r.x + r.w - 4.0) as f64, (r.y + r.h / 2.0) as f64),
            "the paper right of the graph is the workspace's; panel {area:?} in {r:?}"
        );
    }

    /// A replaced `boxes` list drops the selection, whose indices would dangle.
    #[test]
    fn a_set_replaces_the_graph_and_clears_the_selection() {
        let mut p = graph();
        p.selected = vec![1];
        assert!(p.set("boxes", &Value::from(r#"[{"def":"one"}]"#)));
        assert_eq!(p.patch.boxes.len(), 1);
        assert!(p.selected.is_empty());
        assert!(p.set("cords", &Value::from("[]")));
        assert!(!p.set("nope", &Value::from(1.0)));
    }

    /// The workspace around it sizes to the graph, which is the one fact only
    /// the element holds.
    #[test]
    fn it_drives_the_workspace_content_extent() {
        let (w, h) = graph().content_size().unwrap();
        assert!(w > 0.0 && h > 0.0);
        assert_eq!(
            graph().content_size(),
            Some(patch::natural_size(&graph().patch))
        );
    }
}
