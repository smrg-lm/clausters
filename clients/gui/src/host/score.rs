//! The `score` widget's renderer: a verovio display list -> triangle mesh.
//!
//! Music notation is vector art — SMuFL glyph outlines (noteheads, clefs,
//! rests, accidentals, flags) plus engraving strokes and fills (staff lines,
//! stems, ledger lines, beams, slurs, ties). None of it is data-viz, so it does
//! not get its own GPU pipeline: every primitive is tessellated into the same
//! flat-colored [`Mesh`](super::paint::Mesh) the rest of the chrome uses, which
//! keeps it one upload, one draw, and WebGL2-safe by construction.
//!
//! The host is the *renderer*; it never depends on verovio. A client (the
//! Python `clausters.gui` submodule, driving verovio) engraves the score and
//! sends a **semantic display list**: a table of glyph outlines keyed by SMuFL
//! codepoint, plus placed primitives in verovio page units. The host fits that
//! page into the widget rect and tessellates. A future JS/wasm client reuses
//! this same renderer by sending the same display list — no engraving logic is
//! duplicated per language.
//!
//! Curves (glyph outlines, slurs, ties) are filled with lyon's
//! [`FillTessellator`]; strokes (staff/stems/ledger) are the painter's own
//! thick-line quads. Everything is baked into screen coordinates *before*
//! tessellation so the curve-flattening tolerance is expressed in pixels.

use std::collections::HashMap;

use serde_json::{Map, Value};

use lyon::math::point;
use lyon::path::Path as LyonPath;
use lyon::tessellation::{
    BuffersBuilder, FillOptions, FillRule, FillTessellator, FillVertex, VertexBuffers,
};

use super::layout::Rect;
use super::paint::{Color, Mesh};

/// An affine map restricted to translate + non-uniform scale — the only
/// transforms verovio emits (`translate(...)` and `scale(...)`, the glyph's
/// inner `scale(1,-1)` folded into a negative `sy`). Composing two of these
/// stays in the family, so a full matrix is unnecessary.
#[derive(Clone, Copy, Debug)]
pub struct Affine {
    pub tx: f32,
    pub ty: f32,
    pub sx: f32,
    pub sy: f32,
}

impl Affine {
    pub const IDENTITY: Affine = Affine {
        tx: 0.0,
        ty: 0.0,
        sx: 1.0,
        sy: 1.0,
    };

    /// `self` applied after `inner` (i.e. `self ∘ inner`).
    pub fn then(self, inner: Affine) -> Affine {
        Affine {
            tx: self.tx + self.sx * inner.tx,
            ty: self.ty + self.sy * inner.ty,
            sx: self.sx * inner.sx,
            sy: self.sy * inner.sy,
        }
    }

    #[inline]
    pub fn apply(self, x: f32, y: f32) -> [f32; 2] {
        [self.tx + self.sx * x, self.ty + self.sy * y]
    }
}

/// One placed element of the engraved page, in verovio page units.
#[derive(Clone, Debug)]
pub enum Prim {
    /// A SMuFL glyph: its outline (looked up by `cp` in [`ScoreData::glyphs`])
    /// mapped from font units to page units by `xf` (with the y-flip folded in).
    Glyph {
        cp: u32,
        xf: Affine,
        id: Option<String>,
    },
    /// A stroked polyline: staff lines, stems, ledger lines, bar lines.
    Line {
        pts: Vec<[f32; 2]>,
        width: f32,
        id: Option<String>,
    },
    /// A filled region: beams (polygons), slurs and ties (filled cubic
    /// outlines), augmentation dots (ellipses). `d` is the outline in the
    /// element's **local** coordinates; `xf` maps it to page units — mapped in
    /// the host (not baked into `d` on the client) so comma/space coordinate
    /// separators never confuse a numeric rewrite.
    Fill {
        d: String,
        xf: Affine,
        id: Option<String>,
    },
    /// Verbatim text (not SMuFL): volta numbers, tempo, lyrics, titles. `x, y`
    /// is the baseline and `size` the em height, both in page units; the host
    /// draws it in its own font.
    Text {
        s: String,
        x: f32,
        y: f32,
        size: f32,
        id: Option<String>,
    },
}

