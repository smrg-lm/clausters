//! The verovio-SVG -> display-list walk.
//!
//! verovio lays a score out into an SVG of SMuFL glyph outlines and engraving
//! strokes; this walks that SVG into the flat, resolution-independent display
//! list the host tessellates — a glyph-outline table keyed by SMuFL codepoint
//! plus placed glyphs, staff lines, stems, fills and text in verovio page
//! units, each carrying the MEI `xml:id` it was engraved from. The host draws
//! it knowing nothing about MEI or verovio, so any client that produces this
//! list (native libverovio or wasm verovio, both emitting the same SVG) reuses
//! one host renderer.
//!
//! Each primitive carries the id of the element it belongs to, and a **sounding
//! element owns everything drawn inside it**: verovio identifies a note's stem
//! and flag separately, and collapsing them onto the note's id is what makes one
//! note one thing to select and drag. A chord keeps its notes distinct, so one
//! of them can still be transposed alone.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use regex::Regex;
use roxmltree::{Document, Node};
use serde::{Deserialize, Serialize};

const XLINK: &str = "http://www.w3.org/1999/xlink";

/// A `score` display list: what the host draws. `vb` is the `[w, h]` page-unit
/// viewBox, `glyphs` maps a SMuFL codepoint (uppercase hex) to its outline path
/// `d`, `prims` are the placed primitives, and `step` is page units per
/// diatonic step (the quantum a pitch drag on the page counts in).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DisplayList {
    pub vb: [f64; 2],
    pub glyphs: BTreeMap<String, String>,
    pub prims: Vec<Prim>,
    pub step: f64,
}

