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
//! **In the colors of its syntax**: the model says what each part of the
//! rows in view is ([`Code::spans`]), and each kind is drawn in its theme
//! role (`syntax_keyword`, ...); what nothing names is `text`. A text set
//! whole is parsed whole; an edit is parsed again for [`PARSE_SLICE`] at
//! most, and a parse that needs longer asks for the frame tick and goes on
//! in the element's [`tick`](Element::tick), [`TICK_SLICE`] a tick, the
//! text keeping its colors meanwhile.
//!
//! **Tab is the element's** ([`Element::takes_tab`]): it indents, and
//! Ctrl+Tab leaves for the next element.
//!
//! **What the current editor marks, it marks**: the matches of the find
//! bar's query, the one selected among them, the other places the selected
//! text stands, and (while the element has the keys) the bracket by the
//! caret with the one it pairs with, or alone when nothing closes it. The
//! bar ([`bar`]) is the element's own: Ctrl+F opens it to find and replace,
//! Ctrl+Alt+G to go to a line, Escape closes it; F3 and Ctrl+G find the
//! next match and with Shift the previous, the bar open or not, and Ctrl+/
//! puts a line comment on the lines selected or takes it off.
//!
//! **It evaluates by telling its owner**: Shift+Enter reports the line or
//! the selection, Ctrl+Enter the block (the cell, the top-level statement,
//! or with `paragraph` the statements no blank line separates) and moves
//! the caret past it, as `"evaluate" code line`; Ctrl+. reports
//! `"interrupt"`. What was evaluated flashes for [`FLASH`]. The element
//! runs nothing: the session is whoever owns the window
//! ([`crate::host::graphics::code::regions`]). A cell's marker has a rule
//! above it.

use std::cell::Cell;

mod bar;
mod help;

use bar::{Bar, Field, Mode, Part};
use help::{Completing, Help, call_head};

use clausters_core::osc::OscType;
use clausters_editing::verbs::{self, ActionState, ActionTarget, Verb as ActionVerb};
use serde_json::{Map, Value};
use unicode_segmentation::UnicodeSegmentation;

use crate::host::graphics::code::find::is_word;
use crate::host::graphics::code::regions::{Block, is_cell_marker};
use crate::host::graphics::code::syntax::Kind;
use crate::host::graphics::code::{Code, Indent, Language};
use crate::host::graphics::textedit::{self, Caret, Unit};
use crate::host::menu::Entry;
use crate::host::popup::{Note, Own, OwnList};
use crate::host::widget::element::{
    Claim, Ctx, Element, Events, Input, Key, KeyInput, Live, Mods, Needs, Verb,
};
use crate::host::widget::parse;
use crate::host::widget::size::Natural;
use clausters_gui_draw::draw::Draw;
use clausters_gui_draw::font::{self, Family};
use clausters_gui_draw::metrics::Metrics;
use clausters_gui_draw::rect::Rect;
use clausters_gui_draw::theme::{Color, Theme, with_alpha};

/// How long what was evaluated stays marked.
const FLASH: std::time::Duration = std::time::Duration::from_millis(250);

/// The most rows a list of completions shows.
const LIST_ROWS: usize = 10;

/// How many rows a step of the wheel scrolls.
const WHEEL_ROWS: f32 = 3.0;

/// The columns a tab reaches to: the next multiple of these.
const TAB_COLS: usize = 4;

/// How long a parse may take in a key's handling: what is left of a key's
/// budget once it is handled and its frame drawn.
const PARSE_SLICE: std::time::Duration = std::time::Duration::from_micros(1500);

/// How long a parse that is behind may take in a tick, which draws a frame
/// but handles no key. In a page a parse is two or three times slower and
/// the tick a third as frequent, so a slice of a key's would leave the
/// colors behind for a second after a long file is edited.
const TICK_SLICE: std::time::Duration = std::time::Duration::from_micros(2500);

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
    /// Whether it is to be brought to the middle of the view, rather than
    /// only into it: a line gone to.
    center: Cell<bool>,
    /// The rows the text area showed at the last frame: a page.
    page: Cell<usize>,
    /// The widest line, in cells, while the text has not changed.
    widest: Cell<Option<usize>>,
    /// What a press selected and by what unit, while a drag grows it.
    held: Option<((usize, usize), Unit)>,
    /// Whether the text was dirty when last reported.
    was_dirty: bool,
    /// Find and replace, and going to a line.
    bar: Bar,
    /// What a block is when nothing is selected and there are no cells.
    block: Block,
    /// The lines last evaluated, while they flash, and since when.
    flash: Option<(usize, usize, web_time::Instant)>,
    /// Completions, the call's parameters and what a name is.
    help: Help,
}