impl Prim {
    /// The MEI xml:id this primitive was engraved from, if any (the hook for
    /// hit-testing and edit-back once the score view becomes interactive).
    pub fn id(&self) -> Option<&str> {
        match self {
            Prim::Glyph { id, .. }
            | Prim::Line { id, .. }
            | Prim::Fill { id, .. }
            | Prim::Text { id, .. } => id.as_deref(),
        }
    }
}

/// One position of the playback cursor: at musical time `t` (ms) the sounding
/// event sits at page-x `x`, spanning its system's staff from `y0` to `y1`. The
/// track is the bridge from the timemap (onset ms per MEI id, from the client)
/// to geometry (the id's placed x) — precomputed on the client, sorted by `t`.
#[derive(Clone, Copy, Debug)]
pub struct Cursor {
    pub t: f32,
    pub x: f32,
    pub y0: f32,
    pub y1: f32,
}

/// A fully engraved page ready to render: the definition viewBox (for fitting),
/// the glyph-outline table (deduplicated by codepoint), and the placed
/// primitives.
#[derive(Clone, Debug, Default)]
pub struct ScoreData {
    /// The verovio `definition-scale` viewBox, `(width, height)` in page units.
    pub vb_w: f32,
    pub vb_h: f32,
    /// SMuFL codepoint -> outline path `d` (font units, y-up before the flip).
    pub glyphs: HashMap<u32, String>,
    pub prims: Vec<Prim>,
    /// The playback-cursor track (sorted by `t`), empty when the client sent no
    /// timemap.
    pub cursors: Vec<Cursor>,
    /// Current playback time in ms, advanced live via `/gui_set playhead`;
    /// negative means no playhead is shown.
    pub playhead: f32,
}

impl ScoreData {
    /// Parse the `score` widget's display-list props sent by the client:
    /// `vb` = `[width, height]` page-unit viewBox, `glyphs` = an object mapping
    /// a hex SMuFL codepoint string to its outline path `d`, and `prims` = an
    /// array of `{k: "glyph"|"line"|"fill", ...}` primitives. Malformed entries
    /// are skipped rather than rejected, so a partially understood display list
    /// still draws what it can — the "unknown widget is laid out but not
    /// painted" spirit applied within the widget.
    pub fn parse(props: &Map<String, Value>) -> ScoreData {
        let mut data = ScoreData::default();
        if let Some(vb) = props.get("vb").and_then(Value::as_array) {
            data.vb_w = vb.first().and_then(Value::as_f64).unwrap_or(0.0) as f32;
            data.vb_h = vb.get(1).and_then(Value::as_f64).unwrap_or(0.0) as f32;
        }
        if let Some(glyphs) = props.get("glyphs").and_then(Value::as_object) {
            for (code, d) in glyphs {
                if let (Ok(cp), Some(d)) = (u32::from_str_radix(code, 16), d.as_str()) {
                    data.glyphs.insert(cp, d.to_string());
                }
            }
        }
        if let Some(prims) = props.get("prims").and_then(Value::as_array) {
            for p in prims {
                if let Some(prim) = parse_prim(p) {
                    data.prims.push(prim);
                }
            }
        }
        if let Some(cursors) = props.get("cursors").and_then(Value::as_array) {
            for c in cursors {
                if let Some(cur) = parse_cursor(c) {
                    data.cursors.push(cur);
                }
            }
            data.cursors
                .sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
        }
        // A playhead is off unless the client sets a non-negative time.
        data.playhead = props
            .get("playhead")
            .and_then(Value::as_f64)
            .map(|f| f as f32)
            .unwrap_or(-1.0);
        data
    }

    /// The transform fitting the whole page into `rect`, preserving aspect and
    /// centring (uniform scale, no navigation yet). Returns identity for an
    /// empty page so a def with no geometry is a harmless no-op.
    pub fn fit(&self, rect: Rect) -> Affine {
        if self.vb_w <= 0.0 || self.vb_h <= 0.0 {
            return Affine::IDENTITY;
        }
        let s = (rect.w / self.vb_w)
            .min(rect.h / self.vb_h)
            .max(f32::MIN_POSITIVE);
        Affine {
            tx: rect.x + (rect.w - s * self.vb_w) * 0.5,
            ty: rect.y + (rect.h - s * self.vb_h) * 0.5,
            sx: s,
            sy: s,
        }
    }

