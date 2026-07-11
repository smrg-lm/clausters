//! The `graph` widget: a **patcher** view of a bus-wired node graph (a GraphDef).
//!
//! The model's *logical grouping* — members that relate by processing, wired to
//! each other through buses — has a shape of its own, and it is not a timeline:
//! it is a patch. This module draws it, as the data actually is:
//!
//! - a **member** box per node (its def name and its wired controls, each a port);
//! - a **bus** node per internal bus (plus the reserved `OUT`, the hardware);
//! - a **wire** per `(member, control) ↔ bus` pair.
//!
//! Deliberately **bipartite**, because that is what a GraphDef knows: a member's
//! control *touches* a bus. Which side of it writes and which reads is the
//! server's analysis (it sorts the graph), not something the view should guess
//! from a control's name — so the patch shows the connection and leaves the
//! direction to the engine. Rewiring is therefore a well-defined edit: point a
//! control at another bus, or at none.
//!
//! Pure over a [`Mesh`], like the rest of the flat views: layout, hit-test and
//! drawing are unit-testable without a window.

use super::font;
use super::layout::Rect;
use super::paint::{Color, Mesh};

const TEXT: Color = [0.85, 0.87, 0.90, 1.0];
const FIELD: Color = [0.08, 0.09, 0.11, 1.0];
const FRAME: Color = [0.30, 0.34, 0.42, 1.0];
const NODE_FILL: Color = [0.16, 0.22, 0.32, 1.0];
const NODE_EDGE: Color = [0.45, 0.60, 0.85, 1.0];
const BUS_FILL: Color = [0.18, 0.28, 0.24, 1.0];
const BUS_EDGE: Color = [0.40, 0.85, 0.62, 1.0];
const PORT: Color = [0.75, 0.82, 0.92, 1.0];
const WIRE: Color = [0.55, 0.75, 0.95, 0.9];
const WIRE_LIVE: Color = [0.95, 0.72, 0.25, 1.0];

const PAD: f32 = 8.0;
const NODE_W: f32 = 150.0;
const ROW_H: f32 = 16.0;
const HEAD_H: f32 = 20.0;
const BUS_W: f32 = 96.0;
const BUS_H: f32 = 22.0;
const TEXT_SCALE: f32 = 1.5;
/// The port square's half-size (also its hit radius floor), device pixels.
pub const PORT_R: f32 = 4.0;

/// One member of the graph: its def name and the controls that are wired (each
/// drawn as a port on the box's right edge).
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    pub name: String,
    pub ports: Vec<String>,
}

/// One wire: a member's control, and the bus it touches.
#[derive(Clone, Debug, PartialEq)]
pub struct Wire {
    pub member: usize,
    pub control: String,
    pub bus: String,
}

/// A graph to draw: the members, the buses (in declaration order — `OUT` is the
/// hardware, and is shown like any other), and the wires between them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GraphDraw {
    pub members: Vec<Member>,
    pub buses: Vec<String>,
    pub wires: Vec<Wire>,
}

/// The height a member box needs for its ports.
fn member_h(member: &Member) -> f32 {
    HEAD_H + member.ports.len() as f32 * ROW_H + PAD
}

/// The box of member `i`: the members stack down the left column.
pub fn member_rect(area: Rect, graph: &GraphDraw, i: usize) -> Rect {
    let mut y = area.y + PAD;
    for m in &graph.members[..i.min(graph.members.len())] {
        y += member_h(m) + PAD;
    }
    let h = graph.members.get(i).map_or(HEAD_H, member_h);
    Rect::new(area.x + PAD, y, NODE_W.min(area.w - 2.0 * PAD), h)
}

/// The pin of member `i`'s port `p`: the square on the box's right edge, one row
/// per wired control. `(x, y)` is its centre.
pub fn port_pin(area: Rect, graph: &GraphDraw, i: usize, p: usize) -> (f32, f32) {
    let r = member_rect(area, graph, i);
    (r.x + r.w, r.y + HEAD_H + (p as f32 + 0.5) * ROW_H)
}

