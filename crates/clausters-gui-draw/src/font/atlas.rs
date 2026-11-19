//! The `font-atlas` feature's other face: an outline typeface, rasterized on
//! demand into one coverage texture the painter samples.
//!
//! The embedded bitmap ([`super`]) is the floor: with no face loaded, every
//! measurement and every quad is the bitmap's, unchanged. A shell loads the
//! face the crate carries ([`EMBEDDED`]) as it starts, and another arrives
//! through the host's `FontSource` seam -- a file
//! natively, bytes a client hands over -- and from then on [`super::text`]
//! emits textured quads instead of one rectangle per lit font-pixel.
//!
//! **The cell is still declared and the face is still drawn to fit it.** The
//! pixel size a scale rasterizes at is the one whose *cap height* lands on the
//! body box ([`super::GLYPH_H`] rows): a capital drawn through the atlas is
//! exactly as tall as the capital the bitmap drew, so no layout follows the
//! typeface. What legitimately follows it is the width of a **string**
//! ([`super::width`]), measured where the string changes.
//!
//! **Why this one lives in a thread-local when the size table is passed.** A
//! window resolves its own [`Metrics`](crate::metrics::Metrics), so a
//! table cannot be global without lying about which window it belongs to. A
//! face is the opposite: a build points at one, every window draws with it, and
//! the alternative is threading it through every draw function in the crate to
//! say the same thing at each of them. The texture, in turn, *is* per window --
//! [`Atlas::version`] tells a painter when its copy is stale.

use std::cell::RefCell;
use std::collections::HashMap;

use ab_glyph_rasterizer::{Point, Rasterizer, point};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{
    DrawSettings, Engine, GlyphStyles, HintingInstance, HintingOptions, OutlinePen, SmoothMode,
    Target,
};
use skrifa::{FontRef, GlyphId, MetadataProvider};

use super::Family;

use super::GLYPH_H;

/// The width of the atlas sheet, in texels, and the most rows it grows to:
/// a megabyte of coverage holds every glyph of every size a document
/// actually draws, and when one asks for more than fits, the atlas repacks
/// (see [`Atlas::fit`]).
pub const SIDE: u32 = 1024;

/// The rows a sheet starts with. An interface writes one or two sizes of a
/// hundred-odd characters, which is a few shelves: a sheet that began at the
/// megabyte held a megabyte for them, in every window. It doubles as it
/// fills ([`Atlas::fit`]), up to [`SIDE`].
const FIRST_ROWS: u32 = 64;

/// A texel of empty margin around each glyph, so the sampler can never pick up
/// a neighbour's coverage at a quad edge.
const PAD: u32 = 1;

/// The nominal pixel size the face's ratios are measured at. Outline metrics
/// scale linearly, so one measurement answers every size.
const REF_PX: f32 = 100.0;

/// **How many places between two pixels a glyph is drawn at**: its pen is
/// rounded to a quarter of a pixel and the glyph rasterized for that
/// quarter, as a browser places text. A glyph held to whole pixels moves its
/// letters by up to half a pixel each, which is what makes a line of a
/// proportional face, or of code at a size whose advance is not whole, read
/// as unevenly spaced; four places are as close to the exact positions as a
/// capture tells apart, and four copies of a glyph at most.
pub const SUBPIXEL: f32 = 4.0;

/// The size key of a rasterization: sizes are fractional, kept to a 64th
/// of a pixel, which no eye tells apart and no cache is flooded by.
fn size_key(px: f32) -> u32 {
    (px * 64.0).round() as u32
}

/// One rasterized glyph, placed relative to the pen and the baseline.
#[derive(Clone, Copy)]
pub struct Glyph {
    /// Offset from the pen to the quad's left edge, whole pixels.
    pub dx: f32,
    /// Offset from the **baseline** to the quad's top edge (negative = above).
    pub dy: f32,
    pub w: f32,
    pub h: f32,
    /// `[u0, v0, u1, v1]` in **texels** of the sheet. Texels and not
    /// fractions of it: a sheet grows, and a fraction of a sheet that has
    /// since grown is somewhere else, while a texel stays where it was
    /// written -- so the glyphs a window has already drawn outlive every
    /// growth. The shader divides by the texture's own size.
    pub uv: [f32; 4],
    /// How far the pen steps after this glyph -- the face's own advance, which
    /// is what makes the face proportional rather than a grid.
    pub advance: f32,
}

/// One loaded typeface: its bytes, which the outlines, the metrics and the
/// shaper's layout tables are all read from as they are asked for.
struct Loaded {
    data: std::sync::Arc<[u8]>,
    /// **The hinter at each size it drew at** (by the size's bits), made at
    /// the first glyph of that size: `None` where it could not be made, and
    /// the outline is drawn as it is.
    hints: RefCell<HashMap<u32, Option<HintingInstance>>>,
    /// The face's glyph styles, which the automatic hinter reads at every
    /// size: worked out once.
    styles: RefCell<Option<GlyphStyles>>,
}