    /// Tessellate the whole page into `mesh`, mapped into `rect` by [`fit`] and
    /// painted in `color`; the playback cursor (if a `playhead` time is set)
    /// draws over it in `playhead_color`. Geometry is clipped to `rect`
    /// intersected with the caller's `clip` (the enclosing scroll area, if any);
    /// the caller's clip is restored on return so the surrounding frame pass is
    /// unaffected.
    ///
    /// [`fit`]: ScoreData::fit
    pub fn render(
        &self,
        mesh: &mut Mesh,
        rect: Rect,
        clip: Option<Rect>,
        color: Color,
        playhead_color: Color,
    ) {
        let fit = self.fit(rect);
        mesh.set_clip(Some(intersect(rect, clip)));
        // Curve-flattening tolerance in page units so it lands ~1/3 device
        // pixel after fitting — fine enough to read smooth, coarse enough to
        // keep the triangle count bounded by the screen, not the notation.
        let tol_page = 0.33 / fit.sx.max(f32::MIN_POSITIVE);
        let mut tess = FillTessellator::new();
        for prim in &self.prims {
            match prim {
                Prim::Line { pts, width, .. } => {
                    let w = (width * fit.sx).max(1.0);
                    for seg in pts.windows(2) {
                        mesh.line(
                            fit.apply(seg[0][0], seg[0][1]),
                            fit.apply(seg[1][0], seg[1][1]),
                            w,
                            color,
                        );
                    }
                }
                Prim::Glyph { cp, xf, .. } => {
                    if let Some(d) = self.glyphs.get(cp) {
                        // font -> page (xf) -> screen (fit): still translate+scale.
                        fill_path(
                            mesh,
                            &mut tess,
                            d,
                            fit.then(*xf),
                            tol_page * xf_shrink(*xf),
                            color,
                        );
                    }
                }
                Prim::Fill { d, xf, .. } => {
                    fill_path(
                        mesh,
                        &mut tess,
                        d,
                        fit.then(*xf),
                        tol_page * xf_shrink(*xf),
                        color,
                    );
                }
                Prim::Text { s, x, y, size, .. } => {
                    // baseline -> top-left for the host font; em height in px.
                    let em = (size * fit.sy).abs();
                    let scale = (em / super::font::GLYPH_H as f32).max(0.5);
                    let [sx, sy] = fit.apply(*x, *y);
                    super::font::text(mesh, s, sx, sy - em, scale, color);
                }
            }
        }
        self.draw_playhead(mesh, fit, playhead_color);
        mesh.set_clip(clip);
    }

    /// Draw the playback cursor for the current `playhead` time: the vertical
    /// staff-spanning line of the latest cursor at or before it. A no-op when no
    /// playhead is set or no timemap was sent.
    fn draw_playhead(&self, mesh: &mut Mesh, fit: Affine, color: Color) {
        if self.playhead < 0.0 || self.cursors.is_empty() {
            return;
        }
        // the cursor active at `playhead`: the last one whose time is <= it.
        let idx = self
            .cursors
            .partition_point(|c| c.t <= self.playhead)
            .saturating_sub(1);
        let c = self.cursors[idx.min(self.cursors.len() - 1)];
        // points are already in screen pixels after `fit`, so width is px.
        mesh.line(fit.apply(c.x, c.y0), fit.apply(c.x, c.y1), 2.0, color);
    }
}

/// `rect` clamped to `clip` (or `rect` itself when there is no outer clip).
fn intersect(rect: Rect, clip: Option<Rect>) -> Rect {
    let Some(c) = clip else { return rect };
    let x0 = rect.x.max(c.x);
    let y0 = rect.y.max(c.y);
    let x1 = (rect.x + rect.w).min(c.x + c.w);
    let y1 = (rect.y + rect.h).min(c.y + c.h);
    Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
}

/// Read a `[tx, ty, sx, sy]` transform array into an [`Affine`].
fn parse_xf(v: Option<&Value>) -> Option<Affine> {
    let a = v?.as_array()?;
    let n = |i: usize| a.get(i).and_then(Value::as_f64).map(|f| f as f32);
    Some(Affine {
        tx: n(0)?,
        ty: n(1)?,
        sx: n(2)?,
        sy: n(3)?,
    })
}

fn parse_cursor(v: &Value) -> Option<Cursor> {
    let o = v.as_object()?;
    let f = |k: &str| o.get(k).and_then(Value::as_f64).map(|x| x as f32);
    Some(Cursor {
        t: f("t")?,
        x: f("x")?,
        y0: f("y0")?,
        y1: f("y1")?,
    })
}

