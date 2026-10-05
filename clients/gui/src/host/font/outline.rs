//! **Outline glyphs**: characters a window brings its own shape for.
//!
//! An icon is a glyph of the font, and a music font is one no face on a
//! machine can be counted on to have. So a window may carry the outlines of
//! the characters it uses -- `glyphs`, a codepoint to the SVG path of its
//! shape, in the font's own units with `y` upward and [`UNITS_PER_EM`] to the
//! em, which is the form a `score`'s display list already carries them in.
//! From then on that character is drawn as its shape wherever text is: a
//! button's icon, a segment's label, a menu entry.
//!
//! **A shape of its own width, in the line every character is in.** It is
//! centred on the body box's height, at one scale for the whole table
//! ([`EM_PER_CELL`]) so a whole note and a quarter keep their sizes against
//! each other; what is taller than the box overshoots it, as an accent does.
//! Its advance is its own ([`advance`]): the width of its shape and a bearing
//! on either side, never under a character's cell -- the proportional seam a
//! loaded face already steps by, so a dynamic four letters wide does not run
//! into its neighbour and no layout pass learns about a second kind of
//! character.
//!
//! **Drawn small, a stroke is kept a pixel wide.** A music font's thinnest
//! strokes -- the uprights of a sharp, a stem -- are under a fiftieth of the
//! em, half a pixel at the size of an icon, and a fill that thin lands
//! between pixel centres and is not drawn: the sharp loses a line. So under
//! the size where that stroke is a pixel the outline's own edges are drawn
//! over the fill, as wide as what the stroke is short of one ([`HINT`]). At
//! the size of a page nothing is added.
//!
//! The table is the host's, not a window's: a codepoint of a music font means
//! one symbol whoever sent it, and the last outline sent for it is the one
//! drawn. It is filled once, tessellated then, and only read while drawing.
//!
//! Tessellating a fill is the `notation` feature's (`lyon`); a build without
//! it keeps the table empty and draws those characters as the face does.

use std::collections::BTreeMap;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Map, Value};

use super::{GLYPH_H, GLYPH_W};
use crate::host::paint::{Color, Mesh};

/// The units of an outline's coordinates to the em: SMuFL's, and every music
/// font's that follows it.
pub const UNITS_PER_EM: f32 = 1000.0;

/// How many body boxes tall the em is drawn. A note with its stem is about an
/// em, so it stands a little over the capitals beside it, as an icon does.
pub const EM_PER_CELL: f32 = 1.25;

/// The thinnest stroke the hint keeps: a music font's uprights, in its units
/// (a sharp's are 17 of the engraver's own face, a natural's 18).
const THIN: f32 = 17.0;

/// How wide, in pixels, a stroke of [`THIN`] units is kept: a little over one,
/// so the stroke covers a pixel centre wherever it falls.
const HINT: f32 = 1.1;

/// One character's shape, in font units with `y` upward, about the centre of
/// its own bounding box.
struct Shape {
    /// The fill: triangle corners, three to a triangle.
    fill: Vec<[f32; 2]>,
    /// The contours' edges, which the hint draws.
    edges: Vec<[[f32; 2]; 2]>,
    /// The width of the bounding box.
    width: f32,
}

static TABLE: RwLock<BTreeMap<char, Shape>> = RwLock::new(BTreeMap::new());

/// Whether the table holds anything: what every character of every string
/// asks first, so a host no window brought an outline to takes no lock.
static FILLED: AtomicBool = AtomicBool::new(false);

/// The character a table key names: a codepoint in hex (`E1D5`), with or
/// without the `U+` a reference writes it with.
fn key_char(key: &str) -> Option<char> {
    let hex = key.trim_start_matches("U+").trim_start_matches("u+");
    u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
}

/// **Take a window's `glyphs`**: every entry that names a character and whose
/// path fills is added to the table, over what was there for it. An entry that
/// is neither is skipped, as an unknown prop is.
pub fn set(glyphs: &Map<String, Value>) {
    let shapes: Vec<(char, Shape)> = glyphs
        .iter()
        .filter_map(|(key, path)| Some((key_char(key)?, fill(path.as_str()?)?)))
        .collect();
    if shapes.is_empty() {
        return;
    }
    let mut table = TABLE.write().unwrap_or_else(|e| e.into_inner());
    table.extend(shapes);
    FILLED.store(true, Ordering::Relaxed);
}