impl Loaded {
    /// `bytes` as a face: `None` when they are not a font, or a font none
    /// of whose first characters has an outline to draw.
    fn new(bytes: &[u8]) -> Option<Self> {
        let font = FontRef::new(bytes).ok()?;
        let outlines = font.outline_glyphs();
        font.charmap()
            .mappings()
            .take(64)
            .any(|(_, g)| outlines.get(g).is_some())
            .then(|| Self {
                data: std::sync::Arc::from(bytes),
                hints: RefCell::new(HashMap::new()),
                styles: RefCell::new(None),
            })
    }

    /// The face read from its bytes: a table directory, which costs next to
    /// nothing to read again.
    fn font(&self) -> FontRef<'_> {
        FontRef::new(&self.data).expect("read when it was loaded")
    }

    /// The glyph that draws `c`, if the face has one.
    fn glyph_of(&self, c: char) -> Option<GlyphId> {
        self.font().charmap().map(c)
    }

    fn has_glyph(&self, c: char) -> bool {
        self.glyph_of(c).is_some()
    }

    /// How far the pen steps over glyph `id` at `px` pixels to the em.
    fn advance(&self, id: GlyphId, px: f32) -> f32 {
        self.font()
            .glyph_metrics(Size::new(px), LocationRef::default())
            .advance_width(id)
            .unwrap_or(0.0)
    }

    /// The face's line at `px` pixels to the em: ascent, descent and gap.
    fn line(&self, px: f32) -> f32 {
        let m = self.font().metrics(Size::new(px), LocationRef::default());
        m.ascent - m.descent + m.leading
    }

    fn units_per_em(&self) -> f32 {
        f32::from(
            self.font()
                .metrics(Size::unscaled(), LocationRef::default())
                .units_per_em,
        )
    }

    /// **Glyph `id` at `px` pixels to the em**, its pen `x` pixels right of
    /// a whole pixel (a fraction of one): its coverage, one byte a texel,
    /// rows top first, and where that box stands from the whole pixel and
    /// the baseline, `(left, top, w, h)` with `top` above the baseline
    /// negative. `None` for a glyph that inks nothing.
    fn rasterize(&self, id: GlyphId, px: f32, x: f32) -> Option<(i32, i32, u32, u32, Vec<u8>)> {
        let font = self.font();
        let outlines = font.outline_glyphs();
        let outline = outlines.get(id)?;
        let mut path = Path::default();
        // **Hinted lightly**, as the desktop's text is (FreeType's light
        // target, fontconfig's `hintslight`, what GNOME and its webview
        // draw with): the automatic hinter moves edges up and down onto
        // whole pixels and nothing across, so a line keeps its advances and
        // its stems stand sharp instead of greyed over two pixels.
        let mut hints = self.hints.borrow_mut();
        let hinter = hints.entry(px.to_bits()).or_insert_with(|| {
            let styles = self
                .styles
                .borrow_mut()
                .get_or_insert_with(|| GlyphStyles::new(&outlines))
                .clone();
            let options = HintingOptions {
                engine: Engine::Auto(Some(styles)),
                target: Target::Smooth {
                    mode: SmoothMode::Light,
                    symmetric_rendering: true,
                    preserve_linear_metrics: true,
                },
            };
            HintingInstance::new(&outlines, Size::new(px), LocationRef::default(), options).ok()
        });
        let drawn = match hinter {
            Some(hinter) => outline.draw(DrawSettings::hinted(hinter, false), &mut path),
            None => outline.draw(
                DrawSettings::unhinted(Size::new(px), LocationRef::default()),
                &mut path,
            ),
        };
        drawn.ok()?;
        let [x0, y0, x1, y1] = path.bounds()?;
        // the box on whole pixels, y down from the top of the ink
        let left = (x0 + x).floor();
        let top = y1.ceil();
        let (w, h) = ((x1 + x).ceil() - left, top - y0.floor());
        if w < 1.0 || h < 1.0 {
            return None;
        }
        let (w, h) = (w as u32, h as u32);
        let mut r = Rasterizer::new(w as usize, h as usize);
        let at = |(px_, py): (f32, f32)| point(px_ + x - left, top - py);
        path.fill(&mut r, at);
        let mut coverage = vec![0u8; (w * h) as usize];
        r.for_each_pixel(|i, a| coverage[i] = (a.abs().min(1.0) * 255.0).round() as u8);
        Some((left as i32, -(top as i32), w, h, coverage))
    }

    /// How tall a capital stands per pixel to the em: the outline of 'H',
    /// or the ascent for a face whose 'H' has none (a symbol face).
    fn cap(&self) -> f32 {
        let font = self.font();
        let from_h = self.glyph_of('H').and_then(|id| {
            let outline = font.outline_glyphs().get(id)?;
            let mut path = Path::default();
            outline
                .draw(
                    DrawSettings::unhinted(Size::new(REF_PX), LocationRef::default()),
                    &mut path,
                )
                .ok()?;
            let [_, y0, _, y1] = path.bounds()?;
            (y1 - y0 > 0.0).then_some(y1 - y0)
        });
        from_h.unwrap_or_else(|| {
            font.metrics(Size::new(REF_PX), LocationRef::default())
                .ascent
        }) / REF_PX
    }
}

