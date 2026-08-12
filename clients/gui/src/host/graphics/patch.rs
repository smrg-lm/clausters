//! The `patch` widget: a **directed, typed** patcher, drawing both levels.
//!
//! A box has **inlets on its top edge** and **outlets on its bottom edge**, each
//! typed (audio `ar`, control `kr`, or — the Def-view's third — init `ir`), and a
//! **cord** runs `outlet → inlet`. That is the whole surface: the picture reads
//! as signal flow, top to bottom. Direction is not a guess: it comes from the def
//! (a control feeding an `In` is an inlet, one feeding an `Out` an outlet), so
//! drawing it directed is honest. The same widget draws **level 1** — a GraphDef,
//! whole-node boxes wired by server buses (a cord *is* a bus the client's
//! cord→bus pass names, `clausters_core::patch`; audio/control only) — and
//! **level 2** — a SynthDef/FaustDef, UGen boxes wired by internal cords (never a
//! bus; `ir` joins the cord types). The rate is the only thing that differs; the
//! geometry, hit-testing and cords are one implementation.
//!
//! Pure over a [`Mesh`], like the rest of the flat views: layout, hit-test and
//! drawing are unit-testable without a window.

use clausters_core::patch::Rate;

use crate::host::font;
use crate::host::layout::Rect;
use crate::host::paint::{Draw, Mesh};
use crate::host::theme::{Theme, with_alpha};

const PAD: f32 = 8.0;
const OBJ_W: f32 = 96.0;
/// The middle band of a box, holding the def name (the widest band).
const HEAD_H: f32 = 20.0;
/// A port strip: the inlet cells sit in the top strip, the outlet cells in the
/// bottom strip (so a box reads inlets / def / outlets, top to bottom). The
/// strip is a distinct band color, empty when the edge has no ports.
const STRIP_H: f32 = 15.0;
/// The vertical gap the auto-stack leaves between boxes — room for the cord
/// between an outlet strip and the inlet strip of the box below it.
const ROW_GAP: f32 = 40.0;
/// Horizontal padding inside a port cell (the square a cord connects to, its
/// name written inside).
const PORT_PAD: f32 = 5.0;
/// The gap between adjacent port cells along a strip.
const PORT_GAP: f32 = 4.0;
const TEXT_SCALE: f32 = 1.5;
/// The port names are drawn a little smaller than the def name.
const LABEL_SCALE: f32 = 1.1;

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
    /// An init-rate (`ir`) port named `name` — a level-2 (Def-view) cord type.
    pub fn init(name: impl Into<String>) -> Port {
        Port {
            name: name.into(),
            rate: Rate::Init,
        }
    }
}

/// A box's **kind**, tagged by the Def-view decode: a `Source` is a parameter
/// input (a control), a `Const` is a literal **value box**, everything else is an
/// `Object` (a UGen / member def). It classifies a box for *drawing* — a value
/// box takes the distinct `value_fill` — while the layout ranks every box purely
/// by its cords ([`solve`]). Absent on the wire, a box is an `Object`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BoxRole {
    #[default]
    Object,
    /// A parameter/input source (a control box).
    Source,
    /// A literal **value box** (a `const`), drawn with `value_fill`.
    Const,
}

/// One box: a member def with its typed inlets and outlets. `x`/`y` place it
/// freely on the canvas (canvas units, relative to the widget origin); absent,
/// the box takes its place in the solved layout (see [`solve`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Obj {
    pub def: String,
    pub inlets: Vec<Port>,
    pub outlets: Vec<Port>,
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub role: BoxRole,
}

impl Obj {
    /// A box with the given def name and typed ports, auto-placed as an `Object`.
    pub fn new(def: impl Into<String>, inlets: Vec<Port>, outlets: Vec<Port>) -> Obj {
        Obj {
            def: def.into(),
            inlets,
            outlets,
            x: None,
            y: None,
            role: BoxRole::Object,
        }
    }

