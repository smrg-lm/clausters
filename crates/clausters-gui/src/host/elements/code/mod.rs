//! `code` -- an editor of code: a text written in the monospaced face, with
//! its line numbers, the line the caret is on, the selection and the caret,
//! scrolled on both axes, edited by every hand.
//!
//! The model is [`crate::host::graphics::code`]: the text, the caret, the
//! history and the indentation. What this file adds is the window onto it:
//! where each line and each column is drawn, what the pointer lands on, the
//! keys and the menu, and keeping the caret in view.
//!
//! **Code is drawn on a grid.** The face is monospaced
//! ([`Family::Mono`]), so a column is a cell and a tab stands for the cells
//! up to the next level: where a byte is drawn is counted, not measured, and
//! the longest line is a count of cells. Only the rows in view are shaped
//! and drawn.
//!
//! **Tab is the element's** ([`Element::takes_tab`]): it indents, and
//! Ctrl+Tab leaves for the next element.

use std::cell::Cell;

use clausters_core::osc::OscType;
use clausters_editing::verbs::{self, ActionState, ActionTarget, Verb as ActionVerb};
use serde_json::{Map, Value};
use unicode_segmentation::UnicodeSegmentation;

use crate::host::graphics::code::{Code, Indent, Language};
use crate::host::graphics::textedit::{self, Caret, Unit};
use crate::host::menu::Entry;
use crate::host::widget::element::{Claim, Ctx, Element, Events, Input, Key, KeyInput, Verb};
use crate::host::widget::parse;
use crate::host::widget::size::Natural;
use clausters_gui_draw::draw::Draw;
use clausters_gui_draw::font::{self, Family};
use clausters_gui_draw::metrics::Metrics;
use clausters_gui_draw::rect::Rect;

/// How many rows a step of the wheel scrolls.
const WHEEL_ROWS: f32 = 3.0;

/// The columns a tab reaches to: the next multiple of these.
const TAB_COLS: usize = 4;

/// An editor of code.
#[derive(Debug, Clone)]
pub struct CodeView {
    code: Code,
    text_size: f32,
    line_numbers: bool,
    /// Where the view is scrolled to, in pixels from the text's top left.
    scroll: Cell<(f32, f32)>,
    /// Whether the caret is to be brought into view at the next frame.
    reveal: Cell<bool>,
    /// The rows the text area showed at the last frame: a page.
    page: Cell<usize>,
    /// The widest line, in cells, while the text has not changed.
    widest: Cell<Option<usize>>,
    /// What a press selected and by what unit, while a drag grows it.
    held: Option<((usize, usize), Unit)>,
    /// Whether the text was dirty when last reported.
    was_dirty: bool,
}

/// Where the parts of the element are, at the scale it is drawn at.
#[derive(Clone, Copy, Debug)]
struct Geometry {
    gutter: Rect,
    text: Rect,
    row_h: f32,
    cell: f32,
    size: f32,
    bar: f32,
}

pub(super) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(CodeView::new(props)))
}

impl CodeView {
    fn new(props: &Map<String, Value>) -> Self {
        let mut view = CodeView {
            code: Code::default(),
            text_size: parse::text_size(props),
            line_numbers: true,
            scroll: Cell::new((0.0, 0.0)),
            reveal: Cell::new(false),
            page: Cell::new(20),
            widest: Cell::new(None),
            held: None,
            was_dirty: false,
        };
        for (key, value) in props {
            view.set(key, value);
        }
        view
    }

    fn geometry(&self, rect: Rect, scale: f32, m: &Metrics) -> Geometry {
        let size = self.text_size * scale;
        let cell = font::cell_in(Family::Mono, size).max(1.0);
        let digits = self.code.lines().to_string().len().max(3) as f32;
        let gutter_w = if self.line_numbers {
            (digits * cell + 2.0 * m.pad).round()
        } else {
            0.0
        };
        let bar = (m.pad * 0.75).max(3.0 * scale).round();
        Geometry {
            gutter: Rect::new(rect.x, rect.y, gutter_w, rect.h),
            text: Rect::new(
                rect.x + gutter_w + m.pad,
                rect.y + m.pad * 0.5,
                (rect.w - gutter_w - m.pad - bar).max(1.0),
                (rect.h - m.pad * 0.5 - bar).max(1.0),
            ),
            row_h: font::line_advance(size).max(1.0),
            cell,
            size,
            bar,
        }
    }