fn parse_prim(v: &Value) -> Option<Prim> {
    let obj = v.as_object()?;
    let id = obj.get("id").and_then(Value::as_str).map(str::to_string);
    match obj.get("k").and_then(Value::as_str)? {
        "glyph" => {
            let cp = u32::from_str_radix(obj.get("cp")?.as_str()?, 16).ok()?;
            Some(Prim::Glyph {
                cp,
                xf: parse_xf(obj.get("xf"))?,
                id,
            })
        }
        "line" => {
            let pts = obj.get("pts").and_then(Value::as_array)?;
            let pts: Vec<[f32; 2]> = pts
                .iter()
                .filter_map(|p| {
                    let a = p.as_array()?;
                    Some([a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32])
                })
                .collect();
            if pts.len() < 2 {
                return None;
            }
            Some(Prim::Line {
                pts,
                width: obj.get("w").and_then(Value::as_f64).unwrap_or(1.0) as f32,
                id,
            })
        }
        "fill" => Some(Prim::Fill {
            d: obj.get("d")?.as_str()?.to_string(),
            xf: parse_xf(obj.get("xf")).unwrap_or(Affine::IDENTITY),
            id,
        }),
        "text" => Some(Prim::Text {
            s: obj.get("s")?.as_str()?.to_string(),
            x: obj.get("x")?.as_f64()? as f32,
            y: obj.get("y")?.as_f64()? as f32,
            size: obj.get("size").and_then(Value::as_f64).unwrap_or(0.0) as f32,
            id,
        }),
        _ => None,
    }
}

/// How much a glyph's own transform shrinks page units, so the tolerance passed
/// to [`fill_path`] (expressed in the glyph's *local* font units) still lands at
/// the same on-screen size. `fill_path` flattens in the path's own coordinates
/// then maps, so its tolerance must be pre-divided by the local scale.
fn xf_shrink(xf: Affine) -> f32 {
    1.0 / xf.sx.abs().max(f32::MIN_POSITIVE)
}

/// Parse an SVG path `d`, flatten + fill it with lyon, and emit the triangles
/// into `mesh` after mapping each vertex through `xf`. Tessellation happens in
/// the path's own coordinate space (tolerance `tol`), then the resulting
/// vertices are mapped — cheaper than transforming every bezier control point,
/// and correct because `xf` is affine.
fn fill_path(
    mesh: &mut Mesh,
    tess: &mut FillTessellator,
    d: &str,
    xf: Affine,
    tol: f32,
    color: Color,
) {
    let Some(path) = build_path(d) else { return };
    let mut buffers: VertexBuffers<[f32; 2], u32> = VertexBuffers::new();
    let opts = FillOptions::tolerance(tol.max(f32::MIN_POSITIVE)).with_fill_rule(FillRule::NonZero);
    let ok = tess.tessellate_path(
        &path,
        &opts,
        &mut BuffersBuilder::new(&mut buffers, |v: FillVertex| {
            let p = v.position();
            [p.x, p.y]
        }),
    );
    if ok.is_err() {
        return;
    }
    for tri in buffers.indices.chunks_exact(3) {
        let a = buffers.vertices[tri[0] as usize];
        let b = buffers.vertices[tri[1] as usize];
        let c = buffers.vertices[tri[2] as usize];
        mesh.tri(
            xf.apply(a[0], a[1]),
            xf.apply(b[0], b[1]),
            xf.apply(c[0], c[1]),
            color,
        );
    }
}

