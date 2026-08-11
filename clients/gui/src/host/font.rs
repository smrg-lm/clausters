//! A compact embedded bitmap font, drawn as pixel quads through the painter.
//!
//! Controls need legible labels and values; the host stays dependency-free, so
//! rather than a font-rasterizer crate it embeds one tiny fixed-cell font and
//! emits each lit font-pixel as a small filled rectangle into the same [`Mesh`]
//! the rest of the chrome uses — no texture, no second pipeline.
//!
//! **The cell is declared and the face is drawn to fit it, never the other way
//! round.** [`GLYPH_H`] is the **body box**: the seven rows a capital fills, and
//! the height every layout reserves ([`height`]). A glyph may put ink *outside*
//! that box — [`ASCENT`] rows above it for a diacritic, [`DESCENT`] below for a
//! descender — the way a real typeface overshoots its cap height. That split is
//! what let lowercase and Latin-1 land without moving a single rectangle: the
//! box is the same seven rows it always was, drawn at the same origin, so every
//! shipped GuiDef lays out exactly as before.
//!
//! The glyph set covers ASCII (letters in **both** cases, digits, and the
//! punctuation an OSC message and free text need) plus the Latin-1 letters,
//! which are **composed rather than enumerated**: a base glyph plus a mark
//! placed two rows above its own topmost ink ([`decompose`]). So `á` is `a`
//! with an acute, `Ñ` is `N` with a tilde, and adding a mark is one row of a
//! table rather than ninety-six hand-drawn bitmaps. Anything else falls back to
//! a box.

use super::layout::Rect;
use super::paint::{Color, Mesh};

/// Glyph cell width: 5 columns, with one column of spacing after each glyph.
pub const GLYPH_W: usize = 5;
/// The **body box**: the rows a capital fills, and the line height every layout
/// reserves. Ink outside it ([`ASCENT`], [`DESCENT`]) is overshoot, not size.
pub const GLYPH_H: usize = 7;
/// Rows a glyph may reach **above** the body box — where a diacritic goes.
/// Only an accented *capital* uses them: a lowercase letter's mark sits over
/// its x-height, inside the box.
pub const ASCENT: usize = 2;
/// Rows a glyph may reach **below** the body box — a descender's tail, and the
/// cedilla.
pub const DESCENT: usize = 1;
/// The rows one glyph bitmap carries: the ascent, the body, the descent.
const ROWS: usize = ASCENT + GLYPH_H + DESCENT;
const ADVANCE: usize = GLYPH_W + 1;

/// The default glyph scale (the `text_size` prop's default): font-pixels per
/// cell pixel, the size every widget drew at before the prop existed.
pub const DEFAULT_SIZE: f32 = 2.0;

/// One glyph: `ROWS` row bytes, index 0 the topmost ascent row and
/// `ASCENT + GLYPH_H` the descent. Bit 4 (`0x10`) is the leftmost of 5 columns.
type Bitmap = [u8; ROWS];

/// A glyph whose ink stays inside the body box — every capital, every digit,
/// and the lowercase letters without a tail.
const fn body(rows: [u8; GLYPH_H]) -> Bitmap {
    [
        0, 0, rows[0], rows[1], rows[2], rows[3], rows[4], rows[5], rows[6], 0,
    ]
}

/// ...and one that hangs a `tail` row below the box (`g`, `j`, `p`, `q`, `y`).
const fn descending(rows: [u8; GLYPH_H], tail: u8) -> Bitmap {
    [
        0, 0, rows[0], rows[1], rows[2], rows[3], rows[4], rows[5], rows[6], tail,
    ]
}

/// A **mark** a Latin-1 letter is composed with: two rows drawn above the
/// base's own topmost ink, or — the cedilla — one row hung under it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    Acute,
    Grave,
    Circumflex,
    Diaeresis,
    Tilde,
    Ring,
    /// Under the letter rather than over it, so it lands in the descent row.
    Cedilla,
    /// Neither over nor under: a stroke drawn *through* the body (`ø`).
    Slash,
}

