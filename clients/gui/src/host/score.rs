//! The `score` widget's renderer: a verovio display list -> triangle mesh.
//!
//! Music notation is vector art — SMuFL glyph outlines (noteheads, clefs,
//! rests, accidentals, flags) plus engraving strokes and fills (staff lines,
//! stems, ledger lines, beams, slurs, ties). None of it is data-viz, so it does
//! not get its own GPU pipeline: every primitive is tessellated into the same
//! flat-colored [`Mesh`] the rest of the chrome uses, which
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

    /// The inverse map, or `None` when a scale collapsed to zero (a degenerate
    /// transform has no point to map back to).
    pub fn invert(self) -> Option<Affine> {
        if self.sx == 0.0 || self.sy == 0.0 {
            return None;
        }
        Some(Affine {
            tx: -self.tx / self.sx,
            ty: -self.ty / self.sy,
            sx: 1.0 / self.sx,
            sy: 1.0 / self.sy,
        })
    }
}

/// An axis-aligned page-unit box, with `x0 <= x1` and `y0 <= y1`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Bounds {
    /// The box `xf` maps this one onto — still axis-aligned, since `xf` only
    /// translates and scales; a negative scale flips it, so the corners are
    /// re-ordered.
    fn transformed(self, xf: Affine) -> Bounds {
        let [ax, ay] = xf.apply(self.x0, self.y0);
        let [bx, by] = xf.apply(self.x1, self.y1);
        Bounds {
            x0: ax.min(bx),
            y0: ay.min(by),
            x1: ax.max(bx),
            y1: ay.max(by),
        }
    }

    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }

    fn area(&self) -> f32 {
        (self.x1 - self.x0) * (self.y1 - self.y0)
    }

    fn grown(self, m: f32) -> Bounds {
        Bounds {
            x0: self.x0 - m,
            y0: self.y0 - m,
            x1: self.x1 + m,
            y1: self.y1 + m,
        }
    }
}

/// One entry of the hit-testing index: the page-unit extent of an identified
/// primitive, paired with the MEI `xml:id` it was engraved from.
#[derive(Clone, Debug)]
pub struct HitBox {
    pub id: String,
    pub bounds: Bounds,
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

/// The pitch drag in flight: the element being dragged and how many diatonic
/// steps **up** the gesture has moved it so far (negative = down). The page is
/// drawn with that element displaced, so the drag reads as notation while it
/// happens; the release sends the steps to the client, which owns the score and
/// answers with a re-engraved page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScoreDrag {
    pub id: String,
    pub steps: i32,
}

/// One engraved staff: the page-y of its top and bottom lines and the width
/// they are stroked with. Derived from the drawing (the wide horizontal lines,
/// clustered by system), because a pitch dragged off the staff needs ledger
/// lines and only the staff says where they go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Staff {
    pub y0: f32,
    pub y1: f32,
    pub width: f32,
}

/// The default page units per diatonic step: verovio's default `unit` (9) times
/// its definition factor (10). Used when a display list names no `step` — every
/// display list `clausters.gui.notation` builds does.
pub const STEP: f32 = 90.0;

