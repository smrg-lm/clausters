//! `post` -- a post window: the lines a session writes, appended as it
//! writes them, read, selected and copied.
//!
//! The model is [`crate::host::graphics::post`]: the lines, their runs of
//! a kind, the lines kept and what a selection copies. What this file adds
//! is the window onto it: which rows are seen, the wheel and the scroll bar,
//! a press that selects by character, word or line, the keys and the menu.
//!
//! **Only what is seen is measured and drawn.** A line is wrapped to the
//! element's width when it is first shown and kept so until the width or
//! the size changes; the scroll position is a line and a row of it, never a
//! pixel offset over the whole, so ten thousand lines written at once cost
//! the append and the rows on screen -- nothing measures the rest.
//!
//! Its lines are written in the monospaced face ([`Family::Mono`]), as the
//! code they come from is.
//!
//! **It follows the end** while the reader is at it: a write shows the
//! newest lines, a reader who scrolls up stays where they read, and coming
//! back down to the end follows again. `autoscroll` off never follows.

use std::cell::RefCell;
use std::collections::HashMap;

use clausters_core::osc::OscType;
use serde_json::{Map, Value};

use crate::host::graphics::controls::{self, ButtonLook, ToggleView};
use crate::host::graphics::post::{self, Kind, Pos, Post};
use crate::host::graphics::textedit::Unit;
use crate::host::menu::Entry;
use crate::host::pointer::Pointer;
use crate::host::widget::element::{Claim, Ctx, Element, Events, Input, Key, KeyInput, Verb};
use crate::host::widget::parse;
use crate::host::widget::size::{Natural, control_box};
use clausters_gui_draw::draw::Draw;
use clausters_gui_draw::font::{self, Family};
use clausters_gui_draw::metrics::Metrics;
use clausters_gui_draw::rect::Rect;
use clausters_gui_draw::theme::{Color, Theme};

/// How many rows a step of the wheel scrolls.
const WHEEL_ROWS: f64 = 3.0;

/// The clear tool's symbol: erase to the left.
const CLEAR: char = '\u{232B}';

/// A post window: what it holds, how it is drawn, where it is scrolled to
/// and what a hand selected.
#[derive(Debug, Clone)]
pub struct PostView {
    post: Post,
    text_size: f32,
    wrap: bool,
    autoscroll: bool,
    /// Whether it carries its strip of tools (`tools`): clear and
    /// autoscroll, along its top or in its window's header.
    tools: bool,
    /// Whether the newest line is in view and a write keeps it there.
    follow: bool,
    /// The first row seen, while not following: a line and a row of it.
    top: (u64, usize),
    /// The selection: where it was started and where it reaches.
    sel: Option<(Pos, Pos)>,
    /// What a press selected and by what unit, while a drag grows it.
    held: Option<((Pos, Pos), Unit)>,
    /// Whether a press holds the scroll bar's thumb.
    thumb: bool,
    /// The rows of the lines measured, at the width and size they were.
    rows: RefCell<Rows>,
}

/// Where each line measured breaks into rows, and what that was measured at.
#[derive(Debug, Clone, Default)]
struct Rows {
    at: (u32, u32),
    /// By line number: the line's length when measured, and its rows.
    lines: HashMap<u64, (usize, Vec<usize>)>,
}

/// Where the rows of the element are: its text area, its scroll bar, the
/// height of a row and the text size, at the scale it is drawn at.
#[derive(Clone, Copy, Debug)]
struct Geometry {
    text: Rect,
    bar: Rect,
    row_h: f32,
    size: f32,
    /// The strip of tools, when it carries one: along its top, or the slot
    /// of its window's header.
    strip: Option<Strip>,
}

/// The strip's tools, where they are.
#[derive(Clone, Copy, Debug)]
struct Strip {
    rect: Rect,
    clear: Rect,
    autoscroll: Rect,
    /// The host's text size, which the tools are drawn at.
    ui: f32,
}

pub(super) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(PostView::new(props)))
}

impl PostView {
    /// A post window from a def's props: empty, following its end.
    fn new(props: &Map<String, Value>) -> Self {
        let mut view = PostView {
            post: Post::default(),
            text_size: parse::text_size(props),
            wrap: true,
            autoscroll: true,
            tools: false,
            follow: true,
            top: (0, 0),
            sel: None,
            held: None,
            thumb: false,
            rows: RefCell::default(),
        };
        for (key, value) in props {
            view.set(key, value);
        }
        view
    }