/// An outline as the face draws it, in pixels with y up: what a glyph's box
/// is measured from and then filled from.
#[derive(Default)]
struct Path {
    segments: Vec<Segment>,
    start: (f32, f32),
    last: (f32, f32),
}

enum Segment {
    Line((f32, f32), (f32, f32)),
    Quad((f32, f32), (f32, f32), (f32, f32)),
    Cubic((f32, f32), (f32, f32), (f32, f32), (f32, f32)),
}

impl Path {
    /// The box every point stands in, control points included, as
    /// `[x0, y0, x1, y1]`: a curve never leaves the hull of its points, so
    /// the box holds the ink.
    fn bounds(&self) -> Option<[f32; 4]> {
        let mut points = self.segments.iter().flat_map(|s| match *s {
            Segment::Line(a, b) => vec![a, b],
            Segment::Quad(a, b, c) => vec![a, b, c],
            Segment::Cubic(a, b, c, d) => vec![a, b, c, d],
        });
        let first = points.next()?;
        Some(points.fold(
            [first.0, first.1, first.0, first.1],
            |[x0, y0, x1, y1], (x, y)| [x0.min(x), y0.min(y), x1.max(x), y1.max(y)],
        ))
    }

    /// Fills the outline into `r`, each point placed by `at`.
    fn fill(&self, r: &mut Rasterizer, at: impl Fn((f32, f32)) -> Point) {
        for s in &self.segments {
            match *s {
                Segment::Line(a, b) => r.draw_line(at(a), at(b)),
                Segment::Quad(a, b, c) => r.draw_quad(at(a), at(b), at(c)),
                Segment::Cubic(a, b, c, d) => r.draw_cubic(at(a), at(b), at(c), at(d)),
            }
        }
    }
}

impl OutlinePen for Path {
    fn move_to(&mut self, x: f32, y: f32) {
        self.start = (x, y);
        self.last = (x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.segments.push(Segment::Line(self.last, (x, y)));
        self.last = (x, y);
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.segments
            .push(Segment::Quad(self.last, (cx, cy), (x, y)));
        self.last = (x, y);
    }
    fn curve_to(&mut self, c0x: f32, c0y: f32, c1x: f32, c1y: f32, x: f32, y: f32) {
        self.segments
            .push(Segment::Cubic(self.last, (c0x, c0y), (c1x, c1y), (x, y)));
        self.last = (x, y);
    }
    fn close(&mut self) {
        if self.last != self.start {
            self.segments.push(Segment::Line(self.last, self.start));
        }
        self.last = self.start;
    }
}

/// **The faces text is drawn with**: the host's face first, and after it the
/// faces it falls back to for a character the first does not draw -- the
/// face the host embeds, when another was loaded over it, and the
/// Devanagari one ([`DEVANAGARI`]). Every size is derived from the first one's
/// two ratios, so a fallback face is drawn at the size the host's is.
struct Face {
    faces: Vec<Loaded>,
    /// Cap height per pixel size: what makes a capital fill the body box.
    cap: f32,
    /// Each face's cap height per pixel size, which sizes its glyphs: the
    /// host's for the faces it falls back to, which are drawn at its size,
    /// and the monospaced face's own, so a capital of code stands as tall
    /// as a capital of the interface.
    caps: Vec<f32>,
    /// Which of `faces` draws each family that has a face of its own (code's,
    /// [`MONO`], and the documentation's, [`PROSE`] and its weights): none of
    /// them is a fallback of the interface's. The documentation's are loaded
    /// the first time their family is asked for ([`Atlas::ensure`]).
    named: Vec<(Family, usize)>,
    /// The advance of a digit per pixel size -- the *nominal* cell, for the size
    /// roles that reserve room for N characters.
    digit: f32,
}

impl Face {
    /// The host's face.
    fn first(&self) -> &Loaded {
        &self.faces[0]
    }

    /// **The face that draws `c`**: the first that has a glyph for it, or the
    /// host's, which draws its box for a character none has. The monospaced
    /// face draws code only.
    fn of(&self, c: char) -> usize {
        self.faces
            .iter()
            .enumerate()
            .position(|(i, f)| !self.is_named(i) && f.has_glyph(c))
            .unwrap_or(0)
    }

    /// Whether face `i` is a family's own rather than the interface's.
    fn is_named(&self, i: usize) -> bool {
        self.named.iter().any(|&(_, at)| at == i)
    }
}

/// The glyph cache and its texture: coverage bytes, a shelf packer over them,
/// and the map from `(face, glyph id, pixel size)` to where it landed.
pub struct Atlas {
    face: Option<Face>,
    /// The sheet: [`SIDE`] texels wide and [`rows`](Self::rows) high.
    pixels: Vec<u8>,
    rows: u32,
    /// Shelf packing state: the pen on the current shelf, its top and height.
    pen_x: u32,
    shelf_y: u32,
    shelf_h: u32,
    /// By face, glyph, size key ([`size_key`]) and the quarter of a pixel
    /// its pen stands at ([`SUBPIXEL`]).
    map: HashMap<(u8, u16, u32, u8), Glyph>,
    version: u64,
    epoch: u64,
}

impl Atlas {
    fn new() -> Self {
        Self {
            face: None,
            pixels: Vec::new(),
            rows: 0,
            pen_x: PAD,
            shelf_y: PAD,
            shelf_h: 0,
            map: HashMap::new(),
            version: 0,
            epoch: 0,
        }
    }