impl Mark {
    /// The two rows this mark draws, top first. The cedilla and the slash draw
    /// elsewhere and answer with nothing here.
    fn rows(self) -> [u8; 2] {
        match self {
            // A stroke leaning right, and its mirror.
            Mark::Acute => [0x02, 0x04],
            Mark::Grave => [0x08, 0x04],
            Mark::Circumflex => [0x04, 0x0A],
            Mark::Diaeresis => [0x00, 0x0A],
            // The hump on the upper row, its two ends on the lower one.
            Mark::Tilde => [0x0C, 0x13],
            Mark::Ring => [0x0E, 0x0A],
            Mark::Cedilla | Mark::Slash => [0x00, 0x00],
        }
    }
}

/// A Latin-1 letter as **the base it is drawn from and the mark over it**, or
/// `None` for a character that is a bitmap of its own.
///
/// `i` and `j` decompose to their **dotless** forms, so an accent replaces the
/// dot instead of stacking on it — which is what the letters mean, and what
/// keeps `í` inside the body box like every other lowercase.
fn decompose(c: char) -> Option<(char, Mark)> {
    Some(match c {
        'À' => ('A', Mark::Grave),
        'Á' => ('A', Mark::Acute),
        'Â' => ('A', Mark::Circumflex),
        'Ã' => ('A', Mark::Tilde),
        'Ä' => ('A', Mark::Diaeresis),
        'Å' => ('A', Mark::Ring),
        'Ç' => ('C', Mark::Cedilla),
        'È' => ('E', Mark::Grave),
        'É' => ('E', Mark::Acute),
        'Ê' => ('E', Mark::Circumflex),
        'Ë' => ('E', Mark::Diaeresis),
        'Ì' => ('I', Mark::Grave),
        'Í' => ('I', Mark::Acute),
        'Î' => ('I', Mark::Circumflex),
        'Ï' => ('I', Mark::Diaeresis),
        'Ñ' => ('N', Mark::Tilde),
        'Ò' => ('O', Mark::Grave),
        'Ó' => ('O', Mark::Acute),
        'Ô' => ('O', Mark::Circumflex),
        'Õ' => ('O', Mark::Tilde),
        'Ö' => ('O', Mark::Diaeresis),
        'Ø' => ('O', Mark::Slash),
        'Ù' => ('U', Mark::Grave),
        'Ú' => ('U', Mark::Acute),
        'Û' => ('U', Mark::Circumflex),
        'Ü' => ('U', Mark::Diaeresis),
        'Ý' => ('Y', Mark::Acute),
        'à' => ('a', Mark::Grave),
        'á' => ('a', Mark::Acute),
        'â' => ('a', Mark::Circumflex),
        'ã' => ('a', Mark::Tilde),
        'ä' => ('a', Mark::Diaeresis),
        'å' => ('a', Mark::Ring),
        'ç' => ('c', Mark::Cedilla),
        'è' => ('e', Mark::Grave),
        'é' => ('e', Mark::Acute),
        'ê' => ('e', Mark::Circumflex),
        'ë' => ('e', Mark::Diaeresis),
        // The dotless forms: the mark takes the dot's place.
        'ì' => ('\u{131}', Mark::Grave),
        'í' => ('\u{131}', Mark::Acute),
        'î' => ('\u{131}', Mark::Circumflex),
        'ï' => ('\u{131}', Mark::Diaeresis),
        'ñ' => ('n', Mark::Tilde),
        'ò' => ('o', Mark::Grave),
        'ó' => ('o', Mark::Acute),
        'ô' => ('o', Mark::Circumflex),
        'õ' => ('o', Mark::Tilde),
        'ö' => ('o', Mark::Diaeresis),
        'ø' => ('o', Mark::Slash),
        'ù' => ('u', Mark::Grave),
        'ú' => ('u', Mark::Acute),
        'û' => ('u', Mark::Circumflex),
        'ü' => ('u', Mark::Diaeresis),
        'ý' => ('y', Mark::Acute),
        'ÿ' => ('y', Mark::Diaeresis),
        _ => return None,
    })
}

