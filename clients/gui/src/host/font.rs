//! A compact embedded 5x7 bitmap font, drawn as pixel quads through the painter.
//!
//! Controls need legible labels and values; the host stays dependency-free, so
//! rather than a font-rasterizer crate it embeds one tiny fixed-cell font and
//! emits each lit font-pixel as a small filled rectangle into the same [`Mesh`]
//! the rest of the chrome uses — no texture, no second pipeline. The glyph set
//! covers `A`-`Z`, `0`-`9`, space and a handful of symbols; lowercase renders as
//! uppercase, and anything else falls back to a box, which is enough for
//! instrument-panel labels and numeric read-outs. Pretty proportional text is a
//! future refinement.

use super::layout::Rect;
use super::paint::{Color, Mesh};

/// Glyph cell: 5 columns x 7 rows, with one column of spacing after each glyph.
pub const GLYPH_W: usize = 5;
pub const GLYPH_H: usize = 7;
const ADVANCE: usize = GLYPH_W + 1;

/// Each glyph is 7 row bytes; bit 4 (`0x10`) is the leftmost of 5 columns.
fn glyph(c: char) -> [u8; 7] {
    let c = c.to_ascii_uppercase();
    match c {
        ' ' => [0, 0, 0, 0, 0, 0, 0],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '.' => [0, 0, 0, 0, 0, 0x0C, 0x0C],
        ',' => [0, 0, 0, 0, 0x0C, 0x04, 0x08],
        '-' => [0, 0, 0, 0x1F, 0, 0, 0],
        '+' => [0, 0x04, 0x04, 0x1F, 0x04, 0x04, 0],
        ':' => [0, 0x0C, 0x0C, 0, 0x0C, 0x0C, 0],
        '/' => [0x01, 0x02, 0x02, 0x04, 0x08, 0x08, 0x10],
        '(' => [0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02],
        ')' => [0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08],
        '%' => [0x19, 0x1A, 0x04, 0x08, 0x0B, 0x13, 0x00],
        _ => [0x1F, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1F], // fallback box
    }
}

/// The pixel width of `s` rendered at `scale` (font-pixels per cell-pixel).
pub fn width(s: &str, scale: f32) -> f32 {
    (s.chars().count() * ADVANCE) as f32 * scale
}

/// The pixel height of one line at `scale`.
pub fn height(scale: f32) -> f32 {
    GLYPH_H as f32 * scale
}

/// Appends `s` to `mesh` with its top-left at `(x, y)`, each font-pixel a
/// `scale` x `scale` rectangle of `color`.
pub fn text(mesh: &mut Mesh, s: &str, x: f32, y: f32, scale: f32, color: Color) {
    let mut pen_x = x;
    for ch in s.chars() {
        let g = glyph(ch);
        for (row, bits) in g.iter().enumerate() {
            for col in 0..GLYPH_W {
                if bits & (0x10 >> col) != 0 {
                    mesh.rect(
                        Rect::new(
                            pen_x + col as f32 * scale,
                            y + row as f32 * scale,
                            scale,
                            scale,
                        ),
                        color,
                    );
                }
            }
        }
        pen_x += ADVANCE as f32 * scale;
    }
}

/// Appends `s` centered horizontally in `area` and vertically, clipped to it.
pub fn text_centered(mesh: &mut Mesh, s: &str, area: Rect, scale: f32, color: Color) {
    let tw = width(s, scale);
    let th = height(scale);
    let x = area.x + (area.w - tw) * 0.5;
    let y = area.y + (area.h - th) * 0.5;
    text(mesh, s, x.max(area.x), y.max(area.y), scale, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_scales_with_length_and_scale() {
        assert_eq!(width("AB", 2.0), (2 * ADVANCE) as f32 * 2.0);
        assert_eq!(width("", 2.0), 0.0);
    }

    #[test]
    fn space_emits_no_pixels_but_advances() {
        let mut m = Mesh::new();
        text(&mut m, " ", 0.0, 0.0, 2.0, [1.0; 4]);
        assert!(m.is_empty(), "space has no lit pixels");
    }

    #[test]
    fn lit_pixels_render_as_quads() {
        // text() emits exactly one quad (6 vertices) per lit font-pixel.
        let mut m = Mesh::new();
        text(&mut m, "I", 0.0, 0.0, 1.0, [1.0; 4]);
        let lit: u32 = glyph('I').iter().map(|b| b.count_ones()).sum();
        assert!(lit > 0);
        assert_eq!(m.vertex_count(), 6 * lit);
    }

    #[test]
    fn lowercase_maps_to_uppercase() {
        assert_eq!(glyph('a'), glyph('A'));
    }
}
