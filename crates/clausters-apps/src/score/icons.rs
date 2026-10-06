//! **The symbols a music font does not hold**, drawn here.
//!
//! A tool and a palette entry are labelled with a glyph, and the glyphs are
//! the engraver's ([`super::tools::Outlines`]). But a face draws what a page
//! is *set* with, and an editor names more than that: a slur and a tie are
//! curves the engraver computes, a barline is a line it rules, a hairpin two
//! strokes -- none of them a glyph of the face -- and the face an engraver
//! ships may leave out glyphs the standard has (a grace note, a repeat sign).
//! Left to the fallback, each of those is a word cut to a letter in a cell
//! one glyph wide.
//!
//! So the table is **completed** ([`complete`]): for each codepoint below
//! that the engraver handed no outline for, the drawing here is the outline.
//! Where the standard names the symbol the codepoint is SMuFL's, so a face
//! that has it wins; where it names none -- a slur, an empty measure, a break
//! -- the codepoint is one of the editor's own, in a private plane no face
//! writes in ([`OWN`]).
//!
//! An outline is an SVG path in a font's units -- a thousand to the em, `y`
//! upward, a staff of five lines one em high -- which is the form the
//! engraver's own come in and the host draws. A drawing that reuses a glyph
//! of the face (a grace note is the eighth, smaller) is written from that
//! glyph's outline, so it is the face's own note; without the glyph that
//! drawing is left out, and the entry keeps its name.

use std::fmt::Write as _;

use super::tools::Outlines;

/// Where the editor's own codepoints start: the first of the private plane
/// (`U+F0000`), which no music font writes in -- SMuFL's are all in the basic
/// plane's private area.
pub const OWN: u32 = 0xF0000;

/// A slur: a curve over a phrase.
pub const SLUR: &str = "F0001";
/// A tie: two notes of one pitch joined.
pub const TIE: &str = "F0002";
/// The other voice of the staff.
pub const VOICE: &str = "F0003";
/// Nothing: what takes a mark away.
pub const NONE: &str = "F0004";
/// An empty measure opened before.
pub const MEASURE_BEFORE: &str = "F0005";
/// An empty measure opened after.
pub const MEASURE_AFTER: &str = "F0006";
/// Measures taken out.
pub const MEASURE_REMOVE: &str = "F0007";
/// A barline that is not drawn.
pub const BARLINE_INVISIBLE: &str = "F0008";
/// A system break.
pub const BREAK_SYSTEM: &str = "F0009";
/// A page break.
pub const BREAK_PAGE: &str = "F000A";
/// No break: the engraver's to fill.
pub const BREAK_NONE: &str = "F000B";

/// Back to the start: the transport's, which the host's own symbols lack.
pub const REWIND: &str = "F000C";
/// Note entry: a pencil.
pub const ENTRY: &str = "F000D";
/// Let it ring: a tie into nothing.
pub const LET_RING: &str = "F000E";
/// A glissando: a line from one note to another.
pub const GLISSANDO: &str = "F000F";
/// A phrase mark: a slur with its ends turned down.
pub const PHRASE: &str = "F0010";
/// A bracket over a stretch of notes.
pub const BRACKET: &str = "F0011";
/// A beam across a barline.
pub const BEAM_SPAN: &str = "F0012";
/// Two notes alternating: two notes joined by tremolo strokes.
pub const TWO_NOTE_TREMOLO: &str = "F0013";
/// A rehearsal mark: a letter in a box.
pub const REHEARSAL: &str = "F0014";
/// An ending: the bracket of a first-time bar.
pub const ENDING: &str = "F0015";
/// A staff of one line.
pub const ONE_LINE: &str = "F0016";
/// A staff of five lines.
pub const FIVE_LINES: &str = "F0017";
/// Staves joined by a line.
pub const GROUP_LINE: &str = "F0018";
/// A chord rolled up: a wavy line, its arrow at the top.
pub const ARPEGGIO_UP: &str = "F0019";
/// ...and down.
pub const ARPEGGIO_DOWN: &str = "F001A";
/// The window as pages: a sheet with its systems.
pub const PAGE_VIEW: &str = "F001B";
/// The window as one system running on.
pub const LINE_VIEW: &str = "F001C";

/// **A key signature**, by its tonic: the face's sharps or flats in a row, a
/// natural for none -- under codepoints of the editor's own, from `F0020`.
pub const KEY_SIGNS: [(&str, &str, i32); 9] = [
    ("C", "F0020", 0),
    ("G", "F0021", 1),
    ("D", "F0022", 2),
    ("A", "F0023", 3),
    ("E", "F0024", 4),
    ("F", "F0025", -1),
    ("Bb", "F0026", -2),
    ("Eb", "F0027", -3),
    ("Ab", "F0028", -4),
];

/// SMuFL's own, for the symbols it names and a face may leave out.
pub const BARLINE_SINGLE: &str = "E030";
pub const BARLINE_DOUBLE: &str = "E031";
pub const BARLINE_FINAL: &str = "E032";
pub const REPEAT_START: &str = "E040";
pub const REPEAT_END: &str = "E041";
pub const REPEAT_BOTH: &str = "E042";
pub const CRESCENDO: &str = "E53E";
pub const DIMINUENDO: &str = "E53F";
pub const ACCIACCATURA: &str = "E560";
pub const APPOGGIATURA: &str = "E562";
/// A quarter note with its stem down: the second voice.
pub const QUARTER_DOWN: &str = "E1D6";