    /// Where byte `at` of `line` is drawn, from the line's left: its cells,
    /// a tab reaching to the next stop.
    fn x_of(line: &str, at: usize, g: &Geometry) -> f32 {
        let mut x = 0.0;
        let mut from = 0;
        for (i, _) in line[..at.min(line.len())].match_indices('\t') {
            x += font::width_in(Family::Mono, &line[from..i], g.size);
            x = next_stop(x, g.cell);
            from = i + 1;
        }
        x + font::width_in(Family::Mono, &line[from..at.min(line.len())], g.size)
    }

    /// The byte of `line` nearest `dx` from its left: where a click lands.
    fn byte_at(line: &str, dx: f32, g: &Geometry) -> usize {
        let mut x = 0.0;
        let mut from = 0;
        for (i, _) in line.match_indices('\t') {
            let w = font::width_in(Family::Mono, &line[from..i], g.size);
            if dx < x + w {
                return from + font::byte_at_in(Family::Mono, &line[from..i], dx - x, g.size);
            }
            let stop = next_stop(x + w, g.cell);
            if dx < x + w + (stop - x - w) * 0.5 {
                return i;
            }
            if dx < stop {
                return i + 1;
            }
            x = stop;
            from = i + 1;
        }
        from + font::byte_at_in(Family::Mono, &line[from..], dx - x, g.size)
    }

    /// The widest line, in cells: what the horizontal scroll reaches.
    fn widest(&self) -> usize {
        if let Some(w) = self.widest.get() {
            return w;
        }
        let w = (0..self.code.lines())
            .filter_map(|n| self.code.line(n))
            .map(|(_, line)| cells(line))
            .max()
            .unwrap_or(0);
        self.widest.set(Some(w));
        w
    }

    /// The scroll position kept inside what there is to scroll, and -- when
    /// asked for -- moved so the caret is in view.
    fn settle(&self, g: &Geometry) -> (f32, f32) {
        let (mut sx, mut sy) = self.scroll.get();
        if self.reveal.replace(false) {
            let (line, _) = self.code.line_col();
            let top = line as f32 * g.row_h;
            if top < sy {
                sy = top;
            } else if top + g.row_h > sy + g.text.h {
                sy = top + g.row_h - g.text.h;
            }
            let (start, text) = self.code.line(line).unwrap_or((0, ""));
            let x = Self::x_of(text, self.code.caret().pos - start, g);
            let margin = 2.0 * g.cell;
            if x < sx + margin {
                sx = (x - margin).max(0.0);
            } else if x > sx + g.text.w - margin {
                sx = x - g.text.w + margin;
            }
        }
        let max_y = (self.code.lines() as f32 * g.row_h - g.text.h + g.row_h).max(0.0);
        let max_x = (self.widest() as f32 * g.cell + 2.0 * g.cell - g.text.w).max(0.0);
        let at = (sx.clamp(0.0, max_x), sy.clamp(0.0, max_y));
        self.scroll.set(at);
        self.page
            .set((g.text.h / g.row_h).floor().max(1.0) as usize);
        at
    }

    /// The byte under `(x, y)`, in the window's pixels.
    fn pos_at(&self, (x, y): (f64, f64), g: &Geometry) -> usize {
        let (sx, sy) = self.settle(g);
        let row = ((y as f32 - g.text.y + sy) / g.row_h).floor().max(0.0) as usize;
        let line = row.min(self.code.lines() - 1);
        let (start, text) = self.code.line(line).unwrap_or((0, ""));
        start + Self::byte_at(text, x as f32 - g.text.x + sx, g)
    }

    /// What a change reports: the text going dirty or clean.
    fn changed(&mut self) -> Events {
        self.widest.set(None);
        self.reveal.set(true);
        let dirty = self.code.dirty();
        if dirty == self.was_dirty {
            return Events::none();
        }
        self.was_dirty = dirty;
        Events::message(vec![
            OscType::String("dirty".into()),
            OscType::Int(i32::from(dirty)),
        ])
    }