/// One placed primitive. The `k` discriminator names the kind; every primitive
/// carries the `id` of the element it belongs to (omitted when it belongs to no
/// element — layer/staff furniture above the note level).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k")]
pub enum Prim {
    /// A SMuFL glyph placed by an affine `xf` = `[tx, ty, sx, sy]`; its outline
    /// is looked up by `cp` in [`DisplayList::glyphs`].
    #[serde(rename = "glyph")]
    Glyph {
        cp: String,
        xf: [f64; 4],
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    /// A stroked polyline (staff line, stem, ledger line, hairpin) of page-unit
    /// points, `w` the page-unit stroke width.
    #[serde(rename = "line")]
    Line {
        pts: Vec<[f64; 2]>,
        w: f64,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    /// A filled region (slur, tie, dot, notehead-free outline): the path data
    /// `d` in its own units, placed by the affine `xf` (the host tessellates).
    #[serde(rename = "fill")]
    Fill {
        d: String,
        xf: [f64; 4],
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    /// Verbatim text (volta numbers, tempo, lyrics, titles) at a page-mapped
    /// baseline `x, y`, `size` the em height; the host draws it in its own font.
    #[serde(rename = "text")]
    Text {
        s: String,
        x: f64,
        y: f64,
        size: f64,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
}

impl Prim {
    /// The id of the element this primitive belongs to, if any.
    pub fn id(&self) -> Option<&str> {
        match self {
            Prim::Glyph { id, .. }
            | Prim::Line { id, .. }
            | Prim::Fill { id, .. }
            | Prim::Text { id, .. } => id.as_deref(),
        }
    }
}

/// Walk a verovio SVG string into a `score` display list. Split out of the
/// engraving step so it is testable on a captured SVG without verovio; the
/// producer of the SVG (native libverovio or wasm verovio) is interchangeable.
///
/// A malformed SVG (never a verovio output, always a bug upstream) yields an
/// empty display list rather than an error, keeping the pure function total.
pub fn svg_to_display_list(svg: &str) -> DisplayList {
    let doc = match Document::parse(svg) {
        Ok(doc) => doc,
        Err(_) => return DisplayList::default(),
    };
    let root = doc.root_element();
    let glyph_defs = collect_glyph_defs(root);

    // The drawing lives inside the inner <svg class="definition-scale">.
    let inner = find_definition_scale(root);
    let (target, vb) = match inner {
        Some(inner) => (inner, viewbox(inner)),
        None => (root, viewbox(root)),
    };

    let mut glyphs = BTreeMap::new();
    let mut prims = Vec::new();
    walk(
        target,
        IDENTITY,
        None,
        false,
        &glyph_defs,
        &mut glyphs,
        &mut prims,
    );
    let step = staff_step(&prims);
    DisplayList {
        vb,
        glyphs,
        prims,
        step,
    }
}

// -- the SVG walk -----------------------------------------------------------
// verovio only emits translate()/scale() transforms; an (offset, scale) pair
// composes them exactly, so we carry that instead of a full matrix.

/// An affine as (tx, ty, sx, sy) — the only transforms verovio emits.
type Xf = (f64, f64, f64, f64);
const IDENTITY: Xf = (0.0, 0.0, 1.0, 1.0);

/// The classes that name a *sounding element* rather than a piece of one; a
/// chord is absent on purpose, since its notes nest inside it and each one has
/// to stay addressable on its own.
fn is_element_class(class: &str) -> bool {
    class
        .split_whitespace()
        .any(|c| c == "note" || c == "rest" || c == "mRest")
}

fn compose(parent: Xf, child: Xf) -> Xf {
    let (ptx, pty, psx, psy) = parent;
    let (ctx, cty, csx, csy) = child;
    (ptx + psx * ctx, pty + psy * cty, psx * csx, psy * csy)
}

fn apply(xf: Xf, x: f64, y: f64) -> (f64, f64) {
    let (tx, ty, sx, sy) = xf;
    (tx + sx * x, ty + sy * y)
}

fn transform_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(translate|scale)\(\s*([-\d.eE]+)\s*[, ]?\s*([-\d.eE]+)?\s*\)").unwrap()
    })
}

fn parse_transform(s: Option<&str>) -> Xf {
    let Some(s) = s else { return IDENTITY };
    let mut xf = IDENTITY;
    for cap in transform_re().captures_iter(s) {
        let kind = &cap[1];
        let a: f64 = cap[2].parse().unwrap_or(0.0);
        let b: f64 = match cap.get(3) {
            Some(m) => m.as_str().parse().unwrap_or(0.0),
            None if kind == "scale" => a,
            None => 0.0,
        };
        let local = if kind == "translate" {
            (a, b, 1.0, 1.0)
        } else {
            (0.0, 0.0, a, b)
        };
        xf = compose(xf, local);
    }
    xf
}

fn collect_glyph_defs<'a>(root: Node<'a, 'a>) -> BTreeMap<String, &'a str> {
    let mut out = BTreeMap::new();
    for g in root.descendants().filter(|n| n.has_tag_name("g")) {
        let Some(cp) = codepoint_at_start(g.attribute("id").unwrap_or("")) else {
            continue;
        };
        // a direct child <path> with a `d`
        let path = g
            .children()
            .find(|c| c.is_element() && c.has_tag_name("path"));
        if let Some(d) = path.and_then(|p| p.attribute("d")) {
            out.entry(cp).or_insert(d);
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn walk(
    node: Node,
    parent_xf: Xf,
    mei_id: Option<&str>,
    mut owned: bool,
    glyph_defs: &BTreeMap<String, &str>,
    glyphs: &mut BTreeMap<String, String>,
    prims: &mut Vec<Prim>,
) {
    let xf = compose(parent_xf, parse_transform(node.attribute("transform")));
    // Which element a primitive belongs to. verovio gives a note's *parts* ids
    // of their own (notehead, a `stem` group holding the stem and its `flag`),
    // and taking the innermost would scatter one note across three ids. So a
    // sounding element claims everything drawn inside it (`owned`), dropping its
    // parts' ids; everything above it still takes its own id, or the layer and
    // staff would swallow the clefs and bar lines.
    let own = node.attribute("id");
    let nid: Option<&str> = match own {
        Some(own) if !owned => {
            owned = is_element_class(node.attribute("class").unwrap_or(""));
            Some(own)
        }
        _ => mei_id,
    };
    let tag = node.tag_name().name();

    match tag {
        "use" => {
            let href = node
                .attribute((XLINK, "href"))
                .or_else(|| node.attribute("href"))
                .unwrap_or("");
            if let Some(cp) = codepoint_search(href.trim_start_matches('#'))
                && let Some(d) = glyph_defs.get(&cp)
            {
                glyphs.entry(cp.clone()).or_insert_with(|| (*d).to_string());
                // fold the glyph's inner scale(1,-1) flip into a negative sy so
                // the host maps font units -> page.
                let (tx, ty, sx, sy) = xf;
                prims.push(Prim::Glyph {
                    cp,
                    xf: [r(tx, 2), r(ty, 2), r(sx, 4), r(-sy, 4)],
                    id: nid.map(str::to_string),
                });
            }
        }
        "path" if node.attribute("d").is_some() => {
            let d = node.attribute("d").unwrap().trim();
            if let Some((x1, y1, x2, y2)) = parse_line(d) {
                let p1 = apply(xf, x1, y1);
                let p2 = apply(xf, x2, y2);
                prims.push(Prim::Line {
                    pts: vec![[r(p1.0, 1), r(p1.1, 1)], [r(p2.0, 1), r(p2.1, 1)]],
                    w: stroke_width(node, xf),
                    id: nid.map(str::to_string),
                });
            } else {
                // a filled outline (slur, tie): keep `d` verbatim, let the host
                // apply the transform, so coordinate separators are untouched.
                prims.push(Prim::Fill {
                    d: d.to_string(),
                    xf: xf_list(xf),
                    id: nid.map(str::to_string),
                });
            }
        }
        "polygon" if node.attribute("points").is_some() => {
            prims.push(Prim::Fill {
                d: points_to_path(node.attribute("points").unwrap()),
                xf: xf_list(xf),
                id: nid.map(str::to_string),
            });
        }
        "polyline" if node.attribute("points").is_some() => {
            // a stroked open path (hairpin, some brackets): a thick polyline,
            // not a fill — filling its endpoints would paint a solid wedge.
            let pts = points(node.attribute("points").unwrap());
            if pts.len() >= 2 {
                prims.push(Prim::Line {
                    pts: pts
                        .iter()
                        .map(|&(x, y)| {
                            let p = apply(xf, x, y);
                            [r(p.0, 1), r(p.1, 1)]
                        })
                        .collect(),
                    w: stroke_width(node, xf),
                    id: nid.map(str::to_string),
                });
            }
        }
        "rect" => prims.push(Prim::Fill {
            d: rect_to_path(node),
            xf: xf_list(xf),
            id: nid.map(str::to_string),
        }),
        "ellipse" => prims.push(Prim::Fill {
            d: ellipse_to_path(node),
            xf: xf_list(xf),
            id: nid.map(str::to_string),
        }),
        "text" => {
            if let Some(prim) = text_prim(node, xf, nid) {
                prims.push(prim);
            }
            // its tspans are consumed here, not walked as elements
        }
        _ => {
            for child in node.children().filter(Node::is_element) {
                walk(child, xf, nid, owned, glyph_defs, glyphs, prims);
            }
        }
    }
}

fn stroke_width(node: Node, xf: Xf) -> f64 {
    match node
        .attribute("stroke-width")
        .and_then(|w| w.parse::<f64>().ok())
    {
        Some(w) => r(w * xf.2, 1),
        None => 1.0,
    }
}

fn xf_list(xf: Xf) -> [f64; 4] {
    let (tx, ty, sx, sy) = xf;
    [r(tx, 2), r(ty, 2), r(sx, 4), r(sy, 4)]
}

/// Parse an SVG `points` list into `[(x, y), ...]` (local coordinates), dropping
/// a trailing odd coordinate.
fn points(points: &str) -> Vec<(f64, f64)> {
    let coords: Vec<f64> = points
        .split([' ', ','])
        .filter(|v| !v.is_empty())
        .filter_map(|v| v.parse().ok())
        .collect();
    coords
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| (c[0], c[1]))
        .collect()
}

fn points_to_path(points_str: &str) -> String {
    let parts: Vec<String> = points(points_str)
        .iter()
        .enumerate()
        .map(|(i, (x, y))| format!("{}{x:.1} {y:.1}", if i == 0 { 'M' } else { 'L' }))
        .collect();
    format!("{} Z", parts.join(" "))
}

fn text_prim(node: Node, xf: Xf, nid: Option<&str>) -> Option<Prim> {
    // the string lives in nested <tspan>s; the baseline x, y on the outer <text>
    let s: String = node
        .descendants()
        .filter(Node::is_text)
        .filter_map(|n| n.text())
        .collect();
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let x: f64 = node
        .attribute("x")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);
    let y: f64 = node
        .attribute("y")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);
    let (px, py) = apply(xf, x, y);
    let size = text_font_size(node) * xf.2;
    Some(Prim::Text {
        s: s.to_string(),
        x: r(px, 1),
        y: r(py, 1),
        size: r(size, 1),
        id: nid.map(str::to_string),
    })
}

fn fontsize_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^([-\d.eE]+)px").unwrap())
}