/// The glyphs of the face a drawing here is written from.
const EIGHTH_UP: &str = "E1D7";
const QUARTER_UP: &str = "E1D5";
const SHARP: &str = "E262";
const FLAT: &str = "E260";
const NATURAL: &str = "E261";

/// The glyphs of the face the drawings here are written from: what the editor
/// asks the engraver for beside the symbols its tools show.
#[must_use]
pub fn drawn_from() -> Vec<&'static str> {
    vec![EIGHTH_UP, QUARTER_UP, SHARP, FLAT, NATURAL]
}

/// Whether `code` is one of the editor's own, which no engraver is asked for.
#[must_use]
pub fn is_own(code: &str) -> bool {
    u32::from_str_radix(code, 16).is_ok_and(|c| c >= OWN)
}

/// **Complete `outlines`**: every symbol this module draws that the table
/// does not hold is added to it. What the engraver handed out stays.
pub fn complete(outlines: &mut Outlines) {
    let drawn: Vec<(&str, Option<String>)> = vec![
        (BARLINE_SINGLE, Some(barlines(&[Bar::Thin]))),
        (BARLINE_DOUBLE, Some(barlines(&[Bar::Thin, Bar::Thin]))),
        (BARLINE_FINAL, Some(barlines(&[Bar::Thin, Bar::Thick]))),
        (
            REPEAT_START,
            Some(barlines(&[Bar::Thick, Bar::Thin, Bar::Dots])),
        ),
        (
            REPEAT_END,
            Some(barlines(&[Bar::Dots, Bar::Thin, Bar::Thick])),
        ),
        (
            REPEAT_BOTH,
            Some(barlines(&[
                Bar::Dots,
                Bar::Thin,
                Bar::Thick,
                Bar::Thin,
                Bar::Dots,
            ])),
        ),
        (BARLINE_INVISIBLE, Some(barlines(&[Bar::Dashed]))),
        (CRESCENDO, Some(hairpin(true))),
        (DIMINUENDO, Some(hairpin(false))),
        (SLUR, Some(slur())),
        (TIE, Some(tie())),
        (NONE, Some(none())),
        (MEASURE_BEFORE, Some(measure(Sign::Plus, true))),
        (MEASURE_AFTER, Some(measure(Sign::Plus, false))),
        (MEASURE_REMOVE, Some(measure(Sign::Minus, true))),
        (BREAK_SYSTEM, Some(break_system())),
        (BREAK_PAGE, Some(break_page())),
        (BREAK_NONE, Some(break_none())),
        (REWIND, Some(rewind())),
        (ENTRY, Some(pencil())),
        (LET_RING, Some(let_ring())),
        (GLISSANDO, Some(glissando())),
        (PHRASE, Some(phrase())),
        (BRACKET, Some(bracket())),
        (BEAM_SPAN, Some(beam_span())),
        (TWO_NOTE_TREMOLO, Some(two_note_tremolo())),
        (REHEARSAL, Some(rehearsal())),
        (ENDING, Some(ending())),
        (ONE_LINE, Some(staff_lines(1))),
        (FIVE_LINES, Some(staff_lines(5))),
        (GROUP_LINE, Some(group_line())),
        (ARPEGGIO_UP, Some(arpeggio(true))),
        (ARPEGGIO_DOWN, Some(arpeggio(false))),
        (PAGE_VIEW, Some(page_view())),
        (LINE_VIEW, Some(line_view())),
        (
            APPOGGIATURA,
            outlines.get(EIGHTH_UP).map(|eighth| grace(eighth, false)),
        ),
        (
            ACCIACCATURA,
            outlines.get(EIGHTH_UP).map(|eighth| grace(eighth, true)),
        ),
        (
            VOICE,
            outlines.get(QUARTER_UP).map(|quarter| voices(quarter)),
        ),
        (
            QUARTER_DOWN,
            outlines.get(QUARTER_UP).map(|quarter| stem_down(quarter)),
        ),
    ];
    let mut drawn = drawn;
    for (_, code, count) in KEY_SIGNS {
        let glyph = match count {
            0 => NATURAL,
            n if n > 0 => SHARP,
            _ => FLAT,
        };
        drawn.push((code, outlines.get(glyph).map(|d| signature(d, count))));
    }
    for (code, path) in drawn {
        if let (false, Some(path)) = (outlines.contains_key(code), path) {
            outlines.insert(code.to_string(), path);
        }
    }
}

// ---- the pen ----

/// A stroke's weight, in font units: a barline's, a hairpin's, a staff's in
/// an icon. Twice a face's own hairline, since these are read small.
const LINE: f64 = 40.0;
/// A staff space.
const SPACE: f64 = 250.0;
/// The height of a five-line staff.
const STAFF: f64 = 4.0 * SPACE;

/// A path being written, every contour of it wound one way.
///
/// **One winding, because the fill is non-zero.** Two contours that overlap
/// fill where they overlap only when they wind alike: a slash wound against
/// the stem it crosses would cut a hole in both. So a polygon is turned to
/// the pen's winding whatever order its corners were given in, and a pen that
/// starts from a glyph of the face takes that glyph's ([`Pen::over`]).
struct Pen {
    d: String,
    /// Whether contours are wound clockwise (in the font's `y`-up frame).
    clockwise: bool,
}

