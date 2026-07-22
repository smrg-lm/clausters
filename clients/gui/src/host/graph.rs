//! The `graph` widget: a **directed, typed** patcher (a level-1 GraphDef).
//!
//! A box has **inlets on its top edge** and **outlets on its bottom edge**, each
//! typed (audio or control), and a **cord** runs `outlet → inlet`. That is the
//! whole surface: the picture reads as signal flow, top to bottom, and the buses
//! are not drawn — a cord *is* a bus (the client's cord→bus pass,
//! `clausters_core::patch`, names them; the user never does). Direction is not a
//! guess: it comes from the def (a control feeding an `In` is an inlet, one
//! feeding an `Out` an outlet), so drawing it directed is honest.
//!
//! Pure over a [`Mesh`], like the rest of the flat views: layout, hit-test and
//! drawing are unit-testable without a window.

use clausters_core::patch::Rate;

use super::font;
use super::layout::Rect;
use super::paint::Mesh;
use super::theme::{Theme, with_alpha};

const PAD: f32 = 8.0;
const OBJ_W: f32 = 96.0;
/// The middle band of a box, holding the def name.
const HEAD_H: f32 = 18.0;
/// A port-name row: the inlet names sit under the top edge, the outlet names
/// over the bottom edge (so a box reads inlets / def / outlets, top to bottom).
const LABEL_H: f32 = 13.0;
/// The vertical gap the auto-stack leaves between boxes — wider than a port's
/// hit diameter, so an outlet on one box and the inlet on the box below it stay
/// separately hittable (and there is room for the cord between them).
const ROW_GAP: f32 = 40.0;
/// The gap between a port's pin and its label.
const PORT_TEXT_GAP: f32 = 3.0;
const TEXT_SCALE: f32 = 1.5;
/// The port names are drawn a little smaller than the def name.
const LABEL_SCALE: f32 = 1.1;
/// The port square's half-size (also its hit radius floor), device pixels.
pub const PORT_R: f32 = 4.0;

/// Which edge a port sits on: an inlet (top) reads, an outlet (bottom) writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    In,
    Out,
}

/// One port of a box: the def control it stands for, and its cord type (rate).
#[derive(Clone, Debug, PartialEq)]
pub struct Port {
    pub name: String,
    pub rate: Rate,
}

impl Port {
    /// An audio port named `name`.
    pub fn audio(name: impl Into<String>) -> Port {
        Port {
            name: name.into(),
            rate: Rate::Audio,
        }
    }
    /// A control port named `name`.
    pub fn control(name: impl Into<String>) -> Port {
        Port {
            name: name.into(),
            rate: Rate::Control,
        }
    }
}

/// One box: a member def with its typed inlets and outlets. `x`/`y` place it
/// freely on the canvas (canvas units, relative to the widget origin); absent,
/// the box auto-stacks down the left column (the default).
#[derive(Clone, Debug, PartialEq)]
pub struct Obj {
    pub def: String,
    pub inlets: Vec<Port>,
    pub outlets: Vec<Port>,
    pub x: Option<f32>,
    pub y: Option<f32>,
}

impl Obj {
    /// A box with the given def name and typed ports, auto-placed.
    pub fn new(def: impl Into<String>, inlets: Vec<Port>, outlets: Vec<Port>) -> Obj {
        Obj {
            def: def.into(),
            inlets,
            outlets,
            x: None,
            y: None,
        }
    }

    fn ports(&self, side: Side) -> &[Port] {
        match side {
            Side::In => &self.inlets,
            Side::Out => &self.outlets,
        }
    }
}

/// One directed cord: box `from`'s outlet `from_out` → box `to`'s inlet `to_in`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cord {
    pub from: usize,
    pub from_out: usize,
    pub to: usize,
    pub to_in: usize,
}

/// A patch to draw: the boxes and the directed cords between their ports.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GraphDraw {
    pub boxes: Vec<Obj>,
    pub cords: Vec<Cord>,
}

/// One port's slot: the pin, a gap, then its left-justified label. Ports flow
/// left to right, so the slot width is what the next port starts after.
fn slot_w(port: &Port) -> f32 {
    2.0 * PORT_R + PORT_TEXT_GAP + font::width(&port.name, LABEL_SCALE) + PAD
}