    /// Whether a face is loaded -- the one question every entry point in
    /// [`super`] asks before taking this path.
    pub fn has_face(&self) -> bool {
        self.face.is_some()
    }

    /// Loads `bytes` as the host's typeface, replacing whatever was there and
    /// emptying the cache. `false` (and nothing changed) if the bytes are not a
    /// font this rasterizer reads.
    pub fn set_face(&mut self, bytes: &[u8]) -> bool {
        let Some(first) = Loaded::new(bytes) else {
            return false;
        };
        // the faces it falls back to: the embedded one, unless it is the one
        // loaded, and the Devanagari one
        let mut faces = vec![first];
        for fallback in [EMBEDDED, DEVANAGARI] {
            if fallback == bytes {
                continue;
            }
            faces.extend(Loaded::new(fallback));
        }
        // the monospaced face, which code is drawn with whatever the host's is
        let mono = Loaded::new(MONO).map(|face| {
            faces.push(face);
            faces.len() - 1
        });
        let cap = faces[0].cap();
        let digit = faces[0]
            .glyph_of('0')
            .map_or(0.0, |id| faces[0].advance(id, REF_PX))
            / REF_PX;
        let caps = (0..faces.len())
            .map(|i| if Some(i) == mono { faces[i].cap() } else { cap })
            .collect();
        self.face = Some(Face {
            faces,
            cap,
            caps,
            named: mono.map(|at| (Family::Mono, at)).into_iter().collect(),
            digit,
        });
        self.reset();
        true
    }

    /// Empties the cache and the texture, bumping the version so every window's
    /// copy is re-uploaded -- and the **epoch**, since every glyph a window
    /// has drawn now names texels that hold something else.
    fn reset(&mut self) {
        self.map.clear();
        self.pixels.clear();
        self.rows = 0;
        self.pen_x = PAD;
        self.shelf_y = PAD;
        self.shelf_h = 0;
        self.version += 1;
        self.epoch += 1;
    }

    /// The pixel size `scale` rasterizes at: the size whose cap height is the
    /// body box the bitmap face draws at that scale. **Fractional**, as a
    /// browser's sizes are: an outline is drawn at any size, so code asked
    /// for at 14 pixels to the em is drawn at 14 and not at the whole size
    /// its cap height rounds to.
    pub fn px(&self, scale: f32) -> f32 {
        let cap = self.face.as_ref().map_or(1.0, |f| f.cap).max(0.01);
        (GLYPH_H as f32 * scale / cap).max(1.0)
    }

    /// The scale at which `family` draws `px` pixels to the em: the inverse
    /// of [`px_of`](Self::px_of), for a size given as a browser gives one.
    pub fn scale_for_em(&self, family: Family, px: f32) -> f32 {
        let face = self.face_of(family).unwrap_or(0);
        let cap = self
            .face
            .as_ref()
            .and_then(|f| f.caps.get(face).copied())
            .unwrap_or(1.0);
        px * cap / GLYPH_H as f32
    }

    /// The pixel size face `face`'s glyphs rasterize at for `scale`: the
    /// host's ([`px`](Self::px)) for its fallbacks, and for the monospaced
    /// face the size whose own capital fills the body box.
    pub fn px_of(&self, face: usize, scale: f32) -> f32 {
        let cap = self
            .face
            .as_ref()
            .and_then(|f| f.caps.get(face).copied())
            .unwrap_or(1.0)
            .max(0.01);
        (GLYPH_H as f32 * scale / cap).max(1.0)
    }

    /// Which face is the monospaced one, when one is loaded.
    pub fn mono(&self) -> Option<usize> {
        self.face_of(Family::Mono)
    }

    /// Which face draws `family`, when it has one of its own and it is
    /// loaded.
    pub fn face_of(&self, family: Family) -> Option<usize> {
        let face = self.face.as_ref()?;
        face.named
            .iter()
            .find(|(f, _)| *f == family)
            .map(|&(_, at)| at)
    }

    /// Whether face `i` is a family's own, which the interface never falls
    /// back to.
    pub fn is_named(&self, i: usize) -> bool {
        self.face.as_ref().is_some_and(|f| f.is_named(i))
    }

    /// **`family`'s own face loaded, the first time it is asked for**: the
    /// documentation's faces are a megabyte and a half, read only by a host
    /// that draws documentation. Its capitals stand as tall as the
    /// interface's.
    pub fn ensure(&mut self, family: Family) {
        let Some(bytes) = family_bytes(family) else {
            return;
        };
        let Some(face) = self.face.as_mut() else {
            return;
        };
        if face.named.iter().any(|(f, _)| *f == family) {
            return;
        }
        let Some(loaded) = Loaded::new(bytes) else {
            return;
        };
        face.caps.push(loaded.cap());
        face.faces.push(loaded);
        face.named.push((family, face.faces.len() - 1));
    }