impl Pen {
    fn new() -> Self {
        Self {
            d: String::new(),
            clockwise: false,
        }
    }

    /// A pen that draws over the glyph `d` as written: `d` is the path so
    /// far, and what is drawn next winds as its largest contour does.
    fn over(d: &str) -> Self {
        Self {
            d: d.to_string(),
            clockwise: winding(d) < 0.0,
        }
    }

    /// A closed polygon through `points`.
    fn polygon(&mut self, points: &[(f64, f64)]) {
        let area: f64 = points
            .iter()
            .zip(points.iter().cycle().skip(1))
            .map(|(a, b)| a.0 * b.1 - b.0 * a.1)
            .sum();
        let turn = (area < 0.0) != self.clockwise;
        let ordered: Vec<(f64, f64)> = if turn {
            points.iter().rev().copied().collect()
        } else {
            points.to_vec()
        };
        for (i, (x, y)) in ordered.iter().enumerate() {
            let _ = write!(
                self.d,
                "{}{} {}",
                if i == 0 { "M" } else { "L" },
                number(*x),
                number(*y)
            );
        }
        self.d.push('z');
    }

    fn rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        self.polygon(&[(x, y), (x + w, y), (x + w, y + h), (x, y + h)]);
    }

    /// A straight stroke from `a` to `b`, `weight` wide.
    fn stroke(&mut self, a: (f64, f64), b: (f64, f64), weight: f64) {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = dx.hypot(dy).max(1e-6);
        let (nx, ny) = (-dy / len * weight / 2.0, dx / len * weight / 2.0);
        self.polygon(&[
            (a.0 + nx, a.1 + ny),
            (b.0 + nx, b.1 + ny),
            (b.0 - nx, b.1 - ny),
            (a.0 - nx, a.1 - ny),
        ]);
    }

    /// An ellipse about `centre`, its long axis tilted by `tilt` radians.
    fn ellipse(&mut self, centre: (f64, f64), rx: f64, ry: f64, tilt: f64) {
        self.polygon(&round(centre, rx, ry, tilt));
    }

    /// A hole in what was drawn: an ellipse wound against the pen.
    fn hole(&mut self, centre: (f64, f64), rx: f64, ry: f64) {
        self.clockwise = !self.clockwise;
        self.polygon(&round(centre, rx, ry, 0.0));
        self.clockwise = !self.clockwise;
    }

    /// A curve from `x0` to `x1` at height `y`, bowed by `rise` (upward when
    /// positive), `ends` thick where it starts and stops and `middle` thick
    /// at its top: the shape of a slur.
    fn bow(&mut self, x0: f64, x1: f64, y: f64, rise: f64, ends: f64, middle: f64) {
        const STEPS: usize = 16;
        let side = rise.signum();
        let at =
            |u: f64, lift: f64, base: f64| (x0 + (x1 - x0) * u, base + lift * 4.0 * u * (1.0 - u));
        let outer = (0..=STEPS).map(|i| at(i as f64 / STEPS as f64, rise, y));
        let inner = (0..=STEPS).rev().map(|i| {
            at(
                i as f64 / STEPS as f64,
                rise - side * (middle - ends),
                y - side * ends,
            )
        });
        let points: Vec<(f64, f64)> = outer.chain(inner).collect();
        self.polygon(&points);
    }

    fn done(self) -> String {
        self.d
    }
}

/// The corners of an ellipse, enough of them to be round at any icon's size.
fn round(centre: (f64, f64), rx: f64, ry: f64, tilt: f64) -> Vec<(f64, f64)> {
    const CORNERS: usize = 24;
    let (sin, cos) = tilt.sin_cos();
    (0..CORNERS)
        .map(|i| {
            let a = std::f64::consts::TAU * i as f64 / CORNERS as f64;
            let (x, y) = (rx * a.cos(), ry * a.sin());
            (centre.0 + x * cos - y * sin, centre.1 + x * sin + y * cos)
        })
        .collect()
}

/// A coordinate as a path writes it: whole where it is whole, else to a tenth.
fn number(v: f64) -> String {
    let tenths = (v * 10.0).round() / 10.0;
    if (tenths - tenths.round()).abs() < 1e-9 {
        format!("{}", tenths.round() as i64)
    } else {
        format!("{tenths:.1}")
    }
}

// ---- a glyph of the face, read and placed ----

/// One command of a path and its numbers.
struct Command {
    letter: char,
    numbers: Vec<f64>,
}

/// The commands of the path `d`: the letters the engraver writes (`M L H V C
/// S Z`, either case), each with the numbers that follow it.
fn commands(d: &str) -> Vec<Command> {
    let mut out: Vec<Command> = Vec::new();
    let mut number = String::new();
    let flush = |number: &mut String, out: &mut Vec<Command>| {
        if let (Ok(v), Some(last)) = (number.parse::<f64>(), out.last_mut()) {
            last.numbers.push(v);
        }
        number.clear();
    };
    for c in d.chars() {
        match c {
            'M' | 'm' | 'L' | 'l' | 'H' | 'h' | 'V' | 'v' | 'C' | 'c' | 'S' | 's' | 'Z' | 'z' => {
                flush(&mut number, &mut out);
                out.push(Command {
                    letter: c,
                    numbers: Vec::new(),
                });
            }
            '-' | '+' if !number.is_empty() && !number.ends_with(['e', 'E']) => {
                flush(&mut number, &mut out);
                number.push(c);
            }
            '0'..='9' | '.' | '-' | '+' | 'e' | 'E' => number.push(c),
            _ => flush(&mut number, &mut out),
        }
    }
    flush(&mut number, &mut out);
    out
}

