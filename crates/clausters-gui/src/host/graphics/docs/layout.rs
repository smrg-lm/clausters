//! **A document laid out at a width**: every piece of text where it is
//! drawn, in its face, its size and its ink; the fills under it (a code
//! block, a quote's bar, a table's lines); and what a hand can land on --
//! a link, a code block's copy button -- with the places a link to a
//! heading or an anchor lands on.
//!
//! The look is the current editor's documentation: Inter for the text, in
//! its weights for headings and strong words and its italic for emphasis,
//! JetBrains Mono for code, the sizes and the spacing of a book page, and a
//! code block in a language the editor runs colored by its syntax. A line
//! wraps at a space; a code block's lines do not wrap and are cut at the
//! page's edge.

use clausters_gui_draw::font::{self, Family};
use clausters_gui_draw::paint::Mesh;
use clausters_gui_draw::rect::Rect;
use clausters_gui_draw::theme::Color;

use super::markdown::{Align, Block, Run};
use crate::host::graphics::code::Language;
use crate::host::graphics::code::syntax::{Kind, Syntax};

/// What a piece is drawn in: a role of the theme, chosen when it is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ink {
    Text,
    /// What reads quieter: a quote, an image's description, a list's mark.
    Dim,
    /// Code, in the color of what it is, or plain.
    Code(Option<Kind>),
    /// A code block's ground.
    CodeGround,
    /// A rule, a quote's bar, a table's lines.
    Line,
}

/// One thing drawn.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Text {
        x: f32,
        y: f32,
        text: String,
        family: Family,
        size: f32,
        ink: Ink,
        underline: bool,
        strike: bool,
    },
    Fill {
        rect: Rect,
        ink: Ink,
    },
}

impl Item {
    /// The top of what it draws.
    pub fn top(&self) -> f32 {
        match self {
            Item::Text { y, .. } => *y,
            Item::Fill { rect, .. } => rect.y,
        }
    }

    /// The bottom of what it draws.
    pub fn bottom(&self) -> f32 {
        match self {
            Item::Text { y, size, .. } => y + font::line_advance(*size),
            Item::Fill { rect, .. } => rect.y + rect.h,
        }
    }
}

/// **A document laid out**, its places from its top left.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Laid {
    pub items: Vec<Item>,
    pub height: f32,
    /// The places a link lands on: a heading's id, an anchor.
    pub anchors: Vec<(String, f32)>,
    /// What a press on a link follows.
    pub links: Vec<(Rect, String)>,
    /// A code block's copy button, and the code it copies.
    pub copies: Vec<(Rect, String)>,
}

impl Laid {
    /// Where `id` lands, from the top.
    pub fn anchor(&self, id: &str) -> Option<f32> {
        self.anchors.iter().find(|(a, _)| a == id).map(|(_, y)| *y)
    }

    /// The link under `(x, y)`.
    pub fn link_at(&self, x: f32, y: f32) -> Option<&str> {
        self.links
            .iter()
            .find(|(r, _)| r.contains(f64::from(x), f64::from(y)))
            .map(|(_, href)| href.as_str())
    }

    /// The copy button under `(x, y)`, and its code.
    pub fn copy_at(&self, x: f32, y: f32) -> Option<&str> {
        self.copies
            .iter()
            .find(|(r, _)| r.contains(f64::from(x), f64::from(y)))
            .map(|(_, code)| code.as_str())
    }
}