    /// Where its parts are in `rect`; its strip of tools along its top, or
    /// in `header`, the slot of its window's header.
    fn geometry(&self, rect: Rect, scale: f32, m: &Metrics, header: Option<Rect>) -> Geometry {
        let size = self.text_size * scale;
        let bar_w = m.scrollbar_w;
        let pad = m.pad;
        let ui = font::DEFAULT_SIZE * scale;
        let row = control_box(ui, m);
        let (strip, rect) = match (self.tools, header) {
            (false, _) => (None, rect),
            (true, Some(slot)) => (Some(slot), rect),
            (true, None) => {
                let strip = Rect::new(rect.x, rect.y, rect.w, row.min(rect.h));
                let below = Rect::new(
                    rect.x,
                    rect.y + strip.h,
                    rect.w,
                    (rect.h - strip.h).max(1.0),
                );
                (Some(strip), below)
            }
        };
        let strip = strip.map(|s| {
            let y = s.y + ((s.h - row) * 0.5).max(0.0);
            Strip {
                rect: s,
                clear: Rect::new(s.x, y, row, row),
                autoscroll: Rect::new(s.x + row, y, row, row),
                ui,
            }
        });
        Geometry {
            strip,
            text: Rect::new(
                rect.x + pad,
                rect.y + pad * 0.5,
                (rect.w - 2.0 * pad - bar_w).max(1.0),
                (rect.h - pad).max(1.0),
            ),
            bar: Rect::new(rect.x + rect.w - bar_w, rect.y, bar_w, rect.h),
            row_h: font::code_line(size).max(1.0),
            size,
        }
    }

    /// The rows line `n` breaks into at `g`: measured once and kept.
    fn rows_of(&self, n: u64, g: &Geometry) -> Vec<usize> {
        let Some(line) = self.post.line(n) else {
            return vec![0];
        };
        if !self.wrap {
            return vec![0];
        }
        let at = (g.text.w.to_bits(), g.size.to_bits());
        let mut rows = self.rows.borrow_mut();
        if rows.at != at {
            *rows = Rows {
                at,
                lines: HashMap::new(),
            };
        }
        if let Some((len, kept)) = rows.lines.get(&n)
            && *len == line.text.len()
        {
            return kept.clone();
        }
        let shown = display(&line.text);
        let breaks = post::rows(&shown, g.text.w, |s| {
            font::width_in(Family::Mono, s, g.size)
        });
        rows.lines.insert(n, (line.text.len(), breaks.clone()));
        breaks
    }

    /// How many rows the text area shows.
    fn seen(g: &Geometry) -> usize {
        ((g.text.h / g.row_h).floor() as usize).max(1)
    }

    /// **The first row seen**: where the reader left it, or -- following --
    /// the row that puts the last line's last row at the bottom.
    fn top_now(&self, g: &Geometry) -> (u64, usize) {
        if !self.follow {
            return self.clamp_top(self.top);
        }
        let mut left = Self::seen(g);
        let mut n = self.post.end();
        while n > self.post.first() {
            n -= 1;
            let count = self.rows_of(n, g).len();
            if count >= left {
                return (n, count - left);
            }
            left -= count;
        }
        (self.post.first(), 0)
    }

    fn clamp_top(&self, (line, row): (u64, usize)) -> (u64, usize) {
        if line < self.post.first() {
            (self.post.first(), 0)
        } else if line >= self.post.end() {
            (self.post.end().saturating_sub(1).max(self.post.first()), 0)
        } else {
            (line, row)
        }
    }

    /// **The rows seen**, top down: each a line, the bytes of it, and its
    /// top edge.
    fn visible(&self, g: &Geometry) -> Vec<(u64, usize, usize, f32)> {
        let (mut n, mut row) = self.top_now(g);
        let mut y = g.text.y;
        let mut out = Vec::new();
        while n < self.post.end() && y < g.text.y + g.text.h {
            let rows = self.rows_of(n, g);
            let len = self.post.line(n).map_or(0, |l| l.text.len());
            while row < rows.len() && y < g.text.y + g.text.h {
                let end = rows.get(row + 1).copied().unwrap_or(len);
                out.push((n, rows[row], end, y));
                y += g.row_h;
                row += 1;
            }
            n += 1;
            row = 0;
        }
        out
    }