/// The width one edge needs: the left inset plus every port's slot.
fn edge_w(ports: &[Port]) -> f32 {
    PAD + ports.iter().map(slot_w).sum::<f32>()
}

/// The x of port `p`'s pin **centre**, canvas units from the box's left edge:
/// ports are laid out left-justified in flow order.
fn port_offset(o: &Obj, side: Side, p: usize) -> f32 {
    let ports = o.ports(side);
    let before: f32 = ports[..p.min(ports.len())].iter().map(slot_w).sum();
    PAD + before + PORT_R
}

/// The width a box needs: the busier labelled edge (or the def name), floored at
/// [`OBJ_W`].
fn obj_w(o: &Obj) -> f32 {
    let def = 2.0 * PAD + font::width(&o.def, TEXT_SCALE);
    OBJ_W
        .max(edge_w(&o.inlets))
        .max(edge_w(&o.outlets))
        .max(def)
}

/// The box of `i`, at `scale` (the enclosing workspace's zoom, `1.0` bare):
/// placed at its explicit `x`/`y` when it has them, else auto-stacked down the
/// left column.
pub fn obj_rect(area: Rect, graph: &GraphDraw, i: usize, scale: f32) -> Rect {
    let h = (HEAD_H + 2.0 * LABEL_H) * scale;
    let w = graph.boxes.get(i).map_or(OBJ_W, obj_w) * scale;
    if let Some(o) = graph.boxes.get(i)
        && let (Some(x), Some(y)) = (o.x, o.y)
    {
        return Rect::new(area.x + x * scale, area.y + y * scale, w, h);
    }
    let y = area.y + (PAD + i as f32 * (HEAD_H + 2.0 * LABEL_H + ROW_GAP)) * scale;
    Rect::new(area.x + PAD * scale, y, w, h)
}

/// The pin of box `i`'s port `p` on `side`: ports are laid out left-justified in
/// flow order along the top edge (inlets) / bottom edge (outlets). `(x, y)` is
/// the pin's centre.
pub fn port_pin(
    area: Rect,
    graph: &GraphDraw,
    i: usize,
    side: Side,
    p: usize,
    scale: f32,
) -> (f32, f32) {
    let r = obj_rect(area, graph, i, scale);
    let off = graph
        .boxes
        .get(i)
        .map_or(PAD + PORT_R, |o| port_offset(o, side, p));
    let x = r.x + off * scale;
    let y = match side {
        Side::In => r.y,
        Side::Out => r.y + r.h,
    };
    (x, y)
}

/// The port under `(x, y)`, as `(box, side, port)` — the grab point of a cord
/// drag. Inlets and outlets both hit; the caller pairs an outlet with an inlet.
pub fn port_hit(
    area: Rect,
    graph: &GraphDraw,
    x: f64,
    y: f64,
    scale: f32,
) -> Option<(usize, Side, usize)> {
    let radius = ((PORT_R * scale) * 2.0).max(6.0) as f64;
    let hit = |i: usize, side: Side, p: usize| {
        let (px, py) = port_pin(area, graph, i, side, p, scale);
        ((x - px as f64).powi(2) + (y - py as f64).powi(2)).sqrt() <= radius
    };
    for (i, o) in graph.boxes.iter().enumerate() {
        for p in 0..o.inlets.len() {
            if hit(i, Side::In, p) {
                return Some((i, Side::In, p));
            }
        }
        for p in 0..o.outlets.len() {
            if hit(i, Side::Out, p) {
                return Some((i, Side::Out, p));
            }
        }
    }
    None
}

/// The box under `(x, y)` — the grab point of a move drag and the click target
/// of the selection. A port hit is the caller's business and wins over this.
pub fn box_hit(area: Rect, graph: &GraphDraw, x: f64, y: f64, scale: f32) -> Option<usize> {
    (0..graph.boxes.len()).find(|&i| obj_rect(area, graph, i, scale).contains(x, y))
}

/// The current position of box `i` in canvas units (its explicit `x`/`y`, or
/// where the auto layout put it) — the value a starting move drag latches.
pub fn box_pos(area: Rect, graph: &GraphDraw, i: usize, scale: f32) -> (f32, f32) {
    let r = obj_rect(area, graph, i, scale);
    ((r.x - area.x) / scale, (r.y - area.y) / scale)
}