/// The bitmap of a character that is not composed from another one.
fn base(c: char) -> Bitmap {
    match c {
        ' ' | '\u{a0}' => body([0, 0, 0, 0, 0, 0, 0]),
        '0' => body([0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E]),
        '1' => body([0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E]),
        '2' => body([0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F]),
        '3' => body([0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E]),
        '4' => body([0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02]),
        '5' => body([0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E]),
        '6' => body([0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E]),
        '7' => body([0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08]),
        '8' => body([0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E]),
        '9' => body([0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C]),
        'A' => body([0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11]),
        'B' => body([0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E]),
        'C' => body([0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E]),
        'D' => body([0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E]),
        'E' => body([0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F]),
        'F' => body([0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10]),
        'G' => body([0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F]),
        'H' => body([0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11]),
        'I' => body([0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E]),
        'J' => body([0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C]),
        'K' => body([0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11]),
        'L' => body([0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F]),
        'M' => body([0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11]),
        'N' => body([0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11]),
        'O' => body([0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E]),
        'P' => body([0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10]),
        'Q' => body([0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D]),
        'R' => body([0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11]),
        'S' => body([0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E]),
        'T' => body([0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04]),
        'U' => body([0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E]),
        'V' => body([0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04]),
        'W' => body([0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11]),
        'X' => body([0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11]),
        'Y' => body([0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04]),
        'Z' => body([0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F]),
        // Lowercase: an x-height of five rows (the box's rows 2-6), the
        // ascenders reaching its top and the tails hanging one row under it.
        'a' => body([0, 0, 0x0E, 0x01, 0x0F, 0x11, 0x0F]),
        'b' => body([0x10, 0x10, 0x1E, 0x11, 0x11, 0x11, 0x1E]),
        'c' => body([0, 0, 0x0E, 0x11, 0x10, 0x11, 0x0E]),
        'd' => body([0x01, 0x01, 0x0F, 0x11, 0x11, 0x11, 0x0F]),
        'e' => body([0, 0, 0x0E, 0x11, 0x1F, 0x10, 0x0E]),
        'f' => body([0x06, 0x08, 0x1C, 0x08, 0x08, 0x08, 0x08]),
        'g' => descending([0, 0, 0x0F, 0x11, 0x0F, 0x01, 0x11], 0x0E),
        'h' => body([0x10, 0x10, 0x1E, 0x11, 0x11, 0x11, 0x11]),
        'i' => body([0x04, 0, 0x0C, 0x04, 0x04, 0x04, 0x0E]),
        'j' => descending([0x02, 0, 0x06, 0x02, 0x02, 0x02, 0x12], 0x0C),
        'k' => body([0x10, 0x10, 0x12, 0x14, 0x18, 0x14, 0x12]),
        'l' => body([0x0C, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E]),
        'm' => body([0, 0, 0x1A, 0x15, 0x15, 0x15, 0x15]),
        'n' => body([0, 0, 0x1E, 0x11, 0x11, 0x11, 0x11]),
        'o' => body([0, 0, 0x0E, 0x11, 0x11, 0x11, 0x0E]),
        'p' => descending([0, 0, 0x1E, 0x11, 0x11, 0x11, 0x1E], 0x10),
        'q' => descending([0, 0, 0x0F, 0x11, 0x11, 0x11, 0x0F], 0x01),
        'r' => body([0, 0, 0x16, 0x18, 0x10, 0x10, 0x10]),
        's' => body([0, 0, 0x0F, 0x10, 0x0E, 0x01, 0x1E]),
        't' => body([0x08, 0x08, 0x1C, 0x08, 0x08, 0x09, 0x06]),
        'u' => body([0, 0, 0x11, 0x11, 0x11, 0x13, 0x0D]),
        'v' => body([0, 0, 0x11, 0x11, 0x11, 0x0A, 0x04]),
        'w' => body([0, 0, 0x11, 0x11, 0x15, 0x15, 0x0A]),
        'x' => body([0, 0, 0x11, 0x0A, 0x04, 0x0A, 0x11]),
        'y' => descending([0, 0, 0x11, 0x11, 0x0F, 0x01, 0x11], 0x0E),
        'z' => body([0, 0, 0x1F, 0x02, 0x04, 0x08, 0x1F]),
        // The dotless forms an accented `i`/`j` is composed from.
        '\u{131}' => body([0, 0, 0x0C, 0x04, 0x04, 0x04, 0x0E]),
        '.' => body([0, 0, 0, 0, 0, 0x0C, 0x0C]),
        ',' => body([0, 0, 0, 0, 0x0C, 0x04, 0x08]),
        '-' => body([0, 0, 0, 0x1F, 0, 0, 0]),
        '+' => body([0, 0x04, 0x04, 0x1F, 0x04, 0x04, 0]),
        ':' => body([0, 0x0C, 0x0C, 0, 0x0C, 0x0C, 0]),
        ';' => body([0, 0x0C, 0x0C, 0, 0x0C, 0x04, 0x08]),
        '/' => body([0x01, 0x02, 0x02, 0x04, 0x08, 0x08, 0x10]),
        '\\' => body([0x10, 0x08, 0x08, 0x04, 0x02, 0x02, 0x01]),
        '(' => body([0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02]),
        ')' => body([0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08]),
        '[' => body([0x0E, 0x08, 0x08, 0x08, 0x08, 0x08, 0x0E]),
        ']' => body([0x0E, 0x02, 0x02, 0x02, 0x02, 0x02, 0x0E]),
        '{' => body([0x06, 0x08, 0x08, 0x10, 0x08, 0x08, 0x06]),
        '}' => body([0x0C, 0x02, 0x02, 0x01, 0x02, 0x02, 0x0C]),
        '%' => body([0x19, 0x1A, 0x04, 0x08, 0x0B, 0x13, 0x00]),
        // The characters an OSC message and free text need beyond labels: an
        // underscore (SynthDef/param names), the bracket/quote/compare/sign set.
        '_' => body([0, 0, 0, 0, 0, 0, 0x1F]),
        '=' => body([0, 0, 0x1F, 0, 0x1F, 0, 0]),
        '<' => body([0x02, 0x04, 0x08, 0x10, 0x08, 0x04, 0x02]),
        '>' => body([0x08, 0x04, 0x02, 0x01, 0x02, 0x04, 0x08]),
        '"' => body([0x0A, 0x0A, 0x0A, 0, 0, 0, 0]),
        '\'' => body([0x04, 0x04, 0x04, 0, 0, 0, 0]),
        '?' => body([0x0E, 0x11, 0x01, 0x02, 0x04, 0, 0x04]),
        '!' => body([0x04, 0x04, 0x04, 0x04, 0x04, 0, 0x04]),
        '*' => body([0, 0x04, 0x15, 0x0E, 0x15, 0x04, 0]),
        '#' => body([0x0A, 0x0A, 0x1F, 0x0A, 0x1F, 0x0A, 0x0A]),
        '&' => body([0x08, 0x14, 0x14, 0x08, 0x15, 0x12, 0x0D]),
        '@' => body([0x0E, 0x11, 0x17, 0x15, 0x17, 0x10, 0x0E]),
        '|' => body([0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04]),
        '~' => body([0, 0, 0x0C, 0x13, 0, 0, 0]),
        '^' => body([0x04, 0x0A, 0x11, 0, 0, 0, 0]),
        '$' => body([0x04, 0x0F, 0x14, 0x0E, 0x05, 0x1E, 0x04]),
        // The Latin-1 marks and signs that are not a letter with something over
        // it — the ones a Spanish, French or German label actually reaches for.
        '¡' => body([0x04, 0, 0x04, 0x04, 0x04, 0x04, 0x04]),
        '¿' => body([0x04, 0, 0x04, 0x08, 0x10, 0x11, 0x0E]),
        '«' => body([0, 0x05, 0x0A, 0x14, 0x0A, 0x05, 0]),
        '»' => body([0, 0x14, 0x0A, 0x05, 0x0A, 0x14, 0]),
        '°' => body([0x0E, 0x0A, 0x0E, 0, 0, 0, 0]),
        '·' => body([0, 0, 0, 0x0C, 0x0C, 0, 0]),
        '±' => body([0x04, 0x04, 0x1F, 0x04, 0x04, 0, 0x1F]),
        '×' => body([0, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0]),
        '÷' => body([0, 0x04, 0, 0x1F, 0, 0x04, 0]),
        'µ' => descending([0, 0, 0x11, 0x11, 0x11, 0x13, 0x1D], 0x10),
        '§' => body([0x0E, 0x10, 0x0E, 0x11, 0x0E, 0x01, 0x0E]),
        '©' => body([0x0E, 0x11, 0x17, 0x15, 0x17, 0x11, 0x0E]),
        'ª' => body([0x0E, 0x02, 0x0E, 0x0A, 0x0E, 0, 0]),
        'º' => body([0x0E, 0x0A, 0x0E, 0, 0x0E, 0, 0]),
        '¢' => body([0x04, 0x0E, 0x14, 0x14, 0x14, 0x0E, 0x04]),
        '£' => body([0x06, 0x08, 0x08, 0x1C, 0x08, 0x08, 0x1F]),
        '¥' => body([0x11, 0x0A, 0x04, 0x1F, 0x04, 0x1F, 0x04]),
        // The letters Latin-1 spells that no base plus a mark makes.
        'ß' => body([0x0C, 0x12, 0x12, 0x1C, 0x12, 0x12, 0x1C]),
        'Æ' => body([0x0F, 0x14, 0x14, 0x1F, 0x14, 0x14, 0x17]),
        'æ' => body([0, 0, 0x1A, 0x05, 0x1F, 0x14, 0x0B]),
        // The Icelandic pair: an eth is a struck D, a thorn a P on a stem.
        'Ð' => body([0x0E, 0x09, 0x09, 0x1D, 0x09, 0x09, 0x0E]),
        'ð' => body([0x0C, 0x0A, 0x06, 0x0B, 0x11, 0x11, 0x0E]),
        'Þ' => body([0x10, 0x1C, 0x12, 0x12, 0x1C, 0x10, 0x10]),
        'þ' => descending([0x10, 0x10, 0x1C, 0x12, 0x12, 0x1C, 0x10], 0x10),
        // The single-cell ellipsis clipped text ends in.
        '\u{2026}' => body([0, 0, 0, 0, 0, 0, 0x15]),
        _ => body([0x1F, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1F]), // fallback box
    }
}