/// The deepest `font-size` in `px` under `node` (verovio puts a real size on the
/// innermost tspan and `0px` on the wrapper), defaulting to a readable size.
fn text_font_size(node: Node) -> f64 {
    let mut best = 0.0f64;
    for el in node.descendants().filter(Node::is_element) {
        if let Some(fs) = el.attribute("font-size")
            && let Some(cap) = fontsize_re().captures(fs)
            && let Ok(v) = cap[1].parse::<f64>()
        {
            best = best.max(v);
        }
    }
    if best != 0.0 { best } else { 400.0 }
}

fn ellipse_to_path(node: Node) -> String {
    let attr = |k: &str| {
        node.attribute(k)
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    let (cx, cy, rx, ry) = (attr("cx"), attr("cy"), attr("rx"), attr("ry"));
    let k = 0.5522847498; // 4/3 * (sqrt(2)-1): control offset for a quarter arc
    let pt = |x: f64, y: f64| format!("{x:.1} {y:.1}");
    format!(
        "M{} C{} {} {} C{} {} {} C{} {} {} C{} {} {} Z",
        pt(cx + rx, cy),
        pt(cx + rx, cy + ry * k),
        pt(cx + rx * k, cy + ry),
        pt(cx, cy + ry),
        pt(cx - rx * k, cy + ry),
        pt(cx - rx, cy + ry * k),
        pt(cx - rx, cy),
        pt(cx - rx, cy - ry * k),
        pt(cx - rx * k, cy - ry),
        pt(cx, cy - ry),
        pt(cx + rx * k, cy - ry),
        pt(cx + rx, cy - ry * k),
        pt(cx + rx, cy),
    )
}

fn rect_to_path(node: Node) -> String {
    let attr = |k: &str| {
        node.attribute(k)
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    let (x, y, w, h) = (attr("x"), attr("y"), attr("width"), attr("height"));
    let pts = [(x, y), (x + w, y), (x + w, y + h), (x, y + h)];
    let parts: Vec<String> = pts
        .iter()
        .enumerate()
        .map(|(i, (px, py))| format!("{}{px:.1} {py:.1}", if i == 0 { 'M' } else { 'L' }))
        .collect();
    format!("{} Z", parts.join(" "))
}

fn find_definition_scale<'a>(root: Node<'a, 'a>) -> Option<Node<'a, 'a>> {
    root.descendants()
        .find(|n| n.has_tag_name("svg") && n.attribute("class") == Some("definition-scale"))
}

fn viewbox(node: Node) -> [f64; 2] {
    let vb: Vec<f64> = node
        .attribute("viewBox")
        .unwrap_or("")
        .split_whitespace()
        .filter_map(|v| v.parse().ok())
        .collect();
    if vb.len() == 4 {
        [vb[2], vb[3]]
    } else {
        [0.0, 0.0]
    }
}

// -- staff geometry ---------------------------------------------------------

/// The page-y of every staff line, ascending. A staff line is a wide horizontal
/// `line` prim — the one geometry the same on every system, the ruler both the
/// system clustering and the diatonic step are measured against.
pub(super) fn staff_line_ys(prims: &[Prim]) -> Vec<f64> {
    let mut ys: Vec<f64> = prims
        .iter()
        .filter_map(|p| match p {
            Prim::Line { pts, .. }
                if pts.len() == 2
                    && (pts[0][1] - pts[1][1]).abs() < 1.0
                    && (pts[0][0] - pts[1][0]).abs() > 500.0 =>
            {
                Some(r(pts[0][1], 1))
            }
            _ => None,
        })
        .collect();
    ys.sort_by(|a, b| a.total_cmp(b));
    ys.dedup();
    ys
}

/// Page units per **diatonic step**: half the staff-line spacing (a line-to-space
/// move is one step). Measured from the drawing (the median gap within a system)
/// so any producer gets it right; falls back to verovio's default `unit` (9 x
/// the definition factor 10 = 90) when the page has no staff to measure.
fn staff_step(prims: &[Prim]) -> f64 {
    let ys = staff_line_ys(prims);
    let mut gaps: Vec<f64> = ys
        .windows(2)
        .map(|w| w[1] - w[0])
        .filter(|&g| g < 500.0)
        .collect();
    gaps.sort_by(|a, b| a.total_cmp(b));
    if gaps.is_empty() {
        return 90.0;
    }
    r(gaps[gaps.len() / 2] / 2.0, 3)
}

// -- small numeric / string helpers -----------------------------------------

/// Round `x` to `n` decimal places (half away from zero), the display list's
/// coordinate quantization.
pub(super) fn r(x: f64, n: i32) -> f64 {
    let p = 10f64.powi(n);
    (x * p).round() / p
}

/// The leading SMuFL codepoint of `s`: 4..=6 hex digits at the very start,
/// uppercased. verovio glyph def ids are `<CODEPOINT>-<suffix>`.
fn codepoint_at_start(s: &str) -> Option<String> {
    let run = leading_hex(s);
    (run >= 4).then(|| s[..run.min(6)].to_uppercase())
}

/// The first SMuFL codepoint anywhere in `s` (a `use` href, `#`-stripped): the
/// leftmost run of 4..=6 hex digits, uppercased.
fn codepoint_search(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    for i in 0..bytes.len() {
        let run = leading_hex(&s[i..]);
        if run >= 4 {
            return Some(s[i..i + run.min(6)].to_uppercase());
        }
    }
    None
}

fn leading_hex(s: &str) -> usize {
    s.bytes().take_while(u8::is_ascii_hexdigit).count()
}

/// A single `M x y L x y` segment (a staff line / stem / ledger line), else
/// `None` — mirrors the anchored `^M..L..$` match on the trimmed path data.
fn parse_line(d: &str) -> Option<(f64, f64, f64, f64)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"^M\s*([-\d.eE]+)\s+([-\d.eE]+)\s+L\s*([-\d.eE]+)\s+([-\d.eE]+)\s*$").unwrap()
    });
    let cap = re.captures(d)?;
    Some((
        cap[1].parse().ok()?,
        cap[2].parse().ok()?,
        cap[3].parse().ok()?,
        cap[4].parse().ok()?,
    ))
}