/// A fully engraved page ready to render: the definition viewBox (for fitting),
/// the glyph-outline table (deduplicated by codepoint), and the placed
/// primitives.
#[derive(Clone, Debug)]
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
    /// A **static** playback time in ms, set with `/gui_set playhead`; negative
    /// means none. It stands still — a stopped transport located on a note keeps
    /// its cursor there, and it must not drift with the engine clock.
    pub playhead: f32,
    /// The playhead origin: the engine sample-clock value at score time 0
    /// (negative = not playing, fall back to the static `playhead`). Set once at
    /// the start of a pass and the cursor then *sweeps* on its own — the host
    /// reads the clock every frame, so playback needs zero messages.
    pub playhead_at: f64,
    /// The sample rate converting the clock to musical ms (0 = unknown, use the
    /// server's own rate).
    pub sample_rate: f64,
    /// The hit-testing index: the page-unit extent of every identified
    /// primitive, derived from `prims` and `glyphs` when the display list is
    /// parsed (see [`ScoreData::index`]).
    pub hits: Vec<HitBox>,
    /// The engraved staves, top to bottom — derived with the hit index, and
    /// what tells a dragged pitch when it has left the staff.
    pub staves: Vec<Staff>,
    /// The selected element's MEI `xml:id`, drawn highlighted; `None` = nothing
    /// selected. Set by a click on the page and by `/gui_set selected`.
    pub selected: Option<String>,
    /// Page units per **diatonic step** — half the staff-line spacing, the
    /// quantum a pitch drag counts in. It comes from the client with the page
    /// (it depends on verovio's `unit` option, not on the staff scale), so the
    /// host quantizes exactly what the engraver drew.
    pub step: f32,
    /// The pitch drag in flight, drawn as a displacement of its element. It
    /// stands after the release until the client sends the re-engraved page:
    /// the answer is one message away, and snapping back first would show the
    /// old pitch for a frame.
    pub drag: Option<ScoreDrag>,
    /// Whether a drag on an element **edits** it (a pitch drag → `"transpose"`).
    /// Off by default: a score is a view, and the host holds no score, so an
    /// edit the client will not apply is a gesture that cannot be fulfilled — an
    /// editor opts in (`editable: true`). Selection and the `"element"` click are
    /// not gated by this: inspecting a read-only page is not editing it.
    pub editable: bool,
}

impl Default for ScoreData {
    fn default() -> ScoreData {
        ScoreData {
            vb_w: 0.0,
            vb_h: 0.0,
            glyphs: HashMap::new(),
            prims: Vec::new(),
            cursors: Vec::new(),
            playhead: -1.0,
            playhead_at: -1.0,
            sample_rate: 0.0,
            hits: Vec::new(),
            staves: Vec::new(),
            selected: None,
            step: STEP,
            drag: None,
            editable: false,
        }
    }
}