    /// The monospaced face's cell at `scale`: its advance, every
    /// character's.
    pub fn mono_advance(&self, scale: f32) -> Option<f32> {
        let at = self.mono()?;
        let face = &self.face.as_ref()?.faces[at];
        Some(face.advance(face.glyph_of('0')?, self.px_of(at, scale)))
    }

    /// The nominal advance at `scale` -- a digit's, since the roles that ask are
    /// reserving room for a number.
    pub fn nominal_advance(&self, scale: f32) -> f32 {
        self.face
            .as_ref()
            .map_or(0.0, |f| (f.digit * self.px(scale)).max(1.0))
    }

    /// How far the pen steps over `c` at `scale`, without rasterizing it, in
    /// the face that draws it.
    pub fn advance_of(&self, c: char, scale: f32) -> f32 {
        self.face.as_ref().map_or(0.0, |f| {
            let face = &f.faces[f.of(c)];
            face.glyph_of(c)
                .map_or(0.0, |id| face.advance(id, self.px(scale)))
        })
    }

    /// How many faces text is drawn with: the host's and its fallbacks.
    pub fn faces(&self) -> usize {
        self.face.as_ref().map_or(0, |f| f.faces.len())
    }

    /// The bytes of face `face`, which the shaper reads.
    pub fn data(&self, face: usize) -> Option<std::sync::Arc<[u8]>> {
        Some(self.face.as_ref()?.faces.get(face)?.data.clone())
    }

    /// Whether face `face` has a glyph for `c`.
    pub fn covers(&self, face: usize, c: char) -> bool {
        self.face
            .as_ref()
            .and_then(|f| f.faces.get(face))
            .is_some_and(|f| f.has_glyph(c))
    }

    /// Pixels per font unit of face `face` at `scale`: what turns the
    /// shaper's positions, which are in the face's units, into the pixels
    /// its glyphs are rasterized at.
    pub fn units_to_px(&self, face: usize, scale: f32) -> f32 {
        let units = self
            .face
            .as_ref()
            .and_then(|f| f.faces.get(face))
            .map_or(1.0, Loaded::units_per_em);
        self.px_of(face, scale) / units.max(1.0)
    }

    /// The distance from the body box's top to the baseline at `scale` -- where
    /// [`super::text`] sits the face, so an accented capital overshoots upward
    /// exactly as the bitmap's does.
    pub fn baseline(&self, scale: f32) -> f32 {
        GLYPH_H as f32 * scale
    }

    /// The face's own line height at `scale`, never less than the bitmap's
    /// (the body box plus the room a mark and a tail take). A real typeface
    /// reaches further above and below its cap height than a 5x7 bitmap does,
    /// and wrapped lines have to clear each other's ink; this is a *drawing*
    /// distance, not a size a layout reserves, so following the face here
    /// relayouts nothing.
    pub fn line_advance(&self, scale: f32) -> f32 {
        let px = self.px(scale);
        self.face.as_ref().map_or(0.0, |f| f.first().line(px))
    }

    /// `c` at `scale`, in the face that draws it, rasterized into the texture
    /// if it is not there yet. `None` only when no face is loaded.
    pub fn glyph(&mut self, c: char, scale: f32) -> Option<Glyph> {
        let face = self.face.as_ref()?;
        let at = face.of(c);
        let id = face.faces[at].glyph_of(c).map_or(0, |g| g.to_u32() as u16);
        self.glyph_id(at, id, scale, 0)
    }

    /// **Glyph `id` of face `face`** at `scale` -- what the shaper names, which
    /// a ligature, a contextual form or a mark is and a character is not --
    /// with its pen `quarter` quarters of a pixel right of a whole one
    /// ([`SUBPIXEL`]), rasterized into the texture if it is not there yet.
    /// `None` when no face is loaded or there is no such face.
    pub fn glyph_id(&mut self, face: usize, id: u16, scale: f32, quarter: u8) -> Option<Glyph> {
        let px = self.px_of(face, scale);
        let key = (face as u8, id, size_key(px), quarter);
        if let Some(g) = self.map.get(&key) {
            return Some(*g);
        }
        let loaded = self.face.as_ref()?.faces.get(face)?;
        let gid = GlyphId::new(u32::from(id));
        let advance = loaded.advance(gid, px);
        let shift = f32::from(quarter) / SUBPIXEL;
        let glyph = match loaded.rasterize(gid, px, shift) {
            // A space (or any inked-nothing): it advances and draws no quad.
            None => Glyph {
                dx: 0.0,
                dy: 0.0,
                w: 0.0,
                h: 0.0,
                uv: [0.0; 4],
                advance,
            },
            Some((left, top, w, h, coverage)) => {
                let (ox, oy) = self.fit(w, h)?;
                // Rows arrive top-first, one coverage byte per texel.
                for row in 0..h {
                    let src = (row * w) as usize;
                    let dst = ((oy + row) * SIDE + ox) as usize;
                    self.pixels[dst..dst + w as usize]
                        .copy_from_slice(&coverage[src..src + w as usize]);
                }
                self.version += 1;
                Glyph {
                    dx: left as f32,
                    dy: top as f32,
                    w: w as f32,
                    h: h as f32,
                    uv: [ox as f32, oy as f32, (ox + w) as f32, (oy + h) as f32],
                    advance,
                }
            }
        };
        self.map.insert(key, glyph);
        Some(glyph)
    }

