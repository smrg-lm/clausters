//! **The code element's bar**: find and replace, or go to a line, in a strip
//! along the element's bottom, as the current editor has them.
//!
//! The bar is the element's own space: the text is drawn above it, and its
//! fields take the keys while one of them is focused, the text's caret
//! waiting meanwhile. A field is a line of text as every field of the host
//! is ([`edit_key`], [`controls::field`]); a switch is a latch and a step a
//! button, drawn as the host's controls are.
//!
//! **Find**: the query in the first row, with the next and the previous
//! match, the three switches (case, a regular expression, a whole word)
//! and how many matches there are; the replacement in the second, with
//! Replace and All. **Go to a line**: one row, a field and Go. The query
//! and the switches stay when the bar closes, as the next and the previous
//! match go on being found with it closed.

use std::cell::RefCell;

use crate::host::elements::text::edit_key;
use crate::host::graphics::code::find::{Finder, Query};
use crate::host::graphics::controls::{self, ButtonLook, ToggleView};
use crate::host::graphics::textedit::{self, Caret, Unit};
use crate::host::widget::element::{Key, KeyInput};
use crate::host::widget::size::{control_box, text_box};
use clausters_gui_draw::draw::Draw;
use clausters_gui_draw::font;
use clausters_gui_draw::metrics::Metrics;
use clausters_gui_draw::rect::Rect;

/// What the bar is open for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    Find,
    Line,
}

/// A field of the bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Field {
    Find,
    Replace,
    Line,
}

/// **A part of the bar**, for the hand: what a press on it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Part {
    Field(Field),
    Next,
    Previous,
    Case,
    Regexp,
    Word,
    Replace,
    All,
    Go,
    Close,
}

/// The most matches counted: past it the bar says there are more.
const COUNTED: usize = 10_000;

/// The widest a field is, in characters, and the narrowest.
const FIELD_CHARS: (f32, f32) = (6.0, 28.0);

/// The matches of a query, and the text revision they were found in.
type Found = (u64, Vec<(usize, usize)>);

/// A line of text typed into the bar.
#[derive(Clone, Debug, Default)]
struct Entry {
    value: String,
    caret: Caret,
}

/// The bar of a code element: open or not, its fields, its switches and
/// the matches of its query.
#[derive(Clone, Debug)]
pub(super) struct Bar {
    mode: Option<Mode>,
    find: Entry,
    replace: Entry,
    line: Entry,
    /// The field the keys go to; none while they go to the text.
    focus: Option<Field>,
    finder: Finder,
    /// Every match of the query, for the text revision they were found in:
    /// the count, the one selected among them and the ones a frame shows.
    matches: RefCell<Option<Found>>,
    /// What a press in a field selected and by what unit, while a drag
    /// grows it.
    held: Option<(Field, (usize, usize), Unit)>,
}

/// **Where the parts of the bar are**: the strip, every part's box and the
/// room the count is said in.
pub(super) struct Layout {
    pub strip: Rect,
    parts: Vec<(Part, Rect)>,
    /// Where the bar says what it has to: the count, or its field's name.
    note: Rect,
    size: f32,
}

impl Default for Bar {
    fn default() -> Self {
        Bar {
            mode: None,
            find: Entry::default(),
            replace: Entry::default(),
            line: Entry::default(),
            focus: None,
            finder: Finder::new(&Query::default()),
            matches: RefCell::new(None),
            held: None,
        }
    }
}

impl Bar {
    pub fn mode(&self) -> Option<Mode> {
        self.mode
    }

    /// The field the keys go to, if the bar has them.
    pub fn focus(&self) -> Option<Field> {
        self.focus.filter(|_| self.mode.is_some())
    }

    /// The keys back to the text, the bar left open.
    pub fn unfocus(&mut self) {
        self.focus = None;
        self.held = None;
    }

    pub fn finder(&self) -> &Finder {
        &self.finder
    }

    pub fn replacement(&self) -> &str {
        &self.replace.value
    }

    /// **The bar opened to find**, its query field focused with what it
    /// holds selected; `seed`, the text selected, becomes the query.
    pub fn open_find(&mut self, seed: Option<&str>) {
        if let Some(seed) = seed.filter(|s| !s.is_empty() && s.len() < 100) {
            // the text as the query reads it: escaped for an expression,
            // its line ends and backslashes written out for a plain one
            self.find.value = if self.finder.query().regexp {
                regex::escape(seed)
            } else {
                seed.replace('\\', "\\\\").replace('\n', "\\n")
            };
            self.requery();
        }
        self.mode = Some(Mode::Find);
        self.focus_field(Field::Find);
    }