/// The transient canvas state the renderer passes per frame: a cord drag in
/// flight (the grabbed port and the cursor, drawn as a cord to the pointer),
/// the selected set, the marquee rectangle, and the workspace zoom.
pub struct CanvasState<'a> {
    #[allow(clippy::type_complexity)] // (box, side, index), (cursor) — a grabbed port
    pub live: Option<((usize, Side, usize), (f32, f32))>,
    pub selected: &'a [usize],
    pub marquee: Option<Rect>,
    pub scale: f32,
}

/// The stroke width a cord of `rate` is drawn with: audio heavy, control thin.
fn cord_weight(rate: Rate, scale: f32) -> f32 {
    let base = match rate {
        Rate::Audio => 2.5,
        Rate::Control => 1.2,
    };
    base * scale
}

/// Draws the patch: the boxes with their inlet/outlet pins, the directed cords
/// (typed by weight), and the transient canvas chrome in `state`.
pub fn draw(
    mesh: &mut Mesh,
    area: Rect,
    graph: &GraphDraw,
    label: Option<&str>,
    state: &CanvasState<'_>,
    theme: &Theme,
) {
    let CanvasState {
        live,
        selected,
        marquee,
        scale,
    } = *state;
    let ts = TEXT_SCALE * scale;
    let port_r = PORT_R * scale;
    mesh.rect(area, theme.view_field);
    mesh.border(area, 1.0, theme.frame);
    if let Some(text) = label {
        font::text(
            mesh,
            text,
            area.x + PAD * scale,
            area.y + 2.0,
            ts,
            theme.text,
        );
    }

    // The cords, first, so the boxes and pins sit over them.
    for cord in &graph.cords {
        let (Some(src), Some(dst)) = (graph.boxes.get(cord.from), graph.boxes.get(cord.to)) else {
            continue;
        };
        let Some(port) = src.outlets.get(cord.from_out) else {
            continue;
        };
        if dst.inlets.get(cord.to_in).is_none() {
            continue;
        }
        let (x0, y0) = port_pin(area, graph, cord.from, Side::Out, cord.from_out, scale);
        let (x1, y1) = port_pin(area, graph, cord.to, Side::In, cord.to_in, scale);
        mesh.line(
            [x0, y0],
            [x1, y1],
            cord_weight(port.rate, scale),
            with_alpha(theme.selection, 0.9),
        );
    }

    // The boxes: three bands top to bottom — inlet names in the top strip, the
    // def name in the middle, outlet names in the bottom strip — so a box reads
    // like its signal flow (in on top, out on the bottom). Everything is placed
    // and sized in canvas (scale-1) units, multiplied by `scale` once, so it
    // stays anchored to the box under zoom. All text is left-justified, and a
    // port's name sits just right of its pin, ports flowing left to right.
    let lts = LABEL_SCALE * scale;
    let fh = font::height(TEXT_SCALE); // scale-1 heights, to centre in a band
    let lh = font::height(LABEL_SCALE);
    let def_y = LABEL_H + (HEAD_H - fh) * 0.5; // the def name's band
    let in_y = (LABEL_H - lh) * 0.5; // the inlet strip
    let out_y = LABEL_H + HEAD_H + (LABEL_H - lh) * 0.5; // the outlet strip
    let label_dx = PORT_R + PORT_TEXT_GAP; // a label starts just right of its pin
    for (i, o) in graph.boxes.iter().enumerate() {
        let r = obj_rect(area, graph, i, scale);
        mesh.rect(r, theme.object_fill);
        let sel = selected.contains(&i);
        let edge = if sel {
            theme.selected_edge
        } else {
            theme.object_edge
        };
        mesh.border(r, if sel { 2.0 } else { 1.0 }, edge);
        font::text(
            mesh,
            &o.def,
            r.x + PAD * scale,
            r.y + def_y * scale,
            ts,
            theme.text,
        );
        let pin_rect =
            |px: f32, py: f32| Rect::new(px - port_r, py - port_r, port_r * 2.0, port_r * 2.0);
        for (p, port) in o.inlets.iter().enumerate() {
            let (px, py) = port_pin(area, graph, i, Side::In, p, scale);
            mesh.rect(pin_rect(px, py), theme.port);
            font::text(
                mesh,
                &port.name,
                px + label_dx * scale,
                r.y + in_y * scale,
                lts,
                theme.text,
            );
        }
        for (p, port) in o.outlets.iter().enumerate() {
            let (px, py) = port_pin(area, graph, i, Side::Out, p, scale);
            mesh.rect(pin_rect(px, py), theme.port);
            font::text(
                mesh,
                &port.name,
                px + label_dx * scale,
                r.y + out_y * scale,
                lts,
                theme.text,
            );
        }
    }

    // The cord being dragged, from its grabbed port to the cursor.
    if let Some(((i, side, p), (cx, cy))) = live {
        let (x0, y0) = port_pin(area, graph, i, side, p, scale);
        mesh.line([x0, y0], [cx, cy], 1.5 * scale, theme.live);
    }

    // The selection marquee in flight, over everything.
    if let Some(r) = marquee {
        mesh.rect(r, with_alpha(theme.selection, 0.15));
        mesh.border(r, 1.0, theme.selection);
    }
}