    /// Scrolls by `by` rows, down for a positive one; reaching the end
    /// follows it again. Answers whether anything moved.
    fn scroll(&mut self, by: i64, g: &Geometry) -> bool {
        let before = self.top_now(g);
        let (mut n, mut row) = before;
        if by < 0 {
            for _ in 0..by.unsigned_abs() {
                if row > 0 {
                    row -= 1;
                } else if n > self.post.first() {
                    n -= 1;
                    row = self.rows_of(n, g).len() - 1;
                } else {
                    break;
                }
            }
        } else {
            for _ in 0..by {
                if row + 1 < self.rows_of(n, g).len() {
                    row += 1;
                } else if n + 1 < self.post.end() {
                    n += 1;
                    row = 0;
                } else {
                    break;
                }
            }
        }
        self.top = (n, row);
        self.follow = false;
        // at or past where following would put it: follow again
        let end = self.top_now_following(g);
        if (n, row) >= end && self.autoscroll {
            self.follow = true;
        } else if (n, row) > end {
            self.top = end;
        }
        self.top_now(g) != before || by == 0
    }

    fn top_now_following(&self, g: &Geometry) -> (u64, usize) {
        let mut probe = self.clone();
        probe.follow = true;
        probe.top_now(g)
    }

    /// The place in the text under `(x, y)`, in the window's pixels.
    fn pos_at(&self, (x, y): (f64, f64), g: &Geometry) -> Pos {
        let rows = self.visible(g);
        let (x, y) = (x as f32, y as f32);
        let hit = rows
            .iter()
            .find(|r| y < r.3 + g.row_h)
            .or(rows.last())
            .copied();
        let Some((n, from, to, _)) = hit else {
            return Pos {
                line: self.post.first(),
                byte: 0,
            };
        };
        if y < g.text.y {
            return Pos {
                line: n,
                byte: from,
            };
        }
        let text = self.post.line(n).map_or("", |l| l.text.as_str());
        let shown = display(&text[from..to]);
        let byte = from + font::byte_at_in(Family::Mono, &shown, x - g.text.x, g.size);
        Pos { line: n, byte }
    }

    fn selected(&self) -> Option<String> {
        let (a, b) = self.sel?;
        (a != b).then(|| self.post.text_between(a, b))
    }

    fn select_all(&mut self) {
        if self.post.is_empty() {
            return;
        }
        self.sel = Some((
            Pos {
                line: self.post.first(),
                byte: 0,
            },
            Pos {
                line: self.post.end() - 1,
                byte: usize::MAX,
            },
        ));
        self.sel = self.sel.map(|(a, b)| (a, self.post.clamp(b)));
    }

    fn clear(&mut self) {
        self.post.clear();
        self.sel = None;
        self.held = None;
        self.follow = true;
        self.rows.borrow_mut().lines.clear();
    }

    /// The thumb of the scroll bar, when there is more than is seen.
    fn thumb_rect(&self, g: &Geometry) -> Option<Rect> {
        let total = self.post.len() as f32;
        let seen = Self::seen(g) as f32;
        let (top, _) = self.top_now(g);
        let visible: f32 = {
            let rows = self.visible(g);
            let lines = rows.last().map_or(0, |r| r.0 + 1) - rows.first().map_or(0, |r| r.0);
            lines as f32
        };
        if total <= visible && top == self.post.first() && total <= seen {
            return None;
        }
        let share = (visible / total.max(1.0)).clamp(0.05, 1.0);
        let h = (g.bar.h * share).max(g.row_h);
        let at = (top - self.post.first()) as f32 / total.max(1.0);
        let y = g.bar.y + (g.bar.h - h) * (at / (1.0 - share).max(1e-6)).min(1.0);
        Some(Rect::new(g.bar.x + g.bar.w * 0.25, y, g.bar.w * 0.5, h))
    }

    /// The scroll bar pressed or dragged at `y`: the line there at the top.
    /// Autoscroll turned the other way, and the owner told
    /// (`"autoscroll" <0|1>`), so it keeps what the reader chose.
    fn toggle_autoscroll(&mut self) -> Events {
        self.autoscroll = !self.autoscroll;
        self.follow = self.autoscroll;
        Events::message(vec![
            OscType::String("autoscroll".into()),
            OscType::Int(i32::from(self.autoscroll)),
        ])
    }