    /// The bar opened to go to a line, its field focused and empty.
    pub fn open_line(&mut self) {
        self.mode = Some(Mode::Line);
        self.line = Entry::default();
        self.focus_field(Field::Line);
    }

    pub fn close(&mut self) {
        self.mode = None;
        self.unfocus();
    }

    /// The keys to `field`, what it holds selected.
    pub fn focus_field(&mut self, field: Field) {
        self.focus = Some(field);
        let entry = self.entry_mut(field);
        textedit::select_all(&entry.value.clone(), &mut entry.caret);
    }

    fn entry(&self, field: Field) -> &Entry {
        match field {
            Field::Find => &self.find,
            Field::Replace => &self.replace,
            Field::Line => &self.line,
        }
    }

    fn entry_mut(&mut self, field: Field) -> &mut Entry {
        match field {
            Field::Find => &mut self.find,
            Field::Replace => &mut self.replace,
            Field::Line => &mut self.line,
        }
    }

    /// What the line field holds.
    pub fn line_spec(&self) -> &str {
        &self.line.value
    }

    /// The query typed or switched: searched again from now.
    fn requery(&mut self) {
        let q = self.finder.query();
        let query = Query {
            text: self.find.value.clone(),
            ..q.clone()
        };
        self.finder = Finder::new(&query);
        self.matches.replace(None);
    }

    /// **A switch turned**: case, a regular expression or a whole word.
    pub fn switch(&mut self, part: Part) {
        let mut q = self.finder.query().clone();
        match part {
            Part::Case => q.case = !q.case,
            Part::Regexp => q.regexp = !q.regexp,
            Part::Word => q.word = !q.word,
            _ => return,
        }
        self.finder = Finder::new(&q);
        self.matches.replace(None);
    }

    /// **Every match in `text`**, at revision `rev`: found once a revision,
    /// at most [`COUNTED`] of them.
    pub fn with_matches<R>(
        &self,
        text: &str,
        rev: u64,
        f: impl FnOnce(&[(usize, usize)]) -> R,
    ) -> R {
        let mut cache = self.matches.borrow_mut();
        if cache.as_ref().is_none_or(|(r, _)| *r != rev) {
            let found = self.finder.from(text, 0).take(COUNTED + 1).collect();
            *cache = Some((rev, found));
        }
        f(cache.as_ref().map_or(&[], |(_, m)| m.as_slice()))
    }

    /// **What the bar says of its query in `text`**: which match is
    /// selected and how many there are, or that there is none, or why the
    /// query is not one; the last two are read as a warning.
    pub fn note(
        &self,
        text: &str,
        rev: u64,
        selected: Option<(usize, usize)>,
    ) -> Option<(String, bool)> {
        if let Some(e) = self.finder.error() {
            return Some((e.to_string(), true));
        }
        if !self.finder.ready() {
            return None;
        }
        Some(self.with_matches(text, rev, |all| {
            if all.is_empty() {
                return ("No match".to_string(), true);
            }
            let total = if all.len() > COUNTED {
                format!("{COUNTED}+")
            } else {
                all.len().to_string()
            };
            match selected.and_then(|s| all.binary_search(&s).ok()) {
                Some(i) => (format!("{} of {total}", i + 1), false),
                None => (format!("{total} found"), false),
            }
        }))
    }

    /// **A key while a field has the keys**: `Some(Ok(()))` edited the
    /// field, `Some(Err(Field))` is Enter in that field for the element to
    /// act on, `None` a key the field does not take.
    pub fn key(&mut self, key: &Key, input: &mut KeyInput) -> Option<Result<(), Field>> {
        let field = self.focus()?;
        match key {
            Key::Enter => return Some(Err(field)),
            Key::Tab => {
                if field != Field::Line {
                    let other = if field == Field::Find {
                        Field::Replace
                    } else {
                        Field::Find
                    };
                    self.focus_field(other);
                }
                return Some(Ok(()));
            }
            _ => {}
        }
        let entry = self.entry_mut(field);
        let changed = edit_key(&mut entry.value, &mut entry.caret, false, key, input)?;
        if changed && field == Field::Find {
            self.requery();
        }
        Some(Ok(()))
    }

