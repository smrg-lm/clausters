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
    /// **A code block's copy button's ground**, by the button's index in
    /// [`Laid::copies`]: none at rest, the look of a button held down while
    /// the hand holds it, which the caller's ink says.
    Button(usize),
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
        /// The [`Pan`] it scrolls across with, for a code block's line.
        pan: Option<usize>,
        /// **The stretch of text it is part of** -- a paragraph, a
        /// heading, a cell, a line of code, up to a line break -- by its
        /// number in the page (`Laid::paras`): what a reading of the page as
        /// one text ends a line at. `None` for what is drawn and not read,
        /// a code block's copy button and a list's mark.
        para: Option<usize>,
    },
    Fill {
        rect: Rect,
        ink: Ink,
    },
    /// **A picture**, in `rect`: the image at `url`, as the page's
    /// [`Pictures`] found it.
    Picture {
        rect: Rect,
        url: String,
    },
}

/// **How large a picture is**, by the `url` a page writes it with: its
/// natural size in the page's pixels, `None` while it is not read (or could
/// not be), when the page writes its description instead.
pub type Pictures<'a> = &'a dyn Fn(&str) -> Option<(f32, f32)>;

impl Item {
    /// The top of what it draws.
    pub fn top(&self) -> f32 {
        match self {
            Item::Text { y, .. } => *y,
            Item::Fill { rect, .. } | Item::Picture { rect, .. } => rect.y,
        }
    }

    /// The bottom of what it draws.
    pub fn bottom(&self) -> f32 {
        match self {
            Item::Text { y, size, .. } => y + font::line_advance(*size),
            Item::Fill { rect, .. } | Item::Picture { rect, .. } => rect.y + rect.h,
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
    /// How many stretches of text the page holds ([`Item::Text`]'s `para`).
    pub paras: usize,

    /// **The code blocks wider than the page**, each scrolled across on its
    /// own ([`Pan`]).
    pub pans: Vec<Pan>,
}

/// **A code block whose longest line is wider than the page**: its ground,
/// which its lines are cut to, and how wide they are, so it scrolls across
/// on its own -- the page never does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pan {
    pub rect: Rect,
    /// The width its lines take, their margins included.
    pub content: f32,
    /// Its copy button, which a line beside it stops short of, so the
    /// button's label never reads through the code.
    pub button: Rect,
}

impl Pan {
    /// How far it scrolls at most.
    pub fn most(&self) -> f32 {
        (self.content - self.rect.w).max(0.0)
    }
}

impl Laid {
    /// Where `id` lands, from the top.
    pub fn anchor(&self, id: &str) -> Option<f32> {
        self.anchors.iter().find(|(a, _)| a == id).map(|(_, y)| *y)
    }

    /// The link under `(x, y)`.
    /// A new stretch of text: its number.
    fn next_para(&mut self) -> usize {
        self.paras += 1;
        self.paras - 1
    }

    pub fn link_at(&self, x: f32, y: f32) -> Option<&str> {
        self.links
            .iter()
            .find(|(r, _)| r.contains(f64::from(x), f64::from(y)))
            .map(|(_, href)| href.as_str())
    }

    /// The copy button under `(x, y)`, and its code.
    pub fn copy_at(&self, x: f32, y: f32) -> Option<&str> {
        self.copy_index_at(x, y).map(|i| self.copies[i].1.as_str())
    }

    /// The copy button under `(x, y)`, by its index in [`Self::copies`].
    pub fn copy_index_at(&self, x: f32, y: f32) -> Option<usize> {
        self.copies
            .iter()
            .position(|(r, _)| r.contains(f64::from(x), f64::from(y)))
    }

    /// **The scroll bar of code block `i` wider than the page**, the page's
    /// top left at `(left, top)` and the block scrolled across by `pan`: a
    /// bar along its foot, which says how far and which a hand drags.
    /// `None` for a block that fits.
    pub fn pan_bar(
        &self,
        i: usize,
        (left, top): (f32, f32),
        pan: f32,
        m: &clausters_gui_draw::metrics::Metrics,
    ) -> Option<crate::host::graphics::controls::ScrollBar> {
        let block = self.pans.get(i)?;
        let track = Rect::new(
            left + block.rect.x,
            top + block.rect.y + block.rect.h - m.scrollbar_w,
            block.rect.w,
            m.scrollbar_w,
        );
        crate::host::graphics::controls::ScrollBar::new(
            false,
            track,
            block.rect.w,
            block.most(),
            pan.clamp(0.0, block.most()),
            m.pad * 2.0,
        )
    }

    /// **The code block whose bar is under `at`**, as a press takes it: its
    /// index, the thumb in hand and where the block is scrolled to.
    pub fn pan_press(
        &self,
        origin: (f32, f32),
        at: (f64, f64),
        pan: &dyn Fn(usize) -> f32,
        m: &clausters_gui_draw::metrics::Metrics,
    ) -> Option<(usize, crate::host::graphics::controls::ThumbGrip, f32)> {
        (0..self.pans.len()).find_map(|i| {
            let bar = self.pan_bar(i, origin, pan(i), m)?;
            let (grip, to) = bar.press(at, pan(i).clamp(0.0, self.pans[i].most()))?;
            Some((i, grip, to))
        })
    }

