//! The `font-atlas` feature's other face: an outline typeface, rasterized on
//! demand into one coverage texture the painter samples.
//!
//! The embedded bitmap ([`super`]) is the floor and this is the option, which is
//! the whole shape of the milestone: with the feature compiled in but no face
//! loaded, every measurement and every quad is the bitmap's, unchanged. A face
//! arrives through the [`FontSource`](crate::host::FontSource) seam — a file
//! natively, a fetch in the page — and from then on [`super::text`] emits
//! textured quads instead of one rectangle per lit font-pixel.
//!
//! **The cell is still declared and the face is still drawn to fit it.** The
//! pixel size a scale rasterizes at is the one whose *cap height* lands on the
//! body box ([`super::GLYPH_H`] rows): a capital drawn through the atlas is
//! exactly as tall as the capital the bitmap drew, so no layout follows the
//! typeface. What legitimately follows it is the width of a **string**
//! ([`super::width`]), measured where the string changes.
//!
//! **Why this one lives in a thread-local when the size table is passed.** A
//! window resolves its own [`Metrics`](crate::host::metrics::Metrics), so a
//! table cannot be global without lying about which window it belongs to. A
//! face is the opposite: a build points at one, every window draws with it, and
//! the alternative is threading it through every draw function in the crate to
//! say the same thing at each of them. The texture, in turn, *is* per window —
//! [`Atlas::version`] tells a painter when its copy is stale.

use std::cell::RefCell;
use std::collections::HashMap;

use fontdue::{Font, FontSettings};

use super::GLYPH_H;

/// The side of the atlas texture, in texels. One 1 MiB R8 texture per window
/// holds every glyph of every size a document actually draws; when a document
/// asks for more than fits, the atlas repacks (see [`Atlas::fit`]).
pub const SIDE: u32 = 1024;

/// A texel of empty margin around each glyph, so the sampler can never pick up
/// a neighbour's coverage at a quad edge.
const PAD: u32 = 1;

/// The nominal pixel size the face's ratios are measured at. Outline metrics
/// scale linearly, so one measurement answers every size.
const REF_PX: f32 = 100.0;

/// One rasterized glyph, placed relative to the pen and the baseline.
#[derive(Clone, Copy)]
pub struct Glyph {
    /// Offset from the pen to the quad's left edge, whole pixels.
    pub dx: f32,
    /// Offset from the **baseline** to the quad's top edge (negative = above).
    pub dy: f32,
    pub w: f32,
    pub h: f32,
    /// `[u0, v0, u1, v1]` in texture coordinates.
    pub uv: [f32; 4],
    /// How far the pen steps after this glyph — the face's own advance, which
    /// is what makes the face proportional rather than a grid.
    pub advance: f32,
}

/// The loaded typeface and the two ratios every size is derived from.
struct Face {
    font: Font,
    /// Cap height per pixel size: what makes a capital fill the body box.
    cap: f32,
    /// The advance of a digit per pixel size — the *nominal* cell, for the size
    /// roles that reserve room for N characters.
    digit: f32,
}

/// The glyph cache and its texture: coverage bytes, a shelf packer over them,
/// and the map from `(character, pixel size)` to where it landed.
pub struct Atlas {
    face: Option<Face>,
    pixels: Vec<u8>,
    /// Shelf packing state: the pen on the current shelf, its top and height.
    pen_x: u32,
    shelf_y: u32,
    shelf_h: u32,
    map: HashMap<(char, u32), Glyph>,
    version: u64,
}

impl Atlas {
    fn new() -> Self {
        Self {
            face: None,
            pixels: Vec::new(),
            pen_x: PAD,
            shelf_y: PAD,
            shelf_h: 0,
            map: HashMap::new(),
            version: 0,
        }
    }

    /// Whether a face is loaded — the one question every entry point in
    /// [`super`] asks before taking this path.
    pub fn has_face(&self) -> bool {
        self.face.is_some()
    }

    /// Loads `bytes` as the host's typeface, replacing whatever was there and
    /// emptying the cache. `false` (and nothing changed) if the bytes are not a
    /// font this rasterizer reads.
    pub fn set_face(&mut self, bytes: &[u8]) -> bool {
        let settings = FontSettings {
            scale: REF_PX,
            ..Default::default()
        };
        let Ok(font) = Font::from_bytes(bytes, settings) else {
            return false;
        };
        // The cap height: how tall the face draws a capital at the reference
        // size. Falling back to the ascent covers a face whose 'H' has no
        // outline (a symbol face), which still measures and still draws.
        let cap = match font.metrics('H', REF_PX).height as f32 {
            h if h > 0.0 => h,
            _ => font
                .horizontal_line_metrics(REF_PX)
                .map_or(REF_PX * 0.7, |m| m.ascent),
        } / REF_PX;
        let digit = font.metrics('0', REF_PX).advance_width / REF_PX;
        self.face = Some(Face { font, cap, digit });
        self.reset();
        true
    }

    /// Empties the cache and the texture, bumping the version so every window's
    /// copy is re-uploaded.
    fn reset(&mut self) {
        self.map.clear();
        self.pixels.clear();
        self.pen_x = PAD;
        self.shelf_y = PAD;
        self.shelf_h = 0;
        self.version += 1;
    }