/// The bitmap `c` draws: its own, or a base with a mark composed onto it.
fn glyph(c: char) -> Bitmap {
    let Some((from, mark)) = decompose(c) else {
        return base(c);
    };
    let mut rows = base(from);
    match mark {
        // Under the letter: the cedilla hangs in the descent row.
        Mark::Cedilla => rows[ROWS - 1] = 0x0C,
        // Through it: the stroke crosses the body's middle rows.
        Mark::Slash => {
            for (i, bits) in [0x02, 0x04, 0x08].into_iter().enumerate() {
                rows[ASCENT + 2 + i] |= bits;
            }
        }
        // Over it, two rows above the base's **own** topmost ink — which puts a
        // lowercase mark over its x-height, inside the body box, and only lifts
        // a capital's into the ascent.
        over => {
            let top = rows.iter().position(|&b| b != 0).unwrap_or(ASCENT);
            for (i, bits) in over.rows().into_iter().enumerate() {
                if let Some(row) = rows.get_mut((top + i).wrapping_sub(2)) {
                    *row |= bits;
                }
            }
        }
    }
    rows
}

/// **One character's nominal advance** at `scale` — the cell the fixed-pitch
/// face steps by, and the unit of the size roles that are sized to hold text.
///
/// It is a property of the *face*, so a table that reserves room for N
/// characters asks here instead of restating the cell. What a proportional face
/// would make variable is the width of a **string** ([`width`]), which is
/// measured where the string changes, never in a layout pass.
pub fn advance(scale: f32) -> f32 {
    ADVANCE as f32 * scale
}

