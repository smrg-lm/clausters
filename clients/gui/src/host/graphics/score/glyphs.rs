//! **The outlines**: an SVG path `d` into a lyon path, and its extent.
//!
//! A SMuFL glyph reaches the host as the outline the client's font gave it —
//! one path string per codepoint, sent once with the page and referenced by
//! every note that draws it. This module is the only place that string is
//! understood: [`build_path`] scans it into a [`LyonPath`] the tessellator can
//! fill, [`path_bounds`] measures it for the hit index, and [`Tokens`] is the
//! scanner both go through.
//!
//! Only the subset verovio emits is supported, and a malformed path yields
//! `None` rather than a panic — the primitive is then skipped, which is the
//! same forgiveness the display-list decode applies one level up.

use lyon::math::point;
use lyon::path::Path as LyonPath;

use super::Bounds;

/// The extent of an SVG path `d` in its own coordinates, from the bezier control
/// hull — a slight over-estimate of the true curve extent, which is what a hit
/// target wants anyway (a click just off a notehead's edge still names it).
pub(super) fn path_bounds(d: &str) -> Option<Bounds> {
    let path = build_path(d)?;
    let b = lyon::algorithms::aabb::fast_bounding_box(&path);
    Some(Bounds {
        x0: b.min.x,
        y0: b.min.y,
        x1: b.max.x,
        y1: b.max.y,
    })
}

/// Build a lyon [`LyonPath`] from an SVG path `d`. Supports the subset verovio
/// emits: `M/m` moveto, `L/l` lineto, `H/h`/`V/v` axis lines, `C/c` cubic,
/// `S/s` smooth cubic, and `Z/z` close — absolute and relative. Returns `None`
/// on a malformed/empty path (the primitive is then skipped, never a panic).
pub(super) fn build_path(d: &str) -> Option<LyonPath> {
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