/// Which axis the `i`-th number of a command of `letter` is on: `true` for x.
fn is_x(letter: char, i: usize) -> bool {
    match letter.to_ascii_uppercase() {
        'H' => true,
        'V' => false,
        _ => i.is_multiple_of(2),
    }
}

/// **The path `d` scaled by `scale` and then moved by `by`** -- a glyph of
/// the face made smaller, turned over (a negative scale on both axes is half
/// a turn, which keeps its winding) and put where a drawing wants it.
fn placed(d: &str, scale: (f64, f64), by: (f64, f64)) -> String {
    let mut out = String::new();
    for command in commands(d) {
        out.push(command.letter);
        let absolute = command.letter.is_ascii_uppercase();
        for (i, v) in command.numbers.iter().enumerate() {
            let x = is_x(command.letter, i);
            let (k, shift) = if x { (scale.0, by.0) } else { (scale.1, by.1) };
            let moved = v * k + if absolute { shift } else { 0.0 };
            if i > 0 {
                out.push(' ');
            }
            out.push_str(&number(moved));
        }
    }
    out
}

/// The signed area of the largest contour of `d`, through the points its
/// commands end at: positive when it winds counter-clockwise in the font's
/// `y`-up frame. Its sign is all that is read, so a curve's bulge is not
/// missed.
fn winding(d: &str) -> f64 {
    let (mut at, mut start) = ((0.0f64, 0.0f64), (0.0f64, 0.0f64));
    let (mut area, mut largest) = (0.0f64, 0.0f64);
    let close = |area: &mut f64, largest: &mut f64| {
        if area.abs() > largest.abs() {
            *largest = *area;
        }
        *area = 0.0;
    };
    for command in commands(d) {
        let relative = command.letter.is_ascii_lowercase();
        let n = &command.numbers;
        // how many numbers one step of the command takes, and where in them
        // the point it ends at is
        let (step, end) = match command.letter.to_ascii_uppercase() {
            'M' | 'L' => (2, 0),
            'C' => (6, 4),
            'S' => (4, 2),
            'H' | 'V' => (1, 0),
            _ => {
                area += at.0 * start.1 - start.0 * at.1;
                close(&mut area, &mut largest);
                at = start;
                continue;
            }
        };
        for (i, chunk) in n.chunks_exact(step).enumerate() {
            let to = match command.letter.to_ascii_uppercase() {
                'H' => (if relative { at.0 + chunk[0] } else { chunk[0] }, at.1),
                'V' => (at.0, if relative { at.1 + chunk[0] } else { chunk[0] }),
                _ if relative => (at.0 + chunk[end], at.1 + chunk[end + 1]),
                _ => (chunk[end], chunk[end + 1]),
            };
            if command.letter.eq_ignore_ascii_case(&'M') && i == 0 {
                close(&mut area, &mut largest);
                start = to;
            } else {
                area += at.0 * to.1 - to.0 * at.1;
            }
            at = to;
        }
    }
    close(&mut area, &mut largest);
    largest
}

// ---- the drawings ----

/// What stands at one place of a barline.
#[derive(Clone, Copy)]
enum Bar {
    Thin,
    Thick,
    Dots,
    Dashed,
}

/// A barline, part by part from the left, a staff high.
fn barlines(parts: &[Bar]) -> String {
    const GAP: f64 = 80.0;
    const THICK: f64 = 3.0 * LINE;
    const DOT: f64 = 55.0;
    let mut pen = Pen::new();
    let mut x = 0.0;
    for part in parts {
        match part {
            Bar::Thin => {
                pen.rect(x, 0.0, LINE, STAFF);
                x += LINE;
            }
            Bar::Thick => {
                pen.rect(x, 0.0, THICK, STAFF);
                x += THICK;
            }
            Bar::Dots => {
                // in the two middle spaces of the staff
                for y in [1.5 * SPACE, 2.5 * SPACE] {
                    pen.ellipse((x + DOT, y), DOT, DOT, 0.0);
                }
                x += 2.0 * DOT;
            }
            Bar::Dashed => {
                let dash = STAFF / 9.0;
                for i in 0..5 {
                    pen.rect(x, 2.0 * dash * f64::from(i), LINE, dash);
                }
                x += LINE;
            }
        }
        x += GAP;
    }
    pen.done()
}

/// A hairpin: two strokes that open (`opening`) or close, a staff long.
fn hairpin(opening: bool) -> String {
    const SPREAD: f64 = 190.0;
    let mut pen = Pen::new();
    let (tip, mouth) = if opening { (0.0, STAFF) } else { (STAFF, 0.0) };
    for side in [SPREAD, -SPREAD] {
        pen.stroke((tip, 0.0), (mouth, side), LINE);
    }
    pen.done()
}

/// A slur: one bow, a staff long.
fn slur() -> String {
    let mut pen = Pen::new();
    pen.bow(0.0, STAFF, 0.0, 300.0, 22.0, 70.0);
    pen.done()
}

/// A notehead of a drawing made here, where the face's own is not at hand.
fn head(pen: &mut Pen, centre: (f64, f64)) {
    pen.ellipse(centre, 150.0, 100.0, 0.35);
}