/// The three roles a score paints in: the engraving ink, the playback cursor
/// and the selection highlight — bundled so the theme travels as one argument.
#[derive(Clone, Copy, Debug)]
pub struct ScoreColors {
    pub ink: Color,
    pub playhead: Color,
    pub selection: Color,
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
        // A playhead is off unless the client sets a non-negative time (static)
        // or anchors one to the engine clock (sweeping).
        data.playhead = props
            .get("playhead")
            .and_then(Value::as_f64)
            .map(|f| f as f32)
            .unwrap_or(-1.0);
        data.playhead_at = props
            .get("playhead_at")
            .and_then(Value::as_f64)
            .unwrap_or(-1.0);
        data.sample_rate = props
            .get("sample_rate")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        data.selected = props
            .get("selected")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        data.step = props
            .get("step")
            .and_then(Value::as_f64)
            .map(|s| s as f32)
            .filter(|s| *s > 0.0)
            .unwrap_or(STEP);
        data.editable = props
            .get("editable")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        data.index();
        data
    }

    /// Rebuild the hit-testing index from the placed primitives: one page-unit
    /// box per identified primitive, so a click can name the element under it.
    /// Done once when the display list arrives — the geometry never moves
    /// afterwards, and re-deriving it per click would mean re-parsing every
    /// glyph outline on every press.
    pub fn index(&mut self) {
        // glyph outlines repeat all over a page (one notehead shape, hundreds of
        // notes), so each codepoint's local extent is measured once.
        let mut local: HashMap<u32, Option<Bounds>> = HashMap::new();
        self.hits.clear();
        for prim in &self.prims {
            let Some(id) = prim.id() else { continue };
            let bounds = match prim {
                Prim::Glyph { cp, xf, .. } => {
                    let b = *local
                        .entry(*cp)
                        .or_insert_with(|| self.glyphs.get(cp).and_then(|d| path_bounds(d)));
                    b.map(|b| b.transformed(*xf))
                }
                Prim::Fill { d, xf, .. } => path_bounds(d).map(|b| b.transformed(*xf)),
                Prim::Line { pts, width, .. } => {
                    let mut b = Bounds {
                        x0: f32::MAX,
                        y0: f32::MAX,
                        x1: f32::MIN,
                        y1: f32::MIN,
                    };
                    for p in pts {
                        b.x0 = b.x0.min(p[0]);
                        b.y0 = b.y0.min(p[1]);
                        b.x1 = b.x1.max(p[0]);
                        b.y1 = b.y1.max(p[1]);
                    }
                    // a stroke is a hairline in one axis: give it its width.
                    Some(b.grown(width * 0.5))
                }
                Prim::Text { s, x, y, size, .. } => Some(Bounds {
                    x0: *x,
                    // the host font is roughly 0.6 em wide per character
                    x1: x + 0.6 * size * s.chars().count() as f32,
                    y0: y - size,
                    y1: *y,
                }),
            };
            if let Some(bounds) = bounds {
                self.hits.push(HitBox {
                    id: id.to_string(),
                    bounds,
                });
            }
        }
        self.index_staves();
    }

    /// Cluster the staff lines into staves. A staff line is the one primitive
    /// every system draws the same way — a horizontal stroke running most of
    /// the page — and within a staff they sit exactly one space (two diatonic
    /// steps) apart, so a wider gap starts the next system.
    fn index_staves(&mut self) {
        let mut lines: Vec<(f32, f32)> = self
            .prims
            .iter()
            .filter_map(|p| match p {
                Prim::Line { pts, width, .. }
                    if pts.len() == 2
                        && (pts[0][1] - pts[1][1]).abs() < 1.0
                        && (pts[0][0] - pts[1][0]).abs() > 0.3 * self.vb_w =>
                {
                    Some((pts[0][1], *width))
                }
                _ => None,
            })
            .collect();
        lines.sort_by(|a, b| a.0.total_cmp(&b.0));
        lines.dedup_by(|a, b| (a.0 - b.0).abs() < 0.5);
        self.staves.clear();
        let gap = 2.5 * self.step;
        let mut group: Option<Staff> = None;
        for (y, width) in lines {
            match &mut group {
                Some(s) if y - s.y1 <= gap => s.y1 = y,
                other => {
                    if let Some(s) = other.take() {
                        self.staves.push(s);
                    }
                    *other = Some(Staff {
                        y0: y,
                        y1: y,
                        width,
                    });
                }
            }
        }
        self.staves.extend(group);
    }

    /// The staff a page-y belongs to: the nearest one, since a note off the
    /// staff still belongs to it (that is what ledger lines are for).
    pub fn staff_at(&self, y: f32) -> Option<Staff> {
        self.staves
            .iter()
            .copied()
            .min_by(|a, b| staff_distance(a, y).total_cmp(&staff_distance(b, y)))
    }

    /// The ledger lines a notehead centred on page-y `y` needs on `staff`,
    /// outward from it — empty while the note is on the staff. One line per
    /// whole line position past the staff's own, and a note in the space
    /// *beyond* the last one gains no further line: the engraving rule, and the
    /// reason this is not simply "one line per step".
    pub fn ledger_ys(&self, staff: Staff, y: f32) -> Vec<f32> {
        let space = 2.0 * self.step;
        let (mut ly, dir) = if y < staff.y0 {
            (staff.y0 - space, -1.0)
        } else {
            (staff.y1 + space, 1.0)
        };
        let mut out = Vec::new();
        // the cap keeps a degenerate page (or a drag off into nowhere) finite
        while (y - ly) * dir >= -0.01 && out.len() < 32 {
            out.push(ly);
            ly += dir * space;
        }
        out
    }

    /// The MEI `xml:id` of the element under the screen point `(x, y)`, with the
    /// page fitted into `rect` — the smallest box containing it, so a notehead
    /// wins over the staff line it sits on. `None` when the click lands on blank
    /// paper.
    pub fn hit(&self, rect: Rect, x: f32, y: f32) -> Option<&str> {
        let inv = self.fit(rect).invert()?;
        let [px, py] = inv.apply(x, y);
        self.hits
            .iter()
            .filter(|h| h.bounds.contains(px, py))
            .min_by(|a, b| {
                a.bounds
                    .area()
                    .partial_cmp(&b.bounds.area())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|h| h.id.as_str())
    }

    /// The musical time (ms) the cursor sits at this frame: the engine clock
    /// mapped through the `playhead_at` origin while a pass is playing, else the
    /// static `playhead`. `sample_clock` is the engine's sample count and
    /// `host_rate` the server's sample rate, used when the widget names none.
    pub fn head_ms(&self, sample_clock: f64, host_rate: f64) -> f32 {
        let rate = if self.sample_rate > 0.0 {
            self.sample_rate
        } else {
            host_rate
        };
        if self.playhead_at >= 0.0 && sample_clock > 0.0 && rate > 0.0 {
            (((sample_clock - self.playhead_at) / rate) * 1000.0) as f32
        } else {
            self.playhead
        }
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

    /// How many **diatonic steps** a vertical drag of `dy` screen pixels
    /// amounts to, with the page fitted into `rect`: dragging up (a negative
    /// `dy`) is positive, and the result is whole steps — a pitch has no
    /// in-between position, so the gesture quantizes rather than the client.
    pub fn steps_for(&self, rect: Rect, dy: f32) -> i32 {
        let px = self.step * self.fit(rect).sy;
        if px <= 0.0 {
            return 0;
        }
        (-dy / px).round() as i32
    }

    /// The ledger lines the drag preview owes the page: where the displaced
    /// notehead needs them, how wide, and the box whose engraved ones it
    /// replaces. `None` when nothing is being dragged, the element is not on a
    /// staff, or it left no measurable mark.
    fn drag_ledgers(&self) -> Option<Ledgers> {
        let drag = self.drag.as_ref()?;
        // the element's first primitive is its notehead (verovio draws it
        // before the stem), which is what a ledger line is centred on and sized
        // from — the stem and flag would stretch the box out of shape.
        let head = self.hits.iter().find(|h| h.id == drag.id)?.bounds;
        let y = 0.5 * (head.y0 + head.y1);
        let staff = self.staff_at(y)?;
        let pad = LEDGER_OVERHANG * (head.x1 - head.x0);
        Some(Ledgers {
            ys: self.ledger_ys(staff, y - drag.steps as f32 * self.step),
            x0: head.x0 - pad,
            x1: head.x1 + pad,
            width: staff.width * LEDGER_WEIGHT,
            staff,
        })
    }

    /// `fit`, shifted by the drag preview when `id` is the element being
    /// dragged — the one place the displacement enters the drawing, so every
    /// primitive of a note (notehead, stem, dots) travels with it.
    fn prim_fit(&self, fit: Affine, id: Option<&str>) -> Affine {
        match &self.drag {
            // page y grows downward, so a step up is a negative offset
            Some(d) if id == Some(d.id.as_str()) => Affine {
                ty: fit.ty - fit.sy * d.steps as f32 * self.step,
                ..fit
            },
            _ => fit,
        }
    }

    /// Tessellate the whole page into `mesh`, mapped into `rect` by [`fit`] and
    /// painted in `colors`: the engraving in the ink, the selected element
    /// highlighted under it, and the playback cursor over it at musical time
    /// `head` ms (negative = none, as returned by [`head_ms`]). Geometry is
    /// clipped to `rect` intersected with the caller's `clip` (the enclosing
    /// scroll area, if any); the caller's clip is restored on return so the
    /// surrounding frame pass is unaffected.
    ///
    /// [`fit`]: ScoreData::fit
    /// [`head_ms`]: ScoreData::head_ms
    pub fn render(
        &self,
        mesh: &mut Mesh,
        rect: Rect,
        clip: Option<Rect>,
        head: f32,
        colors: ScoreColors,
    ) {
        let color = colors.ink;
        let fit = self.fit(rect);
        mesh.set_clip(Some(intersect(rect, clip)));
        // Curve-flattening tolerance in page units so it lands ~1/3 device
        // pixel after fitting — fine enough to read smooth, coarse enough to
        // keep the triangle count bounded by the screen, not the notation.
        let tol_page = 0.33 / fit.sx.max(f32::MIN_POSITIVE);
        // under the ink, so the engraving still reads through the highlight
        self.draw_selection(mesh, fit, colors.selection);
        // A dragged notehead takes its ledger lines with it: the engraved ones
        // stay where the staff put them, so they are dropped and re-derived at
        // the displaced pitch — which is also how they disappear when the note
        // comes back onto the staff.
        let ledgers = self.drag_ledgers();
        if let Some(l) = &ledgers {
            let w = (l.width * fit.sx).max(1.0);
            for y in &l.ys {
                mesh.line(fit.apply(l.x0, *y), fit.apply(l.x1, *y), w, color);
            }
        }
        let mut tess = FillTessellator::new();
        for prim in &self.prims {
            if ledgers.as_ref().is_some_and(|l| l.covers(prim, self.vb_w)) {
                continue;
            }
            // the page fit, displaced while this element is being dragged
            let fit = self.prim_fit(fit, prim.id());
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
        self.draw_playhead(mesh, fit, head, colors.playhead);
        mesh.set_clip(clip);
    }

    /// Highlight every primitive of the selected element — one MEI id can own
    /// several (a note is a notehead plus its stem), so the whole gesture of it
    /// lights up rather than one glyph of it.
    fn draw_selection(&self, mesh: &mut Mesh, fit: Affine, color: Color) {
        let Some(sel) = self.selected.as_deref() else {
            return;
        };
        let fit = self.prim_fit(fit, Some(sel));
        for h in self.hits.iter().filter(|h| h.id == sel) {
            // a hair of page-unit padding so a hairline stem still shows a band
            let b = h.bounds.grown(20.0).transformed(fit);
            mesh.rect(
                Rect::new(b.x0, b.y0, b.x1 - b.x0, b.y1 - b.y0),
                super::theme::with_alpha(color, 0.30),
            );
        }
    }

    /// Draw the playback cursor at musical time `head` (ms): the vertical
    /// staff-spanning line of the latest cursor at or before it. A no-op when no
    /// playhead is set or no timemap was sent.
    fn draw_playhead(&self, mesh: &mut Mesh, fit: Affine, head: f32, color: Color) {
        if head < 0.0 || self.cursors.is_empty() {
            return;
        }
        // the cursor active at `head`: the last one whose time is <= it.
        let idx = self
            .cursors
            .partition_point(|c| c.t <= head)
            .saturating_sub(1);
        let c = self.cursors[idx.min(self.cursors.len() - 1)];
        // points are already in screen pixels after `fit`, so width is px.
        mesh.line(fit.apply(c.x, c.y0), fit.apply(c.x, c.y1), 2.0, color);
    }
}

/// A ledger line reaches past the notehead by about a fifth of its width on
/// each side, and is stroked heavier than a staff line — verovio's proportions,
/// so a previewed ledger sits among the engraved ones without looking foreign.
const LEDGER_OVERHANG: f32 = 0.22;
const LEDGER_WEIGHT: f32 = 1.7;

/// What the drag preview owes the page in ledger lines: `ys` to draw across
/// `x0..x1`, and the `staff` they belong to — which is also what identifies the
/// engraved ledger lines the dragged notehead is leaving behind.
struct Ledgers {
    ys: Vec<f32>,
    x0: f32,
    x1: f32,
    width: f32,
    staff: Staff,
}

impl Ledgers {
    /// Whether this primitive is a ledger line of the dragged notehead — a
    /// short horizontal stroke off the staff, over the notehead's own column.
    /// The engraver draws them per staff, not inside the note, so they carry
    /// the staff's id and cannot travel with it: they are dropped from the
    /// drawing and re-derived at the displaced position instead.
    fn covers(&self, prim: &Prim, vb_w: f32) -> bool {
        let Prim::Line { pts, .. } = prim else {
            return false;
        };
        if pts.len() != 2 || (pts[0][1] - pts[1][1]).abs() > 1.0 {
            return false;
        }
        let (x0, x1) = (pts[0][0].min(pts[1][0]), pts[0][0].max(pts[1][0]));
        let y = pts[0][1];
        (x1 - x0) < 0.15 * vb_w
            && staff_distance(&self.staff, y) > 0.0
            && x0 < self.x1
            && x1 > self.x0
    }
}

/// How far a page-y sits outside a staff (zero anywhere between its lines).
fn staff_distance(staff: &Staff, y: f32) -> f32 {
    (staff.y0 - y).max(y - staff.y1).max(0.0)
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

/// The extent of an SVG path `d` in its own coordinates, from the bezier control
/// hull — a slight over-estimate of the true curve extent, which is what a hit
/// target wants anyway (a click just off a notehead's edge still names it).
fn path_bounds(d: &str) -> Option<Bounds> {
    let path = build_path(d)?;
    let b = lyon::algorithms::aabb::fast_bounding_box(&path);
    Some(Bounds {
        x0: b.min.x,
        y0: b.min.y,
        x1: b.max.x,
        y1: b.max.y,
    })
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

    /// One opaque palette for the render tests: they assert that geometry
    /// lands, not which role painted it.
    const INK: ScoreColors = ScoreColors {
        ink: [1.0; 4],
        playhead: [1.0; 4],
        selection: [1.0; 4],
    };

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
            -1.0,
            INK,
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
            -1.0,
            INK,
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
            data.head_ms(0.0, 0.0),
            INK,
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
            data.head_ms(0.0, 0.0),
            INK,
        );
        assert!(mesh.is_empty(), "no playhead when time is negative");
    }

    #[test]
    fn playhead_at_sweeps_off_the_engine_clock() {
        let props: Map<String, Value> = serde_json::from_str(
            r#"{"vb":[1000,400],"glyphs":{},"prims":[],
                "cursors":[{"t":0,"x":100,"y0":50,"y1":150},
                           {"t":500,"x":300,"y0":50,"y1":150}],
                "playhead":250,"playhead_at":48000,"sample_rate":48000}"#,
        )
        .unwrap();
        let data = ScoreData::parse(&props);
        // one second of clock past the origin is one second of musical time
        assert_eq!(data.head_ms(96_000.0, 44_100.0), 1000.0);
        // the widget's own rate wins over the server's; without one, the
        // server's places the time
        let host_rate = ScoreData {
            sample_rate: 0.0,
            ..data.clone()
        };
        assert_eq!(host_rate.head_ms(96_000.0, 24_000.0), 2000.0);
        // not playing: the static time stands still whatever the clock says
        let stopped = ScoreData {
            playhead_at: -1.0,
            ..data.clone()
        };
        assert_eq!(stopped.head_ms(96_000.0, 48_000.0), 250.0);
        // a rate nobody knows leaves the static time in place too
        let no_rate = ScoreData {
            sample_rate: 0.0,
            ..data
        };
        assert_eq!(no_rate.head_ms(96_000.0, 0.0), 250.0);
    }

    /// A page with a notehead glyph at (500, 200) sitting on a full-width staff
    /// line — the two overlapping hit targets a click has to choose between.
    fn indexed_page() -> ScoreData {
        let props: Map<String, Value> = serde_json::from_str(
            r#"{
                "vb": [1000, 400],
                "glyphs": {"E0A4": "M0 -39c0 68 73 172 200 172c66 0 114 -37 114 -95c0 -84 -106 -171 -218 -171c-58 0 -96 34 -96 93Z"},
                "prims": [
                    {"k": "line", "pts": [[0, 200], [1000, 200]], "w": 13, "id": "staff"},
                    {"k": "glyph", "cp": "E0A4", "xf": [500, 200, 0.72, -0.72], "id": "n1"}
                ]
            }"#,
        )
        .unwrap();
        ScoreData::parse(&props)
    }

    #[test]
    fn a_click_names_the_smallest_element_under_it() {
        let data = indexed_page();
        assert_eq!(data.hits.len(), 2);
        // fitted 1:1 (a 1000x400 page into a 1000x400 rect), so page == screen
        let rect = Rect::new(0.0, 0.0, 1000.0, 400.0);
        assert_eq!(data.fit(rect).sx, 1.0);
        // over the notehead, where both boxes overlap: the note wins
        assert_eq!(data.hit(rect, 550.0, 190.0), Some("n1"));
        // on the staff line away from the note: only the line is there
        assert_eq!(data.hit(rect, 100.0, 200.0), Some("staff"));
        // blank paper names nothing
        assert_eq!(data.hit(rect, 100.0, 380.0), None);
    }

    #[test]
    fn hit_testing_follows_the_page_fit() {
        let data = indexed_page();
        // half scale, so the notehead's page x=500 lands at screen x=250
        let rect = Rect::new(0.0, 0.0, 500.0, 200.0);
        assert_eq!(data.fit(rect).sx, 0.5);
        assert_eq!(data.hit(rect, 275.0, 95.0), Some("n1"));
        // the same screen point on the unscaled page is blank paper
        assert_eq!(
            data.hit(Rect::new(0.0, 0.0, 1000.0, 400.0), 275.0, 95.0),
            None
        );
    }

    #[test]
    fn a_selected_element_is_highlighted() {
        let props: Map<String, Value> = serde_json::from_str(
            r#"{"vb":[1000,400],"glyphs":{},
                "prims":[{"k":"line","pts":[[0,200],[1000,200]],"w":13,"id":"staff"}],
                "selected":"staff"}"#,
        )
        .unwrap();
        let mut data = ScoreData::parse(&props);
        assert_eq!(data.selected.as_deref(), Some("staff"));
        let rect = Rect::new(0.0, 0.0, 1000.0, 400.0);
        let mut with = Mesh::new();
        data.render(&mut with, rect, None, -1.0, INK);
        data.selected = None;
        let mut without = Mesh::new();
        data.render(&mut without, rect, None, -1.0, INK);
        assert!(
            with.positions().count() > without.positions().count(),
            "the highlight should add geometry over the engraving"
        );
    }

    #[test]
    fn a_vertical_drag_counts_whole_diatonic_steps() {
        let data = indexed_page();
        // fitted 1:1, so a step is the page's own 90 units
        let rect = Rect::new(0.0, 0.0, 1000.0, 400.0);
        assert_eq!(data.step, STEP);
        // up the staff is up in pitch, and the count rounds to whole steps
        assert_eq!(data.steps_for(rect, -90.0), 1);
        assert_eq!(data.steps_for(rect, -44.0), 0);
        assert_eq!(data.steps_for(rect, -46.0), 1);
        assert_eq!(data.steps_for(rect, 270.0), -3);
        // half the scale halves the pixels a step takes
        assert_eq!(data.steps_for(Rect::new(0.0, 0.0, 500.0, 200.0), -90.0), 2);
    }

    #[test]
    fn a_dragged_element_is_drawn_displaced() {
        let mut data = indexed_page();
        let rect = Rect::new(0.0, 0.0, 1000.0, 400.0);
        let fit = data.fit(rect);
        // the note travels a step up; the staff line it sat on does not move
        data.drag = Some(ScoreDrag {
            id: "n1".into(),
            steps: 1,
        });
        assert_eq!(data.prim_fit(fit, Some("n1")).ty, fit.ty - STEP);
        assert_eq!(data.prim_fit(fit, Some("staff")).ty, fit.ty);
        assert_eq!(data.prim_fit(fit, None).ty, fit.ty);
        // and down again the other way
        data.drag = Some(ScoreDrag {
            id: "n1".into(),
            steps: -2,
        });
        assert_eq!(data.prim_fit(fit, Some("n1")).ty, fit.ty + 2.0 * STEP);
    }

    #[test]
    fn the_drag_moves_the_drawn_geometry_by_whole_steps() {
        // one notehead alone on the page, so every triangle drawn is its own
        let mut data = ScoreData {
            vb_w: 1000.0,
            vb_h: 400.0,
            ..indexed_page()
        };
        data.prims.retain(|p| p.id() == Some("n1"));
        let rect = Rect::new(0.0, 0.0, 1000.0, 400.0); // fitted 1:1
        let top = |data: &ScoreData| {
            let mut mesh = Mesh::new();
            data.render(&mut mesh, rect, None, -1.0, INK);
            mesh.positions().map(|p| p.1).fold(f32::MAX, f32::min)
        };
        let before = top(&data);
        // down the staff, which on the page is down in y (and keeps the whole
        // notehead inside the page, where the render clips)
        data.drag = Some(ScoreDrag {
            id: "n1".into(),
            steps: -2,
        });
        assert!((top(&data) - (before + 2.0 * STEP)).abs() < 0.01);
    }

    /// A treble staff with a middle C below it, in verovio's own numbers (the
    /// engraved page of `4CDEF/` at scale 40): five lines 180 apart, the
    /// notehead a whole line position below the last, and the ledger line the
    /// engraver drew for it — tagged with the staff, as verovio tags it.
    fn staffed_page() -> ScoreData {
        let props: Map<String, Value> = serde_json::from_str(
            r#"{
                "vb": [11000, 3000], "step": 90,
                "glyphs": {"E0A4": "M0 -39c0 68 73 172 200 172c66 0 114 -37 114 -95c0 -84 -106 -171 -218 -171c-58 0 -96 34 -96 93Z"},
                "prims": [
                    {"k": "line", "pts": [[600, 1040], [10400, 1040]], "w": 13, "id": "staff"},
                    {"k": "line", "pts": [[600, 1220], [10400, 1220]], "w": 13, "id": "staff"},
                    {"k": "line", "pts": [[600, 1400], [10400, 1400]], "w": 13, "id": "staff"},
                    {"k": "line", "pts": [[600, 1580], [10400, 1580]], "w": 13, "id": "staff"},
                    {"k": "line", "pts": [[600, 1760], [10400, 1760]], "w": 13, "id": "staff"},
                    {"k": "glyph", "cp": "E0A4", "xf": [1783, 1940, 0.72, -0.72], "id": "n1"},
                    {"k": "line", "pts": [[1735, 1940], [2057, 1940]], "w": 22, "id": "staff"}
                ]
            }"#,
        )
        .unwrap();
        ScoreData::parse(&props)
    }

    #[test]
    fn the_staves_are_read_back_out_of_the_engraving() {
        let data = staffed_page();
        assert_eq!(
            data.staves,
            vec![Staff {
                y0: 1040.0,
                y1: 1760.0,
                width: 13.0
            }]
        );
        // the ledger line is too short to be a staff line, and the notehead is
        // not a line at all
        assert_eq!(data.staff_at(1940.0), data.staves.first().copied());
    }

    #[test]
    fn ledger_lines_follow_the_engraving_rule() {
        let data = staffed_page();
        let staff = data.staves[0];
        let ys = |y: f32| data.ledger_ys(staff, y);
        // on the staff, and in the space just outside it: nothing to draw
        assert!(ys(1400.0).is_empty());
        assert!(ys(1850.0).is_empty()); // one step below the last line
        assert!(ys(950.0).is_empty()); // one step above the first
        // on the first ledger position, and in the space beyond it: one line
        assert_eq!(ys(1940.0), vec![1940.0]);
        assert_eq!(ys(2030.0), vec![1940.0]);
        // two positions out: two lines, and upward is symmetrical
        assert_eq!(ys(2120.0), vec![1940.0, 2120.0]);
        assert_eq!(ys(860.0), vec![860.0]);
        assert_eq!(ys(680.0), vec![860.0, 680.0]);
    }

    #[test]
    fn dragging_off_the_staff_draws_ledger_lines_and_back_on_drops_them() {
        let mut data = staffed_page();
        let rect = Rect::new(0.0, 0.0, 1100.0, 300.0);
        let verts = |data: &ScoreData| {
            let mut mesh = Mesh::new();
            data.render(&mut mesh, rect, None, -1.0, INK);
            mesh.positions().count()
        };
        let engraved = verts(&data); // one ledger line, the engraved one
        // dragged up onto the staff: its ledger line goes with it, which means
        // it stops being drawn
        data.drag = Some(ScoreDrag {
            id: "n1".into(),
            steps: 2,
        });
        let on_staff = verts(&data);
        // dragged two positions below: the engraved one plus a second
        data.drag = Some(ScoreDrag {
            id: "n1".into(),
            steps: -2,
        });
        let below = verts(&data);
        assert!(on_staff < engraved && engraved < below);
        assert_eq!(engraved - on_staff, below - engraved);
        // and the drag that moves nothing redraws exactly what was engraved
        data.drag = Some(ScoreDrag {
            id: "n1".into(),
            steps: 0,
        });
        assert_eq!(verts(&data), engraved);
    }

    #[test]
    fn the_page_carries_its_own_step() {
        // the step follows verovio's `unit`, not the staff scale, so the client
        // sends it with the page; a nonsensical one falls back to the default.
        let props: Map<String, Value> =
            serde_json::from_str(r#"{"vb":[1000,400],"step":120}"#).unwrap();
        assert_eq!(ScoreData::parse(&props).step, 120.0);
        let props: Map<String, Value> =
            serde_json::from_str(r#"{"vb":[1000,400],"step":0}"#).unwrap();
        assert_eq!(ScoreData::parse(&props).step, STEP);
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
            -1.0,
            INK,
        );
        assert!(!mesh.is_empty(), "a staff line should paint");
    }
}