    fn scroll_to(&mut self, y: f64, g: &Geometry) {
        let frac = ((y as f32 - g.bar.y) / g.bar.h.max(1.0)).clamp(0.0, 1.0);
        let n = self.post.first() + (frac * self.post.len() as f32) as u64;
        self.top = (n, 0);
        self.follow = false;
        let end = self.top_now_following(g);
        if self.top >= end {
            self.top = end;
            self.follow = self.autoscroll;
        }
    }

    /// **Its strip of tools**: clear and autoscroll, on the strip's ground
    /// along its top, or in its window's header, whose ground the header
    /// draws.
    fn paint_strip(&self, d: &mut Draw, ctx: &Ctx, s: &Strip) {
        let kept = d.mesh.clip();
        d.mesh
            .set_clip(Some(kept.map_or(s.rect, |k| k.intersect(s.rect))));
        if ctx.header.is_none() {
            let (mesh, m, theme) = d.parts();
            mesh.rect(s.rect, theme.status_bar);
            let r = s.rect;
            mesh.rect(
                Rect::new(r.x, r.y + r.h - m.divider_w, r.w, m.divider_w),
                theme.separator,
            );
        }
        let inset = (d.m.pad * 0.5).round();
        let flat = ButtonLook {
            flat: true,
            ..ButtonLook::default()
        };
        let clear = controls::icon("eraser", CLEAR);
        controls::button(d, None, Some(clear), s.clear.inset(inset), flat, s.ui);
        controls::toggle(
            d,
            self.autoscroll,
            None,
            Some(controls::icon("arrow-down-to-line", font::symbol::DOWN)),
            s.autoscroll.inset(inset),
            ToggleView::Button,
            false,
            s.ui,
        );
        d.mesh.set_clip(kept);
    }

    fn paint(&self, d: &mut Draw, ctx: &Ctx) {
        let g = self.geometry(ctx.rect, ctx.scale, ctx.metrics, ctx.header);
        if let Some(s) = &g.strip {
            self.paint_strip(d, ctx, s);
        }
        let (mesh, _, theme) = d.parts();
        let kept = mesh.clip();
        let area = match (&g.strip, ctx.header) {
            // the strip along its top is drawn; the rest is the text's
            (Some(s), None) => Rect::new(
                ctx.rect.x,
                s.rect.y + s.rect.h,
                ctx.rect.w,
                (ctx.rect.h - s.rect.h).max(0.0),
            ),
            _ => ctx.rect,
        };
        mesh.set_clip(Some(kept.map_or(area, |k| k.intersect(area))));
        mesh.rect(area, theme.view_field);
        let glyph = font::height(g.size);
        let sel = self
            .sel
            .filter(|(a, b)| a != b)
            .map(|(a, b)| (a.min(b), a.max(b)));
        for (n, from, to, y) in self.visible(&g) {
            let Some(line) = self.post.line(n) else {
                continue;
            };
            let row = &line.text[from..to];
            let shown = display(row);
            let ty = y + (g.row_h - glyph) * 0.5;
            // the selection under the row's text
            if let Some((a, b)) = sel
                && a.line <= n
                && n <= b.line
            {
                let s = if a.line == n {
                    a.byte.clamp(from, to)
                } else {
                    from
                };
                let e = if b.line == n {
                    b.byte.clamp(from, to)
                } else {
                    to
                };
                let ends = n < b.line && to == line.text.len();
                let [r, gr, bl, _] = theme.selection;
                for (l, rr) in font::spans_in(Family::Mono, &shown, s - from, e - from, g.size) {
                    mesh.rect(
                        Rect::new(g.text.x + l, y, rr - l, g.row_h),
                        [r, gr, bl, 0.35],
                    );
                }
                if ends {
                    // a line's end selected: a sliver past its text
                    let x = g.text.x + font::width_in(Family::Mono, &shown, g.size);
                    mesh.rect(Rect::new(x, y, g.size * 2.0, g.row_h), [r, gr, bl, 0.35]);
                }
            }
            for (s, e, kind) in line.spans() {
                let (s, e) = (s.max(from), e.min(to));
                if s >= e {
                    continue;
                }
                let x = g.text.x + font::width_in(Family::Mono, &shown[..s - from], g.size);
                font::text_in(
                    mesh,
                    Family::Mono,
                    &shown[s - from..e - from],
                    x,
                    ty,
                    g.size,
                    color(kind, theme),
                );
            }
        }
        if let Some(thumb) = self.thumb_rect(&g) {
            controls::scroll_thumb(mesh, thumb, ctx.metrics, theme);
        }
        mesh.set_clip(kept);
    }
}