    /// This box with its layout role set (chaining after [`Obj::new`]).
    pub fn with_role(mut self, role: BoxRole) -> Obj {
        self.role = role;
        self
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
pub struct PatchDraw {
    pub boxes: Vec<Obj>,
    pub cords: Vec<Cord>,
}

/// One port cell's width: its name plus padding, floored so an unnamed port is
/// still a legible square. The cell is the square a cord connects to.
fn cell_w(port: &Port) -> f32 {
    (font::width(&port.name, LABEL_SCALE) + 2.0 * PORT_PAD).max(STRIP_H)
}

/// The width one edge needs: both margins, every cell, and the gaps between.
fn edge_w(ports: &[Port]) -> f32 {
    if ports.is_empty() {
        return 0.0;
    }
    2.0 * PAD + ports.iter().map(cell_w).sum::<f32>() + PORT_GAP * (ports.len() as f32 - 1.0)
}

/// The x of port `p`'s cell **left**, canvas units from the box's left edge:
/// cells flow left to right from the left margin.
fn cell_left(o: &Obj, side: Side, p: usize) -> f32 {
    let ports = o.ports(side);
    let before: f32 = ports[..p.min(ports.len())]
        .iter()
        .map(|q| cell_w(q) + PORT_GAP)
        .sum();
    PAD + before
}

/// The x of port `p`'s pin **centre** (a cord attaches at the cell's outer-edge
/// midpoint), canvas units from the box's left edge.
fn port_offset(o: &Obj, side: Side, p: usize) -> f32 {
    let w = o.ports(side).get(p).map_or(0.0, cell_w);
    cell_left(o, side, p) + w * 0.5
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

/// The height of one layout row: a box plus the gap that leaves room for a cord.
const ROW_H: f32 = HEAD_H + 2.0 * STRIP_H + ROW_GAP;
/// The horizontal gap between adjacent boxes in a layout row.
const COL_GAP: f32 = 26.0;
/// Barycenter sweeps for the **ordering** phase (crossing reduction).
const ORDER_PASSES: usize = 8;
/// Barycenter relaxation passes for the x-coordinate assignment.
const LAYOUT_PASSES: usize = 16;
/// The width a **dummy node** reserves in its rank — a thin lane a long edge
/// bends through, so the wire clears the boxes without a wide gutter.
const DUMMY_W: f32 = 6.0;

/// The longest directed path from box `i` **down to a sink** (a box with no
/// outgoing cord), memoized in `depth`. Ranking by this distance is what
/// distributes the inputs: a box sits as far above the bottom as its deepest
/// downstream use, so a control feeding a late stage lands just above it rather
/// than piling into one top row. `active` guards the (nominally impossible)
/// feedback cycle so the layout cannot loop.
fn depth_to_sink(
    i: usize,
    consumers: &[Vec<usize>],
    depth: &mut [Option<usize>],
    active: &mut [bool],
) -> usize {
    if let Some(d) = depth[i] {
        return d;
    }
    if active[i] {
        return 0;
    }
    active[i] = true;
    let mut m = 0;
    for c in 0..consumers[i].len() {
        let w = consumers[i][c];
        m = m.max(depth_to_sink(w, consumers, depth, active) + 1);
    }
    active[i] = false;
    depth[i] = Some(m);
    m
}

/// Records each box's index **within its layer** into `pos`.
fn record_positions(layers: &[Vec<usize>], pos: &mut [usize]) {
    for layer in layers {
        for (k, &i) in layer.iter().enumerate() {
            pos[i] = k;
        }
    }
}

/// **Order** the boxes within each layer to reduce cord crossings, by the
/// classic Sugiyama **barycenter sweep**: alternately sweep down (order a layer
/// by the mean position of its *producers*, the layers above) and up (by its
/// *consumers*, below), re-recording positions after each layer so the next one
/// sees the fresh order. Alternating the direction is what propagates a sensible
/// order through *every* level — a single downward pass only settles the rows
/// near the sinks. A box with no neighbour on the swept side keeps its slot.
///
/// The barycenter is **port-aware**: a neighbour contributes its order position
/// plus the *fraction* of its width where the connecting pin sits (`up`/`down`
/// carry that fraction). So two boxes feeding the two inlets of one box below are
/// ordered left-to-right the way those inlets are — the placement then only has
/// to align pins that are already on the correct sides.
fn order_layers(layers: &mut [Vec<usize>], up: &[Vec<(usize, f32)>], down: &[Vec<(usize, f32)>]) {
    let n = up.len();
    let mut pos = vec![0usize; n];
    record_positions(layers, &mut pos);
    for pass in 0..ORDER_PASSES {
        let downward = pass % 2 == 0;
        let order: Vec<usize> = if downward {
            (0..layers.len()).collect()
        } else {
            (0..layers.len()).rev().collect()
        };
        for l in order {
            let keys: Vec<f32> = layers[l]
                .iter()
                .enumerate()
                .map(|(k, &i)| {
                    let nbrs = if downward { &up[i] } else { &down[i] };
                    if nbrs.is_empty() {
                        k as f32
                    } else {
                        nbrs.iter().map(|&(u, f)| pos[u] as f32 + f).sum::<f32>()
                            / nbrs.len() as f32
                    }
                })
                .collect();
            let mut idx: Vec<usize> = (0..layers[l].len()).collect();
            idx.sort_by(|&a, &b| {
                keys[a]
                    .partial_cmp(&keys[b])
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.cmp(&b))
            });
            layers[l] = idx.into_iter().map(|k| layers[l][k]).collect();
            record_positions(layers, &mut pos);
        }
    }
}

/// The least-squares **non-decreasing** fit of `v` (Pool Adjacent Violators):
/// the closest sequence to `v` that never decreases. It is the exact,
/// bias-free way to separate a layer — each box is pulled to its target and
/// only the boxes that would overlap are pooled to their shared mean, so a
/// group centres on its neighbours instead of piling against one margin.
fn isotonic(v: &[f32]) -> Vec<f32> {
    // Each block is a pooled run: (mean, length). A new value that would break
    // monotonicity is merged left until the means are non-decreasing again.
    let mut blocks: Vec<(f32, usize)> = Vec::with_capacity(v.len());
    for &x in v {
        let (mut mean, mut len) = (x, 1usize);
        while let Some(&(m, c)) = blocks.last() {
            if m <= mean {
                break;
            }
            blocks.pop();
            mean = (m * c as f32 + mean * len as f32) / (c + len) as f32;
            len += c;
        }
        blocks.push((mean, len));
    }
    let mut out = Vec::with_capacity(v.len());
    for (mean, len) in blocks {
        out.extend(std::iter::repeat_n(mean, len));
    }
    out
}

/// **Place** the boxes on the x axis with the layer order now fixed: iterated
/// **barycenter** relaxation, then separating each layer with an [`isotonic`] fit
/// so overlapping boxes spread around their shared centre — keeping the layout
/// centred (no left/right pile-up) and, unlike a re-sorting pack, never changing
/// the order [`order_layers`] fixed.
///
/// The relaxation is **port-aware**: `pnbr[i]` holds, per incident cord, the
/// neighbour node and the pixel `delta` that would land box `i`'s *pin* exactly
/// under the neighbour's *pin* (`x[neighbour] + delta`). Aligning pin-to-pin
/// rather than centre-to-centre is what pulls a source straight down onto the
/// specific inlet it feeds, so cords into a multi-inlet box stop crossing.
fn assign_x(
    layers: &[Vec<usize>],
    pnbr: &[Vec<(usize, f32)>],
    width: &impl Fn(usize) -> f32,
    n: usize,
) -> Vec<f32> {
    let mut x = vec![0.0f32; n];
    for layer in layers {
        let mut cursor = 0.0;
        for &i in layer {
            x[i] = cursor;
            cursor += width(i) + COL_GAP;
        }
    }
    for _ in 0..LAYOUT_PASSES {
        let mut target = x.clone();
        for i in 0..n {
            if !pnbr[i].is_empty() {
                // Aim box i's left edge so its pin lands under each neighbour's
                // pin (the delta already folds in both port offsets); average the
                // targets of all its cords.
                let sum: f32 = pnbr[i].iter().map(|&(u, d)| x[u] + d).sum();
                target[i] = sum / pnbr[i].len() as f32;
            }
        }
        // Separate each layer: subtract the cumulative left offset so the minimum
        // spacing becomes plain monotonicity, isotonic-fit the residual, then add
        // the offset back. The result is the closest non-overlapping placement to
        // the targets, centred by construction.
        for layer in layers {
            let mut offset = 0.0;
            let residual: Vec<f32> = layer
                .iter()
                .map(|&i| {
                    let v = target[i] - offset;
                    offset += width(i) + COL_GAP;
                    v
                })
                .collect();
            let fit = isotonic(&residual);
            let mut offset = 0.0;
            for (&i, &y) in layer.iter().zip(&fit) {
                x[i] = y + offset;
                offset += width(i) + COL_GAP;
            }
        }
    }
    x
}

/// A solved layout in canvas units (before any centring offset): each real
/// box's top-left, the per-cord routing waypoints, and the boxes' bounding box.
struct Solved {
    /// Real box top-left positions, indexed by box.
    boxes: Vec<(f32, f32)>,
    /// The intermediate routing points of each cord (indexed by cord), one per
    /// **dummy node** the cord threads through — empty for a cord between
    /// adjacent rows.
    waypoints: Vec<Vec<(f32, f32)>>,
    /// The boxes' bounding box `(x0, y0, x1, y1)`, before centring.
    bounds: (f32, f32, f32, f32),
}

/// Solve the **layered (Sugiyama-style)** layout of the graph in canvas units —
/// a def is a DAG (fan-in, fan-out, shared sub-graphs, several `Out` sinks), not
/// a single-root tree. Phases: **(1) layer** each box by its longest path down to
/// a sink (signal flows top to bottom, inputs just above their use); **(1.5)** add
/// a **dummy node** on every rank a long edge skips, so each edge spans exactly
/// one rank and the wire can bend through the gap instead of cutting across boxes;
/// **(2) order** every node (real and dummy) within its rank to cut crossings
/// ([`order_layers`]); **(3) place** them on the x axis with that order fixed
/// ([`assign_x`]). The real boxes give [`Solved::boxes`]; the dummies give the
/// cords' [`Solved::waypoints`]. Centring against the view happens later
/// ([`center_offset`]); boxes with an explicit `x`/`y` bypass all of this (in
/// [`obj_rect`]).
fn solve(patch: &PatchDraw) -> Solved {
    let n = patch.boxes.len();
    if n == 0 {
        return Solved {
            boxes: Vec::new(),
            waypoints: Vec::new(),
            bounds: (0.0, 0.0, 0.0, 0.0),
        };
    }
    // (1) Layer: rank = maxDepth - (longest path to a sink), so sinks sit at the
    // bottom row and every box is strictly below the boxes that feed it.
    let mut consumers = vec![Vec::new(); n];
    for c in &patch.cords {
        if c.from < n && c.to < n && c.from != c.to {
            consumers[c.from].push(c.to);
        }
    }
    let mut depth = vec![None; n];
    let mut active = vec![false; n];
    for i in 0..n {
        depth_to_sink(i, &consumers, &mut depth, &mut active);
    }
    let depth: Vec<usize> = depth.into_iter().map(|d| d.unwrap_or(0)).collect();
    let max_depth = depth.iter().copied().max().unwrap_or(0);
    let mut rank: Vec<usize> = (0..n).map(|i| max_depth - depth[i]).collect();
    // (1.5) Build the extended node set: the `n` real boxes, then a dummy node per
    // rank each long edge skips, so every segment spans exactly one rank. Each
    // segment records, for the ordering (`up`/`down`), the neighbour's order slot
    // plus the *fraction* of its width where the pin sits, and, for the placement
    // (`pnbr`), the pixel `delta` that lands this node's pin under the neighbour's.
    let mut width: Vec<f32> = (0..n).map(|i| obj_w(&patch.boxes[i])).collect();
    let mut up: Vec<Vec<(usize, f32)>> = vec![Vec::new(); n];
    let mut down: Vec<Vec<(usize, f32)>> = vec![Vec::new(); n];
    let mut pnbr: Vec<Vec<(usize, f32)>> = vec![Vec::new(); n];
    let mut chains: Vec<Vec<usize>> = vec![Vec::new(); patch.cords.len()];
    for (ci, c) in patch.cords.iter().enumerate() {
        if !(c.from < n && c.to < n && c.from != c.to) {
            continue;
        }
        let (a, b) = (c.from, c.to);
        // The chain of nodes this cord runs through: the two real boxes, with a
        // dummy inserted on every rank strictly between them (none for an adjacent
        // pair, or when b is not strictly below a — the cycle guard).
        let mut nodes = vec![a];
        if rank[b] > rank[a] {
            for r in (rank[a] + 1)..rank[b] {
                let d = rank.len();
                rank.push(r);
                width.push(DUMMY_W);
                up.push(Vec::new());
                down.push(Vec::new());
                pnbr.push(Vec::new());
                chains[ci].push(d);
                nodes.push(d);
            }
        }
        nodes.push(b);
        // The pin offset (canvas units from the node's left edge) at each end of a
        // segment: a real box's port pin at its terminal end, else a dummy centre.
        let last = nodes.len() - 1;
        for j in 0..last {
            let (p, cc) = (nodes[j], nodes[j + 1]);
            let off_p = if j == 0 {
                port_offset(&patch.boxes[a], Side::Out, c.from_out)
            } else {
                width[p] * 0.5
            };
            let off_c = if j + 1 == last {
                port_offset(&patch.boxes[b], Side::In, c.to_in)
            } else {
                width[cc] * 0.5
            };
            down[p].push((cc, off_c / width[cc]));
            up[cc].push((p, off_p / width[p]));
            pnbr[p].push((cc, off_c - off_p));
            pnbr[cc].push((p, off_p - off_c));
        }
    }
    let total = rank.len();
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); max_depth + 1];
    for v in 0..total {
        layers[rank[v]].push(v);
    }
    // (2) order to reduce crossings, then (3) assign the x coordinate.
    order_layers(&mut layers, &up, &down);
    let w = |v: usize| width[v];
    let x = assign_x(&layers, &pnbr, &w, total);
    let min_x = x.iter().copied().fold(f32::MAX, f32::min);
    let node_x: Vec<f32> = (0..total).map(|v| PAD + x[v] - min_x).collect();
    // Rank 0 sits a label-height below the top so the frame (which reserves that
    // room above the first row) still anchors at the content origin, not above it.
    let row_top = |r: usize| (HEAD_H + PAD) + r as f32 * ROW_H;
    let band = HEAD_H + 2.0 * STRIP_H;
    let boxes: Vec<(f32, f32)> = (0..n).map(|i| (node_x[i], row_top(rank[i]))).collect();
    // A dummy's waypoint is its cell centre — the wire bends through the lane the
    // dummy reserved in its rank, at the row's vertical middle.
    let waypoints: Vec<Vec<(f32, f32)>> = chains
        .iter()
        .map(|chain| {
            chain
                .iter()
                .map(|&d| (node_x[d] + width[d] * 0.5, row_top(rank[d]) + band * 0.5))
                .collect()
        })
        .collect();
    // The bounding box hugs the real boxes and the routing waypoints, so the frame
    // contains the bent wires too.
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (i, &(px, py)) in boxes.iter().enumerate() {
        x0 = x0.min(px);
        y0 = y0.min(py);
        x1 = x1.max(px + width[i]);
        y1 = y1.max(py + band);
    }
    for pts in &waypoints {
        for &(wx, wy) in pts {
            x0 = x0.min(wx);
            y0 = y0.min(wy);
            x1 = x1.max(wx);
            y1 = y1.max(wy);
        }
    }
    Solved {
        boxes,
        waypoints,
        // Grow by a pad all round, plus label room above the top row.
        bounds: (x0 - PAD, y0 - (HEAD_H + PAD), x1 + PAD, y1 + PAD),
    }
}