/// One engraved staff: the page-y of its top and bottom lines.
///
/// Derived from the drawing rather than from the document, because a page is
/// all a reader of a display list has — which is the same position the GUI host
/// is in, and the reason this lives here instead of in either consumer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Staff {
    pub y0: f64,
    pub y1: f64,
}

impl DisplayList {
    /// The staves on this page: its longest horizontal strokes, clustered into
    /// systems.
    ///
    /// Within a staff the lines sit one space — two diatonic steps — apart, so
    /// a wider gap starts the next system. Sorted top to bottom.
    ///
    /// A staff line is picked out by being **long relative to the other
    /// horizontal strokes on the page**, not relative to the page: a short
    /// phrase engraved onto a wide sheet draws a system across a fraction of
    /// it, and measuring against the viewBox would find no staff at all — which
    /// is exactly what it did before this rule replaced it. What it has to be
    /// told apart from is ledger lines (a notehead wide) and beams (a few
    /// noteheads), both an order of magnitude shorter, so the threshold has
    /// room either way.
    pub fn staves(&self) -> Vec<Staff> {
        let horizontals: Vec<(f64, f64)> = self
            .prims
            .iter()
            .filter_map(|p| match p {
                Prim::Line { pts, .. } if pts.len() == 2 && (pts[0][1] - pts[1][1]).abs() < 1.0 => {
                    Some(((pts[0][0] - pts[1][0]).abs(), pts[0][1]))
                }
                _ => None,
            })
            .collect();
        let longest = horizontals.iter().fold(0.0f64, |m, (len, _)| m.max(*len));
        let mut ys: Vec<f64> = horizontals
            .into_iter()
            .filter(|(len, _)| *len >= 0.5 * longest && longest > 0.0)
            .map(|(_, y)| y)
            .collect();
        ys.sort_by(f64::total_cmp);
        ys.dedup_by(|a, b| (*a - *b).abs() < 0.5);
        let gap = 2.5 * self.step;
        let mut out: Vec<Staff> = Vec::new();
        for y in ys {
            match out.last_mut() {
                Some(s) if y - s.y1 <= gap => s.y1 = y,
                _ => out.push(Staff { y0: y, y1: y }),
            }
        }
        out
    }