/// Whether a cord can be drawn between two ports: one must be an outlet and the
/// other an inlet, and their rates must match. Returns the normalized cord
/// `(from_box, outlet, to_box, inlet)` — regardless of which end was grabbed —
/// or `None` when the pair is illegal (same side, or a rate mismatch).
pub fn cord_between(
    graph: &GraphDraw,
    a: (usize, Side, usize),
    b: (usize, Side, usize),
) -> Option<Cord> {
    let (out, inl) = match (a.1, b.1) {
        (Side::Out, Side::In) => (a, b),
        (Side::In, Side::Out) => (b, a),
        _ => return None, // both inlets or both outlets
    };
    let out_rate = graph.boxes.get(out.0)?.outlets.get(out.2)?.rate;
    let in_rate = graph.boxes.get(inl.0)?.inlets.get(inl.2)?.rate;
    if out_rate != in_rate {
        return None;
    }
    Some(Cord {
        from: out.0,
        from_out: out.2,
        to: inl.0,
        to_in: inl.2,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// tone (out) → trem (in, out) → dac (in, out), no cords yet placed.
    fn boxes() -> Vec<Obj> {
        vec![
            Obj::new("tone", vec![], vec![Port::audio("out")]),
            Obj::new("trem", vec![Port::audio("in")], vec![Port::audio("out")]),
            Obj::new("dac", vec![Port::audio("in")], vec![Port::audio("out")]),
        ]
    }

    fn chain() -> GraphDraw {
        GraphDraw {
            boxes: boxes(),
            cords: vec![
                Cord {
                    from: 0,
                    from_out: 0,
                    to: 1,
                    to_in: 0,
                }, // tone.out -> trem.in
                Cord {
                    from: 1,
                    from_out: 0,
                    to: 2,
                    to_in: 0,
                }, // trem.out -> dac.in
            ],
        }
    }

    fn area() -> Rect {
        Rect::new(0.0, 0.0, 600.0, 400.0)
    }

    #[test]
    fn boxes_stack_down_the_column_and_scale_with_the_zoom() {
        let g = chain();
        let a = area();
        let b0 = obj_rect(a, &g, 0, 1.0);
        let b1 = obj_rect(a, &g, 1, 1.0);
        assert!(b1.y > b0.y + b0.h, "the second box sits below the first");
        // Under a 2x workspace zoom everything doubles from the area origin.
        let b0z = obj_rect(a, &g, 0, 2.0);
        assert_eq!(b0z.x - a.x, (b0.x - a.x) * 2.0);
        assert_eq!(b0z.h, b0.h * 2.0);
    }

    #[test]
    fn explicit_positions_win() {
        let mut g = chain();
        g.boxes[1].x = Some(300.0);
        g.boxes[1].y = Some(40.0);
        let a = area();
        let b1 = obj_rect(a, &g, 1, 1.0);
        assert_eq!((b1.x, b1.y), (a.x + 300.0, a.y + 40.0));
    }

    #[test]
    fn inlets_sit_on_top_and_outlets_on_the_bottom_edge() {
        let g = chain();
        let a = area();
        let r = obj_rect(a, &g, 1, 1.0); // trem: one inlet, one outlet
        let (_ix, iy) = port_pin(a, &g, 1, Side::In, 0, 1.0);
        let (_ox, oy) = port_pin(a, &g, 1, Side::Out, 0, 1.0);
        assert_eq!(iy, r.y, "the inlet is on the top edge");
        assert_eq!(oy, r.y + r.h, "the outlet is on the bottom edge");
    }

    #[test]
    fn a_port_is_hit_where_its_pin_is_drawn_with_its_side() {
        let g = chain();
        let a = area();
        let (ox, oy) = port_pin(a, &g, 0, Side::Out, 0, 1.0); // tone's outlet
        assert_eq!(
            port_hit(a, &g, ox as f64, oy as f64, 1.0),
            Some((0, Side::Out, 0))
        );
        let (ix, iy) = port_pin(a, &g, 2, Side::In, 0, 1.0); // dac's inlet
        assert_eq!(
            port_hit(a, &g, ix as f64, iy as f64, 1.0),
            Some((2, Side::In, 0))
        );
        // Away from any pin: nothing (a cord drag starts only on a port).
        assert_eq!(port_hit(a, &g, ox as f64 + 40.0, oy as f64, 1.0), None);
    }

    #[test]
    fn a_box_is_hit_and_reports_its_position() {
        let mut g = chain();
        g.boxes[0].x = Some(200.0);
        g.boxes[0].y = Some(100.0);
        let a = area();
        // Hit through a 2x transform: the box sits at 2x its canvas position.
        let r = obj_rect(a, &g, 0, 2.0);
        let hit = box_hit(a, &g, (r.x + 3.0) as f64, (r.y + 3.0) as f64, 2.0);
        assert_eq!(hit, Some(0));
        assert_eq!(box_pos(a, &g, 0, 2.0), (200.0, 100.0));
        // Empty canvas hits nothing (the marquee's surface).
        assert_eq!(box_hit(a, &g, 500.0, 390.0, 1.0), None);
    }

    #[test]
    fn a_cord_pairs_an_outlet_with_an_inlet_regardless_of_grab_order() {
        let g = chain();
        // grab tone.out, drop trem.in -> tone.out -> trem.in
        assert_eq!(
            cord_between(&g, (0, Side::Out, 0), (1, Side::In, 0)),
            Some(Cord {
                from: 0,
                from_out: 0,
                to: 1,
                to_in: 0
            })
        );
        // the reverse grab order normalizes to the same cord.
        assert_eq!(
            cord_between(&g, (1, Side::In, 0), (0, Side::Out, 0)),
            Some(Cord {
                from: 0,
                from_out: 0,
                to: 1,
                to_in: 0
            })
        );
    }

    #[test]
    fn two_outlets_or_two_inlets_make_no_cord() {
        let g = chain();
        assert_eq!(cord_between(&g, (0, Side::Out, 0), (1, Side::Out, 0)), None);
        assert_eq!(cord_between(&g, (1, Side::In, 0), (2, Side::In, 0)), None);
    }

    #[test]
    fn a_rate_mismatch_makes_no_cord() {
        let g = GraphDraw {
            boxes: vec![
                Obj::new("lfo", vec![], vec![Port::control("out")]),
                Obj::new("dac", vec![Port::audio("in")], vec![]),
            ],
            cords: vec![],
        };
        assert_eq!(cord_between(&g, (0, Side::Out, 0), (1, Side::In, 0)), None);
    }

    #[test]
    fn the_patch_draws_its_boxes_and_cords() {
        let mut m = Mesh::new();
        draw(
            &mut m,
            area(),
            &chain(),
            Some("chain"),
            &CanvasState {
                live: None,
                selected: &[],
                marquee: None,
                scale: 1.0,
            },
            &Theme::default(),
        );
        assert!(!m.is_empty());

        // A cord in flight adds the line to the cursor; a selection and a
        // marquee add their chrome.
        let mut more = Mesh::new();
        draw(
            &mut more,
            area(),
            &chain(),
            Some("chain"),
            &CanvasState {
                live: Some(((0, Side::Out, 0), (400.0, 200.0))),
                selected: &[0],
                marquee: Some(Rect::new(50.0, 50.0, 120.0, 90.0)),
                scale: 1.0,
            },
            &Theme::default(),
        );
        assert!(more.vertex_count() > m.vertex_count());
    }
}
