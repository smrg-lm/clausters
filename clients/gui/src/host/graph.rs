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
use super::paint::Mesh;
use super::theme::{Theme, with_alpha};

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
/// drawn as a port on the box's right edge). `x`/`y` place the box freely on
/// the canvas (canvas units, relative to the widget's origin); absent, the box
/// auto-places in the classic stacked left column.
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    pub name: String,
    pub ports: Vec<String>,
    pub x: Option<f32>,
    pub y: Option<f32>,
}

/// One bus node: its name, and the same optional free placement (absent, the
/// stacked right column).
#[derive(Clone, Debug, PartialEq)]
pub struct Bus {
    pub name: String,
    pub x: Option<f32>,
    pub y: Option<f32>,
}

impl Bus {
    pub fn named(name: impl Into<String>) -> Bus {
        Bus {
            name: name.into(),
            x: None,
            y: None,
        }
    }
}

/// Which kind of box a canvas gesture addresses — the `"move"` event's tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxKind {
    Member,
    Bus,
}

impl BoxKind {
    /// The wire form of the kind (the `"move"` payload's second argument).
    pub fn as_str(self) -> &'static str {
        match self {
            BoxKind::Member => "member",
            BoxKind::Bus => "bus",
        }
    }
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
    pub buses: Vec<Bus>,
    pub wires: Vec<Wire>,
}

/// The height a member box needs for its ports.
fn member_h(member: &Member) -> f32 {
    HEAD_H + member.ports.len() as f32 * ROW_H + PAD
}

/// The box of member `i`, at `scale` (the enclosing workspace's zoom, `1.0`
/// bare): placed at its explicit `x`/`y` when it has them, else auto-stacked
/// down the left column (the classic layout, and still the default).
pub fn member_rect(area: Rect, graph: &GraphDraw, i: usize, scale: f32) -> Rect {
    let h = graph.members.get(i).map_or(HEAD_H, member_h) * scale;
    let w = (NODE_W * scale).min(area.w - 2.0 * PAD * scale);
    if let Some(m) = graph.members.get(i)
        && let (Some(x), Some(y)) = (m.x, m.y)
    {
        return Rect::new(area.x + x * scale, area.y + y * scale, w, h);
    }
    let mut y = area.y + PAD * scale;
    for m in &graph.members[..i.min(graph.members.len())] {
        y += (member_h(m) + PAD) * scale;
    }
    Rect::new(area.x + PAD * scale, y, w, h)
}

/// The pin of member `i`'s port `p`: the square on the box's right edge, one row
/// per wired control. `(x, y)` is its centre.
pub fn port_pin(area: Rect, graph: &GraphDraw, i: usize, p: usize, scale: f32) -> (f32, f32) {
    let r = member_rect(area, graph, i, scale);
    (r.x + r.w, r.y + (HEAD_H + (p as f32 + 0.5) * ROW_H) * scale)
}

/// The box of bus `b`: at its explicit `x`/`y` when it has them, else
/// auto-stacked down the right column.
pub fn bus_rect(area: Rect, graph: &GraphDraw, b: usize, scale: f32) -> Rect {
    let w = (BUS_W * scale).min(area.w * 0.4);
    let h = BUS_H * scale;
    if let Some(bus) = graph.buses.get(b)
        && let (Some(x), Some(y)) = (bus.x, bus.y)
    {
        return Rect::new(area.x + x * scale, area.y + y * scale, w, h);
    }
    let x = area.x + area.w - PAD * scale - w;
    let y = area.y + (PAD + b as f32 * (BUS_H + PAD)) * scale;
    Rect::new(x, y, w, h)
}

/// The pin of bus `b`: the point wires land on (its left edge, centred).
pub fn bus_pin(area: Rect, graph: &GraphDraw, b: usize, scale: f32) -> (f32, f32) {
    let r = bus_rect(area, graph, b, scale);
    (r.x, r.y + r.h * 0.5)
}