    /// Where an engraved element sits on its staff, in **whole diatonic steps
    /// from the staff's top line**, positive upward.
    ///
    /// This is the absolute coordinate a pitch edit names, in place of a
    /// displacement: an edit stated as a position is idempotent and needs no
    /// rebasing against a page that was re-engraved under it, which is what a
    /// gesture crossing a wire requires.
    ///
    /// The element's **first** primitive is its notehead — verovio draws it
    /// before the stem — and that is what the position is measured from; the
    /// stem and flag would pull the box off the pitch. The staff is the
    /// **nearest** one, since a note off the staff still belongs to it (that is
    /// what ledger lines are for). `None` when the id is not on the page or the
    /// page has no staff to measure against.
    pub fn staff_position(&self, id: &str) -> Option<i32> {
        let y = self.head_y(id)?;
        let staff = self
            .staves()
            .into_iter()
            .min_by(|a, b| staff_distance(a, y).total_cmp(&staff_distance(b, y)))?;
        Some(((staff.y0 - y) / self.step).round() as i32)
    }

    /// The page-y an element's notehead is centred on: the vertical middle of
    /// its first primitive's bounds.
    fn head_y(&self, id: &str) -> Option<f64> {
        let prim = self.prims.iter().find(|p| p.id() == Some(id))?;
        match prim {
            // A glyph's outline is placed by `xf` = [tx, ty, sx, sy]; the
            // baseline `ty` is where the notehead is centred, since a SMuFL
            // notehead is drawn about its own origin.
            Prim::Glyph { xf, .. } | Prim::Fill { xf, .. } => Some(xf[1]),
            Prim::Line { pts, .. } => {
                let (lo, hi) = pts.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| {
                    (lo.min(p[1]), hi.max(p[1]))
                });
                (lo <= hi).then_some(0.5 * (lo + hi))
            }
            Prim::Text { y, .. } => Some(*y),
        }
    }
}