    /// Reserves a `w` x `h` box on the current shelf, opening the next one
    /// when it does not fit -- and, where that runs past the sheet, **growing
    /// the sheet** by doubling its rows, or once it is at [`SIDE`] of them,
    /// repacking from scratch. `None` for a glyph larger than the whole sheet.
    fn fit(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if w + 2 * PAD > SIDE || h + 2 * PAD > SIDE {
            return None;
        }
        if self.pen_x + w + PAD > SIDE {
            self.pen_x = PAD;
            self.shelf_y += self.shelf_h + PAD;
            self.shelf_h = 0;
        }
        if self.shelf_y + h + PAD > SIDE {
            // The sheet is full at its largest. Repacking loses the glyphs a
            // mesh being built right now already placed, so that frame may
            // sample the wrong texels once; the epoch says so, and a window
            // that kept a picture from before it draws a whole one again. A
            // document with more glyphs than a megabyte of coverage holds is
            // what this costs.
            self.reset();
        }
        // Rows for this box: what the sheet has, doubled until it fits. The
        // texels already written stay where they are, so nothing drawn from
        // them moves.
        let needed = self.shelf_y + h + PAD;
        if needed > self.rows {
            let mut rows = self.rows.max(FIRST_ROWS);
            while rows < needed {
                rows *= 2;
            }
            self.rows = rows.min(SIDE);
            self.pixels.resize((SIDE * self.rows) as usize, 0);
        }
        let at = (self.pen_x, self.shelf_y);
        self.pen_x += w + PAD;
        self.shelf_h = self.shelf_h.max(h);
        Some(at)
    }

    /// Bumped whenever the texture's contents change: a painter compares it
    /// with the version its own texture holds and re-uploads only then.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Bumped whenever the sheet was **emptied and packed again** -- a new
    /// face, or more glyphs than it holds at its largest. Every glyph drawn
    /// before it names texels that now hold something else, so a picture kept
    /// from an earlier epoch is drawn again rather than shown.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The coverage sheet, [`SIDE`] bytes a row and [`rows`](Self::rows) of
    /// them -- empty until a glyph has been rasterized.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// How many rows the sheet has: none before the first glyph, then
    /// a power of two that doubles as it fills.
    pub fn rows(&self) -> u32 {
        self.rows
    }

    /// The top of the shelf being filled. Everything above it is finished: a
    /// copy of the sheet that has those rows needs only the ones from here.
    pub fn shelf_top(&self) -> u32 {
        self.shelf_y
    }

