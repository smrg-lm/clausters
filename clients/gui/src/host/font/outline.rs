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
//! **A shape in the bitmap's cell, as the symbol set is.** It takes the
//! nominal advance and is centred in the body box, at one scale for the whole
//! table ([`EM_PER_CELL`]) so a whole note and a quarter keep their sizes
//! against each other; what is taller than the cell overshoots it, as an
//! accent does. That keeps every measurement of a string what it was -- no
//! layout pass learns about a second kind of character.
//!
//! The table is the host's, not a window's: a codepoint of a music font means
//! one symbol whoever sent it, and the last outline sent for it is the one
//! drawn. It is filled once, tessellated then, and only read while drawing.
//!
//! Tessellating a fill is the `notation` feature's (`lyon`); a build without
//! it keeps the table empty and draws those characters as the face does.

use std::collections::BTreeMap;
use std::sync::RwLock;

use serde_json::{Map, Value};

use super::{GLYPH_H, GLYPH_W};
use crate::host::paint::{Color, Mesh};

/// The units of an outline's coordinates to the em: SMuFL's, and every music
/// font's that follows it.
pub const UNITS_PER_EM: f32 = 1000.0;

/// How many body boxes tall the em is drawn. A note with its stem is about an
/// em, so it stands a little over the capitals beside it, as an icon does.
pub const EM_PER_CELL: f32 = 1.25;

/// One character's fill: triangle corners in font units with `y` upward,
/// about the centre of its own bounding box.
type Shape = Vec<[f32; 2]>;

static TABLE: RwLock<BTreeMap<char, Shape>> = RwLock::new(BTreeMap::new());

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
}

/// Whether `c` has an outline.
pub fn has(c: char) -> bool {
    TABLE
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(&c)
}

/// The fill of the path `d`, centred on its bounding box, or `None` when it
/// fills nothing.
#[cfg(feature = "notation")]
fn fill(d: &str) -> Option<Shape> {
    // A fifth of a pixel at the sizes text is drawn at.
    const TOLERANCE: f32 = 4.0;
    let mut corners = crate::host::graphics::score::triangles(d, TOLERANCE);
    if corners.is_empty() {
        return None;
    }
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for p in &corners {
        for axis in 0..2 {
            lo[axis] = lo[axis].min(p[axis]);
            hi[axis] = hi[axis].max(p[axis]);
        }
    }
    let centre = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
    for p in &mut corners {
        p[0] -= centre[0];
        p[1] -= centre[1];
    }
    Some(corners)
}

#[cfg(not(feature = "notation"))]
fn fill(_d: &str) -> Option<Shape> {
    None
}

/// **Draws `c` as its outline** in the cell whose body box's top-left is
/// `(x, y)`, and answers whether it has one.
pub fn draw(mesh: &mut Mesh, c: char, x: f32, y: f32, scale: f32, color: Color) -> bool {
    let table = TABLE.read().unwrap_or_else(|e| e.into_inner());
    let Some(shape) = table.get(&c) else {
        return false;
    };
    let (w, h) = (GLYPH_W as f32 * scale, GLYPH_H as f32 * scale);
    let (cx, cy) = (x + w * 0.5, y + h * 0.5);
    let k = h * EM_PER_CELL / UNITS_PER_EM;
    // the font's `y` goes up and the screen's down
    let at = |p: [f32; 2]| [cx + p[0] * k, cy - p[1] * k];
    for corner in shape.as_chunks::<3>().0 {
        mesh.tri(at(corner[0]), at(corner[1]), at(corner[2]), color);
    }
    true
}

#[cfg(all(test, feature = "notation"))]
mod tests {
    use serde_json::json;

    use super::*;

    /// A square of 500 units, off centre -- in a private-use codepoint no
    /// other test draws.
    const SQUARE: char = '\u{F4A0}';

    fn table() -> Map<String, Value> {
        json!({"F4A0": "M100 100h500v500h-500z", "F4A1": "not a path", "zz": "M0 0h1v1z"})
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
    fn a_character_with_an_outline_is_drawn_as_its_shape_centred_in_the_cell() {
        set(&table());
        assert!(has(SQUARE));
        assert!(!has('\u{F4A1}'), "a path that fills nothing is skipped");
        let mut mesh = Mesh::new();
        assert!(draw(&mut mesh, SQUARE, 10.0, 20.0, 2.0, [1.0; 4]));
        let [left, top, right, bottom] = bounds(&mesh);
        // 500 units of an em 1.25 body boxes tall, the body box 14 px
        let side = 500.0 / UNITS_PER_EM * EM_PER_CELL * 14.0;
        let (cx, cy) = (10.0 + 5.0, 20.0 + 7.0);
        assert!((right - left - side).abs() < 0.01 && (bottom - top - side).abs() < 0.01);
        assert!((left - (cx - side / 2.0)).abs() < 0.01);
        assert!((top - (cy - side / 2.0)).abs() < 0.01);
        // and one without is left to the face
        assert!(!draw(&mut mesh, 'a', 0.0, 0.0, 2.0, [1.0; 4]));
    }

    #[test]
    fn it_takes_the_cell_every_character_takes_and_text_draws_it() {
        use crate::host::font;

        set(&table());
        assert_eq!(font::advance_of(SQUARE, 2.0), font::advance(2.0));
        let mut mesh = Mesh::new();
        font::text(&mut mesh, "\u{F4A0}", 0.0, 0.0, 2.0, [1.0; 4]);
        // the square's two triangles, and not the box an unknown character is
        let [left, _, right, _] = bounds(&mesh);
        let side = 500.0 / UNITS_PER_EM * EM_PER_CELL * 14.0;
        assert!((right - left - side).abs() < 0.01);
    }
}