/// Where the parts of the element are, at the scale it is drawn at.
#[derive(Clone, Copy, Debug)]
struct Geometry {
    gutter: Rect,
    text: Rect,
    row_h: f32,
    cell: f32,
    size: f32,
    /// How thick a scroll bar is.
    thumb: f32,
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
            center: Cell::new(false),
            page: Cell::new(20),
            widest: Cell::new(None),
            held: None,
            was_dirty: false,
            bar: Bar::default(),
            block: Block::default(),
            flash: None,
            help: Help::default(),
        };
        for (key, value) in props {
            view.set(key, value);
        }
        view
    }

    /// Where the text and its gutter are: the element's rect, above the
    /// bar while it is open.
    fn geometry(&self, rect: Rect, scale: f32, m: &Metrics) -> Geometry {
        let rect = match self.bar.layout(rect, scale, m) {
            Some(l) => Rect::new(rect.x, rect.y, rect.w, (l.strip.y - rect.y).max(1.0)),
            None => rect,
        };
        let size = self.text_size * scale;
        let cell = font::cell_in(Family::Mono, size).max(1.0);
        let digits = self.code.lines().to_string().len().max(3) as f32;
        let gutter_w = if self.line_numbers {
            (digits * cell + 2.0 * m.pad).round()
        } else {
            0.0
        };
        let thumb = m.scrollbar_w;
        Geometry {
            gutter: Rect::new(rect.x, rect.y, gutter_w, rect.h),
            text: Rect::new(
                rect.x + gutter_w + m.pad,
                rect.y + m.pad * 0.5,
                (rect.w - gutter_w - m.pad - thumb).max(1.0),
                (rect.h - m.pad * 0.5 - thumb).max(1.0),
            ),
            row_h: font::code_line(size).max(1.0),
            cell,
            size,
            thumb,
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
            if self.center.replace(false) {
                sy = top - ((g.text.h - g.row_h) * 0.5).max(0.0);
            } else if top < sy {
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
        self.code.parse(Some(PARSE_SLICE));
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

    // -- evaluation ----------------------------------------------------------

    /// **`region` told to the owner to evaluate**, as `"evaluate" code
    /// line`, the line from 1; its lines flash.
    fn evaluate(&mut self, region: std::ops::Range<usize>) -> Events {
        let code = self.code.text()[region.clone()].to_string();
        let first = self.code.line_of(region.start);
        let last = self.code.line_of(region.end);
        self.flash = Some((first, last, web_time::Instant::now()));
        Events::message(vec![
            OscType::String("evaluate".into()),
            OscType::String(code),
            OscType::Int(first as i32 + 1),
        ])
    }

    /// Shift+Enter: the selection or the caret's line.
    fn evaluate_line(&mut self) -> Events {
        match self.code.line_or_selection() {
            Some(region) => self.evaluate(region),
            None => Events::none(),
        }
    }

    /// Ctrl+Enter: the block, and the caret past it.
    fn evaluate_block(&mut self) -> Events {
        let Some((region, cell)) = self.code.block(self.block) else {
            return Events::none();
        };
        let events = self.evaluate(region.clone());
        self.code.advance(&region, cell);
        self.reveal.set(true);
        events
    }

    // -- language help -------------------------------------------------------

    /// **Completions asked of the owner** at the caret, as `"complete"
    /// source line column explicit`; nothing in a string or a comment.
    fn ask_complete(&mut self, explicit: bool) -> Events {
        let pos = self.code.caret().pos;
        if self.code.caret().selection().is_some() || self.code.in_text(pos) {
            self.help.completing = None;
            return Events::none();
        }
        self.help.completing = Some(Completing::asked_at(pos));
        let (line, column) = self.code.position(pos);
        Events::message(vec![
            OscType::String("complete".into()),
            OscType::String(self.code.text().to_string()),
            OscType::Int(line as i32),
            OscType::Int(column as i32),
            OscType::Int(i32::from(explicit)),
        ])
    }

    /// **The call the caret is in asked of the owner**, as `"signatures"
    /// source line column`; with a selection, or in a string, the call is
    /// no longer followed.
    fn ask_signature(&mut self) -> Events {
        let pos = self.code.caret().pos;
        if self.code.caret().selection().is_some() || self.code.in_text(pos) {
            self.help.calling = false;
            self.help.signature = None;
            return Events::none();
        }
        let (line, column) = self.code.position(pos);
        Events::message(vec![
            OscType::String("signatures".into()),
            OscType::String(self.code.text().to_string()),
            OscType::Int(line as i32),
            OscType::Int(column as i32),
        ])
    }

    /// **Help asked for** (Ctrl+D): on the text selected, or on the name at
    /// the caret, as `"help" text source line column`, the place the end of
    /// the name.
    fn ask_help(&mut self) -> Events {
        let (subject, at) = match self.code.caret().selection() {
            Some((a, b)) => (self.code.text()[a..b].trim().to_string(), b),
            None => match self.code.word_at(self.code.caret().pos) {
                Some((a, b)) => (self.code.text()[a..b].to_string(), b),
                None => return Events::none(),
            },
        };
        if subject.is_empty() {
            return Events::none();
        }
        let (line, column) = self.code.position(at);
        Events::message(vec![
            OscType::String("help".into()),
            OscType::String(subject),
            OscType::String(self.code.text().to_string()),
            OscType::Int(line as i32),
            OscType::Int(column as i32),
        ])
    }

    /// The completion the keys are on, put in place of the word typed.
    fn accept(&mut self) -> Events {
        let Some(c) = self.help.completing.take() else {
            return Events::none();
        };
        let (Some(from), Some(offer)) = (c.from, c.chosen().cloned()) else {
            return Events::none();
        };
        let pos = self.code.caret().pos;
        if self.code.replace_range(from, pos, &offer.name) {
            self.changed()
        } else {
            Events::none()
        }
    }

    /// **The keys the help takes before the editing**: Ctrl+Space asks for
    /// completions, Ctrl+Shift+Space for the call, Ctrl+D for help; a list
    /// shown is walked by Up, Down and the pages, taken by Enter or Tab and
    /// closed by Escape; Escape takes a call's note down.
    fn help_key(&mut self, key: &Key, input: &KeyInput) -> Option<Events> {
        if self.bar.focus().is_some() {
            return None;
        }
        let mods = input.mods;
        match key {
            Key::Char(' ') if mods.ctrl && mods.shift => {
                self.help.calling = true;
                return Some(self.ask_signature());
            }
            Key::Char(' ') if mods.ctrl => return Some(self.ask_complete(true)),
            Key::Char(c) if mods.ctrl && !mods.alt && c.eq_ignore_ascii_case(&'d') => {
                return Some(self.ask_help());
            }
            _ => {}
        }
        if !mods.ctrl
            && !mods.alt
            && let Some(c) = self.help.completing.as_mut().filter(|c| c.showing())
        {
            let page = LIST_ROWS as isize;
            match key {
                Key::Up => c.walk(-1),
                Key::Down => c.walk(1),
                Key::PageUp => c.walk(-page),
                Key::PageDown => c.walk(page),
                Key::Enter | Key::Tab if !mods.shift => return Some(self.accept()),
                Key::Escape => self.help.completing = None,
                _ => return None,
            }
            return Some(Events::none());
        }
        if *key == Key::Escape && self.help.calling {
            self.help.calling = false;
            self.help.signature = None;
            return Some(Events::none());
        }
        None
    }

    /// **What the help makes of a key the editing took**: a name begun or
    /// a dot asks for completions, a name going on filters them, anything
    /// else lets them go; a `(` or a `,` starts following the call, and
    /// while it is followed every change asks for it again.
    fn after_key(&mut self, key: &Key, mods: Mods, before: (u64, Caret)) -> Events {
        if self.bar.focus().is_some() {
            return Events::none();
        }
        let named = |c: char| is_word(c) || c == '$';
        let pos = self.code.caret().pos;
        let mut out = Events::none();
        match key {
            Key::Char(ch) if !mods.ctrl && !mods.alt => {
                if named(*ch) {
                    match self.help.completing.as_mut() {
                        Some(c) if c.from.is_some() => {
                            c.filter(&self.code);
                            if c.shown.is_empty() {
                                self.help.completing = None;
                            }
                        }
                        Some(_) => {}
                        None => {
                            let prev = self.code.text()[..pos].chars().rev().nth(1);
                            if !prev.is_some_and(named) {
                                out = self.ask_complete(false);
                            }
                        }
                    }
                } else if *ch == '.' {
                    out = self.ask_complete(false);
                } else {
                    self.help.completing = None;
                }
                if *ch == '(' || *ch == ',' {
                    self.help.calling = true;
                }
            }
            Key::Backspace => {
                if let Some(c) = self.help.completing.as_mut() {
                    match c.from {
                        Some(from) if pos >= from => {
                            c.filter(&self.code);
                            if c.shown.is_empty() {
                                self.help.completing = None;
                            }
                        }
                        Some(_) => self.help.completing = None,
                        None => {}
                    }
                }
            }
            _ => self.help.completing = None,
        }
        if self.help.calling && (self.code.revision(), self.code.caret()) != before {
            out = out.chain(self.ask_signature());
        }
        out
    }

    /// Where byte `at` is drawn in `rect`, a cell wide and a row tall; none
    /// when its row is out of view.
    fn spot(&self, at: usize, g: &Geometry) -> Option<Rect> {
        let (sx, sy) = self.scroll.get();
        let line = self.code.line_of(at);
        let (start, text) = self.code.line(line)?;
        let y = g.text.y + line as f32 * g.row_h - sy;
        if y + g.row_h < g.text.y || y > g.text.y + g.text.h {
            return None;
        }
        let x = g.text.x - sx + Self::x_of(text, at - start, g);
        Some(Rect::new(
            x.clamp(g.text.x, g.text.x + g.text.w),
            y,
            g.cell,
            g.row_h,
        ))
    }

    /// **Where the name from byte `a` to byte `b` is drawn**: what a hover
    /// card explains, and so where the pointer keeps it up.
    fn word_spot(&self, a: usize, b: usize, g: &Geometry) -> Option<Rect> {
        let first = self.spot(a, g)?;
        let x1 = self.spot(b, g).map_or(first.x + first.w, |r| r.x);
        Some(Rect::new(
            first.x,
            first.y,
            (x1 - first.x).max(first.w),
            first.h,
        ))
    }

    /// A key's editing: the bar's, the text's.
    fn edit_key(&mut self, key: &Key, input: &mut KeyInput) -> Option<Events> {
        if let Some(events) = self.bar_key(key, input) {
            return Some(events);
        }
        // a field of the bar has the keys
        if self.bar.focus().is_some() {
            return match self.bar.key(key, input)? {
                Ok(()) => Some(Events::none()),
                Err(Field::Find) => Some(self.find(!input.mods.shift)),
                Err(Field::Replace) => Some(self.replace()),
                Err(Field::Line) => Some(self.go()),
            };
        }
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
                '.' if !mods.alt => Events::message(vec![OscType::String("interrupt".into())]),
                '/' if !mods.alt => {
                    if self.code.toggle_comment() {
                        self.changed()
                    } else {
                        Events::none()
                    }
                }
                _ => return None,
            },
            Key::Char(_) if mods.alt => return None,
            Key::Char(c) => {
                let before = self.code.revision();
                if !self.code.type_char(*c) {
                    Events::none()
                } else if self.code.revision() == before {
                    // a closer typed over: the caret moved, nothing changed
                    self.moved()
                } else {
                    self.changed()
                }
            }
            Key::Enter if mods.ctrl => self.evaluate_block(),
            Key::Enter if shift => self.evaluate_line(),
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

    // -- the bar -------------------------------------------------------------

    /// **Find**: the bar opened, the text selected as its query.
    fn open_find(&mut self) -> Events {
        let seed = self.code.selected().map(str::to_string);
        self.bar.open_find(seed.as_deref());
        Events::none()
    }

    /// **The next match, or the previous**, the bar open or not: with no
    /// query yet the bar opens for one.
    fn find(&mut self, forward: bool) -> Events {
        if !self.bar.finder().ready() {
            return self.open_find();
        }
        if self.code.find(self.bar.finder(), forward) {
            self.reveal.set(true);
        }
        Events::none()
    }

    /// The match selected replaced, and the next one selected.
    fn replace(&mut self) -> Events {
        let (finder, with) = (
            self.bar.finder().clone(),
            self.bar.replacement().to_string(),
        );
        let before = self.code.revision();
        if !self.code.replace(&finder, &with) {
            return Events::none();
        }
        if self.code.revision() == before {
            return self.moved();
        }
        self.changed()
    }

    /// Every match replaced, as one step.
    fn replace_all(&mut self) -> Events {
        let (finder, with) = (
            self.bar.finder().clone(),
            self.bar.replacement().to_string(),
        );
        if self.code.replace_all(&finder, &with) > 0 {
            self.changed()
        } else {
            Events::none()
        }
    }

    /// **The line the bar names gone to**, in the middle of the view, the
    /// bar closed and the keys back to the text.
    fn go(&mut self) -> Events {
        let spec = self.bar.line_spec().to_string();
        if self.code.go_to(&spec) {
            self.bar.close();
            self.center.set(true);
            return self.moved();
        }
        Events::none()
    }

    /// What a press on a part of the bar does.
    fn bar_part(&mut self, part: Part) -> Events {
        match part {
            Part::Field(_) => Events::none(),
            Part::Next => self.find(true),
            Part::Previous => self.find(false),
            Part::Case | Part::Regexp | Part::Word => {
                self.bar.switch(part);
                Events::none()
            }
            Part::Replace => self.replace(),
            Part::All => self.replace_all(),
            Part::Go => self.go(),
            Part::Close => {
                self.bar.close();
                Events::none()
            }
        }
    }

    /// **The keys that hold wherever the keys go**, the text or the bar:
    /// opening the bar, the next and the previous match, closing it.
    fn bar_key(&mut self, key: &Key, input: &KeyInput) -> Option<Events> {
        let mods = input.mods;
        Some(match key {
            Key::Char(c) if mods.ctrl && mods.alt && c.eq_ignore_ascii_case(&'g') => {
                self.bar.open_line();
                Events::none()
            }
            Key::Char(c) if mods.ctrl && !mods.alt && c.eq_ignore_ascii_case(&'f') => {
                self.open_find()
            }
            Key::Char(c) if mods.ctrl && !mods.alt && c.eq_ignore_ascii_case(&'g') => {
                self.find(!mods.shift)
            }
            Key::F(3) => self.find(!mods.shift),
            Key::Escape if self.bar.mode().is_some() => {
                self.bar.close();
                Events::none()
            }
            _ => return None,
        })
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
        let shown = first..(first + rows).min(self.code.lines());
        let spans = self.code.spans(shown.clone());
        let mut next_span = 0;
        // the keys are the text's, not the bar's
        let keys_here = ctx.focused && self.bar.focus().is_none();
        let sel_color = with_alpha(theme.selection, if keys_here { 0.35 } else { 0.2 });
        let marks = self.marks(theme, shown.clone(), keys_here);
        for n in shown {
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
            // a range of the text on this row: from where to where it is
            // drawn, past the line's end when it runs on into the next
            let across = |a: usize, b: usize| -> Option<(f32, f32)> {
                let end = start + line.len();
                if a > end || b < start {
                    return None;
                }
                let from = Self::x_of(line, a.max(start) - start, &g);
                let to = if b > end {
                    Self::x_of(line, line.len(), &g) + g.cell * 0.5
                } else {
                    Self::x_of(line, b - start, &g)
                };
                (to > from).then_some((from, to))
            };
            // what was just evaluated, across the text
            if self.flash.is_some_and(|(a, b, _)| (a..=b).contains(&n)) {
                mesh.rect(
                    Rect::new(
                        g.text.x - ctx.metrics.pad,
                        y,
                        g.text.w + ctx.metrics.pad,
                        g.row_h,
                    ),
                    with_alpha(theme.selection, 0.25),
                );
            }
            // a rule above a cell's marker
            if is_cell_marker(line) {
                mesh.rect(
                    Rect::new(
                        g.text.x - ctx.metrics.pad,
                        y,
                        g.text.w + ctx.metrics.pad,
                        ctx.metrics.divider_w,
                    ),
                    theme.separator,
                );
            }
            if let Some((from, to)) = sel.and_then(|(a, b)| across(a, b)) {
                mesh.rect(Rect::new(left + from, y, to - from, g.row_h), sel_color);
            }
            for &(a, b, color) in &marks {
                if let Some((from, to)) = across(a, b) {
                    mesh.rect(Rect::new(left + from, y, to - from, g.row_h), color);
                }
            }
            // the text, a run of one color at a time, a tab reaching to its
            // stop
            let mut x = 0.0;
            let mut at = 0;
            while at < line.len() {
                if line.as_bytes()[at] == b'\t' {
                    x = next_stop(x, g.cell);
                    at += 1;
                    continue;
                }
                let abs = start + at;
                while spans.get(next_span).is_some_and(|s| s.end <= abs) {
                    next_span += 1;
                }
                let (color, edge) = match spans.get(next_span) {
                    Some(s) if s.start <= abs => (role(theme, s.kind), s.end - start),
                    Some(s) => (theme.text, s.start - start),
                    None => (theme.text, line.len()),
                };
                // a tree behind the text could name a place inside a letter
                let mut edge = edge.min(line.len());
                while !line.is_char_boundary(edge) {
                    edge += 1;
                }
                let end = line[at..edge].find('\t').map_or(edge, |i| at + i);
                let piece = &line[at..end];
                font::text_in(mesh, Family::Mono, piece, left + x, ty, g.size, color);
                x += font::width_in(Family::Mono, piece, g.size);
                at = end;
            }
            if n == caret_line && keys_here {
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
            crate::host::graphics::controls::scroll_thumb(
                mesh,
                Rect::new(area.x + area.w - g.thumb, y, g.thumb, h),
                ctx.metrics,
                theme,
            );
        }
        let total_w = self.widest() as f32 * g.cell + 2.0 * g.cell;
        if total_w > g.text.w + 0.5 {
            let w = (g.text.w * g.text.w / total_w).max(g.cell);
            let x = g.text.x + (g.text.w - w) * (sx / (total_w - g.text.w)).min(1.0);
            crate::host::graphics::controls::scroll_thumb(
                mesh,
                Rect::new(x, g.text.y + g.text.h, w, g.thumb),
                ctx.metrics,
                theme,
            );
        }
        mesh.set_clip(kept);
        if let Some(layout) = self.bar.layout(area, ctx.scale, ctx.metrics) {
            let note = self.bar.note(
                self.code.text(),
                self.code.revision(),
                self.code.caret().selection(),
            );
            d.mesh.set_clip(Some(clip(area)));
            self.bar.draw(d, &layout, ctx.focused, note);
            d.mesh.set_clip(kept);
        }
    }

    /// **What is marked on rows `shown`**, in the order it is painted: the
    /// other places the selected text stands; the matches of the bar's
    /// query, the one selected among them apart; and, while the keys are
    /// the text's, the bracket by the caret and its pair, or it alone
    /// when nothing closes it.
    fn marks(
        &self,
        theme: &Theme,
        shown: std::ops::Range<usize>,
        keys_here: bool,
    ) -> Vec<(usize, usize, Color)> {
        let mut marks: Vec<(usize, usize, Color)> = self
            .code
            .occurrences(shown.clone())
            .into_iter()
            .map(|(a, b)| (a, b, with_alpha(theme.selection, 0.2)))
            .collect();
        if self.bar.mode() == Some(Mode::Find) && self.bar.finder().ready() {
            let from = self.code.line(shown.start).map_or(0, |l| l.0);
            let to = shown
                .end
                .checked_sub(1)
                .and_then(|n| self.code.line(n))
                .map_or(from, |(s, l)| s + l.len());
            let sel = self.code.caret().selection();
            self.bar
                .with_matches(self.code.text(), self.code.revision(), |all| {
                    let i = all.partition_point(|m| m.1 <= from);
                    for &(a, b) in all[i..].iter().take_while(|m| m.0 <= to) {
                        let color = if sel == Some((a, b)) {
                            theme.find_current
                        } else {
                            theme.find_match
                        };
                        marks.push((a, b, color));
                    }
                });
        }
        if keys_here {
            match self.code.bracket_pair() {
                Some((a, Some(b))) => {
                    marks.push((a, a + 1, theme.bracket_match));
                    marks.push((b, b + 1, theme.bracket_match));
                }
                Some((a, None)) => marks.push((a, a + 1, theme.bracket_mismatch)),
                None => {}
            }
        }
        marks
    }
}

/// The color a kind of code is drawn in: its theme role.
pub(crate) fn role(theme: &Theme, kind: Kind) -> Color {
    match kind {
        Kind::Keyword => theme.syntax_keyword,
        Kind::Control => theme.syntax_control,
        Kind::Name => theme.syntax_name,
        Kind::Function => theme.syntax_function,
        Kind::Type => theme.syntax_type,
        Kind::Number => theme.syntax_number,
        Kind::String => theme.syntax_string,
        Kind::Regexp => theme.syntax_regexp,
        Kind::Comment => theme.syntax_comment,
        Kind::Operator => theme.syntax_operator,
    }
}

/// The next tab stop after `x`, `cell` wide columns.
fn next_stop(x: f32, cell: f32) -> f32 {
    let col = (x / cell + 0.01).floor() as usize;
    ((col / TAB_COLS + 1) * TAB_COLS) as f32 * cell
}

/// How many cells `line` takes: a grapheme each, a tab to its stop. Every
/// line is asked after every edit, so a line of ASCII, most of any code,
/// is counted by its bytes.
fn cells(line: &str) -> usize {
    if line.is_ascii() {
        if !line.contains('\t') {
            return line.len();
        }
        return line.bytes().fold(0, |n, b| {
            if b == b'\t' {
                (n / TAB_COLS + 1) * TAB_COLS
            } else {
                n + 1
            }
        });
    }
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
                    self.code.parse(None);
                    self.was_dirty = false;
                    self.widest.set(None);
                })
                .is_some(),
            "completions" => {
                self.help.completions(&self.code, v);
                true
            }
            "signature" => {
                self.help.signature(v);
                true
            }
            "inspected" => {
                self.help.inspected(v);
                true
            }
            "saved" => {
                self.code.mark_saved();
                self.was_dirty = false;
                true
            }
            "language" => v
                .as_str()
                .map(|s| {
                    self.code.set_language(Language::named(s));
                    self.code.parse(None);
                })
                .is_some(),
            "indent" => Indent::parse(v).map(|i| self.code.indent = i).is_some(),
            "line_numbers" => parse::truthy(v).map(|b| self.line_numbers = b).is_some(),
            "paragraph" => parse::truthy(v)
                .map(|b| {
                    self.block = if b {
                        Block::Paragraph
                    } else {
                        Block::Statement
                    }
                })
                .is_some(),
            "text_size" => v
                .as_f64()
                .filter(|n| *n > 0.0)
                .map(|n| self.text_size = n as f32)
                .is_some(),
            "id" | "type" | "name" => true,
            _ => false,
        }
    }

    /// What a set writes rather than keeps: the text marked saved, and the
    /// owner's answers to the help.
    fn writes(&self, key: &str) -> bool {
        matches!(key, "saved" | "completions" | "signature" | "inspected")
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

    /// The frame tick while a parse is behind the text.
    fn needs(&self) -> Needs {
        Needs {
            animated: self.code.parsing() || self.flash.is_some(),
            ..Needs::default()
        }
    }

    /// A slice of the parse that is behind; the colors change when it ends.
    fn tick(&mut self, _live: &Live) -> bool {
        let over = self
            .flash
            .is_some_and(|(_, _, since)| since.elapsed() >= FLASH);
        if over {
            self.flash = None;
        }
        let parsed = self.code.parsing() && self.code.parse(Some(TICK_SLICE));
        over || parsed
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
            (
                "find".into(),
                Value::from(self.bar.finder().query().text.clone()),
            ),
            (
                "found".into(),
                Value::from(self.bar.with_matches(
                    self.code.text(),
                    self.code.revision(),
                    <[(usize, usize)]>::len,
                )),
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
        if let Some(layout) = self.bar.layout(input.rect, input.scale, input.metrics)
            && layout.strip.contains(at.0, at.1)
        {
            let part = self
                .bar
                .press(&layout, at, input.clicks, input.mods.shift, input.metrics);
            return match part {
                Some(part) => Claim::events(self.bar_part(part)),
                None => Claim::take(),
            };
        }
        // a press on the text gives it the keys back, and lets the
        // completions go
        self.bar.unfocus();
        self.help.completing = None;
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
        if let Some(layout) = self.bar.layout(input.rect, input.scale, input.metrics)
            && self.bar.drag(&layout, at, input.metrics)
        {
            return Events::none();
        }
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
        self.bar.release();
        Events::none()
    }

    fn wheel(&mut self, _at: (f64, f64), delta: (f64, f64), input: &Input) -> Option<Events> {
        // Ctrl+wheel is the size of the text, not a scroll: declined, the
        // window's own zoom takes it.
        if input.mods.ctrl {
            return None;
        }
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

    /// The help's keys first (a list shown walks, Ctrl+Space asks), then
    /// the editing, and after it what the help asks of the owner.
    fn key(&mut self, key: &Key, input: &mut KeyInput) -> Option<Events> {
        if let Some(events) = self.help_key(key, input) {
            return Some(events);
        }
        let before = (self.code.revision(), self.code.caret());
        let events = self.edit_key(key, input)?;
        Some(events.chain(self.after_key(key, input.mods, before)))
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

    /// The code's own commands, for a menu, a key a program binds or a
    /// client's verb: undo and redo, the bar's, and the comment.
    fn command(&mut self, name: &str, _input: &mut KeyInput) -> Option<Events> {
        Some(match name {
            "undo" => self.undo(false),
            "redo" => self.undo(true),
            "find" => self.open_find(),
            "find_next" => self.find(true),
            "find_previous" => self.find(false),
            "replace_all" => self.replace_all(),
            "go_to_line" => {
                self.bar.open_line();
                Events::none()
            }
            "evaluate_line" => self.evaluate_line(),
            "evaluate_block" => self.evaluate_block(),
            "interrupt" | "restart" => Events::message(vec![OscType::String(name.into())]),
            "toggle_comment" => {
                if self.code.toggle_comment() {
                    self.changed()
                } else {
                    Events::none()
                }
            }
            _ => return None,
        })
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
            Entry::action("Select All", "select_all"),
        ])
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }

    /// The help goes with the keys.
    fn blur(&mut self) -> Events {
        self.help.clear();
        Events::none()
    }

    /// **The completions, and a note**: the parameters of the call while it
    /// is followed, else what the name the pointer rests on is.
    fn own_popups(&self, rect: Rect, scale: f32, m: &Metrics) -> Option<Own> {
        let g = self.geometry(rect, scale, m);
        let list = self
            .help
            .completing
            .as_ref()
            .filter(|c| c.showing())
            .and_then(|c| {
                Some(OwnList {
                    entries: c.entries(),
                    hover: Some(c.hover),
                    anchor: self.spot(c.from?, &g)?,
                    text_size: g.size,
                    rows: LIST_ROWS,
                })
            });
        let note_size = (g.size * 0.9).max(1.0);
        let note = match (&self.help.signature, &self.help.resting) {
            (Some(sig), _) if self.help.calling => {
                let at = self.code.at_position(sig.bracket.0, sig.bracket.1);
                self.spot(at, &g).map(|anchor| Note {
                    anchor,
                    head: call_head(sig),
                    aside: String::new(),
                    doc: sig.doc.clone(),
                    doc_lines: 3,
                    code_size: note_size,
                    text_size: note_size,
                    card: false,
                })
            }
            (_, Some((a, b, Some(info)))) => self.word_spot(*a, *b, &g).map(|anchor| Note {
                anchor,
                head: vec![(
                    if info.signature.is_empty() {
                        info.name.clone()
                    } else {
                        info.signature.clone()
                    },
                    false,
                )],
                aside: info.kind.clone(),
                doc: info.doc.clone(),
                doc_lines: 0,
                code_size: note_size,
                text_size: note_size,
                card: true,
            }),
            _ => None,
        };
        (list.is_some() || note.is_some()).then_some(Own { list, note })
    }

    /// A completion pressed is taken.
    fn pick_own(&mut self, row: usize) -> Option<Events> {
        let c = self.help.completing.as_mut()?;
        c.hover = row.min(c.shown.len().saturating_sub(1));
        Some(self.accept())
    }

    fn takes_rests(&self) -> bool {
        true
    }

    /// **The pointer rested on a name**: what it is asked of the owner, as
    /// `"inspect" source line column`, the place the end of the name; it
    /// moving on takes the note down.
    fn rest(&mut self, at: Option<(f64, f64)>, input: &Input) -> Option<Events> {
        let Some(at) = at else {
            return self.help.resting.take().map(|_| Events::none());
        };
        let g = self.geometry(input.rect, input.scale, input.metrics);
        if !g.text.contains(at.0, at.1) {
            return None;
        }
        let pos = self.pos_at(at, &g);
        let (a, b) = self.code.word_at(pos)?;
        if self.code.in_text(a) {
            return None;
        }
        self.help.resting = Some((a, b, None));
        let (line, column) = self.code.position(b);
        Some(Events::message(vec![
            OscType::String("inspect".into()),
            OscType::String(self.code.text().to_string()),
            OscType::Int(line as i32),
            OscType::Int(column as i32),
        ]))
    }
}

#[cfg(test)]
mod tests;