    /// One past the last row that holds a glyph.
    pub fn used_rows(&self) -> u32 {
        (self.shelf_y + self.shelf_h + PAD).min(self.rows)
    }
}

thread_local! {
    /// The host's face and its glyph cache. One per thread rather than one
    /// per host: the front that builds meshes is single-threaded on both
    /// platforms, and a face is a property of the build, not of a window.
    static ATLAS: RefCell<Atlas> = RefCell::new(Atlas::new());
}

/// Runs `f` with the process's atlas.
pub fn with<R>(f: impl FnOnce(&mut Atlas) -> R) -> R {
    ATLAS.with(|a| f(&mut a.borrow_mut()))
}

/// The process's atlas epoch ([`Atlas::epoch`]).
pub fn epoch() -> u64 {
    with(|a| a.epoch())
}

/// Loads `bytes` as the host's typeface. `false` if they are not a readable
/// font, in which case the bitmap face keeps drawing.
pub fn set_face(bytes: &[u8]) -> bool {
    with(|a| a.set_face(bytes))
}

/// Whether a face is loaded (and therefore whether text draws through the
/// atlas).
pub fn has_face() -> bool {
    with(|a| a.has_face())
}

/// **The face the host draws its interface with when nobody names
/// another**: Inter, whole, the face the specification of the host's look
/// sets the interface in, its regular weight -- compiled into the host, so a
/// native window and a page draw the same text with nothing to find and
/// nothing to fetch. It is also the documentation's ([`PROSE`]).
pub const INTERFACE: &[u8] = include_bytes!("../../assets/fonts/Inter-Regular.ttf");

/// Inter's medium weight: a menu bar's titles, a label over its field, a
/// button's text ([`Family::UiMedium`](super::Family)).
pub const INTERFACE_MEDIUM: &[u8] = include_bytes!("../../assets/fonts/Inter-Medium.ttf");

/// **The face the host falls back to** for a character the interface's face
/// does not draw: a subset of DejaVu Sans -- Latin, Greek, Cyrillic, Arabic
/// and Hebrew, and the punctuation, arrows and symbols an interface writes,
/// with its layout tables for the shaper. `assets/fonts/` holds it, its
/// license and the script that makes it.
pub const EMBEDDED: &[u8] = include_bytes!("../../assets/fonts/DejaVuSans.ttf");

/// **The face the host draws Devanagari with**, which [`EMBEDDED`] does not
/// draw: a subset of Noto Sans Devanagari, compiled in beside it and fallen
/// back to for a character the host's face has no glyph for.
pub const DEVANAGARI: &[u8] = include_bytes!("../../assets/fonts/NotoSansDevanagari.ttf");

/// **The face the host draws code with**: JetBrains Mono, whole, the face
/// the editor writes its code in, compiled in beside the others and
/// loaded with whichever face the interface is drawn with
/// ([`Family::Mono`](super::Family)).
pub const MONO: &[u8] = include_bytes!("../../assets/fonts/JetBrainsMono.ttf");

/// JetBrains Mono's bold weight, read the first time a call's help marks a
/// parameter with it ([`Family::MonoBold`](super::Family)).
pub const MONO_BOLD: &[u8] = include_bytes!("../../assets/fonts/JetBrainsMono-Bold.ttf");

/// **The face the host draws documentation with**: the interface's, Inter
/// in its regular weight, read as a family of its own the first time
/// documentation is drawn ([`Atlas::ensure`]), beside its other weights.
pub const PROSE: &[u8] = INTERFACE;

/// Inter's semibold weight: a strong word, a small heading, a table's head.
pub const PROSE_STRONG: &[u8] = include_bytes!("../../assets/fonts/Inter-SemiBold.ttf");

/// Inter's bold weight: the large headings.
pub const PROSE_BOLD: &[u8] = include_bytes!("../../assets/fonts/Inter-Bold.ttf");

/// Inter's italic.
pub const PROSE_ITALIC: &[u8] = include_bytes!("../../assets/fonts/Inter-Italic.ttf");

/// The bytes of `family`'s own face, for one loaded when it is asked for.
fn family_bytes(family: Family) -> Option<&'static [u8]> {
    match family {
        Family::UiMedium => Some(INTERFACE_MEDIUM),
        Family::MonoBold => Some(MONO_BOLD),
        Family::Prose => Some(PROSE),
        Family::ProseStrong => Some(PROSE_STRONG),
        Family::ProseBold => Some(PROSE_BOLD),
        Family::ProseItalic => Some(PROSE_ITALIC),
        Family::Ui | Family::Mono => None,
    }
}

/// Loads [`INTERFACE`] as the host's typeface, where none is loaded: a
/// second host of one page finds the first one's face, or the one a client
/// handed over, and leaves it.
pub fn set_embedded() -> bool {
    has_face() || set_face(INTERFACE)
}