/// The pixel width of `s` rendered at `scale` (font-pixels per cell-pixel).
pub fn width(s: &str, scale: f32) -> f32 {
    s.chars().count() as f32 * advance(scale)
}

/// The pixel height of one line at `scale` — the **body box**, which is what a
/// layout reserves. A diacritic or a descender may draw outside it.
pub fn height(scale: f32) -> f32 {
    GLYPH_H as f32 * scale
}

/// Appends `s` to `mesh` with the **top of its body box** at `(x, y)`, each
/// font-pixel a `scale` x `scale` rectangle of `color`.
///
/// The origin is the body box, not the bitmap: a glyph's ascent rows are drawn
/// *above* `y`. That is what makes an accented label sit on the same baseline
/// as an unaccented one, and what kept this whole family of glyphs from moving
/// any text that was already on screen.
pub fn text(mesh: &mut Mesh, s: &str, x: f32, y: f32, scale: f32, color: Color) {
    let mut pen_x = x;
    let top = y - ASCENT as f32 * scale;
    for ch in s.chars() {
        for (row, bits) in glyph(ch).iter().enumerate() {
            for col in 0..GLYPH_W {
                if bits & (0x10 >> col) != 0 {
                    mesh.rect(
                        Rect::new(
                            pen_x + col as f32 * scale,
                            top + row as f32 * scale,
                            scale,
                            scale,
                        ),
                        color,
                    );
                }
            }
        }
        pen_x += advance(scale);
    }
}