/// Build a lyon [`LyonPath`] from an SVG path `d`. Supports the subset verovio
/// emits: `M/m` moveto, `L/l` lineto, `H/h`/`V/v` axis lines, `C/c` cubic,
/// `S/s` smooth cubic, and `Z/z` close — absolute and relative. Returns `None`
/// on a malformed/empty path (the primitive is then skipped, never a panic).
fn build_path(d: &str) -> Option<LyonPath> {
    let mut b = LyonPath::builder();
    let mut toks = Tokens::new(d);
    let (mut cx, mut cy) = (0.0f32, 0.0f32); // current point
    let (mut sx, mut sy) = (0.0f32, 0.0f32); // subpath start
    let mut open = false;
    let mut prev_ctrl: Option<(f32, f32)> = None; // 2nd control of last cubic, for S/s
    let mut cmd = ' ';
    loop {
        let next = toks.peek_cmd();
        if let Some(c) = next {
            cmd = c;
            toks.bump();
        } else if toks.at_end() {
            break;
        }
        // implicit repeat: after M the default becomes L, otherwise cmd repeats
        let rel = cmd.is_ascii_lowercase();
        match cmd.to_ascii_uppercase() {
            'M' => {
                let (mut x, mut y) = (toks.num()?, toks.num()?);
                if rel {
                    x += cx;
                    y += cy;
                }
                if open {
                    b.end(false);
                }
                b.begin(point(x, y));
                open = true;
                cx = x;
                cy = y;
                sx = x;
                sy = y;
                prev_ctrl = None;
                cmd = if rel { 'l' } else { 'L' };
            }
            'L' => {
                let (mut x, mut y) = (toks.num()?, toks.num()?);
                if rel {
                    x += cx;
                    y += cy;
                }
                b.line_to(point(x, y));
                cx = x;
                cy = y;
                prev_ctrl = None;
            }
            'H' => {
                let mut x = toks.num()?;
                if rel {
                    x += cx;
                }
                b.line_to(point(x, cy));
                cx = x;
                prev_ctrl = None;
            }
            'V' => {
                let mut y = toks.num()?;
                if rel {
                    y += cy;
                }
                b.line_to(point(cx, y));
                cy = y;
                prev_ctrl = None;
            }
            'C' => {
                let (mut x1, mut y1) = (toks.num()?, toks.num()?);
                let (mut x2, mut y2) = (toks.num()?, toks.num()?);
                let (mut x, mut y) = (toks.num()?, toks.num()?);
                if rel {
                    x1 += cx;
                    y1 += cy;
                    x2 += cx;
                    y2 += cy;
                    x += cx;
                    y += cy;
                }
                b.cubic_bezier_to(point(x1, y1), point(x2, y2), point(x, y));
                prev_ctrl = Some((x2, y2));
                cx = x;
                cy = y;
            }
            'S' => {
                let (mut x2, mut y2) = (toks.num()?, toks.num()?);
                let (mut x, mut y) = (toks.num()?, toks.num()?);
                if rel {
                    x2 += cx;
                    y2 += cy;
                    x += cx;
                    y += cy;
                }
                // reflect the previous cubic's 2nd control about the current point
                let (x1, y1) = match prev_ctrl {
                    Some((px, py)) => (2.0 * cx - px, 2.0 * cy - py),
                    None => (cx, cy),
                };
                b.cubic_bezier_to(point(x1, y1), point(x2, y2), point(x, y));
                prev_ctrl = Some((x2, y2));
                cx = x;
                cy = y;
            }
            'Z' => {
                if open {
                    b.end(true);
                    open = false;
                }
                cx = sx;
                cy = sy;
                prev_ctrl = None;
            }
            _ => return None, // unsupported command
        }
        if next.is_none() && toks.at_end() {
            break;
        }
    }
    if open {
        b.end(false);
    }
    Some(b.build())
}

/// A tiny tokenizer over an SVG path `d`: yields either a command letter or a
/// number, skipping the whitespace and commas SVG allows between them.
struct Tokens<'a> {
    bytes: &'a [u8],
    i: usize,
}

impl<'a> Tokens<'a> {
    fn new(s: &'a str) -> Self {
        Tokens {
            bytes: s.as_bytes(),
            i: 0,
        }
    }

    fn skip_sep(&mut self) {
        while self.i < self.bytes.len() {
            let c = self.bytes[self.i];
            if c == b' ' || c == b',' || c == b'\t' || c == b'\n' || c == b'\r' {
                self.i += 1;
            } else {
                break;
            }
        }
    }

    fn at_end(&mut self) -> bool {
        self.skip_sep();
        self.i >= self.bytes.len()
    }

    /// If the next token is a command letter, return it (without consuming).
    fn peek_cmd(&mut self) -> Option<char> {
        self.skip_sep();
        if self.i < self.bytes.len() {
            let c = self.bytes[self.i];
            if c.is_ascii_alphabetic() {
                return Some(c as char);
            }
        }
        None
    }

    fn bump(&mut self) {
        self.i += 1;
    }