/// How far a page-y sits from a staff — zero anywhere between its outer lines.
fn staff_distance(s: &Staff, y: f64) -> f64 {
    (s.y0 - y).max(y - s.y1).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A minimal verovio-shaped SVG: the outer <svg>, a <defs> glyph, the inner
    // definition-scale <svg> with a staff line, a placed notehead, a slur fill
    // and a text — one of each primitive kind.
    const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink">
      <defs><g id="E0A4-abc"><path d="M0 0 C1 1 2 2 3 3Z"/></g></defs>
      <svg class="definition-scale" viewBox="0 0 1000 400">
        <g id="note-1" class="note">
          <use xlink:href="#E0A4-abc" x="0" y="0" transform="translate(500,200)scale(0.72,-0.72)"/>
          <g id="stem-1"><path d="M600 100 L600 250" stroke-width="18"/></g>
        </g>
        <path id="line-1" d="M0 100 L1000 100" stroke-width="13"/>
        <path id="slur-1" d="M10 10 C20 20 30 30 40 40Z"/>
        <text id="dyn-1" x="500" y="300"><tspan font-size="0px"><tspan font-size="80px">mf</tspan></tspan></text>
      </svg>
    </svg>"##;

    #[test]
    fn walks_the_viewbox_and_glyph_table() {
        let dl = svg_to_display_list(SVG);
        assert_eq!(dl.vb, [1000.0, 400.0]);
        assert_eq!(
            dl.glyphs.get("E0A4").map(String::as_str),
            Some("M0 0 C1 1 2 2 3 3Z")
        );
    }

    #[test]
    fn a_note_owns_its_stem_under_one_id() {
        let dl = svg_to_display_list(SVG);
        // the notehead glyph and the stem line both take the note's id, not the
        // stem group's own id
        let note_parts: Vec<&Prim> = dl
            .prims
            .iter()
            .filter(|p| p.id() == Some("note-1"))
            .collect();
        assert_eq!(note_parts.len(), 2);
        assert!(matches!(note_parts[0], Prim::Glyph { cp, .. } if cp == "E0A4"));
        assert!(matches!(note_parts[1], Prim::Line { .. }));
        assert!(dl.prims.iter().all(|p| p.id() != Some("stem-1")));
    }

    #[test]
    fn the_glyph_flip_folds_into_a_negative_sy() {
        let dl = svg_to_display_list(SVG);
        let glyph = dl.prims.iter().find_map(|p| match p {
            Prim::Glyph { xf, .. } => Some(*xf),
            _ => None,
        });
        // translate(500,200) scale(0.72,-0.72), the -sy flipped back to positive
        assert_eq!(glyph, Some([500.0, 200.0, 0.72, 0.72]));
    }

    #[test]
    fn a_simple_segment_is_a_line_a_curve_is_a_fill() {
        let dl = svg_to_display_list(SVG);
        let line = dl.prims.iter().find(|p| p.id() == Some("line-1")).unwrap();
        assert!(matches!(line, Prim::Line { pts, w, .. }
            if *pts == vec![[0.0, 100.0], [1000.0, 100.0]] && *w == 13.0));
        let slur = dl.prims.iter().find(|p| p.id() == Some("slur-1")).unwrap();
        assert!(matches!(slur, Prim::Fill { d, .. } if d == "M10 10 C20 20 30 30 40 40Z"));
    }

    #[test]
    fn text_takes_the_deepest_font_size() {
        let dl = svg_to_display_list(SVG);
        let text = dl.prims.iter().find(|p| p.id() == Some("dyn-1")).unwrap();
        assert!(matches!(text, Prim::Text { s, x, y, size, .. }
            if s == "mf" && *x == 500.0 && *y == 300.0 && *size == 80.0));
    }

    #[test]
    fn the_step_is_measured_from_the_staff_lines() {
        // two staff lines 90 apart -> a diatonic step is half that, 45. Only one
        // pair here, so the median gap is 90.
        let two_line = r##"<svg xmlns="http://www.w3.org/2000/svg">
          <svg class="definition-scale" viewBox="0 0 1000 400">
            <path d="M0 100 L1000 100" stroke-width="13"/>
            <path d="M0 190 L1000 190" stroke-width="13"/>
          </svg></svg>"##;
        assert_eq!(svg_to_display_list(two_line).step, 45.0);
    }

    #[test]
    fn no_staff_falls_back_to_the_default_step() {
        let empty = r##"<svg xmlns="http://www.w3.org/2000/svg">
          <svg class="definition-scale" viewBox="0 0 10 10"></svg></svg>"##;
        assert_eq!(svg_to_display_list(empty).step, 90.0);
    }

    #[test]
    fn a_polygon_becomes_a_closed_fill_path() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg">
          <svg class="definition-scale" viewBox="0 0 10 10">
            <polygon id="p" points="0,0 10,0 10,10"/>
          </svg></svg>"##;
        let dl = svg_to_display_list(svg);
        let p = dl.prims.iter().find(|p| p.id() == Some("p")).unwrap();
        assert!(matches!(p, Prim::Fill { d, .. } if d == "M0.0 0.0 L10.0 0.0 L10.0 10.0 Z"));
    }

    #[test]
    fn codepoint_scanning_matches_the_python_regex() {
        assert_eq!(codepoint_at_start("E0A4-n1sc384i"), Some("E0A4".into()));
        assert_eq!(codepoint_at_start("abc"), None); // <4 hex
        assert_eq!(codepoint_search("note-E0A4x"), Some("E0A4".into()));
        assert_eq!(codepoint_search("abcdefg"), Some("ABCDEF".into())); // capped at 6
    }

    #[test]
    fn a_malformed_svg_is_an_empty_list_not_a_panic() {
        let dl = svg_to_display_list("<not xml");
        assert_eq!(dl.vb, [0.0, 0.0]);
        assert!(dl.prims.is_empty());
    }

    /// A page of two systems, five lines each one space apart, with a notehead
    /// placed at `y`. The geometry a pitch edit is measured against.
    fn page(y: f64) -> DisplayList {
        let line = |ly: f64| Prim::Line {
            pts: vec![[0.0, ly], [1000.0, ly]],
            w: 4.0,
            id: None,
        };
        let mut prims: Vec<Prim> = (0..5).map(|i| line(160.0 + 180.0 * i as f64)).collect();
        // A second system, far enough down that the gap rule starts it anew.
        prims.extend((0..5).map(|i| line(1300.0 + 180.0 * i as f64)));
        prims.push(Prim::Glyph {
            cp: "E0A4".into(),
            xf: [100.0, y, 1.0, -1.0],
            id: Some("n1".into()),
        });
        DisplayList {
            vb: [1000.0, 2200.0],
            glyphs: BTreeMap::new(),
            prims,
            step: 90.0,
        }
    }

    #[test]
    fn the_staff_lines_cluster_into_systems() {
        let staves = page(250.0).staves();
        assert_eq!(
            staves,
            vec![
                Staff {
                    y0: 160.0,
                    y1: 880.0
                },
                Staff {
                    y0: 1300.0,
                    y1: 2020.0
                }
            ]
        );
    }

    /// The position is measured from the staff's top line, positive upward, in
    /// whole diatonic steps — the absolute coordinate a pitch edit names.
    #[test]
    fn a_position_is_steps_above_the_top_line() {
        assert_eq!(page(160.0).staff_position("n1"), Some(0)); // on the top line
        assert_eq!(page(250.0).staff_position("n1"), Some(-1)); // the space below it
        assert_eq!(page(70.0).staff_position("n1"), Some(1)); // a ledger above
        assert_eq!(page(1300.0).staff_position("n1"), Some(0)); // the second system
    }

    /// A note off the staff still belongs to it, which is what ledger lines are
    /// for — so the position keeps counting rather than jumping to the nearer
    /// system once the note leaves the lines.
    #[test]
    fn a_note_off_the_staff_keeps_counting_from_its_own() {
        assert_eq!(page(-110.0).staff_position("n1"), Some(3));
    }

    #[test]
    fn an_unknown_id_and_a_staffless_page_have_no_position() {
        assert_eq!(page(250.0).staff_position("nope"), None);
        let bare = DisplayList {
            vb: [1000.0, 400.0],
            glyphs: BTreeMap::new(),
            prims: vec![Prim::Glyph {
                cp: "E0A4".into(),
                xf: [100.0, 200.0, 1.0, -1.0],
                id: Some("n1".into()),
            }],
            step: 90.0,
        };
        assert_eq!(bare.staff_position("n1"), None);
    }
}