/// Whether `c` has an outline.
pub fn has(c: char) -> bool {
    FILLED.load(Ordering::Relaxed)
        && TABLE
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&c)
}

/// The shape of the path `d`, centred on its bounding box, or `None` when it
/// fills nothing.
#[cfg(feature = "notation")]
fn fill(d: &str) -> Option<Shape> {
    use crate::host::graphics::score;

    // A fifth of a pixel at the sizes text is drawn at.
    const TOLERANCE: f32 = 4.0;
    let mut fill = score::triangles(d, TOLERANCE);
    if fill.is_empty() {
        return None;
    }
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for p in &fill {
        for axis in 0..2 {
            lo[axis] = lo[axis].min(p[axis]);
            hi[axis] = hi[axis].max(p[axis]);
        }
    }
    let centre = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
    let about = |p: &mut [f32; 2]| {
        p[0] -= centre[0];
        p[1] -= centre[1];
    };
    fill.iter_mut().for_each(about);
    let mut edges = score::edges(d, TOLERANCE);
    edges.iter_mut().flatten().for_each(about);
    Some(Shape {
        fill,
        edges,
        width: hi[0] - lo[0],
    })
}

#[cfg(not(feature = "notation"))]
fn fill(_d: &str) -> Option<Shape> {
    None
}

/// Pixels to the font unit when the body box is drawn at `scale`.
fn pixels(scale: f32) -> f32 {
    GLYPH_H as f32 * scale * EM_PER_CELL / UNITS_PER_EM
}

/// **How far the pen steps over `c`** at `scale`, when it has an outline: the
/// width of its shape and one font pixel of bearing on either side, and never
/// under the cell a character takes.
pub fn advance(c: char, scale: f32) -> Option<f32> {
    if !FILLED.load(Ordering::Relaxed) {
        return None;
    }
    let table = TABLE.read().unwrap_or_else(|e| e.into_inner());
    let shape = table.get(&c)?;
    Some(step(shape, scale))
}

fn step(shape: &Shape, scale: f32) -> f32 {
    let cell = (GLYPH_W + 1) as f32 * scale;
    (shape.width * pixels(scale) + 2.0 * scale).max(cell)
}

/// **Draws `c` as its outline** on the line whose body box's top-left is
/// `(x, y)`, and answers how far the pen steps -- or `None` when it has no
/// outline.
pub fn draw(mesh: &mut Mesh, c: char, x: f32, y: f32, scale: f32, color: Color) -> Option<f32> {
    if !FILLED.load(Ordering::Relaxed) {
        return None;
    }
    let table = TABLE.read().unwrap_or_else(|e| e.into_inner());
    let shape = table.get(&c)?;
    let step = step(shape, scale);
    let k = pixels(scale);
    // Centred in its step less the trailing font pixel a character leaves,
    // and on whole pixels: the same symbol is the same picture wherever a
    // layout puts it.
    let cx = (x + (step - scale) * 0.5).round();
    let cy = (y + GLYPH_H as f32 * scale * 0.5).round();
    // the font's `y` goes up and the screen's down
    let at = |p: [f32; 2]| [cx + p[0] * k, cy - p[1] * k];
    for corner in shape.fill.as_chunks::<3>().0 {
        mesh.tri(at(corner[0]), at(corner[1]), at(corner[2]), color);
    }
    let short = HINT - THIN * k;
    if short > 0.0 {
        for [a, b] in &shape.edges {
            mesh.line(at(*a), at(*b), short, color);
        }
    }
    Some(step)
}

#[cfg(all(test, feature = "notation"))]
mod tests {
    use serde_json::json;

    use super::*;

    /// A square of 500 units, off centre; a bar the width of a sharp's
    /// upright; and a shape two ems wide -- in private-use codepoints no other
    /// test draws.
    const SQUARE: char = '\u{F4A0}';
    const HAIR: char = '\u{F4A2}';
    const WIDE: char = '\u{F4A3}';

    fn table() -> Map<String, Value> {
        json!({
            "F4A0": "M100 100h500v500h-500z",
            "F4A1": "not a path",
            "F4A2": "M0 0h17v700h-17z",
            "F4A3": "M0 0h2000v300h-2000z",
            "zz": "M0 0h1v1z",
        })
        .as_object()
        .cloned()
        .unwrap()
    }