/// The member port under `(x, y)`, as `(member, port)` — the grab point of a
/// rewiring drag.
pub fn port_hit(
    area: Rect,
    graph: &GraphDraw,
    x: f64,
    y: f64,
    scale: f32,
) -> Option<(usize, usize)> {
    let radius = ((PORT_R * scale) * 2.0).max(6.0) as f64;
    for (i, m) in graph.members.iter().enumerate() {
        for p in 0..m.ports.len() {
            let (px, py) = port_pin(area, graph, i, p, scale);
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
pub fn bus_hit(area: Rect, graph: &GraphDraw, x: f64, y: f64, scale: f32) -> Option<usize> {
    (0..graph.buses.len()).find(|&b| bus_rect(area, graph, b, scale).contains(x, y))
}

/// The box under `(x, y)` — the grab point of a move drag and the click
/// target of the selection. Members win over buses (they are the larger
/// boxes); a port hit is the caller's business and wins over both.
pub fn box_hit(
    area: Rect,
    graph: &GraphDraw,
    x: f64,
    y: f64,
    scale: f32,
) -> Option<(BoxKind, usize)> {
    for i in 0..graph.members.len() {
        if member_rect(area, graph, i, scale).contains(x, y) {
            return Some((BoxKind::Member, i));
        }
    }
    (0..graph.buses.len())
        .find(|&b| bus_rect(area, graph, b, scale).contains(x, y))
        .map(|b| (BoxKind::Bus, b))
}

/// The current position of a box in canvas units (its explicit `x`/`y`, or
/// where the auto layout put it) — the value a starting move drag latches, so
/// the first drag of an auto-placed box turns its position explicit.
pub fn box_pos(area: Rect, graph: &GraphDraw, kind: BoxKind, b: usize, scale: f32) -> (f32, f32) {
    let r = match kind {
        BoxKind::Member => member_rect(area, graph, b, scale),
        BoxKind::Bus => bus_rect(area, graph, b, scale),
    };
    ((r.x - area.x) / scale, (r.y - area.y) / scale)
}

/// The transient canvas state the renderer passes per frame: a rewiring drag
/// in flight (the port being dragged and the cursor, drawn as a wire to the
/// pointer), the selected set, the marquee rectangle, and the workspace zoom
/// the patch is seen through.
pub struct CanvasState<'a> {
    pub live: Option<((usize, usize), (f32, f32))>,
    pub selected: &'a [(BoxKind, usize)],
    pub marquee: Option<Rect>,
    pub scale: f32,
}

/// Draws the patch: the member boxes with their ports, the bus nodes, and a
/// wire per connection, plus the transient canvas chrome in `state`.
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

    // The buses: a node per bus, `OUT` among them.
    for (b, bus) in graph.buses.iter().enumerate() {
        let r = bus_rect(area, graph, b, scale);
        mesh.rect(r, theme.bus_fill);
        let sel = selected.contains(&(BoxKind::Bus, b));
        let edge = if sel {
            theme.selected_edge
        } else {
            theme.hilite
        };
        mesh.border(r, if sel { 2.0 } else { 1.0 }, edge);
        font::text(
            mesh,
            &bus.name,
            r.x + PAD * 0.5 * scale,
            r.y + 4.0 * scale,
            ts,
            theme.text,
        );
        let (px, py) = bus_pin(area, graph, b, scale);
        mesh.rect(
            Rect::new(px - port_r, py - port_r, port_r * 2.0, port_r * 2.0),
            theme.port,
        );
    }

    // The members: the def name, then a row per wired control.
    for (i, m) in graph.members.iter().enumerate() {
        let r = member_rect(area, graph, i, scale);
        mesh.rect(r, theme.object_fill);
        let sel = selected.contains(&(BoxKind::Member, i));
        let edge = if sel {
            theme.selected_edge
        } else {
            theme.object_edge
        };
        mesh.border(r, if sel { 2.0 } else { 1.0 }, edge);
        font::text(
            mesh,
            &m.name,
            r.x + PAD * 0.5 * scale,
            r.y + 4.0 * scale,
            ts,
            theme.text,
        );
        for (p, port) in m.ports.iter().enumerate() {
            let (px, py) = port_pin(area, graph, i, p, scale);
            font::text(
                mesh,
                port,
                r.x + PAD * 0.5 * scale,
                py - font::height(ts) * 0.5,
                ts,
                theme.text,
            );
            mesh.rect(
                Rect::new(px - port_r, py - port_r, port_r * 2.0, port_r * 2.0),
                theme.port,
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
        let Some(b) = graph.buses.iter().position(|bus| bus.name == wire.bus) else {
            continue;
        };
        let (x0, y0) = port_pin(area, graph, wire.member, p, scale);
        let (x1, y1) = bus_pin(area, graph, b, scale);
        mesh.line(
            [x0, y0],
            [x1, y1],
            1.5 * scale,
            with_alpha(theme.selection, 0.9),
        );
    }

    // The wire being dragged, from its port to the cursor.
    if let Some(((i, p), (cx, cy))) = live {
        let (x0, y0) = port_pin(area, graph, i, p, scale);
        mesh.line([x0, y0], [cx, cy], 1.5 * scale, theme.live);
    }

    // The selection marquee in flight, over everything.
    if let Some(r) = marquee {
        mesh.rect(r, with_alpha(theme.selection, 0.15));
        mesh.border(r, 1.0, theme.selection);
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
                    x: None,
                    y: None,
                },
                Member {
                    name: "gsink".into(),
                    ports: vec!["in".into(), "out".into()],
                    x: None,
                    y: None,
                },
            ],
            buses: vec![Bus::named("mix"), Bus::named("OUT")],
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
        let m0 = member_rect(area(), &g, 0, 1.0);
        let m1 = member_rect(area(), &g, 1, 1.0);
        assert!(m1.y > m0.y + m0.h, "the second member sits below the first");
        assert!(m1.h > m0.h, "a member with two ports is taller");
        let b0 = bus_rect(area(), &g, 0, 1.0);
        assert!(b0.x > m0.x + m0.w, "the buses are the right column");
    }

    #[test]
    fn explicit_positions_win_and_scale_with_the_zoom() {
        let mut g = chain();
        g.members[1].x = Some(300.0);
        g.members[1].y = Some(40.0);
        g.buses[0].x = Some(120.0);
        g.buses[0].y = Some(250.0);
        let a = area();
        let m1 = member_rect(a, &g, 1, 1.0);
        assert_eq!((m1.x, m1.y), (a.x + 300.0, a.y + 40.0));
        let b0 = bus_rect(a, &g, 0, 1.0);
        assert_eq!((b0.x, b0.y), (a.x + 120.0, a.y + 250.0));
        // Under a 2x workspace zoom everything doubles from the area origin.
        let m1z = member_rect(a, &g, 1, 2.0);
        assert_eq!((m1z.x, m1z.y), (a.x + 600.0, a.y + 80.0));
        assert_eq!(m1z.w, m1.w * 2.0);
        // The auto-placed first member scales its stacked position too.
        let m0 = member_rect(a, &g, 0, 1.0);
        let m0z = member_rect(a, &g, 0, 2.0);
        assert_eq!(m0z.x - a.x, (m0.x - a.x) * 2.0);
    }

    #[test]
    fn boxes_hit_and_report_their_position() {
        let mut g = chain();
        g.members[0].x = Some(200.0);
        g.members[0].y = Some(100.0);
        let a = area();
        // Hit through a 2x transform: the box sits at 2x its canvas position.
        let hit = box_hit(a, &g, (a.x + 410.0) as f64, (a.y + 210.0) as f64, 2.0);
        assert_eq!(hit, Some((BoxKind::Member, 0)));
        assert_eq!(box_pos(a, &g, BoxKind::Member, 0, 2.0), (200.0, 100.0));
        // An auto-placed bus reports where the stack put it (canvas units),
        // so a starting drag can latch it as its explicit position.
        let b = bus_rect(a, &g, 1, 1.0);
        assert_eq!(box_pos(a, &g, BoxKind::Bus, 1, 1.0), (b.x - a.x, b.y - a.y));
        // Empty canvas hits nothing (the marquee's surface).
        assert_eq!(box_hit(a, &g, 500.0, 390.0, 1.0), None);
    }

    #[test]
    fn a_port_is_hit_where_its_pin_is_drawn() {
        let g = chain();
        let (px, py) = port_pin(area(), &g, 1, 1, 1.0); // gsink's `out`
        assert_eq!(
            port_hit(area(), &g, px as f64, py as f64, 1.0),
            Some((1, 1))
        );
        // Away from any pin: nothing (the drag starts only on a port).
        assert_eq!(port_hit(area(), &g, px as f64 + 30.0, py as f64, 1.0), None);
    }

    #[test]
    fn a_bus_is_the_drop_target_and_empty_space_is_not() {
        let g = chain();
        let r = bus_rect(area(), &g, 0, 1.0);
        let (cx, cy) = (r.x as f64 + 4.0, r.y as f64 + 4.0);
        assert_eq!(bus_hit(area(), &g, cx, cy, 1.0), Some(0));
        // Dropping on nothing unwires (the caller reads `None` that way).
        assert_eq!(bus_hit(area(), &g, 300.0, 380.0, 1.0), None);
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

        // A wire in flight adds the line to the cursor; a selection and a
        // marquee add their chrome.
        let mut with_live = Mesh::new();
        draw(
            &mut with_live,
            area(),
            &chain(),
            Some("chain"),
            &CanvasState {
                live: Some(((0, 0), (400.0, 200.0))),
                selected: &[(BoxKind::Member, 0)],
                marquee: Some(Rect::new(50.0, 50.0, 120.0, 90.0)),
                scale: 1.0,
            },
            &Theme::default(),
        );
        assert!(with_live.vertex_count() > m.vertex_count());
    }
}