    /// The pixel size `scale` rasterizes at: the size whose cap height is the
    /// body box the bitmap face draws at that scale. Whole pixels, because a
    /// rasterization is — which is a far finer grid than the half-steps a
    /// bitmap glyph needs, and why `text_size` is continuous here.
    pub fn px(&self, scale: f32) -> f32 {
        let cap = self.face.as_ref().map_or(1.0, |f| f.cap).max(0.01);
        (GLYPH_H as f32 * scale / cap).round().max(1.0)
    }

    /// The nominal advance at `scale` — a digit's, since the roles that ask are
    /// reserving room for a number.
    pub fn nominal_advance(&self, scale: f32) -> f32 {
        self.face
            .as_ref()
            .map_or(0.0, |f| (f.digit * self.px(scale)).max(1.0))
    }

    /// How far the pen steps over `c` at `scale`, without rasterizing it.
    pub fn advance_of(&self, c: char, scale: f32) -> f32 {
        self.face
            .as_ref()
            .map_or(0.0, |f| f.font.metrics(c, self.px(scale)).advance_width)
    }

    /// The distance from the body box's top to the baseline at `scale` — where
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
        self.face
            .as_ref()
            .and_then(|f| f.font.horizontal_line_metrics(px))
            .map_or(0.0, |m| m.ascent - m.descent + m.line_gap)
    }

    /// `c` at `scale`, rasterized into the texture if it is not there yet.
    /// `None` only when no face is loaded.
    pub fn glyph(&mut self, c: char, scale: f32) -> Option<Glyph> {
        let px = self.px(scale);
        let key = (c, px as u32);
        if let Some(g) = self.map.get(&key) {
            return Some(*g);
        }
        let face = self.face.as_ref()?;
        let (metrics, coverage) = face.font.rasterize(c, px);
        let (w, h) = (metrics.width as u32, metrics.height as u32);
        let glyph = if w == 0 || h == 0 {
            // A space (or any inked-nothing): it advances and draws no quad.
            Glyph {
                dx: 0.0,
                dy: 0.0,
                w: 0.0,
                h: 0.0,
                uv: [0.0; 4],
                advance: metrics.advance_width,
            }
        } else {
            let (ox, oy) = self.fit(w, h)?;
            // Rows arrive top-first, one coverage byte per texel.
            if self.pixels.is_empty() {
                self.pixels = vec![0; (SIDE * SIDE) as usize];
            }
            for row in 0..h {
                let src = (row * w) as usize;
                let dst = ((oy + row) * SIDE + ox) as usize;
                self.pixels[dst..dst + w as usize]
                    .copy_from_slice(&coverage[src..src + w as usize]);
            }
            self.version += 1;
            let s = SIDE as f32;
            Glyph {
                dx: metrics.xmin as f32,
                // `ymin` is the bottom of the ink above the baseline, y up; the
                // quad's top is the whole box above that, y down.
                dy: -(metrics.ymin as f32) - h as f32,
                w: w as f32,
                h: h as f32,
                uv: [
                    ox as f32 / s,
                    oy as f32 / s,
                    (ox + w) as f32 / s,
                    (oy + h) as f32 / s,
                ],
                advance: metrics.advance_width,
            }
        };
        self.map.insert(key, glyph);
        Some(glyph)
    }

    /// Reserves a `w` x `h` box on the current shelf, opening the next one (or,
    /// once the sheet is full, repacking from scratch) when it does not fit.
    /// `None` for a glyph larger than the whole sheet.
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
            // The sheet is full. Repacking loses the glyphs a mesh being built
            // right now already placed, so that frame may sample the wrong
            // texels once; the next one is correct. A document with more glyphs
            // than a megabyte of coverage holds is what this costs, and it is
            // cheaper than growing a texture every window has a copy of.
            self.reset();
            self.pixels = vec![0; (SIDE * SIDE) as usize];
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

    /// The coverage sheet, `SIDE` x `SIDE` bytes — empty until a glyph has been
    /// rasterized.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
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

/// A face to test against, taken from the system — the crate embeds none (see
/// `PLAN.md`), so the tests of both this module and [`super`] state what they
/// need and skip where it is absent, rather than shipping a megabyte of test
/// data.
#[cfg(test)]
pub(crate) fn system_face() -> Option<Vec<u8>> {
    [
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "/System/Library/Fonts/Helvetica.ttc",
    ]
    .iter()
    .find_map(|p| std::fs::read(p).ok())
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

    /// The face is proportional, and that is measured per character — the seam
    /// K9 left: a width is asked for where the string is, never in a layout.
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

    /// Two sizes of one character are two entries, and a repeat is a hit — the
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
        assert_eq!(a.pixels().len(), (SIDE * SIDE) as usize);
    }

    /// Every glyph lands inside the sheet with its margin, whatever order the
    /// shelves fill in.
    #[test]
    fn packing_stays_inside_the_sheet() {
        let Some(mut a) = loaded() else { return };
        for scale in [1.0, 2.0, 6.0, 11.0] {
            for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789".chars() {
                let g = a.glyph(c, scale).unwrap();
                let [u0, v0, u1, v1] = g.uv;
                assert!((0.0..=1.0).contains(&u0) && (0.0..=1.0).contains(&u1));
                assert!((0.0..=1.0).contains(&v0) && (0.0..=1.0).contains(&v1));
            }
        }
    }
}