/// The box of bus `b`: the buses stack down the right column.
pub fn bus_rect(area: Rect, _graph: &GraphDraw, b: usize) -> Rect {
    let x = area.x + area.w - PAD - BUS_W.min(area.w * 0.4);
    let y = area.y + PAD + b as f32 * (BUS_H + PAD);
    Rect::new(x, y, BUS_W.min(area.w * 0.4), BUS_H)
}

/// The pin of bus `b`: the point wires land on (its left edge, centred).
pub fn bus_pin(area: Rect, graph: &GraphDraw, b: usize) -> (f32, f32) {
    let r = bus_rect(area, graph, b);
    (r.x, r.y + r.h * 0.5)
}

/// The member port under `(x, y)`, as `(member, port)` — the grab point of a
/// rewiring drag.
pub fn port_hit(area: Rect, graph: &GraphDraw, x: f64, y: f64) -> Option<(usize, usize)> {
    let radius = (PORT_R * 2.0).max(6.0) as f64;
    for (i, m) in graph.members.iter().enumerate() {
        for p in 0..m.ports.len() {
            let (px, py) = port_pin(area, graph, i, p);
            let d = ((x - px as f64).powi(2) + (y - py as f64).powi(2)).sqrt();
            if d <= radius {
                return Some((i, p));
            }
        }
    }
    None
}

/// The bus under `(x, y)` — the drop target of a rewiring drag (its whole box,
/// so it is easy to hit). `None` over empty space, which *unwires*.
pub fn bus_hit(area: Rect, graph: &GraphDraw, x: f64, y: f64) -> Option<usize> {
    (0..graph.buses.len()).find(|&b| bus_rect(area, graph, b).contains(x, y))
}