    /// The box the mesh's triangles span: `[left, top, right, bottom]`.
    fn bounds(mesh: &Mesh) -> [f32; 4] {
        mesh.positions()
            .fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, (x, y)| {
                [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)]
            })
    }

    #[test]
    fn a_character_with_an_outline_is_drawn_as_its_shape_on_the_line() {
        set(&table());
        assert!(has(SQUARE));
        assert!(!has('\u{F4A1}'), "a path that fills nothing is skipped");
        // large enough that nothing is hinted: the shape alone
        let scale = 16.0;
        let mut mesh = Mesh::new();
        let step = draw(&mut mesh, SQUARE, 10.0, 20.0, scale, [1.0; 4]).expect("an outline");
        let [left, top, right, bottom] = bounds(&mesh);
        // 500 units of an em 1.25 body boxes tall
        let side = 500.0 / UNITS_PER_EM * EM_PER_CELL * GLYPH_H as f32 * scale;
        assert!((right - left - side).abs() < 0.01 && (bottom - top - side).abs() < 0.01);
        // centred on the body box's height, and in its own step
        let cy = 20.0 + GLYPH_H as f32 * scale * 0.5;
        assert!(((top + bottom) * 0.5 - cy).abs() <= 0.5);
        assert!(((left + right) * 0.5 - (10.0 + (step - scale) * 0.5)).abs() <= 0.5);
        // and one without is left to the face
        assert!(draw(&mut mesh, 'a', 0.0, 0.0, 2.0, [1.0; 4]).is_none());
    }

    /// **Its advance is its own**: the width of the shape and a bearing, and
    /// never under the cell a character takes.
    #[test]
    fn it_steps_by_its_own_width_and_text_draws_it() {
        use crate::host::font;

        set(&table());
        let cell = (GLYPH_W + 1) as f32 * 2.0;
        // a hairline is narrower than a character and takes a character's cell
        assert_eq!(advance(HAIR, 2.0), Some(cell));
        // a shape two ems wide takes its width and the two bearings
        let wide = 2000.0 / UNITS_PER_EM * EM_PER_CELL * GLYPH_H as f32 * 2.0;
        assert_eq!(advance(WIDE, 2.0), Some(wide + 4.0));
        assert_eq!(font::advance_of(WIDE, 2.0), wide + 4.0);
        assert_eq!(font::width("\u{F4A3}\u{F4A2}", 2.0), wide + 4.0 + cell);
        // text draws the shape, not the box an unknown character is
        let mut mesh = Mesh::new();
        font::text(&mut mesh, "\u{F4A0}", 0.0, 0.0, 16.0, [1.0; 4]);
        let [left, _, right, _] = bounds(&mesh);
        let side = 500.0 / UNITS_PER_EM * EM_PER_CELL * GLYPH_H as f32 * 16.0;
        assert!((right - left - side).abs() < 0.01);
    }

    /// **Drawn small, a stroke thinner than a pixel is kept one wide.** At an
    /// icon's size a sharp's upright is half a pixel; the edges drawn over the
    /// fill bring it to a little over one, so it covers a pixel centre
    /// wherever it lands. Large, nothing is added.
    #[test]
    fn a_stroke_thinner_than_a_pixel_is_kept_a_pixel_wide() {
        set(&table());
        // an em of 26 px: 17 units are 0.45 px
        let scale = 26.0 / (GLYPH_H as f32 * EM_PER_CELL);
        let mut mesh = Mesh::new();
        draw(&mut mesh, HAIR, 0.0, 0.0, scale, [1.0; 4]);
        let [left, _, right, _] = bounds(&mesh);
        assert!((right - left - HINT).abs() < 0.05, "{}", right - left);
        // at the size of a page the upright is its own width
        let big = 400.0 / (GLYPH_H as f32 * EM_PER_CELL);
        let mut mesh = Mesh::new();
        draw(&mut mesh, HAIR, 0.0, 0.0, big, [1.0; 4]);
        let [left, _, right, _] = bounds(&mesh);
        assert!((right - left - 17.0 * 0.4).abs() < 0.01);
    }
}