/// **The items of `laid` drawn into `mesh`**, its top left at `at`, those
/// inside `view` alone, each in the color `ink` gives its role: the
/// documentation panel's page and a hover card's documentation alike.
pub fn draw_items(
    mesh: &mut Mesh,
    laid: &Laid,
    (left, top): (f32, f32),
    view: Rect,
    ink: impl Fn(Ink) -> Color,
) {
    for item in &laid.items {
        if item.bottom() + top < view.y || item.top() + top > view.y + view.h {
            continue;
        }
        match item {
            Item::Fill { rect, ink: role } => mesh.rect(
                Rect::new(left + rect.x, top + rect.y, rect.w, rect.h),
                ink(*role),
            ),
            Item::Text {
                x,
                y,
                text,
                family,
                size,
                ink: role,
                underline,
                strike,
            } => {
                let color = ink(*role);
                font::text_in(mesh, *family, text, left + x, top + y, *size, color);
                let w = font::width_in(*family, text, *size);
                let thick = (size * 0.5).max(1.0).round();
                if *underline {
                    let uy = top + y + font::height(*size) + thick;
                    mesh.rect(Rect::new(left + x, uy, w, thick), color);
                }
                if *strike {
                    let sy = top + y + font::height(*size) * 0.5;
                    mesh.rect(Rect::new(left + x, sy, w, thick), color);
                }
            }
        }
    }
}

/// What a code block's copy button says.
pub const COPY: &str = "Copy";

/// The sizes and spacing a page is laid out with, from its text's size.
#[derive(Clone, Copy, Debug)]
struct Measures {
    size: f32,
    /// The em of the text: a capital is about three quarters of it.
    em: f32,
}

impl Measures {
    fn new(size: f32) -> Measures {
        Measures {
            size,
            em: font::height(size) / 0.727,
        }
    }

    /// The height of a line of text at `size`: a book page's leading.
    fn line(&self, size: f32) -> f32 {
        (font::height(size) / 0.727 * 1.6).max(font::line_advance(size))
    }
}

/// **How far apart two lines of one paragraph stand**, at `size`: a book
/// page's leading, what a reader of a laid-out page (a card's selection)
/// tells a line of a paragraph from the next paragraph by.
pub fn line_pitch(size: f32) -> f32 {
    Measures::new(size).line(size)
}

/// The face a run is written in, over `base`.
fn family_of(run: &Run, base: Family) -> Family {
    if run.code {
        Family::Mono
    } else if run.strong {
        if base == Family::ProseBold {
            base
        } else {
            Family::ProseStrong
        }
    } else if run.emph {
        Family::ProseItalic
    } else {
        base
    }
}

/// **A document laid out** at width `width`, its text at `size`.
pub fn lay(blocks: &[Block], width: f32, size: f32) -> Laid {
    let m = Measures::new(size);
    let mut out = Laid::default();
    let mut y = 0.0;
    lay_blocks(
        &mut out,
        blocks,
        0.0,
        width.max(m.em * 4.0),
        &mut y,
        m,
        Ink::Text,
    );
    out.height = y;
    out.items.sort_by(|a, b| a.top().total_cmp(&b.top()));
    out
}