    /// Parse the next number (SVG allows a leading sign, decimals, exponents,
    /// and a `-` immediately after a digit starting a new number).
    fn num(&mut self) -> Option<f32> {
        self.skip_sep();
        let start = self.i;
        let bytes = self.bytes;
        if self.i < bytes.len() && (bytes[self.i] == b'+' || bytes[self.i] == b'-') {
            self.i += 1;
        }
        let mut seen_digit = false;
        while self.i < bytes.len() && bytes[self.i].is_ascii_digit() {
            self.i += 1;
            seen_digit = true;
        }
        if self.i < bytes.len() && bytes[self.i] == b'.' {
            self.i += 1;
            while self.i < bytes.len() && bytes[self.i].is_ascii_digit() {
                self.i += 1;
                seen_digit = true;
            }
        }
        if seen_digit && self.i < bytes.len() && (bytes[self.i] == b'e' || bytes[self.i] == b'E') {
            self.i += 1;
            if self.i < bytes.len() && (bytes[self.i] == b'+' || bytes[self.i] == b'-') {
                self.i += 1;
            }
            while self.i < bytes.len() && bytes[self.i].is_ascii_digit() {
                self.i += 1;
            }
        }
        if !seen_digit {
            return None;
        }
        std::str::from_utf8(&bytes[start..self.i])
            .ok()?
            .parse()
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn affine_composition_matches_manual() {
        let a = Affine {
            tx: 10.0,
            ty: 20.0,
            sx: 2.0,
            sy: -2.0,
        };
        let b = Affine {
            tx: 1.0,
            ty: 1.0,
            sx: 0.5,
            sy: 0.5,
        };
        let c = a.then(b);
        // c should map (4,4) the same as a(b(4,4))
        let manual = a.apply(b.apply(4.0, 4.0)[0], b.apply(4.0, 4.0)[1]);
        assert_eq!(c.apply(4.0, 4.0), manual);
    }

    #[test]
    fn fit_centres_and_preserves_aspect() {
        let data = ScoreData {
            vb_w: 100.0,
            vb_h: 50.0,
            ..Default::default()
        };
        let fit = data.fit(Rect::new(0.0, 0.0, 200.0, 200.0));
        // width-bound: scale 2, page height 100 centred in 200 -> ty = 50
        assert_eq!(fit.sx, 2.0);
        assert_eq!(fit.sy, 2.0);
        assert_eq!(fit.ty, 50.0);
        assert_eq!(fit.tx, 0.0);
    }

    #[test]
    fn parses_absolute_moveto_lineto_close() {
        // a unit triangle
        let p = build_path("M0 0 L10 0 L0 10 Z").expect("path");
        // lyon builds it; a filled triangle tessellates to >=1 triangle
        let mut mesh = Mesh::new();
        let mut tess = FillTessellator::new();
        fill_path(
            &mut mesh,
            &mut tess,
            "M0 0 L10 0 L0 10 Z",
            Affine::IDENTITY,
            0.1,
            [1.0; 4],
        );
        assert!(
            !mesh.is_empty(),
            "closed triangle should tessellate to geometry"
        );
        drop(p);
    }

    #[test]
    fn parses_relative_cubic_like_a_glyph_outline() {
        // the notehead U+E0A4 outline shape (relative cubics), from verovio
        let d = "M0 -39c0 68 73 172 200 172c66 0 114 -37 114 -95c0 -84 -106 -171 -218 -171c-58 0 -96 34 -96 93Z";
        let mut mesh = Mesh::new();
        let mut tess = FillTessellator::new();
        fill_path(&mut mesh, &mut tess, d, Affine::IDENTITY, 0.5, [1.0; 4]);
        assert!(
            !mesh.is_empty(),
            "glyph outline should tessellate to geometry"
        );
    }

    #[test]
    fn malformed_path_is_skipped_not_panicked() {
        let mut mesh = Mesh::new();
        let mut tess = FillTessellator::new();
        fill_path(
            &mut mesh,
            &mut tess,
            "M0 0 Q nonsense",
            Affine::IDENTITY,
            0.5,
            [1.0; 4],
        );
        assert!(mesh.is_empty());
    }

    #[test]
    fn parses_display_list_props_and_renders() {
        // the shape the Python `notation.engrave` builder sends: a glyph table,
        // a placed glyph (flip folded into a negative sy) and a staff line.
        let props: Map<String, Value> = serde_json::from_str(
            r#"{
                "vb": [1000, 400],
                "glyphs": {"E0A4": "M0 -39c0 68 73 172 200 172c66 0 114 -37 114 -95c0 -84 -106 -171 -218 -171c-58 0 -96 34 -96 93Z"},
                "prims": [
                    {"k": "glyph", "cp": "E0A4", "xf": [500, 200, 0.72, -0.72], "id": "n1"},
                    {"k": "line", "pts": [[0, 100], [1000, 100]], "w": 13, "id": "s1"}
                ]
            }"#,
        )
        .unwrap();
        let data = ScoreData::parse(&props);
        assert_eq!(data.glyphs.len(), 1);
        assert_eq!(data.prims.len(), 2);
        assert_eq!(data.prims[0].id(), Some("n1"));
        let mut mesh = Mesh::new();
        data.render(
            &mut mesh,
            Rect::new(0.0, 0.0, 500.0, 200.0),
            None,
            [1.0; 4],
            [1.0; 4],
        );
        assert!(
            !mesh.is_empty(),
            "a parsed score should tessellate to geometry"
        );
    }

