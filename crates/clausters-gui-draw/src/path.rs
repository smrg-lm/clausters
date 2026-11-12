//! **An SVG path `d`**: scanned into a path, filled into triangles and
//! flattened into edges.
//!
//! An outline reaches the host as a path string -- a notation glyph the
//! client's font gave it, sent once with the page and referenced by every
//! note that draws it, or a symbol a client sent as an outline -- and this
//! module is the only place that string is understood: [`build_path`] scans
//! it, through the scanner [`Tokens`], into the path the tessellator fills
//! and a hit test presses against. It is in the drawing base because the
//! face draws outlines too (`font::outline`), and the score's pictures build
//! on it from above.
//!
//! Only the subset an engraver emits is supported, and a malformed path
//! yields `None` rather than a panic -- the primitive is then skipped, which
//! is the same forgiveness the display-list decode applies one level up.

use lyon::math::point;
use lyon::path::Path as LyonPath;
use lyon::tessellation::{
    BuffersBuilder, FillOptions, FillRule, FillTessellator, FillVertex, VertexBuffers,
};

/// Build a lyon [`LyonPath`] from an SVG path `d`. Supports the subset verovio
/// emits: `M/m` moveto, `L/l` lineto, `H/h`/`V/v` axis lines, `C/c` cubic,
/// `S/s` smooth cubic, and `Z/z` close -- absolute and relative. Returns `None`
/// on a malformed/empty path (the primitive is then skipped, never a panic).
pub fn build_path(d: &str) -> Option<LyonPath> {
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

/// **The fill of the path `d`, as triangle corners** in the path's own
/// coordinates, three to a triangle -- nothing for a path that fills nothing.
///
/// What a caller keeps when it draws one outline many times at sizes it does
/// not know yet: the triangles are tessellated once, at tolerance `tol` in the
/// path's units, and mapped where they are drawn.
pub fn triangles(d: &str, tol: f32) -> Vec<[f32; 2]> {
    let Some(path) = build_path(d) else {
        return Vec::new();
    };
    let mut buffers: VertexBuffers<[f32; 2], u32> = VertexBuffers::new();
    let opts = FillOptions::tolerance(tol.max(f32::MIN_POSITIVE)).with_fill_rule(FillRule::NonZero);
    let filled = FillTessellator::new().tessellate_path(
        &path,
        &opts,
        &mut BuffersBuilder::new(&mut buffers, |v: FillVertex| {
            let p = v.position();
            [p.x, p.y]
        }),
    );
    if filled.is_err() {
        return Vec::new();
    }
    buffers
        .indices
        .iter()
        .map(|&i| buffers.vertices[i as usize])
        .collect()
}

/// **The edges of the path `d`**: its contours flattened at tolerance `tol`,
/// one segment after another, every contour closed -- nothing for a path that
/// draws nothing. What a caller that draws an outline small keeps beside its
/// fill, since an edge is where a stroke thinner than a pixel is lost.
pub fn edges(d: &str, tol: f32) -> Vec<[[f32; 2]; 2]> {
    use lyon::path::PathEvent;
    use lyon::path::iterator::PathIterator;

    let Some(path) = build_path(d) else {
        return Vec::new();
    };
    path.iter()
        .flattened(tol.max(f32::MIN_POSITIVE))
        .filter_map(|event| match event {
            PathEvent::Line { from, to } => Some([[from.x, from.y], [to.x, to.y]]),
            PathEvent::End { last, first, .. } => Some([[last.x, last.y], [first.x, first.y]]),
            _ => None,
        })
        .filter(|[a, b]| a != b)
        .collect()
}