fn lay_blocks(
    out: &mut Laid,
    blocks: &[Block],
    x: f32,
    w: f32,
    y: &mut f32,
    m: Measures,
    ink: Ink,
) {
    let gap = m.em * 1.0;
    for (i, block) in blocks.iter().enumerate() {
        let first = i == 0;
        match block {
            Block::Heading { level, runs, id } => {
                let (scale, family) = match level {
                    1 => (2.14, Family::ProseBold),
                    2 => (1.43, Family::ProseBold),
                    3 => (1.29, Family::ProseStrong),
                    _ => (1.0, Family::ProseStrong),
                };
                let size = m.size * scale;
                if !first {
                    *y += m.em * scale * 1.1;
                }
                out.anchors.push((id.clone(), *y));
                *y = lay_runs(out, runs, x, w, *y, size, m.line(size) * 0.85, family, ink);
                *y += m.em * scale * 0.5;
            }
            Block::Para(runs) => {
                if !first {
                    *y += gap * 0.25;
                }
                *y = lay_runs(
                    out,
                    runs,
                    x,
                    w,
                    *y,
                    m.size,
                    m.line(m.size),
                    Family::Prose,
                    ink,
                );
                *y += gap * 0.75;
            }
            Block::Code { lang, text } => lay_code(out, lang, text, x, w, y, m),
            Block::List { start, items } => {
                let indent = m.em * 1.6;
                for (n, item) in items.iter().enumerate() {
                    let mark = match start {
                        Some(s) => format!("{}.", s + n as u64),
                        None => "\u{2022}".to_string(),
                    };
                    let mark_w = font::width_in(Family::Prose, &mark, m.size);
                    let line = m.line(m.size);
                    out.items.push(Item::Text {
                        x: x + indent - mark_w - m.em * 0.5,
                        y: *y + (line - font::height(m.size)) * 0.5,
                        text: mark,
                        family: Family::Prose,
                        size: m.size,
                        ink: Ink::Dim,
                        underline: false,
                        strike: false,
                    });
                    let mut inner = *y;
                    lay_blocks(out, item, x + indent, w - indent, &mut inner, m, ink);
                    // an item of one paragraph keeps its lines close
                    *y = inner - if item.len() == 1 { gap * 0.55 } else { 0.0 } + m.em * 0.15;
                }
                *y += gap * 0.6;
            }
            Block::Quote(inner) => {
                let top = *y;
                let indent = m.em * 1.0;
                lay_blocks(out, inner, x + indent, w - indent, y, m, Ink::Dim);
                out.items.push(Item::Fill {
                    rect: Rect::new(
                        x,
                        top,
                        (m.em * 0.2).max(1.0),
                        (*y - top - gap * 0.5).max(1.0),
                    ),
                    ink: Ink::Line,
                });
            }
            Block::Table { aligns, head, rows } => lay_table(out, aligns, head, rows, x, w, y, m),
            Block::Rule => {
                *y += gap;
                out.items.push(Item::Fill {
                    rect: Rect::new(x, *y, w, 1.0f32.max(m.em * 0.06)),
                    ink: Ink::Line,
                });
                *y += gap;
            }
            Block::Image { alt, .. } => {
                let text = if alt.is_empty() {
                    "[image]".to_string()
                } else {
                    format!("[image: {alt}]")
                };
                let run = Run {
                    text,
                    emph: true,
                    ..Run::default()
                };
                *y = lay_runs(
                    out,
                    &[run],
                    x,
                    w,
                    *y,
                    m.size,
                    m.line(m.size),
                    Family::Prose,
                    Ink::Dim,
                );
                *y += gap * 0.75;
            }
            Block::Anchor(id) => out.anchors.push((id.clone(), *y)),
        }
    }
}

/// **Runs laid in lines** from `y`, wrapped at a space within `x..x + w`;
/// answers where the lines end.
#[allow(clippy::too_many_arguments)] // a line's place, its face and its ink
fn lay_runs(
    out: &mut Laid,
    runs: &[Run],
    x: f32,
    w: f32,
    y: f32,
    size: f32,
    line_h: f32,
    base: Family,
    ink: Ink,
) -> f32 {
    let right = x + w;
    let mut pen = x;
    let mut top = y;
    let ty = |top: f32| top + (line_h - font::height(size)) * 0.5;
    // what the line holds so far that has not been placed: a space waits
    // for the word after it
    let mut pending_space = false;
    for run in runs {
        let family = family_of(run, base);
        let run_ink = if run.code { Ink::Code(None) } else { ink };
        let mut rest = run.text.as_str();
        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix('\n') {
                top += line_h;
                pen = x;
                pending_space = false;
                rest = after;
                continue;
            }
            let space = rest.starts_with(char::is_whitespace);
            let end = rest
                .find(|c: char| c.is_whitespace() != space || c == '\n')
                .unwrap_or(rest.len());
            let token = &rest[..end];
            rest = &rest[end..];
            if space {
                pending_space = pen > x;
                continue;
            }
            let space_w = if pending_space {
                font::width_in(family, " ", size)
            } else {
                0.0
            };
            let mut word = token;
            loop {
                let ww = font::width_in(family, word, size);
                if pen > x && pen + space_w + ww > right {
                    top += line_h;
                    pen = x;
                    pending_space = false;
                }
                let lead = if pending_space && pen > x {
                    space_w
                } else {
                    0.0
                };
                // a word longer than the line is cut where the line ends
                let fit = if ww > right - pen - lead && pen == x {
                    let at = font::byte_at_in(family, word, right - pen, size).max(1);
                    let mut at = at.min(word.len());
                    while !word.is_char_boundary(at) {
                        at += 1;
                    }
                    at
                } else {
                    word.len()
                };
                let piece = &word[..fit];
                let pw = font::width_in(family, piece, size);
                place(out, pen, lead, ty(top), piece, family, size, run_ink, run);
                if let Some(href) = &run.link {
                    out.links
                        .push((Rect::new(pen + lead, top, pw, line_h), href.clone()));
                }
                pen += lead + pw;
                pending_space = false;
                if fit >= word.len() {
                    break;
                }
                word = &word[fit..];
                top += line_h;
                pen = x;
            }
        }
    }
    top + line_h
}