/// The number of glyph cells that fit in `max_w` pixels at `scale`.
pub fn fit_chars(max_w: f32, scale: f32) -> usize {
    if scale <= 0.0 || max_w <= 0.0 {
        return 0;
    }
    (max_w / advance(scale)) as usize
}

/// Appends `s` clipped to `max_w`: text that fits draws whole; longer text
/// draws one cell short and ends in an ellipsis instead of bleeding past the
/// edge into a neighbor.
pub fn text_ellipsis(
    mesh: &mut Mesh,
    s: &str,
    x: f32,
    y: f32,
    max_w: f32,
    scale: f32,
    color: Color,
) {
    let fit = fit_chars(max_w, scale);
    if s.chars().count() <= fit {
        text(mesh, s, x, y, scale, color);
    } else if fit > 0 {
        let cut: String = s.chars().take(fit - 1).collect();
        text(mesh, &format!("{cut}\u{2026}"), x, y, scale, color);
    }
}

/// Greedy word wrap on the font's fixed advance: `s` split into lines of at
/// most `max_cols` cells, breaking at whitespace (a single word longer than a
/// line hard-breaks mid-word). Cheap width math, no shaping.
pub fn wrap(s: &str, max_cols: usize) -> Vec<String> {
    let max_cols = max_cols.max(1);
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut cols = 0usize;
    for word in s.split_whitespace() {
        let mut chars: Vec<char> = word.chars().collect();
        while !chars.is_empty() {
            let sep = if cols > 0 { 1 } else { 0 };
            let room = max_cols.saturating_sub(cols + sep);
            if chars.len() <= room {
                if sep == 1 {
                    line.push(' ');
                }
                line.extend(chars.drain(..));
                cols = line.chars().count();
            } else if cols == 0 {
                // A word longer than a whole line: hard-break it.
                line.extend(chars.drain(..max_cols));
                lines.push(std::mem::take(&mut line));
                cols = 0;
            } else {
                lines.push(std::mem::take(&mut line));
                cols = 0;
            }
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// The vertical advance from one wrapped line's top to the next: the whole
/// glyph — the body box plus the room a diacritic and a descender may take —
/// so a line with tails never touches the accents of the line under it.
pub fn line_advance(scale: f32) -> f32 {
    ROWS as f32 * scale
}

/// Appends `s` centered horizontally in `area` and vertically, clipped to it
/// (overflow ends in an ellipsis).
pub fn text_centered(mesh: &mut Mesh, s: &str, area: Rect, scale: f32, color: Color) {
    let tw = width(s, scale).min(area.w);
    let th = height(scale);
    let x = area.x + (area.w - tw) * 0.5;
    let y = area.y + (area.h - th) * 0.5;
    text_ellipsis(mesh, s, x.max(area.x), y.max(area.y), area.w, scale, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The body rows of `c`, as they were written before the glyph grew an
    /// ascent and a descent around them.
    fn body_of(c: char) -> Vec<u8> {
        glyph(c)[ASCENT..ASCENT + GLYPH_H].to_vec()
    }

    #[test]
    fn width_scales_with_length_and_scale() {
        assert_eq!(width("AB", 2.0), (2 * ADVANCE) as f32 * 2.0);
        assert_eq!(width("", 2.0), 0.0);
        assert_eq!(width("A", 2.0), advance(2.0));
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

    /// **The whole point of the box/overshoot split**: a capital's ink lands
    /// exactly where it landed before the font grew, so no shipped GuiDef moved
    /// a pixel. `text` draws the *body box* top at `y`.
    #[test]
    fn a_capital_still_draws_at_the_origin_it_always_did() {
        let mut m = Mesh::new();
        text(&mut m, "A", 0.0, 0.0, 1.0, [1.0; 4]);
        let top = m.positions().map(|(_, y)| y).fold(f32::MAX, f32::min);
        let bottom = m.positions().map(|(_, y)| y).fold(f32::MIN, f32::max);
        assert_eq!(top, 0.0, "the body box starts at the origin");
        assert_eq!(bottom, GLYPH_H as f32, "and ends where it always did");
        assert_eq!(
            height(1.0),
            GLYPH_H as f32,
            "which is what a layout reserves"
        );
    }

    /// Lowercase is a real lowercase now, and the letters that need a tail have
    /// one — below the box, which is what the descent rows are for.
    #[test]
    fn lowercase_is_its_own_shape_with_real_descenders() {
        assert_ne!(glyph('a'), glyph('A'), "no longer folded to uppercase");
        for c in ['g', 'j', 'p', 'q', 'y'] {
            assert_ne!(glyph(c)[ROWS - 1], 0, "{c} hangs a tail");
        }
        for c in ['a', 'e', 'o', 'n', 'x'] {
            assert_eq!(glyph(c)[ROWS - 1], 0, "{c} does not");
            // ...and an x-height letter leaves the box's top two rows free,
            // which is where its accent will go.
            assert_eq!(&glyph(c)[..ASCENT + 2], &[0; ASCENT + 2]);
        }
    }

    /// A Latin-1 letter is its base plus a mark, so "canción" writes.
    #[test]
    fn latin1_composes_a_base_with_a_mark() {
        // The base's own ink is untouched; the mark is what was added.
        assert_eq!(body_of('ó')[2..], body_of('o')[2..]);
        assert_ne!(glyph('ó'), glyph('o'));
        assert_ne!(glyph('ó'), glyph('ò'), "acute and grave differ");
        assert_ne!(glyph('ñ'), glyph('n'));
        // A lowercase mark sits over the x-height, **inside** the body box —
        // nothing overshoots, so an accented label needs no extra room.
        assert_eq!(&glyph('ó')[..ASCENT], &[0; ASCENT]);
        assert_eq!(&glyph('ñ')[..ASCENT], &[0; ASCENT]);
        // A capital has no room inside, so its mark lifts into the ascent.
        assert_ne!(glyph('Ñ')[ASCENT - 1], 0);
        // The dotless base: an accent replaces the dot rather than stacking.
        assert_eq!(body_of('í')[2..], body_of('i')[2..]);
        assert_ne!(glyph('í')[ASCENT + 1], glyph('i')[ASCENT + 1]);
        // Below and through, the two marks that are not over the letter.
        assert_ne!(glyph('ç')[ROWS - 1], 0, "the cedilla hangs under c");
        assert_ne!(glyph('ø'), glyph('o'), "the slash crosses o");
    }

    /// Every printable Latin-1 character draws something of its own rather than
    /// the fallback box — the instrument that catches a gap in the table.
    #[test]
    fn every_latin1_letter_has_a_glyph() {
        let fallback = base('\u{fffd}');
        for c in ('\u{a1}'..='\u{ff}').filter(|c| c.is_alphabetic()) {
            assert_ne!(glyph(c), fallback, "{c} falls back to the box");
        }
    }

    #[test]
    fn fit_chars_counts_whole_cells() {
        // One cell is ADVANCE * scale pixels wide.
        assert_eq!(fit_chars((3 * ADVANCE) as f32 * 2.0, 2.0), 3);
        assert_eq!(fit_chars((3 * ADVANCE) as f32 * 2.0 - 1.0, 2.0), 2);
        assert_eq!(fit_chars(0.0, 2.0), 0);
        assert_eq!(fit_chars(100.0, 0.0), 0);
    }

    #[test]
    fn text_ellipsis_clips_to_its_width() {
        // Text that fits draws whole; text that overflows never emits a pixel
        // past `x + max_w`, and ends in the ellipsis cell.
        let max_w = (4 * ADVANCE) as f32 * 2.0;
        let mut m = Mesh::new();
        text_ellipsis(&mut m, "ABCDEFGH", 0.0, 0.0, max_w, 2.0, [1.0; 4]);
        let max_x = m.positions().map(|(x, _)| x).fold(f32::MIN, f32::max);
        assert!(max_x <= max_w, "clipped text bleeds past its width");
        // A fitting string is untouched: same mesh as a plain text() call.
        let mut clipped = Mesh::new();
        text_ellipsis(&mut clipped, "ABC", 0.0, 0.0, max_w, 2.0, [1.0; 4]);
        let mut plain = Mesh::new();
        text(&mut plain, "ABC", 0.0, 0.0, 2.0, [1.0; 4]);
        assert_eq!(clipped.vertex_count(), plain.vertex_count());
    }

    /// Wrapped lines clear each other: the leading is the whole glyph, so a
    /// tail on one line cannot touch an accent on the next.
    #[test]
    fn wrapped_lines_leave_room_for_tails_and_marks() {
        assert!(line_advance(2.0) >= (height(2.0) + (ASCENT + DESCENT) as f32 * 2.0));
    }

    #[test]
    fn wrap_breaks_at_spaces() {
        assert_eq!(wrap("one two three", 7), ["one two", "three"]);
        assert_eq!(wrap("one two three", 13), ["one two three"]);
        assert_eq!(wrap("a b", 1), ["a", "b"]);
    }

    #[test]
    fn wrap_hard_breaks_a_long_word() {
        assert_eq!(wrap("abcdefgh", 3), ["abc", "def", "gh"]);
        // A long word after a short one starts on its own line.
        assert_eq!(wrap("hi abcdef", 5), ["hi", "abcde", "f"]);
    }

    #[test]
    fn wrap_collapses_whitespace_and_empty() {
        assert_eq!(wrap("  a   b  ", 10), ["a b"]);
        assert!(wrap("", 10).is_empty());
    }
}