/// The graph's **intrinsic size** in canvas units — the panel frame that hugs its
/// boxes and wires. The scroll workspace sizes its content from this (see
/// `host::layout::scroll_content`), so a small graph centres in the window and a
/// large one fills the content and pans.
pub fn natural_size(patch: &PatchDraw) -> (f32, f32) {
    if patch.boxes.is_empty() {
        return (OBJ_W, HEAD_H + 2.0 * STRIP_H);
    }
    let (x0, y0, x1, y1) = solve(patch).bounds;
    (x1 - x0, y1 - y0)
}

/// The offset (canvas units) that centres the frame `bounds` inside `area`:
/// the frame's centre lands on the area's centre. Never negative, so a graph as
/// large as the area (its content, sized to `max(view, natural)`) sits flush at
/// the origin and pans rather than drifting off-screen.
fn center_offset(area: Rect, bounds: (f32, f32, f32, f32), scale: f32) -> (f32, f32) {
    let (x0, y0, x1, y1) = bounds;
    let dx = (area.w / scale * 0.5 - (x0 + x1) * 0.5).max(0.0);
    let dy = (area.h / scale * 0.5 - (y0 + y1) * 0.5).max(0.0);
    (dx, dy)
}

/// Whether **every** box takes the auto layout (none carries an explicit
/// `x`/`y`). Both Def-views and a freshly opened patcher are fully auto; a patch
/// becomes mixed only once a box is dragged to a persisted position. The
/// distinction gates only the **frame** and the cord routing (a mixed patch's
/// frame hugs the real boxes and its cords route straight) — **not** the
/// centring [`center_offset`], which is a view transform over the [`solve`]d
/// bounds. Those bounds ignore any explicit `x`/`y`, so the offset is steady
/// across a drag and applies to the auto boxes in either mode: zeroing it the
/// instant a box turned explicit is what made the rest jump; keeping it holds
/// them still while the dragged box (whose stored coordinate already carries the
/// offset) follows the cursor.
fn fully_auto(patch: &PatchDraw) -> bool {
    patch.boxes.iter().all(|o| o.x.is_none() || o.y.is_none())
}