/// A piece of text placed after a space of `lead`: joined to the piece
/// before it when it continues it in the same style.
#[allow(clippy::too_many_arguments)] // a piece and all it is drawn with
fn place(
    out: &mut Laid,
    pen: f32,
    lead: f32,
    y: f32,
    piece: &str,
    family: Family,
    size: f32,
    ink: Ink,
    run: &Run,
) {
    // a link in a heading or in strong words reads as a title, as a
    // book's name and a search's result do; any other is underlined
    let underline =
        run.link.is_some() && matches!(family, Family::Prose | Family::ProseItalic | Family::Mono);
    if let Some(Item::Text {
        y: ly,
        text,
        family: lf,
        size: ls,
        ink: li,
        underline: lu,
        strike: lst,
        x: lx,
    }) = out.items.last_mut()
        && *ly == y
        && *lf == family
        && *ls == size
        && *li == ink
        && *lu == underline
        && *lst == run.strike
        && (*lx + font::width_in(family, text, size) - pen).abs() < 0.5
    {
        if lead > 0.0 {
            text.push(' ');
        }
        text.push_str(piece);
        return;
    }
    out.items.push(Item::Text {
        x: pen + lead,
        y,
        text: piece.to_string(),
        family,
        size,
        ink,
        underline,
        strike: run.strike,
    });
}

/// The language a fence names, for the ones the host colors.
fn language_of(lang: &str) -> Language {
    match lang {
        "python" | "py" => Language::Python,
        "javascript" | "js" | "mjs" => Language::JavaScript,
        _ => Language::Plain,
    }
}

/// **A code block**: its ground, its lines in the code's face colored by
/// their syntax, and a copy button at its top right.
fn lay_code(out: &mut Laid, lang: &str, text: &str, x: f32, w: f32, y: &mut f32, m: Measures) {
    let size = m.size * 0.857;
    let line_h = font::line_advance(size) * 1.3;
    let (pad_x, pad_y) = (m.em, m.em * 0.67);
    *y += m.em * 0.25;
    let lines: Vec<&str> = text.split('\n').collect();
    let h = lines.len() as f32 * line_h + 2.0 * pad_y;
    out.items.push(Item::Fill {
        rect: Rect::new(x, *y, w, h),
        ink: Ink::CodeGround,
    });
    // the colors of what the code is, read by the code element's parser
    let mut syntax = Syntax::new(language_of(lang));
    syntax.parse(text, None);
    let spans = syntax.spans(text, 0, text.len());
    let mut next = 0;
    let mut start = 0;
    for (n, line) in lines.iter().enumerate() {
        let ty = *y + pad_y + n as f32 * line_h + (line_h - font::height(size)) * 0.5;
        let mut pen = x + pad_x;
        let mut at = 0;
        while at < line.len() {
            let abs = start + at;
            while spans.get(next).is_some_and(|s| s.end <= abs) {
                next += 1;
            }
            let (kind, edge) = match spans.get(next) {
                Some(s) if s.start <= abs => (Some(s.kind), s.end - start),
                Some(s) => (None, s.start - start),
                None => (None, line.len()),
            };
            let mut edge = edge.min(line.len()).max(at + 1);
            while !line.is_char_boundary(edge) {
                edge += 1;
            }
            let piece = line[at..edge].replace('\t', "    ");
            let pw = font::width_in(Family::Mono, &piece, size);
            if pen < x + w {
                out.items.push(Item::Text {
                    x: pen,
                    y: ty,
                    text: piece,
                    family: Family::Mono,
                    size,
                    ink: Ink::Code(kind),
                    underline: false,
                    strike: false,
                });
            }
            pen += pw;
            at = edge;
        }
        start += line.len() + 1;
    }
    let label_size = m.size * 0.75;
    let bw = font::width_in(Family::Prose, COPY, label_size) + m.em * 0.8;
    let bh = font::line_advance(label_size) + m.em * 0.3;
    let button = Rect::new(x + w - bw - m.em * 0.3, *y + m.em * 0.3, bw, bh);
    out.items.push(Item::Text {
        x: button.x + m.em * 0.4,
        y: button.y + (bh - font::height(label_size)) * 0.5,
        text: COPY.to_string(),
        family: Family::Prose,
        size: label_size,
        ink: Ink::Dim,
        underline: false,
        strike: false,
    });
    out.copies.push((button, text.to_string()));
    *y += h + m.em * 1.0;
}