    #[test]
    fn parses_and_renders_text_prim() {
        let props: Map<String, Value> = serde_json::from_str(
            r#"{"vb":[1000,400],"glyphs":{},
                "prims":[{"k":"text","s":"mf","x":500,"y":200,"size":80,"id":"d1"}]}"#,
        )
        .unwrap();
        let data = ScoreData::parse(&props);
        assert_eq!(data.prims.len(), 1);
        assert_eq!(data.prims[0].id(), Some("d1"));
        let mut mesh = Mesh::new();
        data.render(
            &mut mesh,
            Rect::new(0.0, 0.0, 500.0, 200.0),
            None,
            [1.0; 4],
            [1.0; 4],
        );
        assert!(
            !mesh.is_empty(),
            "score text should paint through the host font"
        );
    }

    #[test]
    fn playhead_selects_the_active_cursor_and_draws() {
        let props: Map<String, Value> = serde_json::from_str(
            r#"{"vb":[1000,400],"glyphs":{},"prims":[],
                "cursors":[{"t":0,"x":100,"y0":50,"y1":150},
                           {"t":500,"x":300,"y0":50,"y1":150},
                           {"t":1000,"x":500,"y0":50,"y1":150}],
                "playhead":600}"#,
        )
        .unwrap();
        let data = ScoreData::parse(&props);
        assert_eq!(data.cursors.len(), 3);
        assert_eq!(data.playhead, 600.0);
        // active cursor at t=600 is the one at t=500 (x=300)
        let fit = data.fit(Rect::new(0.0, 0.0, 1000.0, 400.0));
        let idx = data
            .cursors
            .partition_point(|c| c.t <= data.playhead)
            .saturating_sub(1);
        assert_eq!(data.cursors[idx].x, 300.0);
        let mut mesh = Mesh::new();
        data.render(
            &mut mesh,
            Rect::new(0.0, 0.0, 1000.0, 400.0),
            None,
            [1.0; 4],
            [1.0; 4],
        );
        assert!(!mesh.is_empty(), "the playhead line should paint");
        let _ = fit;
    }

    #[test]
    fn playhead_off_when_negative() {
        let mut data = ScoreData {
            vb_w: 1000.0,
            vb_h: 400.0,
            ..Default::default()
        };
        data.cursors.push(Cursor {
            t: 0.0,
            x: 100.0,
            y0: 50.0,
            y1: 150.0,
        });
        data.playhead = -1.0;
        let mut mesh = Mesh::new();
        data.render(
            &mut mesh,
            Rect::new(0.0, 0.0, 1000.0, 400.0),
            None,
            [1.0; 4],
            [1.0; 4],
        );
        assert!(mesh.is_empty(), "no playhead when time is negative");
    }

    #[test]
    fn line_prim_renders_within_clip() {
        let mut data = ScoreData {
            vb_w: 100.0,
            vb_h: 100.0,
            ..Default::default()
        };
        data.prims.push(Prim::Line {
            pts: vec![[0.0, 50.0], [100.0, 50.0]],
            width: 2.0,
            id: None,
        });
        let mut mesh = Mesh::new();
        data.render(
            &mut mesh,
            Rect::new(0.0, 0.0, 200.0, 200.0),
            None,
            [1.0; 4],
            [1.0; 4],
        );
        assert!(!mesh.is_empty(), "a staff line should paint");
    }
}