    /// **The strip and its parts**, at the bottom of `area`; none while the
    /// bar is closed.
    pub fn layout(&self, area: Rect, scale: f32, m: &Metrics) -> Option<Layout> {
        let mode = self.mode?;
        let size = font::DEFAULT_SIZE * scale;
        let row = control_box(size, m);
        let rows = if mode == Mode::Find { 2.0 } else { 1.0 };
        let h = (rows * row + m.pad).min(area.h);
        let strip = Rect::new(area.x, area.y + area.h - h, area.w, h);
        let mut parts = Vec::new();
        let square = row;
        let close = Rect::new(
            strip.x + strip.w - square - m.pad * 0.5,
            strip.y + m.pad * 0.5,
            square,
            row,
        );
        parts.push((Part::Close, close));
        let char_w = font::width("0", size).max(1.0);
        let button = |label: &str| text_box(label, size, m) + m.pad;
        let y0 = strip.y + m.pad * 0.5;
        let note = match mode {
            Mode::Find => {
                let tail = [
                    (Part::Next, square),
                    (Part::Previous, square),
                    (Part::Case, button("Aa")),
                    (Part::Regexp, button(".*")),
                    (Part::Word, button("W")),
                ];
                let fixed: f32 = tail.iter().map(|t| t.1).sum::<f32>() + square;
                let field_w = (close.x - strip.x - m.pad - fixed - 10.0 * char_w)
                    .clamp(FIELD_CHARS.0 * char_w, FIELD_CHARS.1 * char_w);
                let mut x = strip.x + m.pad * 0.5;
                parts.push((Part::Field(Field::Find), Rect::new(x, y0, field_w, row)));
                x += field_w;
                for (part, w) in tail {
                    parts.push((part, Rect::new(x, y0, w, row)));
                    x += w;
                }
                let note = Rect::new(x + m.pad, y0, (close.x - x - 2.0 * m.pad).max(0.0), row);
                let y1 = y0 + row;
                let mut x = strip.x + m.pad * 0.5;
                parts.push((Part::Field(Field::Replace), Rect::new(x, y1, field_w, row)));
                x += field_w;
                for (part, label) in [(Part::Replace, "Replace"), (Part::All, "All")] {
                    let w = button(label);
                    parts.push((part, Rect::new(x, y1, w, row)));
                    x += w;
                }
                note
            }
            Mode::Line => {
                let label_w = text_box("Go to line", size, m);
                let mut x = strip.x + m.pad * 0.5;
                let note = Rect::new(x, y0, label_w, row);
                x += label_w;
                let field_w = 12.0 * char_w + 2.0 * m.pad;
                parts.push((Part::Field(Field::Line), Rect::new(x, y0, field_w, row)));
                x += field_w;
                parts.push((Part::Go, Rect::new(x, y0, button("Go"), row)));
                note
            }
        };
        Some(Layout {
            strip,
            parts,
            note,
            size,
        })
    }

    /// **A press on the bar**: the part it lands on, a field taking the keys
    /// and its caret where the press landed.
    pub fn press(
        &mut self,
        layout: &Layout,
        at: (f64, f64),
        clicks: u32,
        shift: bool,
        m: &Metrics,
    ) -> Option<Part> {
        let part = layout.part_at(at)?;
        if let Part::Field(field) = part {
            let rect = layout.rect_of(part)?;
            self.focus = Some(field);
            let size = layout.size;
            let entry = self.entry_mut(field);
            let pos = controls::caret_at(
                rect,
                &entry.value,
                false,
                size,
                false,
                entry.caret,
                at.0,
                at.1,
                m,
            );
            if shift && clicks == 1 {
                let from = entry.caret.anchor.unwrap_or(entry.caret.pos);
                entry.caret = textedit::extend_by(&entry.value, (from, from), pos, Unit::Char);
                self.held = Some((field, (from, from), Unit::Char));
            } else {
                let unit = Unit::of_clicks(clicks);
                let span = textedit::unit_at(&entry.value, pos, unit);
                entry.caret = textedit::extend_by(&entry.value, span, pos, Unit::Char);
                self.held = Some((field, span, unit));
            }
        }
        Some(part)
    }