/// The box of `i`, at `scale` (the enclosing workspace's zoom, `1.0` bare):
/// placed at its explicit `x`/`y` when it has them, else at its solved,
/// view-centred slot.
pub fn obj_rect(area: Rect, patch: &PatchDraw, i: usize, scale: f32) -> Rect {
    let h = (HEAD_H + 2.0 * STRIP_H) * scale;
    let w = patch.boxes.get(i).map_or(OBJ_W, obj_w) * scale;
    if let Some(o) = patch.boxes.get(i)
        && let (Some(x), Some(y)) = (o.x, o.y)
    {
        return Rect::new(area.x + x * scale, area.y + y * scale, w, h);
    }
    let solved = solve(patch);
    let (dx, dy) = center_offset(area, solved.bounds, scale);
    let (ax, ay) = solved.boxes.get(i).copied().unwrap_or((PAD, PAD));
    Rect::new(area.x + (ax + dx) * scale, area.y + (ay + dy) * scale, w, h)
}

/// The pin of box `i`'s port `p` on `side`: ports are laid out left-justified in
/// flow order along the top edge (inlets) / bottom edge (outlets). `(x, y)` is
/// the pin's centre.
pub fn port_pin(
    area: Rect,
    patch: &PatchDraw,
    i: usize,
    side: Side,
    p: usize,
    scale: f32,
) -> (f32, f32) {
    let r = obj_rect(area, patch, i, scale);
    let off = patch.boxes.get(i).map_or(PAD, |o| port_offset(o, side, p));
    let x = r.x + off * scale;
    let y = match side {
        Side::In => r.y,
        Side::Out => r.y + r.h,
    };
    (x, y)
}

/// The cell of box `i`'s port `p` on `side`: the square (in the top strip for an
/// inlet, the bottom strip for an outlet) that holds the port name and is the
/// target a cord connects to.
pub fn port_cell(
    area: Rect,
    patch: &PatchDraw,
    i: usize,
    side: Side,
    p: usize,
    scale: f32,
) -> Rect {
    let r = obj_rect(area, patch, i, scale);
    let (left, w) = patch.boxes.get(i).map_or((PAD, STRIP_H), |o| {
        (
            cell_left(o, side, p),
            o.ports(side).get(p).map_or(STRIP_H, cell_w),
        )
    });
    let y = match side {
        Side::In => r.y,
        Side::Out => r.y + r.h - STRIP_H * scale,
    };
    Rect::new(r.x + left * scale, y, w * scale, STRIP_H * scale)
}

/// The port under `(x, y)`, as `(box, side, port)` — the grab point of a cord
/// drag. Inlets and outlets both hit; the caller pairs an outlet with an inlet.
pub fn port_hit(
    area: Rect,
    patch: &PatchDraw,
    x: f64,
    y: f64,
    scale: f32,
) -> Option<(usize, Side, usize)> {
    // The pin sits on the cell's outer edge (the box's top/bottom border), so the
    // hit region is the cell grown by a small margin: it covers the edge itself
    // (half-open `contains` would drop the exclusive bottom) and gives a click a
    // little grab tolerance around the square.
    let m = (3.0 * scale) as f64;
    let over = |i: usize, side: Side, p: usize| {
        let c = port_cell(area, patch, i, side, p, scale);
        Rect::new(
            c.x - m as f32,
            c.y - m as f32,
            c.w + 2.0 * m as f32,
            c.h + 2.0 * m as f32,
        )
        .contains(x, y)
    };
    for (i, o) in patch.boxes.iter().enumerate() {
        for p in 0..o.inlets.len() {
            if over(i, Side::In, p) {
                return Some((i, Side::In, p));
            }
        }
        for p in 0..o.outlets.len() {
            if over(i, Side::Out, p) {
                return Some((i, Side::Out, p));
            }
        }
    }
    None
}

/// The box under `(x, y)` — the grab point of a move drag and the click target
/// of the selection. A port hit is the caller's business and wins over this.
pub fn box_hit(area: Rect, patch: &PatchDraw, x: f64, y: f64, scale: f32) -> Option<usize> {
    (0..patch.boxes.len()).find(|&i| obj_rect(area, patch, i, scale).contains(x, y))
}