/// Draws the patch: the member boxes with their ports, the bus nodes, and a wire
/// per connection. `live` is a rewiring drag in flight — the port being dragged
/// and the cursor — drawn as a wire to the pointer.
pub fn draw(
    mesh: &mut Mesh,
    area: Rect,
    graph: &GraphDraw,
    label: Option<&str>,
    live: Option<((usize, usize), (f32, f32))>,
) {
    mesh.rect(area, FIELD);
    mesh.border(area, 1.0, FRAME);
    if let Some(text) = label {
        font::text(mesh, text, area.x + PAD, area.y + 2.0, TEXT_SCALE, TEXT);
    }

    // The buses (right column): a node per bus, `OUT` among them.
    for (b, bus) in graph.buses.iter().enumerate() {
        let r = bus_rect(area, graph, b);
        mesh.rect(r, BUS_FILL);
        mesh.border(r, 1.0, BUS_EDGE);
        font::text(mesh, bus, r.x + PAD * 0.5, r.y + 4.0, TEXT_SCALE, TEXT);
        let (px, py) = bus_pin(area, graph, b);
        mesh.rect(
            Rect::new(px - PORT_R, py - PORT_R, PORT_R * 2.0, PORT_R * 2.0),
            PORT,
        );
    }

    // The members (left column): the def name, then a row per wired control.
    for (i, m) in graph.members.iter().enumerate() {
        let r = member_rect(area, graph, i);
        mesh.rect(r, NODE_FILL);
        mesh.border(r, 1.0, NODE_EDGE);
        font::text(mesh, &m.name, r.x + PAD * 0.5, r.y + 4.0, TEXT_SCALE, TEXT);
        for (p, port) in m.ports.iter().enumerate() {
            let (px, py) = port_pin(area, graph, i, p);
            font::text(
                mesh,
                port,
                r.x + PAD * 0.5,
                py - font::height(TEXT_SCALE) * 0.5,
                TEXT_SCALE,
                TEXT,
            );
            mesh.rect(
                Rect::new(px - PORT_R, py - PORT_R, PORT_R * 2.0, PORT_R * 2.0),
                PORT,
            );
        }
    }

    // The wires: control pin -> bus pin.
    for wire in &graph.wires {
        let Some(p) = graph
            .members
            .get(wire.member)
            .and_then(|m| m.ports.iter().position(|c| *c == wire.control))
        else {
            continue;
        };
        let Some(b) = graph.buses.iter().position(|bus| *bus == wire.bus) else {
            continue;
        };
        let (x0, y0) = port_pin(area, graph, wire.member, p);
        let (x1, y1) = bus_pin(area, graph, b);
        mesh.line([x0, y0], [x1, y1], 1.5, WIRE);
    }

    // The wire being dragged, from its port to the cursor.
    if let Some(((i, p), (cx, cy))) = live {
        let (x0, y0) = port_pin(area, graph, i, p);
        mesh.line([x0, y0], [cx, cy], 1.5, WIRE_LIVE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain() -> GraphDraw {
        GraphDraw {
            members: vec![
                Member {
                    name: "gsrc".into(),
                    ports: vec!["out".into()],
                },
                Member {
                    name: "gsink".into(),
                    ports: vec!["in".into(), "out".into()],
                },
            ],
            buses: vec!["mix".into(), "OUT".into()],
            wires: vec![
                Wire {
                    member: 0,
                    control: "out".into(),
                    bus: "mix".into(),
                },
                Wire {
                    member: 1,
                    control: "in".into(),
                    bus: "mix".into(),
                },
                Wire {
                    member: 1,
                    control: "out".into(),
                    bus: "OUT".into(),
                },
            ],
        }
    }

    fn area() -> Rect {
        Rect::new(0.0, 0.0, 600.0, 400.0)
    }

    #[test]
    fn members_stack_down_the_left_and_buses_down_the_right() {
        let g = chain();
        let m0 = member_rect(area(), &g, 0);
        let m1 = member_rect(area(), &g, 1);
        assert!(m1.y > m0.y + m0.h, "the second member sits below the first");
        assert!(m1.h > m0.h, "a member with two ports is taller");
        let b0 = bus_rect(area(), &g, 0);
        assert!(b0.x > m0.x + m0.w, "the buses are the right column");
    }

    #[test]
    fn a_port_is_hit_where_its_pin_is_drawn() {
        let g = chain();
        let (px, py) = port_pin(area(), &g, 1, 1); // gsink's `out`
        assert_eq!(port_hit(area(), &g, px as f64, py as f64), Some((1, 1)));
        // Away from any pin: nothing (the drag starts only on a port).
        assert_eq!(port_hit(area(), &g, px as f64 + 30.0, py as f64), None);
    }

    #[test]
    fn a_bus_is_the_drop_target_and_empty_space_is_not() {
        let g = chain();
        let r = bus_rect(area(), &g, 0);
        let (cx, cy) = (r.x as f64 + 4.0, r.y as f64 + 4.0);
        assert_eq!(bus_hit(area(), &g, cx, cy), Some(0));
        // Dropping on nothing unwires (the caller reads `None` that way).
        assert_eq!(bus_hit(area(), &g, 300.0, 380.0), None);
    }

    #[test]
    fn a_control_touches_one_bus_at_a_time() {
        // The invariant the rewiring edit keeps: one wire per (member, control).
        let g = chain();
        let wired: Vec<_> = g
            .wires
            .iter()
            .map(|w| (w.member, w.control.as_str()))
            .collect();
        let mut seen = wired.clone();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), wired.len(), "no control is wired twice");
    }

    #[test]
    fn the_patch_draws_its_members_buses_and_wires() {
        let mut m = Mesh::new();
        draw(&mut m, area(), &chain(), Some("chain"), None);
        assert!(!m.is_empty());

        // A wire in flight adds the line to the cursor.
        let mut with_live = Mesh::new();
        draw(
            &mut with_live,
            area(),
            &chain(),
            Some("chain"),
            Some(((0, 0), (400.0, 200.0))),
        );
        assert!(with_live.vertex_count() > m.vertex_count());
    }
}