/// A tie: two notes on one line, and the bow that joins them under.
fn tie() -> String {
    let mut pen = Pen::new();
    head(&mut pen, (150.0, 0.0));
    head(&mut pen, (850.0, 0.0));
    pen.bow(150.0, 850.0, -170.0, -210.0, 22.0, 70.0);
    pen.done()
}

/// Nothing: a ring with a stroke through it.
fn none() -> String {
    const OUT: f64 = 330.0;
    const IN: f64 = 270.0;
    let mut pen = Pen::new();
    pen.ellipse((0.0, 0.0), OUT, OUT, 0.0);
    pen.hole((0.0, 0.0), IN, IN);
    let reach = (OUT + IN) / 2.0 * std::f64::consts::FRAC_1_SQRT_2;
    pen.stroke((-reach, -reach), (reach, reach), 60.0);
    pen.done()
}

/// What is done to a measure.
#[derive(Clone, Copy)]
enum Sign {
    Plus,
    Minus,
}

/// A measure -- two barlines and the staff between them -- with `sign` before
/// it or after it.
fn measure(sign: Sign, before: bool) -> String {
    const ARM: f64 = 150.0;
    const WIDTH: f64 = 560.0;
    const GAP: f64 = 130.0;
    const HEIGHT: f64 = 0.7 * STAFF;
    let mut pen = Pen::new();
    let bar = if before { 2.0 * ARM + GAP } else { 0.0 };
    for i in 0..5 {
        pen.rect(bar, HEIGHT / 4.0 * f64::from(i), WIDTH, LINE * 0.6);
    }
    pen.rect(bar, 0.0, LINE, HEIGHT + LINE * 0.6);
    pen.rect(bar + WIDTH - LINE, 0.0, LINE, HEIGHT + LINE * 0.6);
    let centre = (if before { ARM } else { WIDTH + GAP + ARM }, HEIGHT / 2.0);
    pen.rect(centre.0 - ARM, centre.1 - 35.0, 2.0 * ARM, 70.0);
    if matches!(sign, Sign::Plus) {
        // the two arms above and below the bar already drawn, so no contour
        // overlaps another
        pen.rect(centre.0 - 35.0, centre.1 + 35.0, 70.0, ARM - 35.0);
        pen.rect(centre.0 - 35.0, centre.1 - ARM, 70.0, ARM - 35.0);
    }
    pen.done()
}

/// An arrow's head at `tip`, pointing along `(dx, dy)` (a unit vector).
fn arrowhead(pen: &mut Pen, tip: (f64, f64), dx: f64, dy: f64) {
    const LONG: f64 = 230.0;
    const HALF: f64 = 150.0;
    let back = (tip.0 - dx * LONG, tip.1 - dy * LONG);
    pen.polygon(&[
        tip,
        (back.0 - dy * HALF, back.1 + dx * HALF),
        (back.0 + dy * HALF, back.1 - dx * HALF),
    ]);
}

/// A system break: the line turned back to the left margin.
fn break_system() -> String {
    const W: f64 = 60.0;
    let mut pen = Pen::new();
    // down the right side, then back along the foot to the arrow's head
    pen.rect(760.0, 300.0 - W / 2.0, W, 500.0 + W / 2.0);
    pen.rect(230.0, 300.0 - W / 2.0, 530.0, W);
    arrowhead(&mut pen, (0.0, 300.0), -1.0, 0.0);
    pen.done()
}

/// A page break: a sheet, its corner turned.
fn break_page() -> String {
    const W: f64 = 50.0;
    const FOLD: f64 = 220.0;
    let (w, h) = (640.0, 860.0);
    let mut pen = Pen::new();
    pen.rect(0.0, 0.0, W, h);
    pen.rect(W, 0.0, w - W, W);
    pen.rect(w - W, W, W, h - FOLD - W);
    pen.rect(W, h - W, w - FOLD - W, W);
    pen.stroke((w - FOLD, h - W / 2.0), (w - W / 2.0, h - FOLD), W);
    pen.done()
}

/// No break: the line running on.
fn break_none() -> String {
    const W: f64 = 60.0;
    let mut pen = Pen::new();
    pen.rect(0.0, -W / 2.0, 770.0, W);
    arrowhead(&mut pen, (1000.0, 0.0), 1.0, 0.0);
    pen.done()
}

/// Back to the start: a stop, and the arrow that runs into it.
fn rewind() -> String {
    const HIGH: f64 = 520.0;
    let mut pen = Pen::new();
    pen.rect(0.0, 0.0, 80.0, HIGH);
    pen.polygon(&[(150.0, HIGH / 2.0), (560.0, HIGH), (560.0, 0.0)]);
    pen.done()
}

/// Note entry: a pencil, tilted, its point down to the left -- a body, the
/// wood cut to a point, and the graphite at the tip.
fn pencil() -> String {
    // along the pencil's own axis, then turned
    let (sin, cos) = (std::f64::consts::FRAC_PI_4).sin_cos();
    let turn = |(u, v): (f64, f64)| (300.0 + u * cos - v * sin, 300.0 + u * sin + v * cos);
    let shape = |points: &[(f64, f64)]| points.iter().copied().map(turn).collect::<Vec<_>>();
    const HALF: f64 = 70.0;
    let mut pen = Pen::new();
    // the body, and the band at its end
    pen.polygon(&shape(&[
        (-140.0, -HALF),
        (330.0, -HALF),
        (330.0, HALF),
        (-140.0, HALF),
    ]));
    pen.polygon(&shape(&[
        (360.0, -HALF),
        (420.0, -HALF),
        (420.0, HALF),
        (360.0, HALF),
    ]));
    // the wood cut to a point, short of the tip
    pen.polygon(&shape(&[
        (-170.0, -HALF),
        (-170.0, HALF),
        (-300.0, 22.0),
        (-300.0, -22.0),
    ]));
    // and the graphite
    pen.polygon(&shape(&[(-315.0, -18.0), (-315.0, 18.0), (-370.0, 0.0)]));
    pen.done()
}