    /// A drag from a press in a field: its selection grown to the pointer.
    /// Answers whether one was held.
    pub fn drag(&mut self, layout: &Layout, at: (f64, f64), m: &Metrics) -> bool {
        let Some((field, span, unit)) = self.held else {
            return false;
        };
        let Some(rect) = layout.rect_of(Part::Field(field)) else {
            return false;
        };
        let size = layout.size;
        let entry = self.entry_mut(field);
        let pos = controls::caret_at(
            rect,
            &entry.value,
            false,
            size,
            false,
            entry.caret,
            at.0,
            at.1,
            m,
        );
        entry.caret = textedit::extend_by(&entry.value, span, pos, unit);
        true
    }

    pub fn release(&mut self) {
        self.held = None;
    }

    /// **The bar drawn**: its strip, its fields (the focused one with its
    /// caret), its switches as they stand, and what it has to say: how
    /// many matches and which is selected, or why the query is not one.
    pub fn draw(&self, d: &mut Draw, layout: &Layout, focused: bool, note: Option<(String, bool)>) {
        {
            let (mesh, m, theme) = d.parts();
            mesh.rect(layout.strip, theme.status_bar);
            mesh.rect(
                Rect::new(layout.strip.x, layout.strip.y, layout.strip.w, m.divider_w),
                theme.separator,
            );
        }
        let size = layout.size;
        let q = self.finder.query();
        let look = ButtonLook {
            active: false,
            hovered: false,
            flat: false,
        };
        let flat = ButtonLook { flat: true, ..look };
        for (part, rect) in &layout.parts {
            match part {
                Part::Field(field) => {
                    let entry = self.entry(*field);
                    let caret = (focused && self.focus() == Some(*field)).then_some(entry.caret);
                    controls::field(d, &entry.value, None, *rect, size, false, caret);
                }
                Part::Case | Part::Regexp | Part::Word => {
                    let (on, label) = match part {
                        Part::Case => (q.case, "Aa"),
                        Part::Regexp => (q.regexp, ".*"),
                        _ => (q.word, "W"),
                    };
                    let inner = inset(*rect, d.m);
                    controls::toggle(
                        d,
                        on,
                        Some(label),
                        None,
                        inner,
                        ToggleView::Button,
                        false,
                        size,
                    );
                }
                Part::Next | Part::Previous | Part::Close => {
                    let symbol = match part {
                        Part::Next => font::symbol::DOWN,
                        Part::Previous => font::symbol::UP,
                        _ => font::symbol::CLOSE,
                    };
                    controls::button(d, None, Some(symbol), inset(*rect, d.m), flat, size);
                }
                Part::Replace | Part::All | Part::Go => {
                    let label = match part {
                        Part::Replace => "Replace",
                        Part::All => "All",
                        _ => "Go",
                    };
                    controls::button(d, Some(label), None, inset(*rect, d.m), look, size);
                }
            }
        }
        {
            let rect = layout.note;
            let (mesh, m, theme) = d.parts();
            let (text, warn) = match self.mode {
                Some(Mode::Line) => ("Go to line".to_string(), false),
                _ => note.unwrap_or_default(),
            };
            let ty = rect.y + (rect.h - font::height(size)) * 0.5;
            let x = if self.mode == Some(Mode::Line) {
                rect.x + m.pad
            } else {
                rect.x
            };
            let color = if warn { theme.warn } else { theme.text_dim };
            font::text_ellipsis(mesh, &text, x, ty, rect.w, size, color);
        }
    }
}

impl Layout {
    /// The part under `at`.
    pub fn part_at(&self, at: (f64, f64)) -> Option<Part> {
        self.parts
            .iter()
            .find(|(_, r)| r.contains(at.0, at.1))
            .map(|(p, _)| *p)
    }

    pub fn rect_of(&self, part: Part) -> Option<Rect> {
        self.parts.iter().find(|(p, _)| *p == part).map(|(_, r)| *r)
    }
}

/// A button's box inside its cell: the cells of a row stand apart.
fn inset(r: Rect, m: &Metrics) -> Rect {
    let pad = (m.pad * 0.5).round();
    Rect::new(
        r.x + pad,
        r.y + pad,
        (r.w - 2.0 * pad).max(1.0),
        (r.h - 2.0 * pad).max(1.0),
    )
}