    /// A motion: the caret moved, nothing changed, the caret is shown.
    fn moved(&mut self) -> Events {
        self.reveal.set(true);
        Events::none()
    }

    fn copy(&self, input: &mut KeyInput) {
        if let Some(text) = self.code.selected() {
            input.clipboard.set_text(text);
        }
    }

    fn cut(&mut self, input: &mut KeyInput) -> Events {
        match self.code.cut() {
            Some(text) => {
                input.clipboard.set_text(&text);
                self.changed()
            }
            None => Events::none(),
        }
    }

    fn paste(&mut self, input: &mut KeyInput) -> Events {
        // what another program copied since is what is pasted
        input.clipboard.refresh();
        if input.clipboard.is_empty() {
            return Events::none();
        }
        let text = input.clipboard.text();
        if self.code.paste(&text) {
            self.changed()
        } else {
            Events::none()
        }
    }

    fn undo(&mut self, redo: bool) -> Events {
        if self.code.step(redo) {
            self.changed()
        } else {
            Events::none()
        }
    }

    fn paint(&self, d: &mut Draw, ctx: &Ctx) {
        let g = self.geometry(ctx.rect, ctx.scale, ctx.metrics);
        let (sx, sy) = self.settle(&g);
        let (mesh, _, theme) = d.parts();
        let kept = mesh.clip();
        let area = ctx.rect;
        let clip = |r: Rect| kept.map_or(r, |k| k.intersect(r));
        mesh.set_clip(Some(clip(area)));
        mesh.rect(area, theme.background);
        if self.line_numbers {
            mesh.rect(g.gutter, theme.status_bar);
        }
        let glyph = font::height(g.size);
        let (caret_line, _) = self.code.line_col();
        let caret = self.code.caret();
        let sel = caret.selection();
        let first = (sy / g.row_h).floor() as usize;
        let rows = (g.text.h / g.row_h).ceil() as usize + 1;
        let [sr, sg, sb, _] = theme.selection;
        let sel_color = [sr, sg, sb, if ctx.focused { 0.35 } else { 0.2 }];
        for n in first..(first + rows).min(self.code.lines()) {
            let Some((start, line)) = self.code.line(n) else {
                break;
            };
            let y = g.text.y + n as f32 * g.row_h - sy;
            let ty = y + (g.row_h - glyph) * 0.5;
            // the line the caret is on, across the text
            if n == caret_line && sel.is_none() {
                let [r, gr, b, _] = theme.hover;
                mesh.set_clip(Some(clip(Rect::new(
                    g.text.x - ctx.metrics.pad,
                    g.text.y,
                    g.text.w + ctx.metrics.pad,
                    g.text.h,
                ))));
                mesh.rect(
                    Rect::new(g.gutter.x + g.gutter.w, y, area.w, g.row_h),
                    [r, gr, b, 0.45],
                );
            }
            if self.line_numbers {
                mesh.set_clip(Some(clip(Rect::new(
                    g.gutter.x, g.text.y, g.gutter.w, g.text.h,
                ))));
                let label = (n + 1).to_string();
                let w = font::width_in(Family::Mono, &label, g.size);
                font::text_in(
                    mesh,
                    Family::Mono,
                    &label,
                    g.gutter.x + g.gutter.w - ctx.metrics.pad - w,
                    ty,
                    g.size,
                    if n == caret_line {
                        theme.text
                    } else {
                        theme.text_dim
                    },
                );
            }
            mesh.set_clip(Some(clip(g.text)));
            let left = g.text.x - sx;
            if let Some((a, b)) = sel {
                let end = start + line.len();
                if a <= end && b >= start {
                    let from = Self::x_of(line, a.max(start) - start, &g);
                    let to = if b > end {
                        Self::x_of(line, line.len(), &g) + g.cell * 0.5
                    } else {
                        Self::x_of(line, b - start, &g)
                    };
                    if to > from {
                        mesh.rect(Rect::new(left + from, y, to - from, g.row_h), sel_color);
                    }
                }
            }
            // the text, a piece between tabs at a time
            let mut x = 0.0;
            let mut from = 0;
            let pieces = line
                .match_indices('\t')
                .map(|(i, _)| i)
                .chain(std::iter::once(line.len()));
            for end in pieces {
                let piece = &line[from..end];
                if !piece.is_empty() {
                    font::text_in(mesh, Family::Mono, piece, left + x, ty, g.size, theme.text);
                    x += font::width_in(Family::Mono, piece, g.size);
                }
                x = next_stop(x, g.cell);
                from = end + 1;
            }
            if n == caret_line && ctx.focused {
                let cx = left + Self::x_of(line, caret.pos - start, &g);
                let w = (1.5 * ctx.scale).round().max(1.0);
                mesh.rect(Rect::new(cx.round(), y, w, g.row_h), theme.text);
            }
        }
        // the scroll bars, where there is more than is seen
        mesh.set_clip(Some(clip(area)));
        let total_h = self.code.lines() as f32 * g.row_h + g.text.h - g.row_h;
        if total_h > g.text.h + 0.5 {
            let h = (g.text.h * g.text.h / total_h).max(g.row_h);
            let y = g.text.y + (g.text.h - h) * (sy / (total_h - g.text.h)).min(1.0);
            mesh.rect(
                Rect::new(area.x + area.w - g.bar, y, g.bar, h),
                theme.separator,
            );
        }
        let total_w = self.widest() as f32 * g.cell + 2.0 * g.cell;
        if total_w > g.text.w + 0.5 {
            let w = (g.text.w * g.text.w / total_w).max(g.cell);
            let x = g.text.x + (g.text.w - w) * (sx / (total_w - g.text.w)).min(1.0);
            mesh.rect(
                Rect::new(x, area.y + area.h - g.bar, w, g.bar),
                theme.separator,
            );
        }
        mesh.set_clip(kept);
    }
}