/// Let it ring: a notehead and a tie that runs off into nothing.
fn let_ring() -> String {
    let mut pen = Pen::new();
    pen.ellipse((90.0, 200.0), 110.0, 75.0, 0.35);
    pen.bow(150.0, 700.0, 80.0, -110.0, 14.0, 46.0);
    pen.done()
}

/// A glissando: two noteheads and the line between them.
fn glissando() -> String {
    let mut pen = Pen::new();
    pen.ellipse((90.0, 120.0), 110.0, 75.0, 0.35);
    pen.ellipse((620.0, 520.0), 110.0, 75.0, 0.35);
    pen.stroke((200.0, 190.0), (510.0, 450.0), LINE);
    pen.done()
}

/// A phrase mark: a long curve, its ends turned down.
fn phrase() -> String {
    let mut pen = Pen::new();
    pen.bow(0.0, 760.0, 200.0, 260.0, 18.0, 52.0);
    pen.stroke((0.0, 200.0), (0.0, 80.0), LINE);
    pen.stroke((760.0, 200.0), (760.0, 80.0), LINE);
    pen.done()
}

/// A bracket over notes: a line with its ends turned down.
fn bracket() -> String {
    let mut pen = Pen::new();
    pen.rect(0.0, 440.0, 720.0, LINE);
    pen.rect(0.0, 200.0, LINE, 280.0);
    pen.rect(720.0 - LINE, 200.0, LINE, 280.0);
    pen.done()
}

/// A beam across a barline: two stems joined by a beam, a barline between.
fn beam_span() -> String {
    let mut pen = Pen::new();
    pen.ellipse((90.0, 60.0), 110.0, 75.0, 0.35);
    pen.ellipse((620.0, 60.0), 110.0, 75.0, 0.35);
    pen.rect(180.0, 60.0, LINE, 520.0);
    pen.rect(710.0, 60.0, LINE, 520.0);
    pen.polygon(&[
        (180.0, 520.0),
        (750.0, 520.0),
        (750.0, 600.0),
        (180.0, 600.0),
    ]);
    pen.rect(420.0, 0.0, LINE, 700.0);
    pen.done()
}

/// Two notes alternating: two half notes joined by two strokes.
fn two_note_tremolo() -> String {
    let mut pen = Pen::new();
    pen.ellipse((90.0, 60.0), 110.0, 75.0, 0.35);
    pen.hole((90.0, 60.0), 70.0, 35.0);
    pen.ellipse((620.0, 60.0), 110.0, 75.0, 0.35);
    pen.hole((620.0, 60.0), 70.0, 35.0);
    pen.rect(180.0, 60.0, LINE, 560.0);
    pen.rect(710.0, 60.0, LINE, 560.0);
    for y in [330.0, 460.0] {
        pen.polygon(&[
            (150.0, y),
            (760.0, y + 60.0),
            (760.0, y + 130.0),
            (150.0, y + 70.0),
        ]);
    }
    pen.done()
}

/// A rehearsal mark: an A in a box, the letter drawn as two strokes and a bar.
fn rehearsal() -> String {
    let mut pen = Pen::new();
    pen.rect(0.0, 0.0, 600.0, LINE);
    pen.rect(0.0, 620.0, 600.0, LINE);
    pen.rect(0.0, 0.0, LINE, 660.0);
    pen.rect(600.0 - LINE, 0.0, LINE, 660.0);
    pen.stroke((140.0, 120.0), (300.0, 540.0), 60.0);
    pen.stroke((460.0, 120.0), (300.0, 540.0), 60.0);
    pen.rect(200.0, 260.0, 200.0, 50.0);
    pen.done()
}

/// An ending: the bracket of a first-time bar, its number a stroke.
fn ending() -> String {
    let mut pen = Pen::new();
    pen.rect(0.0, 600.0, 760.0, LINE);
    pen.rect(0.0, 200.0, LINE, 440.0);
    pen.stroke((180.0, 300.0), (180.0, 540.0), 60.0);
    pen.stroke((180.0, 540.0), (110.0, 480.0), 50.0);
    pen.done()
}

/// The window as pages: a sheet, and three systems on it.
fn page_view() -> String {
    const W: f64 = 50.0;
    let (w, h) = (640.0, 860.0);
    let mut pen = Pen::new();
    pen.rect(0.0, 0.0, W, h);
    pen.rect(w - W, 0.0, W, h);
    pen.rect(W, 0.0, w - 2.0 * W, W);
    pen.rect(W, h - W, w - 2.0 * W, W);
    for y in [600.0, 420.0, 240.0] {
        pen.rect(140.0, y, w - 280.0, LINE);
    }
    pen.done()
}