/// The face the tests of this module and of [`super`] draw with: the one the
/// host embeds, so none of them depends on what a machine has installed.
#[cfg(any(test, feature = "probe"))]
pub fn system_face() -> Option<Vec<u8>> {
    Some(INTERFACE.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh atlas rather than the thread-local one, so a test never depends
    /// on what another test loaded.
    fn loaded() -> Option<Atlas> {
        let mut a = Atlas::new();
        a.set_face(&system_face()?).then_some(a)
    }

    /// **The face the host carries is a face, and writes what an interface
    /// is written in**: Latin with its accents, Greek, Cyrillic, the arrows
    /// and the signs -- each with ink of its own, not the face's box for a
    /// character it lacks.
    #[test]
    fn the_embedded_face_writes_the_scripts_an_interface_uses() {
        let mut a = Atlas::new();
        assert!(a.set_face(EMBEDDED));
        let missing = a.face.as_ref().unwrap().first().glyph_of('\u{10FFFF}');
        for c in [
            'A', 'g', '7', '\u{F1}', '\u{E9}', '\u{3A9}', '\u{416}', '\u{2192}', '\u{266F}',
        ] {
            let index = a.face.as_ref().unwrap().first().glyph_of(c);
            assert!(
                index.is_some() && index != missing,
                "U+{:04X} is in the face",
                c as u32
            );
            let g = a.glyph(c, 2.0).unwrap();
            assert!(g.w > 0.0 && g.advance > 0.0, "U+{:04X} has ink", c as u32);
        }
        // under a third of a megabyte with Arabic, Hebrew and the layout
        // tables, not the three quarters the whole family is; and the
        // Devanagari face beside it, a seventh of one (the plan's G43a has
        // the measurements)
        assert!(EMBEDDED.len() < 310_000, "{}", EMBEDDED.len());
        assert!(DEVANAGARI.len() < 160_000, "{}", DEVANAGARI.len());
    }

    #[test]
    fn nonsense_bytes_are_not_a_face() {
        let mut a = Atlas::new();
        assert!(!a.set_face(b"not a font at all"));
        assert!(!a.has_face());
    }

    /// The milestone's own invariant: a capital drawn through the atlas is as
    /// tall as the body box the bitmap draws at the same scale, so the layout
    /// never follows the typeface.
    #[test]
    fn a_capital_fills_the_declared_cell() {
        let Some(mut a) = loaded() else { return };
        for scale in [1.0, 2.0, 3.0, 4.5] {
            let g = a.glyph('H', scale).expect("a loaded face rasterizes");
            let box_h = super::super::height(scale);
            assert!(
                (g.h - box_h).abs() <= 1.0,
                "'H' at {scale} is {} px tall, the body box is {box_h}",
                g.h
            );
            // ...and it sits on the baseline, which is the box's bottom.
            assert!(g.dy < 0.0 && (g.dy + g.h).abs() <= 1.0);
        }
    }

    /// The face is proportional, and that is measured per character -- the seam
    /// The rule: a width is asked for where the string is, never in a layout.
    #[test]
    fn advances_differ_per_character() {
        let Some(mut a) = loaded() else { return };
        let (i, m) = (a.advance_of('i', 2.0), a.advance_of('M', 2.0));
        assert!(i < m, "a proportional face: 'i' {i} < 'M' {m}");
        assert!(a.nominal_advance(2.0) > 0.0);
        // A space inks nothing and still steps the pen.
        let g = a.glyph(' ', 2.0).unwrap();
        assert_eq!((g.w, g.h), (0.0, 0.0));
        assert!(g.advance > 0.0);
    }

    /// Two sizes of one character are two entries, and a repeat is a hit -- the
    /// texture only grows when something new is rasterized.
    #[test]
    fn the_cache_keys_on_size_and_the_texture_grows_only_on_a_miss() {
        let Some(mut a) = loaded() else { return };
        a.glyph('A', 2.0).unwrap();
        let after_first = a.version();
        a.glyph('A', 2.0).unwrap();
        assert_eq!(a.version(), after_first, "a hit uploads nothing");
        a.glyph('A', 4.0).unwrap();
        assert!(a.version() > after_first, "another size is another glyph");
        assert_eq!(a.pixels().len(), (SIDE * a.rows()) as usize);
    }

    /// **A sheet starts small and grows without moving anything.** A hundred
    /// characters of an interface's size fit a few shelves, not a megabyte;
    /// when the shelves run past the sheet it doubles, and a glyph placed
    /// before that is the same texels after it, holding the same coverage --
    /// which is what lets a window go on drawing text it uploaded before the
    /// growth. Only a sheet full at its largest is packed again, and that is
    /// a new epoch.
    #[test]
    fn a_sheet_grows_and_leaves_its_glyphs_where_they_were() {
        let Some(mut a) = loaded() else { return };
        let text = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        let first = a.glyph('A', 1.5).unwrap();
        for c in text.chars() {
            a.glyph(c, 1.5).unwrap();
        }
        assert_eq!(a.rows(), FIRST_ROWS, "an interface's text is a few shelves");
        let ink = |a: &Atlas, g: &Glyph| {
            let [u0, v0, u1, v1] = g.uv.map(|t| t as u32);
            (v0..v1)
                .flat_map(|y| (u0..u1).map(move |x| (y * SIDE + x) as usize))
                .map(|at| u32::from(a.pixels()[at]))
                .sum::<u32>()
        };
        let before = ink(&a, &first);
        assert!(before > 0);
        let epoch = a.epoch();
        // Larger sizes until the sheet has had to grow.
        let mut scale = 3.0;
        while a.rows() == FIRST_ROWS {
            for c in text.chars() {
                a.glyph(c, scale).unwrap();
            }
            scale += 1.0;
        }
        assert!(a.rows() > FIRST_ROWS && a.rows().is_power_of_two());
        assert_eq!(a.epoch(), epoch, "growing packs nothing again");
        let again = a.glyph('A', 1.5).unwrap();
        assert_eq!(again.uv, first.uv, "the glyph is where it was");
        assert_eq!(ink(&a, &again), before, "holding what it held");
        assert!(a.used_rows() <= a.rows() && a.shelf_top() < a.used_rows());
        // Past the largest sheet, the atlas packs again and says so.
        let mut scale = 20.0;
        while a.epoch() == epoch && scale < 400.0 {
            for c in text.chars() {
                a.glyph(c, scale);
            }
            scale += 7.0;
        }
        assert!(a.epoch() > epoch, "a full sheet is packed again");
        assert!(a.rows() <= SIDE);
    }

    /// Every glyph lands inside the sheet with its margin, whatever order the
    /// shelves fill in.
    #[test]
    fn packing_stays_inside_the_sheet() {
        let Some(mut a) = loaded() else { return };
        for scale in [1.0, 2.0, 6.0, 11.0] {
            for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789".chars() {
                let g = a.glyph(c, scale).unwrap();
                // In texels of a sheet that is as high as it has had to grow.
                let [u0, v0, u1, v1] = g.uv;
                let (w, h) = (SIDE as f32, a.rows() as f32);
                assert!((0.0..=w).contains(&u0) && (0.0..=w).contains(&u1));
                assert!((0.0..=h).contains(&v0) && (0.0..=h).contains(&v1));
            }
        }
    }
}