/// The next tab stop after `x`, `cell` wide columns.
fn next_stop(x: f32, cell: f32) -> f32 {
    let col = (x / cell + 0.01).floor() as usize;
    ((col / TAB_COLS + 1) * TAB_COLS) as f32 * cell
}

/// How many cells `line` takes: a grapheme each, a tab to its stop.
fn cells(line: &str) -> usize {
    let mut n = 0;
    for g in line.graphemes(true) {
        n = if g == "\t" {
            (n / TAB_COLS + 1) * TAB_COLS
        } else {
            n + 1
        };
    }
    n
}

impl Element for CodeView {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "text" => v
                .as_str()
                .map(|s| {
                    self.code.set_text(s);
                    self.was_dirty = false;
                    self.widest.set(None);
                })
                .is_some(),
            "saved" => {
                self.code.mark_saved();
                self.was_dirty = false;
                true
            }
            "language" => v
                .as_str()
                .map(|s| self.code.language = Language::named(s))
                .is_some(),
            "indent" => Indent::parse(v).map(|i| self.code.indent = i).is_some(),
            "line_numbers" => parse::truthy(v).map(|b| self.line_numbers = b).is_some(),
            "text_size" => v
                .as_f64()
                .filter(|n| *n > 0.0)
                .map(|n| self.text_size = n as f32)
                .is_some(),
            "id" | "type" | "name" => true,
            _ => false,
        }
    }

    fn writes(&self, key: &str) -> bool {
        key == "saved"
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        self.paint(d, ctx);
    }

    fn natural(&self, _m: &Metrics, _scale: f32) -> Natural {
        Natural::default()
    }

    fn value(&self) -> Option<OscType> {
        None
    }

    fn info(&self) -> Vec<(String, Value)> {
        let (line, col) = self.code.line_col();
        vec![
            ("text".into(), Value::from(self.code.text())),
            ("dirty".into(), Value::from(self.code.dirty())),
            ("line".into(), Value::from(line + 1)),
            ("column".into(), Value::from(col + 1)),
            ("lines".into(), Value::from(self.code.lines())),
            (
                "selected".into(),
                Value::from(self.code.selected().unwrap_or_default()),
            ),
        ]
    }

    fn accepts_focus(&self) -> bool {
        true
    }

    fn takes_text(&self) -> bool {
        true
    }

    fn takes_tab(&self) -> bool {
        true
    }

    /// One click places the caret, two select a word, three a line; Shift
    /// extends; a press on a line's number selects the line.
    fn press(&mut self, at: (f64, f64), input: &Input) -> Claim {
        let g = self.geometry(input.rect, input.scale, input.metrics);
        let pos = self.pos_at(at, &g);
        if self.line_numbers && g.gutter.contains(at.0, at.1) {
            let line = self.code.line_of(pos);
            let (start, text) = self.code.line(line).unwrap_or((0, ""));
            let end = (start + text.len() + 1).min(self.code.text().len());
            self.code.set_caret(Caret {
                pos: end,
                anchor: Some(start),
            });
            self.held = Some(((start, end), Unit::Line));
            return Claim::take();
        }
        if input.mods.shift && input.clicks == 1 {
            let from = self.code.caret().anchor.unwrap_or(self.code.caret().pos);
            self.code.extend((from, from), pos, Unit::Char);
            self.held = Some(((from, from), Unit::Char));
            return Claim::take();
        }
        let unit = Unit::of_clicks(input.clicks);
        let span = self.code.unit_at(pos, unit);
        self.code.extend(span, pos, Unit::Char);
        if unit == Unit::Char {
            self.code.place(pos, false);
        }
        self.held = Some((span, unit));
        Claim::take()
    }

    fn drag(&mut self, at: (f64, f64), input: &Input) -> Events {
        let Some((span, unit)) = self.held else {
            return Events::none();
        };
        let g = self.geometry(input.rect, input.scale, input.metrics);
        let (sx, sy) = self.scroll.get();
        // past an edge, the text comes to the pointer
        let dy = if (at.1 as f32) < g.text.y {
            -g.row_h
        } else if (at.1 as f32) > g.text.y + g.text.h {
            g.row_h
        } else {
            0.0
        };
        self.scroll.set((sx, sy + dy));
        let pos = self.pos_at(at, &g);
        self.code.extend(span, pos, unit);
        Events::none()
    }

    fn release(&mut self, _at: (f64, f64), _inside: bool, _input: &Input) -> Events {
        self.held = None;
        Events::none()
    }

    fn wheel(&mut self, _at: (f64, f64), delta: (f64, f64), input: &Input) -> Option<Events> {
        let g = self.geometry(input.rect, input.scale, input.metrics);
        let (sx, sy) = self.scroll.get();
        let (dx, dy) = if input.mods.shift && delta.0 == 0.0 {
            (delta.1, 0.0)
        } else {
            delta
        };
        self.scroll.set((
            sx - dx as f32 * WHEEL_ROWS * g.cell,
            sy - dy as f32 * WHEEL_ROWS * g.row_h,
        ));
        let before = (sx, sy);
        (self.settle(&g) != before).then(Events::none)
    }

    fn key(&mut self, key: &Key, input: &mut KeyInput) -> Option<Events> {
        let mods = input.mods;
        let shift = mods.shift;
        Some(match key {
            Key::Char(c) if mods.ctrl => match c.to_ascii_lowercase() {
                'z' if shift => self.undo(true),
                'z' => self.undo(false),
                'y' => self.undo(true),
                'c' => {
                    self.copy(input);
                    Events::none()
                }
                'x' => self.cut(input),
                'v' => self.paste(input),
                'a' => {
                    self.code.select_all();
                    Events::none()
                }
                _ => return None,
            },
            Key::Char(_) if mods.alt => return None,
            Key::Char(c) => {
                let mut buf = [0u8; 4];
                if self.code.insert(c.encode_utf8(&mut buf)) {
                    self.changed()
                } else {
                    Events::none()
                }
            }
            Key::Enter => {
                self.code.newline();
                self.changed()
            }
            Key::Tab => {
                if shift {
                    self.code.indent_less();
                } else {
                    self.code.indent_more();
                }
                self.changed()
            }
            Key::Backspace => {
                self.code.backspace(mods.ctrl);
                self.changed()
            }
            Key::Delete => {
                self.code.delete(mods.ctrl);
                self.changed()
            }
            Key::Left if mods.ctrl => {
                self.code
                    .motion(|t, c| textedit::move_word_left(t, c, shift));
                self.moved()
            }
            Key::Right if mods.ctrl => {
                self.code
                    .motion(|t, c| textedit::move_word_right(t, c, shift));
                self.moved()
            }
            Key::Left => {
                self.code.motion(|t, c| textedit::move_left(t, c, shift));
                self.moved()
            }
            Key::Right => {
                self.code.motion(|t, c| textedit::move_right(t, c, shift));
                self.moved()
            }
            Key::Up => {
                self.code.rows(-1, shift);
                self.moved()
            }
            Key::Down => {
                self.code.rows(1, shift);
                self.moved()
            }
            Key::PageUp => {
                self.code.rows(-(self.page.get() as isize), shift);
                self.moved()
            }
            Key::PageDown => {
                self.code.rows(self.page.get() as isize, shift);
                self.moved()
            }
            Key::Home if mods.ctrl => {
                self.code.place(0, shift);
                self.moved()
            }
            Key::End if mods.ctrl => {
                let end = self.code.text().len();
                self.code.place(end, shift);
                self.moved()
            }
            Key::Home => {
                self.code.home(shift);
                self.moved()
            }
            Key::End => {
                self.code.motion(|t, c| textedit::move_end(t, c, shift));
                self.moved()
            }
            Key::Escape | Key::F(_) => return None,
        })
    }

    fn selected_text(&self) -> Option<String> {
        self.code.selected().map(str::to_string)
    }

    fn holds_selection(&self) -> Option<bool> {
        Some(self.code.selected().is_some())
    }

    /// The middle button: the caret where the pointer is, the text put there.
    fn paste_at(&mut self, at: (f64, f64), input: &Input, text: &str) -> Option<Events> {
        let g = self.geometry(input.rect, input.scale, input.metrics);
        let pos = self.pos_at(at, &g);
        self.code.place(pos, false);
        Some(if self.code.paste(text) {
            self.changed()
        } else {
            Events::none()
        })
    }

    /// The code's own editing: undo and redo walk its history, a cut, a
    /// copy and a delete need selected text, a paste something to paste.
    fn action_state(
        &self,
        verb: &ActionVerb,
        target: ActionTarget,
        clipboard: &crate::host::clipboard::Clip,
    ) -> Option<ActionState> {
        if !verbs::text_action(verb.name) {
            return None;
        }
        let mut state = ActionState::new(verb, target);
        match verb.name {
            "undo" if !self.code.can_undo() => state.refuse("there is nothing to undo"),
            "redo" if !self.code.can_redo() => state.refuse("there is nothing to redo"),
            "cut" | "copy" | "delete" if self.code.selected().is_none() => {
                state.refuse("no text is selected");
            }
            "paste" if clipboard.is_empty() => state.refuse("the clipboard is empty"),
            _ => {}
        }
        Some(state)
    }

    fn verb(&mut self, verb: Verb, input: &mut KeyInput) -> Option<Events> {
        Some(match verb {
            Verb::Copy => {
                self.copy(input);
                Events::none()
            }
            Verb::Cut => self.cut(input),
            Verb::Paste => self.paste(input),
            Verb::SelectAll => {
                self.code.select_all();
                Events::none()
            }
            Verb::Delete => {
                self.code.delete(false);
                self.changed()
            }
            _ => return None,
        })
    }

    fn command(&mut self, name: &str, _input: &mut KeyInput) -> Option<Events> {
        match name {
            "undo" => Some(self.undo(false)),
            "redo" => Some(self.undo(true)),
            _ => None,
        }
    }

    fn context_menu(&mut self, _at: (f64, f64), _input: &Input) -> Option<Vec<Entry>> {
        let on = |e: Entry, ok: bool| if ok { e } else { e.disabled() };
        let selected = self.code.selected().is_some();
        Some(vec![
            on(Entry::action("Undo", "undo"), self.code.can_undo()),
            on(Entry::action("Redo", "redo"), self.code.can_redo()),
            Entry::separator(),
            on(Entry::action("Cut", "cut"), selected),
            on(Entry::action("Copy", "copy"), selected),
            Entry::action("Paste", "paste"),
            Entry::separator(),
            Entry::action("Select all", "select_all"),
        ])
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests;