/// The window as one system: three lines that run on past the edge.
fn line_view() -> String {
    const LONG: f64 = 760.0;
    let mut pen = Pen::new();
    for y in [180.0, 380.0, 580.0] {
        pen.rect(0.0, y, LONG, LINE);
    }
    arrowhead(&mut pen, (1000.0, 400.0), 1.0, 0.0);
    pen.done()
}

/// A staff of `lines` lines.
fn staff_lines(lines: u32) -> String {
    let mut pen = Pen::new();
    let top = 4.0 * SPACE * 0.6;
    for i in 0..lines {
        let y = if lines == 1 {
            top / 2.0
        } else {
            top * f64::from(i) / f64::from(lines - 1)
        };
        pen.rect(0.0, y, 760.0, LINE);
    }
    pen.done()
}

/// A key signature: `count` of the face's sharps (flats, below zero) in a
/// row, each a step from the one before as a signature staggers them; a
/// natural for none.
fn signature(glyph: &str, count: i32) -> String {
    const STEP: f64 = 230.0;
    let n = count.unsigned_abs().max(1);
    let scale = if n > 3 { 0.7 } else { 0.85 };
    let mut d = String::new();
    for i in 0..n {
        let rise = if i % 2 == 0 { 120.0 } else { -60.0 };
        d.push_str(&placed(
            glyph,
            (scale, scale),
            (f64::from(i) * STEP * scale, rise),
        ));
    }
    d
}

/// A rolled chord: a wavy line beside three noteheads, its arrow the way it
/// is rolled.
fn arpeggio(up: bool) -> String {
    let mut pen = Pen::new();
    // the wave, as short strokes turning left and right
    let (bottom, top) = (-60.0, 640.0);
    let steps = 7;
    let rise = (top - bottom) / f64::from(steps);
    for i in 0..steps {
        let y0 = bottom + rise * f64::from(i);
        let (x0, x1) = if i % 2 == 0 {
            (60.0, 140.0)
        } else {
            (140.0, 60.0)
        };
        pen.stroke((x0, y0), (x1, y0 + rise), 46.0);
    }
    let (tip, base) = if up {
        (top + 120.0, top - 10.0)
    } else {
        (bottom - 120.0, bottom + 10.0)
    };
    pen.polygon(&[(0.0, base), (200.0, base), (100.0, tip)]);
    for y in [40.0, 290.0, 540.0] {
        pen.ellipse((420.0, y), 110.0, 75.0, 0.35);
    }
    pen.done()
}

/// Staves joined by a line: two short staves and the line at their left.
fn group_line() -> String {
    let mut pen = Pen::new();
    pen.rect(0.0, -260.0, 60.0, 1040.0);
    for top in [-200.0, 420.0] {
        for i in 0..3 {
            pen.rect(60.0, top + f64::from(i) * 150.0, 600.0, LINE * 0.6);
        }
    }
    pen.done()
}

/// A grace note: the face's eighth at two thirds of its size, with the stroke
/// through its stem when it is an acciaccatura (`slashed`).
fn grace(eighth: &str, slashed: bool) -> String {
    const SMALL: f64 = 0.66;
    let note = placed(eighth, (SMALL, SMALL), (0.0, 0.0));
    if !slashed {
        return note;
    }
    let mut pen = Pen::over(&note);
    pen.stroke((20.0, 190.0), (400.0, 470.0), 36.0);
    pen.done()
}

/// The other voice: a note with its stem up and one with its stem down, the
/// face's quarter and the same glyph turned over.
/// The box a path's command points span: `[left, bottom, right, top]`.
fn bounds(d: &str) -> [f64; 4] {
    let mut at = (0.0f64, 0.0f64);
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for c in commands(d) {
        let rel = c.letter.is_ascii_lowercase();
        let step = match c.letter.to_ascii_uppercase() {
            'C' => 6,
            'S' => 4,
            'H' | 'V' => 1,
            'Z' => continue,
            _ => 2,
        };
        for chunk in c.numbers.chunks_exact(step) {
            at = match c.letter.to_ascii_uppercase() {
                'H' => (if rel { at.0 + chunk[0] } else { chunk[0] }, at.1),
                'V' => (at.0, if rel { at.1 + chunk[0] } else { chunk[0] }),
                _ if rel => (at.0 + chunk[step - 2], at.1 + chunk[step - 1]),
                _ => (chunk[step - 2], chunk[step - 1]),
            };
            b = [
                b[0].min(at.0),
                b[1].min(at.1),
                b[2].max(at.0),
                b[3].max(at.1),
            ];
        }
    }
    b
}

/// The face's quarter turned over, its stem down from the left of its head
/// -- as a page writes it when the stem goes down.
fn stem_down(quarter: &str) -> String {
    let [left, bottom, right, top] = bounds(quarter);
    placed(quarter, (-1.0, -1.0), (left + right, bottom + top))
}