/// **A table**: its columns as wide as what they hold, narrowed together to
/// the page, its cells wrapped, a line under its head and between its rows.
#[allow(clippy::too_many_arguments)] // a table and its place
fn lay_table(
    out: &mut Laid,
    aligns: &[Align],
    head: &[Vec<Run>],
    rows: &[Vec<Vec<Run>>],
    x: f32,
    w: f32,
    y: &mut f32,
    m: Measures,
) {
    let size = m.size * 0.857;
    let line_h = m.line(size);
    let pad = m.em * 0.57;
    let columns = head.len().max(rows.iter().map(Vec::len).max().unwrap_or(0));
    if columns == 0 {
        return;
    }
    let natural = |cell: &[Run], strong: bool| -> f32 {
        cell.iter()
            .map(|r| {
                let base = if strong {
                    Family::ProseStrong
                } else {
                    Family::Prose
                };
                font::width_in(family_of(r, base), &r.text, size)
            })
            .sum::<f32>()
            + 2.0 * pad
    };
    let mut widths = vec![2.0 * pad; columns];
    for (c, cell) in head.iter().enumerate() {
        widths[c] = widths[c].max(natural(cell, true));
    }
    for row in rows {
        for (c, cell) in row.iter().enumerate().take(columns) {
            widths[c] = widths[c].max(natural(cell, false));
        }
    }
    let total: f32 = widths.iter().sum();
    if total > w {
        for wc in &mut widths {
            *wc *= w / total;
        }
    }
    *y += m.em * 0.25;
    let all: Vec<(&[Vec<Run>], bool)> = std::iter::once((head, true))
        .chain(rows.iter().map(|r| (r.as_slice(), false)))
        .collect();
    for (r, (cells, is_head)) in all.iter().enumerate() {
        if cells.is_empty() {
            continue;
        }
        let top = *y + pad * 0.5;
        let mut bottom = top + line_h;
        let mut cx = x;
        for (c, wc) in widths.iter().enumerate() {
            if let Some(cell) = cells.get(c) {
                let inner = (wc - 2.0 * pad).max(1.0);
                let base = if *is_head {
                    Family::ProseStrong
                } else {
                    Family::Prose
                };
                let shift = match aligns.get(c).copied().unwrap_or_default() {
                    Align::Left => 0.0,
                    a => {
                        let used = (natural(cell, *is_head) - 2.0 * pad).min(inner);
                        if a == Align::Right {
                            inner - used
                        } else {
                            (inner - used) * 0.5
                        }
                    }
                };
                let end = lay_runs(
                    out,
                    cell,
                    cx + pad + shift,
                    inner,
                    top,
                    size,
                    line_h,
                    base,
                    Ink::Text,
                );
                bottom = bottom.max(end);
            }
            cx += wc;
        }
        *y = bottom + pad * 0.5;
        let table_w: f32 = widths.iter().sum();
        if r + 1 < all.len() {
            out.items.push(Item::Fill {
                rect: Rect::new(x, *y, table_w, if *is_head { 1.5 } else { 1.0 }),
                ink: Ink::Line,
            });
        }
    }
    *y += m.em * 1.0;
}