    /// The thumbs of the code blocks wider than the page, where each is
    /// scrolled across: the documentation panel's page and a hover card's
    /// documentation alike.
    pub fn draw_pan_bars(
        &self,
        mesh: &mut Mesh,
        origin: (f32, f32),
        pan: &dyn Fn(usize) -> f32,
        m: &clausters_gui_draw::metrics::Metrics,
        theme: &clausters_gui_draw::theme::Theme,
    ) {
        for i in 0..self.pans.len() {
            if let Some(bar) = self.pan_bar(i, origin, pan(i), m) {
                crate::host::graphics::controls::scroll_thumb(mesh, bar.thumb, m, theme);
            }
        }
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
    draw_items_with(
        mesh,
        laid,
        (left, top),
        view,
        ink,
        &|_| None,
        &|_| 0.0,
        &mut |_| {},
    );
}

/// [`draw_items`], each picture drawn from the texels of the sheet of
/// pictures `uv` names it by (`clausters_gui_draw::image`) -- one it names
/// none for is not drawn -- and each code block's lines moved across by how
/// far `pan` says it is scrolled, cut to its ground. `under` draws between
/// the grounds and the text: **a selection shows over a code block's ground**
/// and under its letters.
#[allow(clippy::too_many_arguments)] // the page's one drawing door, its hooks named
pub fn draw_items_with(
    mesh: &mut Mesh,
    laid: &Laid,
    (left, top): (f32, f32),
    view: Rect,
    ink: impl Fn(Ink) -> Color,
    uv: &dyn Fn(&str) -> Option<[f32; 4]>,
    pan: &dyn Fn(usize) -> f32,
    under: &mut dyn FnMut(&mut Mesh),
) {
    let kept = mesh.clip();
    let seen = |item: &Item| !(item.bottom() + top < view.y || item.top() + top > view.y + view.h);
    // the grounds first, then what goes under the text, then the rest
    for grounds in [true, false] {
        if !grounds {
            under(mesh);
            mesh.set_clip(kept);
        }
        for item in laid
            .items
            .iter()
            .filter(|i| seen(i) && matches!(i, Item::Fill { .. }) == grounds)
        {
            match item {
                Item::Fill {
                    rect,
                    ink: role @ Ink::Button(_),
                } => {
                    let color = ink(*role);
                    if color[3] > 0.0 {
                        let r = Rect::new(left + rect.x, top + rect.y, rect.w, rect.h);
                        mesh.round_rect(r, r.h * 0.2, color);
                    }
                }
                Item::Fill { rect, ink: role } => mesh.rect(
                    Rect::new(left + rect.x, top + rect.y, rect.w, rect.h),
                    ink(*role),
                ),
                Item::Picture { rect, url } => {
                    if let Some(uv) = uv(url) {
                        let r = Rect::new(left + rect.x, top + rect.y, rect.w, rect.h);
                        let r = Rect::new(r.x.round(), r.y.round(), r.w.round(), r.h.round());
                        mesh.image(r, uv);
                    }
                }
                Item::Text {
                    x,
                    y,
                    text,
                    family,
                    size,
                    ink: role,
                    underline,
                    strike,
                    pan: across,
                    ..
                } => {
                    let color = ink(*role);
                    let mut left = left;
                    if let Some(p) = across.and_then(|i| laid.pans.get(i).map(|b| (i, b))) {
                        let (i, block) = p;
                        left -= pan(i).clamp(0.0, block.most());
                        let mut ground = Rect::new(
                            left + pan(i).clamp(0.0, block.most()) + block.rect.x,
                            top + block.rect.y,
                            block.rect.w,
                            block.rect.h,
                        );
                        // a line beside the copy button stops short of it
                        let b = block.button;
                        let row = font::line_advance(*size);
                        if *y < b.y + b.h && y + row > b.y {
                            ground.w = (b.x - block.rect.x - font::height(*size) * 0.5).max(0.0);
                        }
                        mesh.set_clip(Some(kept.map_or(ground, |k| k.intersect(ground))));
                    } else {
                        mesh.set_clip(kept);
                    }
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
    mesh.set_clip(kept);
}

/// What a code block's copy button says where the host carries no icon
/// of it.
pub const COPY: &str = "Copy";

/// **What a code block's copy button shows**: the host's copy icon, as a
/// documentation page's code blocks commonly have, or its word.
fn copy_face() -> String {
    font::outline::icon("copy").map_or_else(|| COPY.to_string(), String::from)
}

/// The sizes and spacing a page is laid out with, from its text's size,
/// and how large its pictures are.
#[derive(Clone, Copy)]
struct Measures<'a> {
    size: f32,
    /// The em of the text: a capital is about three quarters of it.
    em: f32,
    pictures: Pictures<'a>,
}

impl Measures<'_> {
    fn new(size: f32) -> Measures<'static> {
        Measures {
            size,
            em: font::height(size) / 0.727,
            pictures: &|_| None,
        }
    }

    /// The height of a line of text at `size`: a book page's leading.
    fn line(&self, size: f32) -> f32 {
        (font::height(size) / 0.727 * 1.6).max(font::line_advance(size))
    }
}

/// **How far apart two lines of a paragraph stand**, at `size`: a book
/// page's leading, the line box a selection of a line fills.
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
    lay_with(blocks, width, size, &|_| None)
}

/// [`lay`], with its pictures drawn at the sizes `pictures` says: a picture
/// at its natural size, narrowed to the page in proportion where it is
/// wider; its description, as [`lay`] writes it, where it has none.
pub fn lay_with(blocks: &[Block], width: f32, size: f32, pictures: Pictures<'_>) -> Laid {
    let m = Measures {
        pictures,
        ..Measures::new(size)
    };
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
    m: Measures<'_>,
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
                        pan: None,
                        para: None,
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
            Block::Image { url, .. } if let Some((pw, ph)) = (m.pictures)(url) => {
                // its natural size, narrowed to the page in proportion
                let scale = (w / pw.max(1.0)).min(1.0);
                let (pw, ph) = (pw * scale, ph * scale);
                *y += m.em * 0.5;
                out.items.push(Item::Picture {
                    rect: Rect::new(x, *y, pw, ph),
                    url: url.clone(),
                });
                *y += ph + m.em * 0.5;
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
    let mut para = out.next_para();
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
                para = out.next_para();
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
                place(
                    out,
                    pen,
                    lead,
                    ty(top),
                    piece,
                    family,
                    size,
                    run_ink,
                    para,
                    run,
                );
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
    para: usize,
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
        pan: None,
        para: lp,
    }) = out.items.last_mut()
        && *lp == Some(para)
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
        pan: None,
        para: Some(para),
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
fn lay_code(out: &mut Laid, lang: &str, text: &str, x: f32, w: f32, y: &mut f32, m: Measures<'_>) {
    let size = m.size * 0.857;
    let line_h = font::line_advance(size) * 1.3;
    let (pad_x, pad_y) = (m.em, m.em * 0.67);
    *y += m.em * 0.25;
    let lines: Vec<&str> = text.split('\n').collect();
    let mut h = lines.len() as f32 * line_h + 2.0 * pad_y;
    let ground = out.items.len();
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
    let pan = out.pans.len();
    // the copy button at its top right, which a line beside it stops short
    // of (`draw_items_with`)
    let label_size = m.size * 0.75;
    let face = copy_face();
    let bh = font::line_advance(label_size) + m.em * 0.3;
    let fw = font::width_in(Family::Prose, &face, label_size);
    // an icon alone stands in a square, a word inset on both sides
    let bw = if face == COPY { fw + m.em * 0.8 } else { bh };
    let button = Rect::new(x + w - bw - m.em * 0.3, *y + m.em * 0.3, bw, bh);
    let beside = |ty: f32| ty < button.y + button.h && ty + font::line_advance(size) > button.y;
    let short = (button.x - x - font::height(size) * 0.5).max(0.0);
    let mut content = w;
    for (n, line) in lines.iter().enumerate() {
        let para = out.next_para();
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
            out.items.push(Item::Text {
                x: pen,
                y: ty,
                text: piece,
                family: Family::Mono,
                size,
                ink: Ink::Code(kind),
                underline: false,
                strike: false,
                pan: Some(pan),
                para: Some(para),
            });
            pen += pw;
            at = edge;
        }
        content = content.max(pen - x + pad_x);
        // **a line beside the button is seen to its end too**: it is cut
        // short of the button, so the block scrolls far enough to bring its
        // end out from under it
        if beside(ty) {
            content = content.max(pen - x - short + w);
        }
        start += line.len() + 1;
    }
    // a block wider than the page scrolls across, with room under its last
    // line for the bar that says how far; one that fits does not need to,
    // and its lines are cut to its ground all the same
    if content > w + 0.5 {
        h += m.em * 0.75;
        if let Some(Item::Fill { rect, .. }) = out.items.get_mut(ground) {
            rect.h = h;
        }
    }
    out.pans.push(Pan {
        rect: Rect::new(x, *y, w, h),
        content,
        button,
    });
    // its ground, which shows it held down
    out.items.push(Item::Fill {
        rect: button,
        ink: Ink::Button(out.copies.len()),
    });
    out.items.push(Item::Text {
        x: button.x + (bw - fw) * 0.5,
        y: button.y + (bh - font::height(label_size)) * 0.5,
        text: face,
        family: Family::Prose,
        size: label_size,
        ink: Ink::Dim,
        underline: false,
        strike: false,
        pan: None,
        para: None,
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
    m: Measures<'_>,
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