fn voices(quarter: &str) -> String {
    const SMALL: f64 = 0.7;
    let mut d = placed(quarter, (SMALL, SMALL), (0.0, 0.0));
    d.push_str(&placed(quarter, (-SMALL, -SMALL), (560.0, 190.0)));
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The engraver's own outlines of a quarter and an eighth with their
    /// stems up, as it hands them out.
    const QUARTER: &str = "M275 104v735h27v-796c0 -87 -112 -172 -203 -172c-56 0 -99 32 -99 85c0 84 109 177 201 177c31 0 55 -10 74 -29z";
    const EIGHTH: &str = "M309 861c16 -73 51 -137 94 -197c54 -76 96 -166 99 -263v-7c0 -64 -16 -127 -39 -187h-27c21 53 36 110 36 167c0 113 -74 190 -163 255v-586c0 -87 -112 -172 -203 -172c-56 0 -99 32 -99 85c0 84 109 177 201 177c31 0 55 -10 74 -29v757h27z";

    #[test]
    fn the_second_voice_is_the_quarter_turned_over_in_its_own_box() {
        let mut outlines = Outlines::new();
        outlines.insert(QUARTER_UP.into(), QUARTER.into());
        complete(&mut outlines);
        let down = &outlines[QUARTER_DOWN];
        let (up, turned) = (bounds(QUARTER), bounds(down));
        for (a, b) in up.iter().zip(turned) {
            assert!((a - b).abs() < 1e-6, "{up:?} {turned:?}");
        }
        // and a face that has its own keeps it
        let mut own = Outlines::new();
        own.insert(QUARTER_UP.into(), QUARTER.into());
        own.insert(QUARTER_DOWN.into(), "M0 0h1v1z".into());
        complete(&mut own);
        assert_eq!(own[QUARTER_DOWN], "M0 0h1v1z");
    }

    #[test]
    fn the_table_is_completed_and_what_the_engraver_handed_out_stays() {
        let mut outlines = Outlines::new();
        outlines.insert(EIGHTH_UP.into(), EIGHTH.into());
        outlines.insert(QUARTER_UP.into(), QUARTER.into());
        outlines.insert(BARLINE_SINGLE.into(), "M0 0h10v10z".into());
        complete(&mut outlines);
        // a symbol the face had is the face's
        assert_eq!(outlines[BARLINE_SINGLE], "M0 0h10v10z");
        // every other one is drawn: a closed path, and nothing but a path
        for code in [
            BARLINE_DOUBLE,
            BARLINE_FINAL,
            REPEAT_START,
            REPEAT_END,
            REPEAT_BOTH,
            BARLINE_INVISIBLE,
            CRESCENDO,
            DIMINUENDO,
            SLUR,
            TIE,
            NONE,
            MEASURE_BEFORE,
            MEASURE_AFTER,
            MEASURE_REMOVE,
            BREAK_SYSTEM,
            BREAK_PAGE,
            BREAK_NONE,
            REWIND,
            APPOGGIATURA,
            ACCIACCATURA,
            VOICE,
        ] {
            let d = &outlines[code];
            assert!(d.starts_with('M') && d.ends_with('z'), "{code}: {d}");
            assert!(
                d.chars()
                    .all(|c| c.is_ascii_digit() || " .-MLHVCSZmlhvcsz".contains(c)),
                "{code}: {d}"
            );
        }
        assert!(is_own(SLUR) && !is_own(BARLINE_SINGLE));
    }

    #[test]
    fn a_drawing_from_a_glyph_waits_for_the_glyph() {
        let mut outlines = Outlines::new();
        complete(&mut outlines);
        assert!(outlines.contains_key(SLUR));
        for code in [APPOGGIATURA, ACCIACCATURA, VOICE] {
            assert!(
                !outlines.contains_key(code),
                "{code} has no face to draw from"
            );
        }
    }

    #[test]
    fn a_glyph_is_placed_smaller_and_turned_over() {
        let [l, b, r, t] = bounds(QUARTER);
        let small = placed(QUARTER, (0.5, 0.5), (100.0, 10.0));
        let [sl, sb, sr, st] = bounds(&small);
        assert!((sr - sl - (r - l) * 0.5).abs() < 1.0 && (st - sb - (t - b) * 0.5).abs() < 1.0);
        assert!((sl - (l * 0.5 + 100.0)).abs() < 1.0 && (sb - (b * 0.5 + 10.0)).abs() < 1.0);
        // half a turn: the same size, the other way up, wound as it was
        let turned = placed(QUARTER, (-1.0, -1.0), (0.0, 0.0));
        let [tl, tb, tr, tt] = bounds(&turned);
        assert!((tl + r).abs() < 1.0 && (tr + l).abs() < 1.0);
        assert!((tb + t).abs() < 1.0 && (tt + b).abs() < 1.0);
        assert_eq!(winding(QUARTER) < 0.0, winding(&turned) < 0.0);
    }

    #[test]
    fn every_contour_of_a_drawing_winds_one_way() {
        // a polygon given clockwise is written counter-clockwise...
        let mut pen = Pen::new();
        pen.polygon(&[(0.0, 0.0), (0.0, 10.0), (10.0, 10.0), (10.0, 0.0)]);
        assert!(winding(&pen.done()) > 0.0);
        // ...and over a glyph, as the glyph winds: the slash of an
        // acciaccatura fills where it crosses the stem
        let slashed = grace(EIGHTH, true);
        let slash = &slashed[placed(EIGHTH, (0.66, 0.66), (0.0, 0.0)).len()..];
        assert_eq!(winding(slash) < 0.0, winding(EIGHTH) < 0.0);
    }

    #[test]
    fn a_barline_is_a_staff_high_and_a_repeat_wider_than_a_line() {
        let [l, b, r, t] = bounds(&barlines(&[Bar::Thin]));
        assert_eq!((r - l, t - b), (LINE, STAFF));
        let [l, _, r, _] = bounds(&barlines(&[Bar::Dots, Bar::Thin, Bar::Thick]));
        assert!(r - l > 300.0);
    }
}