/// How a run of `kind` is drawn: the theme's roles, not colors of its own.
fn color(kind: Kind, theme: &Theme) -> Color {
    match kind {
        Kind::Out => theme.text,
        Kind::Err => theme.warn,
        Kind::Result => theme.accent,
        Kind::Info => theme.text_dim,
    }
}

/// `s` as drawn: a control character (a tab, a carriage return) takes a
/// space's place, byte for byte, so what is drawn and what is copied
/// address the same bytes.
fn display(s: &str) -> std::borrow::Cow<'_, str> {
    if s.bytes().any(|b| b < b' ') {
        s.chars()
            .map(|c| if (c as u32) < 0x20 { ' ' } else { c })
            .collect::<String>()
            .into()
    } else {
        s.into()
    }
}

impl Element for PostView {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        if let Some(kind) = Kind::named(key) {
            let Some(text) = v.as_str() else {
                return false;
            };
            self.post.append(kind, text);
            if self.follow {
                self.sel = self
                    .sel
                    .map(|(a, b)| (self.post.clamp(a), self.post.clamp(b)));
            }
            return true;
        }
        match key {
            "clear" => {
                self.clear();
                true
            }
            "keep" => v.as_u64().map(|n| self.post.set_keep(n as usize)).is_some(),
            "wrap" => parse::truthy(v).map(|b| self.wrap = b).is_some(),
            "tools" => parse::truthy(v).map(|b| self.tools = b).is_some(),
            "autoscroll" => parse::truthy(v)
                .map(|b| {
                    self.autoscroll = b;
                    if b {
                        self.follow = true;
                    }
                })
                .is_some(),
            "text_size" => v
                .as_f64()
                .filter(|n| *n > 0.0)
                .map(|n| self.text_size = n as f32)
                .is_some(),
            // the props a def carries that are the schema's
            "id" | "type" | "name" => true,
            _ => false,
        }
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        self.paint(d, ctx);
    }

    /// The text of a kind and `clear` write into the post: none is a prop.
    fn writes(&self, key: &str) -> bool {
        key == "clear" || Kind::named(key).is_some()
    }

    /// **Elastic on both axes**: what it holds never sizes it, or every
    /// write would lay the window out again.
    fn natural(&self, _m: &Metrics, _scale: f32) -> Natural {
        Natural::default()
    }

    fn value(&self) -> Option<OscType> {
        None
    }

    fn info(&self) -> Vec<(String, Value)> {
        vec![
            ("lines".into(), Value::from(self.post.len())),
            ("autoscroll".into(), Value::from(self.autoscroll)),
            ("following".into(), Value::from(self.follow)),
            (
                "selected".into(),
                Value::from(self.selected().unwrap_or_default()),
            ),
        ]
    }

    fn accepts_focus(&self) -> bool {
        true
    }

    /// Clear and autoscroll, with `tools`: a strip a window may take as its
    /// header.
    fn strip(&self) -> bool {
        self.tools
    }

    /// One click places nothing but clears the selection, two select a
    /// word, three a line; Shift extends; the bar scrolls.
    /// **The I-beam over the lines**, which a press selects; the arrow over
    /// the strip's buttons and the scroll bar.
    fn pointer(&self, at: (f64, f64), input: &Input) -> Option<Pointer> {
        let g = self.geometry(input.rect, input.scale, input.metrics, input.header);
        let on_strip = g.strip.is_some_and(|s| s.rect.contains(at.0, at.1));
        let on_bar = g.bar.contains(at.0, at.1) && self.thumb_rect(&g).is_some();
        Some(if on_strip || on_bar {
            Pointer::Default
        } else {
            Pointer::Text
        })
    }

    fn press(&mut self, at: (f64, f64), input: &Input) -> Claim {
        let g = self.geometry(input.rect, input.scale, input.metrics, input.header);
        if let Some(s) = g.strip {
            if s.clear.contains(at.0, at.1) {
                self.clear();
                return Claim::events(Events::message(vec![OscType::String("clear".into())]));
            }
            if s.autoscroll.contains(at.0, at.1) {
                return Claim::events(self.toggle_autoscroll());
            }
            if s.rect.contains(at.0, at.1) {
                return Claim::take();
            }
        }
        if g.bar.contains(at.0, at.1) && self.thumb_rect(&g).is_some() {
            self.thumb = true;
            self.scroll_to(at.1, &g);
            return Claim::take();
        }
        let p = self.pos_at(at, &g);
        if input.mods.shift
            && input.clicks == 1
            && let Some((anchor, _)) = self.sel
        {
            self.held = Some(((anchor, anchor), Unit::Char));
            self.sel = Some((anchor, p));
            return Claim::take();
        }
        let unit = Unit::of_clicks(input.clicks);
        let span = self.post.unit_at(p, unit);
        self.held = Some((span, unit));
        self.sel = Some(span);
        Claim::take()
    }

    fn drag(&mut self, at: (f64, f64), input: &Input) -> Events {
        let g = self.geometry(input.rect, input.scale, input.metrics, input.header);
        if self.thumb {
            self.scroll_to(at.1, &g);
            return Events::none();
        }
        let Some((held, unit)) = self.held else {
            return Events::none();
        };
        // past the top or the bottom, the rows come to the pointer
        if (at.1 as f32) < g.text.y {
            self.scroll(-1, &g);
        } else if (at.1 as f32) > g.text.y + g.text.h {
            self.scroll(1, &g);
        }
        let p = self.pos_at(at, &g);
        self.sel = Some(self.post.extend(held, p, unit));
        Events::none()
    }

    fn release(&mut self, _at: (f64, f64), _inside: bool, _input: &Input) -> Events {
        self.held = None;
        self.thumb = false;
        Events::none()
    }

    fn wheel(&mut self, _at: (f64, f64), delta: (f64, f64), input: &Input) -> Option<Events> {
        // Ctrl+wheel is the size of the text, not a scroll: declined, the
        // window's own zoom takes it.
        if input.mods.ctrl {
            return None;
        }
        let g = self.geometry(input.rect, input.scale, input.metrics, input.header);
        let rows = (-delta.1 * WHEEL_ROWS).round() as i64;
        if rows == 0 {
            return None;
        }
        self.scroll(rows, &g);
        Some(Events::none())
    }

    fn key(&mut self, key: &Key, input: &mut KeyInput) -> Option<Events> {
        match key {
            Key::Char(c) if input.mods.ctrl => match c.to_ascii_lowercase() {
                'c' => {
                    if let Some(text) = self.selected() {
                        input.clipboard.set_text(&text);
                    }
                    Some(Events::none())
                }
                'a' => {
                    self.select_all();
                    Some(Events::none())
                }
                _ => None,
            },
            Key::End if input.mods.ctrl || !self.follow => {
                self.follow = true;
                Some(Events::none())
            }
            Key::Home => {
                self.top = (self.post.first(), 0);
                self.follow = false;
                Some(Events::none())
            }
            _ => None,
        }
    }

    fn selected_text(&self) -> Option<String> {
        self.selected()
    }

    fn holds_selection(&self) -> Option<bool> {
        Some(self.selected().is_some())
    }

    fn verb(&mut self, verb: Verb, input: &mut KeyInput) -> Option<Events> {
        match verb {
            Verb::Copy => {
                if let Some(text) = self.selected() {
                    input.clipboard.set_text(&text);
                }
                Some(Events::none())
            }
            Verb::SelectAll => {
                self.select_all();
                Some(Events::none())
            }
            _ => None,
        }
    }

    /// `clear` and `autoscroll` from the menu or a script.
    fn command(&mut self, name: &str, _input: &mut KeyInput) -> Option<Events> {
        match name {
            "clear" => {
                self.clear();
                Some(Events::none())
            }
            "autoscroll" => Some(self.toggle_autoscroll()),
            _ => None,
        }
    }

    fn context_menu(&mut self, _at: (f64, f64), _input: &Input) -> Option<Vec<Entry>> {
        let copy = Entry::action("Copy", "copy");
        Some(vec![
            if self.selected().is_some() {
                copy
            } else {
                copy.disabled()
            },
            Entry::action("Select All", "select_all"),
            Entry::separator(),
            Entry::action("Clear", "clear"),
            Entry::check("Autoscroll", "autoscroll", self.autoscroll),
        ])
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests;