/// The current position of box `i` in canvas units (its explicit `x`/`y`, or
/// where the auto layout put it) — the value a starting move drag latches.
pub fn box_pos(area: Rect, patch: &PatchDraw, i: usize, scale: f32) -> (f32, f32) {
    let r = obj_rect(area, patch, i, scale);
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

/// The stroke width every cord is drawn with — **one** weight for all rates (the
/// average of the old audio/control weights): a fat-vs-thin pair read badly, so
/// the rate is carried by **colour** alone (and, for init, a dash).
fn cord_weight(scale: f32) -> f32 {
    2.25 * scale
}

/// The colour a cord of `rate` is drawn in — the rate reads by **colour** first
/// (weight alone is hard to tell apart): audio red, control blue, init yellow
/// (pastel primaries, for good mutual contrast on the dark field).
fn cord_color(rate: Rate, theme: &Theme) -> crate::host::paint::Color {
    match rate {
        Rate::Audio => theme.cord,
        Rate::Control => theme.cord_control,
        Rate::Init => theme.cord_init,
    }
}

/// Draws a cord of `rate` along the polyline `pts` (outlet pin, any routing
/// waypoints, inlet pin): coloured by rate, and **dashed** for init (`ir`) so a
/// scalar wire also reads apart by its line style.
fn draw_cord(mesh: &mut Mesh, pts: &[[f32; 2]], rate: Rate, theme: &Theme, scale: f32) {
    let w = cord_weight(scale);
    let color = cord_color(rate, theme);
    for seg in pts.windows(2) {
        let (a, b) = (seg[0], seg[1]);
        if rate != Rate::Init {
            mesh.line(a, b, w, color);
            continue;
        }
        // A dash pattern in device pixels, scaled with the zoom: on/off segments
        // stepped along each leg so the count follows its length.
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = (dx * dx + dy * dy).sqrt().max(f32::MIN_POSITIVE);
        let (ux, uy) = (dx / len, dy / len);
        let dash = 6.0 * scale;
        let gap = 4.0 * scale;
        let mut t = 0.0;
        while t < len {
            let e = (t + dash).min(len);
            mesh.line(
                [a[0] + ux * t, a[1] + uy * t],
                [a[0] + ux * e, a[1] + uy * e],
                w,
                color,
            );
            t += dash + gap;
        }
    }
}

/// The panel rectangle that **contains** every box and wire. Fully auto: the
/// solved bounding box (already grown for the margin and the label room), placed
/// at its centred, scaled position. Mixed (some box placed by hand): the union of
/// the real box rects, grown by the margin and the label room — so the frame hugs
/// where the boxes actually are, not a phantom auto layout. Falls back to `area`
/// for an empty patch.
fn content_rect(area: Rect, patch: &PatchDraw, scale: f32) -> Rect {
    if patch.boxes.is_empty() {
        return area;
    }
    if fully_auto(patch) {
        let solved = solve(patch);
        let (dx, dy) = center_offset(area, solved.bounds, scale);
        let (x0, y0, x1, y1) = solved.bounds;
        return Rect::new(
            area.x + (x0 + dx) * scale,
            area.y + (y0 + dy) * scale,
            (x1 - x0) * scale,
            (y1 - y0) * scale,
        );
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for i in 0..patch.boxes.len() {
        let r = obj_rect(area, patch, i, scale);
        x0 = x0.min(r.x);
        y0 = y0.min(r.y);
        x1 = x1.max(r.x + r.w);
        y1 = y1.max(r.y + r.h);
    }
    let pad = PAD * scale;
    let label = (HEAD_H + PAD) * scale;
    Rect::new(
        x0 - pad,
        y0 - label,
        (x1 - x0) + 2.0 * pad,
        (y1 - y0) + label + pad,
    )
}

/// Draws the patch: the boxes with their inlet/outlet pins, the directed cords
/// (typed by weight), and the transient canvas chrome in `state`.
pub fn draw(
    d: &mut Draw,
    area: Rect,
    patch: &PatchDraw,
    label: Option<&str>,
    state: &CanvasState<'_>,
) {
    let (mesh, _m, theme) = d.parts();
    let CanvasState {
        live,
        selected,
        marquee,
        scale,
    } = *state;
    let ts = TEXT_SCALE * scale;
    // The frame hugs the laid-out boxes (plus room for the label above them), so
    // the labelled panel always *contains* every box rather than clipping the
    // ones the fixed widget rect could not hold.
    let frame = content_rect(area, patch, scale);
    mesh.rect(frame, theme.view_field);
    mesh.border(frame, 1.0, theme.frame);
    if let Some(text) = label {
        font::text(
            mesh,
            text,
            frame.x + PAD * scale,
            frame.y + 2.0,
            ts,
            theme.text,
        );
    }

    // The cords, first, so the boxes and pins sit over them. Each runs from its
    // outlet pin through its routing waypoints (the dummy nodes the layout threaded
    // it past, offset into place) to its inlet pin, so a long edge bends around the
    // boxes between its rows instead of cutting across them.
    let solved = solve(patch);
    let (dx, dy) = center_offset(area, solved.bounds, scale);
    for (ci, cord) in patch.cords.iter().enumerate() {
        let (Some(src), Some(dst)) = (patch.boxes.get(cord.from), patch.boxes.get(cord.to)) else {
            continue;
        };
        let Some(port) = src.outlets.get(cord.from_out) else {
            continue;
        };
        if dst.inlets.get(cord.to_in).is_none() {
            continue;
        }
        let (x0, y0) = port_pin(area, patch, cord.from, Side::Out, cord.from_out, scale);
        let (x1, y1) = port_pin(area, patch, cord.to, Side::In, cord.to_in, scale);
        let mut pts = vec![[x0, y0]];
        // The dummy waypoints belong to the auto layout; a mixed patch anchors on
        // the hand-placed boxes, so its cords route straight instead.
        if fully_auto(patch)
            && let Some(waypoints) = solved.waypoints.get(ci)
        {
            pts.extend(
                waypoints
                    .iter()
                    .map(|&(wx, wy)| [area.x + (wx + dx) * scale, area.y + (wy + dy) * scale]),
            );
        }
        pts.push([x1, y1]);
        draw_cord(mesh, &pts, port.rate, theme, scale);
    }

    // The boxes: three bands top to bottom — the inlet strip on top and the
    // outlet strip on the bottom (one color, `port_strip`), the def name in the
    // (widest) middle band (white `box_fill`, black `box_text`), the dark strips
    // framing it — so a box reads like its signal flow (in on top, out on the
    // bottom). An edge with no ports keeps its strip, empty. Everything is sized
    // from the box rect times `scale`, so it stays anchored under zoom.
    let lts = LABEL_SCALE * scale;
    let fh = font::height(ts); // device heights, to centre text in a band
    let lh = font::height(lts);
    let strip_h = STRIP_H * scale;
    let head_h = HEAD_H * scale;
    for (i, o) in patch.boxes.iter().enumerate() {
        let r = obj_rect(area, patch, i, scale);
        let top = Rect::new(r.x, r.y, r.w, strip_h);
        let mid = Rect::new(r.x, r.y + strip_h, r.w, head_h);
        let bot = Rect::new(r.x, r.y + strip_h + head_h, r.w, strip_h);
        // A value box (a `const` literal) reads as data, not a UGen: it takes the
        // distinct cream `value_fill` instead of the white `box_fill`.
        let mid_fill = if o.role == BoxRole::Const {
            theme.value_fill
        } else {
            theme.box_fill
        };
        mesh.rect(top, theme.port_strip);
        mesh.rect(mid, mid_fill);
        mesh.rect(bot, theme.port_strip);
        let sel = selected.contains(&i);
        let edge = if sel {
            theme.selected_edge
        } else {
            theme.object_edge
        };
        mesh.border(r, if sel { 2.0 } else { 1.0 }, edge);
        // The def name, left-justified and centred in the white middle band,
        // drawn in black (`box_text`).
        font::text(
            mesh,
            &o.def,
            mid.x + PAD * scale,
            mid.y + (head_h - fh) * 0.5,
            ts,
            theme.box_text,
        );
        // Each port a labelled cell in its strip: the square a cord connects to,
        // its name written inside.
        for side in [Side::In, Side::Out] {
            for (p, port) in o.ports(side).iter().enumerate() {
                let cell = port_cell(area, patch, i, side, p, scale);
                mesh.rect(cell, with_alpha(theme.port, 0.16));
                mesh.border(cell, 1.0, theme.port);
                font::text(
                    mesh,
                    &port.name,
                    cell.x + PORT_PAD * scale,
                    cell.y + (strip_h - lh) * 0.5,
                    lts,
                    theme.text,
                );
            }
        }
    }

    // The cord being dragged, from its grabbed port to the cursor.
    if let Some(((i, side, p), (cx, cy))) = live {
        let (x0, y0) = port_pin(area, patch, i, side, p, scale);
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
    patch: &PatchDraw,
    a: (usize, Side, usize),
    b: (usize, Side, usize),
) -> Option<Cord> {
    let (out, inl) = match (a.1, b.1) {
        (Side::Out, Side::In) => (a, b),
        (Side::In, Side::Out) => (b, a),
        _ => return None, // both inlets or both outlets
    };
    let out_rate = patch.boxes.get(out.0)?.outlets.get(out.2)?.rate;
    let in_rate = patch.boxes.get(inl.0)?.inlets.get(inl.2)?.rate;
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
    use crate::host::metrics::Metrics;

    /// tone (out) → trem (in, out) → dac (in, out), no cords yet placed.
    fn boxes() -> Vec<Obj> {
        vec![
            Obj::new("tone", vec![], vec![Port::audio("out")]),
            Obj::new("trem", vec![Port::audio("in")], vec![Port::audio("out")]),
            Obj::new("dac", vec![Port::audio("in")], vec![Port::audio("out")]),
        ]
    }

    fn chain() -> PatchDraw {
        PatchDraw {
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
        // Under a 2x workspace zoom a box doubles in size (its position is
        // re-centred against the same area, so it is not a fixed 2x of the
        // origin — the box *size* is what scales rigidly).
        let b0z = obj_rect(a, &g, 0, 2.0);
        assert_eq!(b0z.w, b0.w * 2.0);
        assert_eq!(b0z.h, b0.h * 2.0);
        let b1z = obj_rect(a, &g, 1, 2.0);
        assert!(b1z.y > b0z.y + b0z.h, "the stack holds under zoom");
    }

    #[test]
    fn the_ordering_uncrosses_edges_at_every_level() {
        // Two sources feeding two sinks with swapped indices (s0 -> k1, s1 -> k0):
        // laid out by box index alone the two cords cross. The barycenter ordering
        // must reorder a layer so they do not — the left source's target ends up on
        // the same side as the source.
        let patch = PatchDraw {
            boxes: vec![
                Obj::new("s0", vec![], vec![Port::audio("")]),
                Obj::new("s1", vec![], vec![Port::audio("")]),
                Obj::new("k0", vec![Port::audio("in")], vec![]),
                Obj::new("k1", vec![Port::audio("in")], vec![]),
            ],
            cords: vec![
                Cord {
                    from: 0,
                    from_out: 0,
                    to: 3,
                    to_in: 0,
                }, // s0 -> k1
                Cord {
                    from: 1,
                    from_out: 0,
                    to: 2,
                    to_in: 0,
                }, // s1 -> k0
            ],
        };
        let a = area();
        let x = |i| obj_rect(a, &patch, i, 1.0).x;
        // The two edges do not cross: the order of the sources agrees with the
        // order of the sinks they feed (s0->k1, s1->k0), so (x[s0]-x[s1]) and
        // (x[k1]-x[k0]) share their sign.
        assert!(
            (x(0) - x(1)) * (x(3) - x(2)) > 0.0,
            "s0/s1 and their sinks k1/k0 stay on the same side (no crossing)"
        );
    }

    #[test]
    fn the_placement_aligns_each_source_over_the_inlet_it_feeds() {
        // Two sources feed the two inlets of one sink in *swapped* order: s0 ->
        // sink.in1 (the right pin), s1 -> sink.in0 (the left pin). Centre-to-centre
        // placement would put s0 and s1 both over the sink's middle and leave them
        // ordered by index, so the cords cross. Port-aware ordering + placement must
        // instead put s1 (feeding the left inlet) left of s0 (the right inlet), each
        // source's outlet pin landing under the inlet pin it feeds.
        let patch = PatchDraw {
            boxes: vec![
                Obj::new("s0", vec![], vec![Port::audio("")]),
                Obj::new("s1", vec![], vec![Port::audio("")]),
                Obj::new("sink", vec![Port::audio("a"), Port::audio("b")], vec![]),
            ],
            cords: vec![
                Cord {
                    from: 0,
                    from_out: 0,
                    to: 2,
                    to_in: 1,
                }, // s0 -> sink.in1 (right)
                Cord {
                    from: 1,
                    from_out: 0,
                    to: 2,
                    to_in: 0,
                }, // s1 -> sink.in0 (left)
            ],
        };
        let a = area();
        let s0 = port_pin(a, &patch, 0, Side::Out, 0, 1.0).0; // feeds in1 (right)
        let s1 = port_pin(a, &patch, 1, Side::Out, 0, 1.0).0; // feeds in0 (left)
        let in0 = port_pin(a, &patch, 2, Side::In, 0, 1.0).0;
        let in1 = port_pin(a, &patch, 2, Side::In, 1, 1.0).0;
        // The two cords do not cross: s0->in1 and s1->in0 keep the same sign, i.e.
        // the source feeding the right inlet stays right of the one feeding the
        // left. Centre-to-centre placement would have left them ordered by index
        // and crossing; port-aware ordering swaps them.
        assert!(in1 > in0, "the sink's inlets read left to right");
        assert!(
            (s0 - s1) * (in1 - in0) > 0.0,
            "no crossing: each source sits on the side of the inlet it feeds ({s1} {s0})"
        );
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
    fn dragging_one_box_leaves_the_others_put() {
        // The move round trip: latch a box's on-screen position, add the drag
        // delta, and write it back as an explicit x/y (which is how the host emits
        // a "move"). The dragged box must land exactly there, and — the bug this
        // guards — the still-auto boxes must not shift: the centring offset is a
        // stable view transform over the solved bounds, so making one box explicit
        // does not move the rest.
        let a = area();
        let g = chain();
        let (before0, before2) = (obj_rect(a, &g, 0, 1.0), obj_rect(a, &g, 2, 1.0));
        // Grab box 1 at its current canvas position and drag it by (40, 30).
        let (x0, y0) = box_pos(a, &g, 1, 1.0);
        let (dx, dy) = (40.0, 30.0);
        let mut moved = g.clone();
        moved.boxes[1].x = Some(x0 + dx);
        moved.boxes[1].y = Some(y0 + dy);
        // The dragged box sits at its old spot shifted by the delta...
        let was1 = obj_rect(a, &g, 1, 1.0);
        let now1 = obj_rect(a, &moved, 1, 1.0);
        assert!(
            (now1.x - (was1.x + dx)).abs() < 0.01 && (now1.y - (was1.y + dy)).abs() < 0.01,
            "the dragged box follows the delta: {was1:?} -> {now1:?}"
        );
        // ...and the untouched boxes have not moved at all.
        let (after0, after2) = (obj_rect(a, &moved, 0, 1.0), obj_rect(a, &moved, 2, 1.0));
        assert!(
            (after0.x - before0.x).abs() < 0.01 && (after0.y - before0.y).abs() < 0.01,
            "box 0 stays put: {before0:?} -> {after0:?}"
        );
        assert!(
            (after2.x - before2.x).abs() < 0.01 && (after2.y - before2.y).abs() < 0.01,
            "box 2 stays put: {before2:?} -> {after2:?}"
        );
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
    fn a_port_is_hit_over_its_cell_with_its_side() {
        let g = chain();
        let a = area();
        // A click anywhere over the outlet's cell (the square a cord connects to)
        // hits it; likewise the inlet's cell on the box below.
        let oc = port_cell(a, &g, 0, Side::Out, 0, 1.0); // tone's outlet cell
        assert_eq!(
            port_hit(
                a,
                &g,
                (oc.x + oc.w * 0.5) as f64,
                (oc.y + oc.h * 0.5) as f64,
                1.0
            ),
            Some((0, Side::Out, 0))
        );
        let ic = port_cell(a, &g, 2, Side::In, 0, 1.0); // dac's inlet cell
        assert_eq!(
            port_hit(
                a,
                &g,
                (ic.x + ic.w * 0.5) as f64,
                (ic.y + ic.h * 0.5) as f64,
                1.0
            ),
            Some((2, Side::In, 0))
        );
        // The pin (the cord attach point on the outer edge) sits over its cell.
        let (ox, oy) = port_pin(a, &g, 0, Side::Out, 0, 1.0);
        assert!(oc.contains(ox as f64, (oy - 0.5) as f64));
        // Away from any cell: nothing (a cord drag starts only on a port).
        assert_eq!(
            port_hit(a, &g, (oc.x + 60.0) as f64, oc.y as f64, 1.0),
            None
        );
    }

    #[test]
    fn the_auto_layout_ranks_sources_top_and_sinks_bottom() {
        // freq (a source) -> Sine -> Out (a sink): ranked by longest path to the
        // sink, so rows increase downward and the picture reads top to bottom.
        let patch = PatchDraw {
            boxes: vec![
                Obj::new("freq", vec![], vec![Port::control("")]).with_role(BoxRole::Source),
                Obj::new("Sine", vec![Port::control("freq")], vec![Port::audio("")]),
                Obj::new("Out", vec![Port::audio("sig")], vec![]),
            ],
            cords: vec![
                Cord {
                    from: 0,
                    from_out: 0,
                    to: 1,
                    to_in: 0,
                },
                Cord {
                    from: 1,
                    from_out: 0,
                    to: 2,
                    to_in: 0,
                },
            ],
        };
        let a = area();
        let (y0, y1, y2) = (
            obj_rect(a, &patch, 0, 1.0).y,
            obj_rect(a, &patch, 1, 1.0).y,
            obj_rect(a, &patch, 2, 1.0).y,
        );
        assert!(
            y0 < y1 && y1 < y2,
            "source over ugen over sink: {y0} {y1} {y2}"
        );
    }

    #[test]
    fn a_value_box_ranks_one_row_above_the_box_it_feeds() {
        // A const value box feeding a sink ranks one row above it (its longest
        // path to the sink is 1), and the frame contains both.
        let patch = PatchDraw {
            boxes: vec![
                Obj::new("Out", vec![Port::audio("bus"), Port::audio("sig")], vec![]),
                Obj::new("0.0", vec![], vec![Port::init("")]).with_role(BoxRole::Const),
            ],
            cords: vec![Cord {
                from: 1,
                from_out: 0,
                to: 0,
                to_in: 0,
            }],
        };
        let a = area();
        let out = obj_rect(a, &patch, 0, 1.0);
        let konst = obj_rect(a, &patch, 1, 1.0);
        assert!(konst.y < out.y, "the value box sits above its consumer");
        let frame = content_rect(a, &patch, 1.0);
        assert!(frame.y <= konst.y && frame.x <= konst.x);
    }

    #[test]
    fn same_layer_boxes_are_separated_and_barycentered() {
        // A diamond: src fans out to a, b (same layer), which fan in to sink. The
        // two middle boxes share a row (equal y) without overlapping, and sink
        // centers under them.
        let patch = PatchDraw {
            boxes: vec![
                Obj::new("src", vec![], vec![Port::audio("")]),
                Obj::new("a", vec![Port::audio("in")], vec![Port::audio("")]),
                Obj::new("b", vec![Port::audio("in")], vec![Port::audio("")]),
                Obj::new("sink", vec![Port::audio("x"), Port::audio("y")], vec![]),
            ],
            cords: vec![
                Cord {
                    from: 0,
                    from_out: 0,
                    to: 1,
                    to_in: 0,
                },
                Cord {
                    from: 0,
                    from_out: 0,
                    to: 2,
                    to_in: 0,
                },
                Cord {
                    from: 1,
                    from_out: 0,
                    to: 3,
                    to_in: 0,
                },
                Cord {
                    from: 2,
                    from_out: 0,
                    to: 3,
                    to_in: 1,
                },
            ],
        };
        let a = area();
        let (ra, rb) = (obj_rect(a, &patch, 1, 1.0), obj_rect(a, &patch, 2, 1.0));
        assert!((ra.y - rb.y).abs() < 0.5, "a and b share a row");
        let (left, right) = if ra.x <= rb.x { (ra, rb) } else { (rb, ra) };
        assert!(
            right.x >= left.x + left.w,
            "the two middle boxes do not overlap"
        );
        // src (above) and sink (below) sit between a and b horizontally — the
        // barycenter pulls the shared endpoints to the middle of the pair.
        for shared in [obj_rect(a, &patch, 0, 1.0), obj_rect(a, &patch, 3, 1.0)] {
            let c = shared.x + shared.w * 0.5;
            assert!(
                left.x < c && c < right.x + right.w,
                "shared box centered: {c} vs [{}, {}]",
                left.x,
                right.x + right.w
            );
        }
    }

    #[test]
    fn a_long_edge_routes_through_a_dummy_waypoint() {
        // s -> a -> k and s -> k: the direct s->k edge skips the middle rank, so
        // the layout threads it through one dummy node (a routing waypoint), while
        // the two adjacent-rank edges route straight (no waypoint).
        let patch = PatchDraw {
            boxes: vec![
                Obj::new("s", vec![], vec![Port::audio("")]),
                Obj::new("a", vec![Port::audio("in")], vec![Port::audio("")]),
                Obj::new("k", vec![Port::audio("x"), Port::audio("y")], vec![]),
            ],
            cords: vec![
                Cord {
                    from: 0,
                    from_out: 0,
                    to: 1,
                    to_in: 0,
                }, // s -> a (adjacent)
                Cord {
                    from: 1,
                    from_out: 0,
                    to: 2,
                    to_in: 0,
                }, // a -> k (adjacent)
                Cord {
                    from: 0,
                    from_out: 0,
                    to: 2,
                    to_in: 1,
                }, // s -> k (skips a rank)
            ],
        };
        let solved = solve(&patch);
        assert!(
            solved.waypoints[0].is_empty(),
            "the short s->a edge has no waypoint"
        );
        assert!(
            solved.waypoints[1].is_empty(),
            "the short a->k edge has no waypoint"
        );
        assert_eq!(
            solved.waypoints[2].len(),
            1,
            "the long s->k edge threads one dummy"
        );
        // The waypoint sits on the skipped middle rank (between the two rows).
        let (_, wy) = solved.waypoints[2][0];
        assert!(
            wy > PAD && wy < PAD + 2.0 * ROW_H,
            "waypoint on the middle row: {wy}"
        );
    }

    #[test]
    fn a_small_graph_centres_in_the_view_but_a_large_one_anchors() {
        let g = chain();
        // In an area larger than the graph, the frame's centre sits at the area's.
        let big = Rect::new(0.0, 0.0, 1200.0, 900.0);
        let frame = content_rect(big, &g, 1.0);
        assert!(
            (frame.x + frame.w * 0.5 - 600.0).abs() < 1.0,
            "centred in x"
        );
        assert!(
            (frame.y + frame.h * 0.5 - 450.0).abs() < 1.0,
            "centred in y"
        );
        // In an area smaller than the graph, it anchors at the origin (top-left)
        // and pans — never pushed off-screen toward the centre.
        let small = Rect::new(0.0, 0.0, 40.0, 40.0);
        let frame = content_rect(small, &g, 1.0);
        assert!(
            frame.x >= small.x - 0.01 && frame.y >= small.y - 0.01,
            "anchored"
        );
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
        let g = PatchDraw {
            boxes: vec![
                Obj::new("lfo", vec![], vec![Port::control("out")]),
                Obj::new("dac", vec![Port::audio("in")], vec![]),
            ],
            cords: vec![],
        };
        assert_eq!(cord_between(&g, (0, Side::Out, 0), (1, Side::In, 0)), None);
    }

    #[test]
    fn an_init_cord_pairs_when_both_ends_are_init() {
        // The level-2 (Def-view) third rate: two `ir` ports connect, but an
        // `ir`-to-`ar` pair is a mismatch like any other.
        let g = PatchDraw {
            boxes: vec![
                Obj::new("BufFrames", vec![], vec![Port::init("out")]),
                Obj::new(
                    "PlayBuf",
                    vec![Port::init("frames")],
                    vec![Port::audio("out")],
                ),
            ],
            cords: vec![],
        };
        assert_eq!(
            cord_between(&g, (0, Side::Out, 0), (1, Side::In, 0)),
            Some(Cord {
                from: 0,
                from_out: 0,
                to: 1,
                to_in: 0
            })
        );
    }

    #[test]
    fn an_init_cord_is_drawn_dashed() {
        // A dashed cord is several short segments where a solid one is a single
        // line, so the same geometry draws more vertices at init rate.
        let boxes = vec![
            Obj::new("a", vec![], vec![Port::audio("out")]),
            Obj::new("b", vec![Port::audio("in")], vec![]),
        ];
        let cords = vec![Cord {
            from: 0,
            from_out: 0,
            to: 1,
            to_in: 0,
        }];
        let solid = PatchDraw {
            boxes: boxes.clone(),
            cords: cords.clone(),
        };
        let dashed = PatchDraw {
            boxes: vec![
                Obj::new("a", vec![], vec![Port::init("out")]),
                Obj::new("b", vec![Port::init("in")], vec![]),
            ],
            cords,
        };
        let state = || CanvasState {
            live: None,
            selected: &[],
            marquee: None,
            scale: 1.0,
        };
        let mut ms = Mesh::new();
        draw(
            &mut Draw::new(&mut ms, &Metrics::default(), &Theme::default()),
            area(),
            &solid,
            None,
            &state(),
        );
        let mut md = Mesh::new();
        draw(
            &mut Draw::new(&mut md, &Metrics::default(), &Theme::default()),
            area(),
            &dashed,
            None,
            &state(),
        );
        assert!(
            md.vertex_count() > ms.vertex_count(),
            "the dashed init cord adds segments over the solid audio one"
        );
    }

    #[test]
    fn the_patch_draws_its_boxes_and_cords() {
        let mut m = Mesh::new();
        draw(
            &mut Draw::new(&mut m, &Metrics::default(), &Theme::default()),
            area(),
            &chain(),
            Some("chain"),
            &CanvasState {
                live: None,
                selected: &[],
                marquee: None,
                scale: 1.0,
            },
        );
        assert!(!m.is_empty());

        // A cord in flight adds the line to the cursor; a selection and a
        // marquee add their chrome.
        let mut more = Mesh::new();
        draw(
            &mut Draw::new(&mut more, &Metrics::default(), &Theme::default()),
            area(),
            &chain(),
            Some("chain"),
            &CanvasState {
                live: Some(((0, Side::Out, 0), (400.0, 200.0))),
                selected: &[0],
                marquee: Some(Rect::new(50.0, 50.0, 120.0, 90.0)),
                scale: 1.0,
            },
        );
        assert!(more.vertex_count() > m.vertex_count());
    }
}
