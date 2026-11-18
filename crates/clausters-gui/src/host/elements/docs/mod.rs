//! `docs` -- the documentation: the books a session's packages carry, read
//! as the current editor's documentation panel reads them.
//!
//! The model is [`crate::host::graphics::docs`]: a document read once into
//! blocks, laid out at the panel's width, and searched. What this file adds
//! is the panel: the strip along its top (back, forward, the start page,
//! where the page is, find), the find bar, the page scrolled under them,
//! and the hand on it.
//!
//! **What it reads is given to it.** The owner sets `books` (each book's
//! table of contents) and `documents` (each document's Markdown, by its
//! path), natively and in a page alike, and the panel holds them: so a
//! search across every document and help on a name are answered at once,
//! with nothing asked back. A link out of the documentation is reported
//! (`"link" url`) for the owner to open.
//!
//! **Pages**: the start page lists every book and its chapters; a book's
//! page its whole contents; a document; the results of a search; and a text
//! the owner gave for a name no document has (a session's own help). Back
//! and forward walk them, each kept where it was scrolled.
//!
//! **Finding**: Ctrl+F finds in the page, the matches marked and the
//! current one brought into view; Ctrl+Shift+F searches every document
//! ([`Index`]); and `help` opens the section that is the name asked about,
//! a document named after it, or the section that says it most -- else the
//! text the owner sent with it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use clausters_core::osc::OscType;
use serde_json::{Map, Value};

use crate::host::elements::text::edit_key;
use crate::host::graphics::controls::{self, ButtonLook, ToggleView};
use crate::host::graphics::docs::index::{Hit, Index, talking_about};
use crate::host::graphics::docs::layout::{Ink, Item, Laid, lay};
use crate::host::graphics::docs::markdown::{Block, Run, parse, slugify};
use crate::host::graphics::textedit::{self, Caret};
use crate::host::widget::element::{Claim, Ctx, Element, Events, Input, Key, KeyInput, Verb};
use crate::host::widget::parse as props;
use crate::host::widget::size::{Natural, control_box, text_box};
use clausters_gui_draw::draw::Draw;
use clausters_gui_draw::font;
use clausters_gui_draw::metrics::Metrics;
use clausters_gui_draw::rect::Rect;
use clausters_gui_draw::theme::{Color, Theme};

/// A link to a page the panel writes itself: a book's contents, a document.
const INTERNAL: &str = "clausters-docs:";

/// How many pixels a step of the wheel scrolls, in lines of text.
const WHEEL_LINES: f32 = 3.0;

/// The most results a search lists.
const MOST: usize = 200;

/// A chapter of a book's contents.
#[derive(Clone, Debug, PartialEq)]
struct Chapter {
    title: String,
    /// Its document; none for a chapter not written yet.
    path: Option<String>,
    children: Vec<Chapter>,
}

/// A book: what its documents' paths start with, its title, its parts.
#[derive(Clone, Debug, PartialEq)]
struct Book {
    prefix: String,
    title: String,
    parts: Vec<(Option<String>, Vec<Chapter>)>,
}

fn chapter(v: &Value) -> Chapter {
    Chapter {
        title: v
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        path: v.get("path").and_then(Value::as_str).map(str::to_string),
        children: v
            .get("children")
            .and_then(Value::as_array)
            .map(|c| c.iter().map(chapter).collect())
            .unwrap_or_default(),
    }
}

fn book(v: &Value) -> Book {
    Book {
        prefix: v
            .get("prefix")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        title: v
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        parts: v
            .get("parts")
            .and_then(Value::as_array)
            .map(|parts| {
                parts
                    .iter()
                    .map(|p| {
                        (
                            p.get("title").and_then(Value::as_str).map(str::to_string),
                            p.get("chapters")
                                .and_then(Value::as_array)
                                .map(|c| c.iter().map(chapter).collect())
                                .unwrap_or_default(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// A value that is a structure, or the JSON text of one.
fn structure(v: &Value) -> Option<Value> {
    match v {
        Value::String(s) => serde_json::from_str(s).ok(),
        other => Some(other.clone()),
    }
}

/// One page the panel shows.
#[derive(Clone, Debug, PartialEq)]
enum Page {
    Home,
    Book(String),
    Doc {
        path: String,
        anchor: Option<String>,
    },
    /// A text the owner gave: a session's help for a name.
    Text {
        title: String,
        text: String,
        source: String,
    },
    Search {
        query: String,
        hits: Vec<Hit>,
    },
}

/// A page in the history, and where it was left.
#[derive(Clone, Debug)]
struct Visit {
    page: Page,
    scroll: Option<f32>,
}

/// What the find bar searches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scope {
    Page,
    All,
}

/// A part of the strips along the top, for the hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Back,
    Forward,
    Home,
    /// The book on the trail: its contents.
    Book,
    Find,
    Field,
    Previous,
    Next,
    InPage,
    Everywhere,
    Close,
}

/// Where the parts of the panel are.
#[derive(Clone, Debug)]
struct Geometry {
    strip: Rect,
    bar: Option<Rect>,
    page: Rect,
    /// The page's text, inside its margins.
    text: Rect,
    parts: Vec<(Part, Rect)>,
    /// Where the trail is written.
    trail: Rect,
    /// Where the count of matches is written.
    count: Option<Rect>,
    ui: f32,
    size: f32,
}

/// A page laid out, for which page, at which width and size.
type Kept = (Page, f32, f32, Rc<Laid>);

/// The documentation panel.
#[derive(Debug, Clone)]
pub struct DocsView {
    text_size: f32,
    books: Vec<Book>,
    /// Every document's Markdown, by its path.
    documents: HashMap<String, String>,
    history: Vec<Visit>,
    pos: usize,
    scroll: Cell<f32>,
    /// A place to be brought to the top once the page is laid out.
    reveal: RefCell<Option<String>>,
    /// The page laid out: for which page, at which width and size.
    laid: RefCell<Option<Kept>>,
    /// The search index, built at the first search.
    index: RefCell<Option<Rc<Index>>>,
    find_open: bool,
    scope: Scope,
    find: String,
    caret: Caret,
    /// Whether the find field has the keys.
    find_focus: bool,
    /// The current match of the page's.
    current: usize,
    /// The match to bring into view at the next frame.
    reveal_match: Cell<bool>,
    /// What a press on a copy button left to copy.
    copy: Option<String>,
}

pub(super) fn build(
    p: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(DocsView::new(p)))
}

impl DocsView {
    fn new(p: &Map<String, Value>) -> DocsView {
        let mut view = DocsView {
            text_size: props::text_size(p),
            books: Vec::new(),
            documents: HashMap::new(),
            history: vec![Visit {
                page: Page::Home,
                scroll: None,
            }],
            pos: 0,
            scroll: Cell::new(0.0),
            reveal: RefCell::new(None),
            laid: RefCell::new(None),
            index: RefCell::new(None),
            find_open: false,
            scope: Scope::Page,
            find: String::new(),
            caret: Caret::default(),
            find_focus: false,
            current: 0,
            reveal_match: Cell::new(false),
            copy: None,
        };
        for (key, value) in p {
            view.set(key, value);
        }
        view
    }
}

/// The link a run of a page the panel writes follows.
fn internal(kind: &str, what: &str) -> Option<String> {
    Some(format!("{INTERNAL}{kind}:{what}"))
}

fn run(text: &str) -> Run {
    Run {
        text: text.to_string(),
        ..Run::default()
    }
}

fn linked(text: &str, link: Option<String>) -> Run {
    Run {
        text: text.replace('`', ""),
        link,
        ..Run::default()
    }
}

/// `rel`, a link written in document `from`, as a path from the books' root.
fn resolve(from: &str, rel: &str) -> String {
    let mut parts: Vec<&str> = from.split('/').collect();
    parts.pop();
    for seg in rel.split('/') {
        match seg {
            ".." => {
                parts.pop();
            }
            "" | "." => {}
            s => parts.push(s),
        }
    }
    parts.join("/")
}

impl DocsView {
    fn page(&self) -> &Page {
        &self.history[self.pos].page
    }

    /// **Going to `page`**: where the page on screen was left is kept, and
    /// what was ahead of it goes.
    fn go(&mut self, page: Page) {
        self.history[self.pos].scroll = Some(self.scroll.get());
        self.history.truncate(self.pos + 1);
        if let Page::Doc {
            anchor: Some(a), ..
        } = &page
        {
            self.reveal.replace(Some(a.clone()));
        }
        self.history.push(Visit { page, scroll: None });
        self.pos = self.history.len() - 1;
        self.scroll.set(0.0);
        self.current = 0;
        self.reveal_match.set(self.find_open);
    }

    /// One step back (`-1`) or forward (`1`) through the history, each page
    /// where it was left.
    fn step(&mut self, by: isize) -> bool {
        let to = self.pos as isize + by;
        if to < 0 || to as usize >= self.history.len() {
            return false;
        }
        self.history[self.pos].scroll = Some(self.scroll.get());
        self.pos = to as usize;
        let visit = &self.history[self.pos];
        self.scroll.set(visit.scroll.unwrap_or(0.0));
        if visit.scroll.is_none()
            && let Page::Doc {
                anchor: Some(a), ..
            } = &visit.page
        {
            self.reveal.replace(Some(a.clone()));
        }
        true
    }

    /// The book a document belongs to: the one whose prefix it starts with,
    /// else the one with none.
    fn book_of(&self, path: &str) -> Option<&Book> {
        self.books
            .iter()
            .find(|b| !b.prefix.is_empty() && path.starts_with(&b.prefix))
            .or_else(|| self.books.iter().find(|b| b.prefix.is_empty()))
    }

    /// A document's title as its book's contents give it, else its file's.
    fn chapter_title(&self, path: &str) -> String {
        fn find(chapters: &[Chapter], path: &str) -> Option<String> {
            chapters.iter().find_map(|c| {
                (c.path.as_deref() == Some(path))
                    .then(|| c.title.clone())
                    .or_else(|| find(&c.children, path))
            })
        }
        self.books
            .iter()
            .flat_map(|b| &b.parts)
            .find_map(|(_, chapters)| find(chapters, path))
            .unwrap_or_else(|| {
                path.rsplit('/')
                    .next()
                    .unwrap_or(path)
                    .trim_end_matches(".md")
                    .to_string()
            })
            .replace('`', "")
    }

    /// **The trail**: the book the page is in, and where in it.
    fn trail(&self) -> (Option<String>, String) {
        match self.page() {
            Page::Home => (None, "Documentation".into()),
            Page::Book(prefix) => (
                self.books
                    .iter()
                    .find(|b| &b.prefix == prefix)
                    .map(|b| b.title.clone()),
                String::new(),
            ),
            Page::Doc { path, .. } => (
                self.book_of(path).map(|b| b.title.clone()),
                self.chapter_title(path),
            ),
            Page::Text { title, source, .. } => (None, format!("{title} ({source})")),
            Page::Search { query, .. } => (None, format!("Search: \"{query}\"")),
        }
    }

    /// A chapter as a link, or its title alone when it has no page yet.
    fn chapter_run(c: &Chapter) -> Run {
        linked(&c.title, c.path.as_ref().and_then(|p| internal("doc", p)))
    }

    /// **The blocks of the page on screen.**
    fn blocks(&self) -> Vec<Block> {
        let heading = |level: u8, runs: Vec<Run>| Block::Heading {
            level,
            id: slugify(&crate::host::graphics::docs::markdown::plain(&runs)),
            runs,
        };
        match self.page() {
            Page::Home => {
                let mut out = vec![heading(1, vec![run("Documentation")])];
                for b in &self.books {
                    out.push(heading(
                        2,
                        vec![linked(&b.title, internal("book", &b.prefix))],
                    ));
                    for (title, chapters) in &b.parts {
                        if let Some(t) = title {
                            out.push(heading(4, vec![linked(t, None)]));
                        }
                        out.push(Block::List {
                            start: None,
                            items: chapters
                                .iter()
                                .map(|c| vec![Block::Para(vec![Self::chapter_run(c)])])
                                .collect(),
                        });
                    }
                }
                if self.books.is_empty() {
                    out.push(Block::Para(vec![run("No documentation is loaded.")]));
                }
                out
            }
            Page::Book(prefix) => {
                let Some(b) = self.books.iter().find(|b| &b.prefix == prefix) else {
                    return vec![Block::Para(vec![run("No such book.")])];
                };
                fn list(chapters: &[Chapter]) -> Block {
                    Block::List {
                        start: None,
                        items: chapters
                            .iter()
                            .map(|c| {
                                let mut item = vec![Block::Para(vec![DocsView::chapter_run(c)])];
                                if !c.children.is_empty() {
                                    item.push(list(&c.children));
                                }
                                item
                            })
                            .collect(),
                    }
                }
                let mut out = vec![heading(1, vec![linked(&b.title, None)])];
                for (title, chapters) in &b.parts {
                    if let Some(t) = title {
                        out.push(heading(2, vec![linked(t, None)]));
                    }
                    out.push(list(chapters));
                }
                out
            }
            Page::Doc { path, .. } => match self.documents.get(path) {
                Some(source) => parse(source),
                None => vec![Block::Para(vec![run(&format!(
                    "No document {path} is loaded."
                ))])],
            },
            Page::Text { text, .. } => vec![Block::Code {
                lang: String::new(),
                text: text.clone(),
            }],
            Page::Search { query, hits } => {
                let summary = if hits.is_empty() {
                    format!("Nothing in the documentation matches \"{query}\".")
                } else {
                    let n = hits.len();
                    format!(
                        "\"{query}\": {n} {}{}, the most relevant first.",
                        if n == 1 { "section" } else { "sections" },
                        if n >= MOST { " (the first 200)" } else { "" }
                    )
                };
                let mut out = vec![Block::Para(vec![run(&summary)])];
                for h in hits {
                    let book = self
                        .book_of(&h.path)
                        .map(|b| b.title.clone())
                        .unwrap_or_default();
                    let mut title = format!("{book} \u{203A} {}", self.chapter_title(&h.path));
                    if let Some(heading) = &h.heading {
                        title.push_str(&format!(" \u{203A} {heading}"));
                    }
                    let target = match &h.heading {
                        Some(heading) => format!("{}#{}", h.path, slugify(heading)),
                        None => h.path.clone(),
                    };
                    let mut item = vec![Block::Para(vec![Run {
                        strong: true,
                        ..linked(&title, internal("doc", &target))
                    }])];
                    if !h.line.is_empty() {
                        item.push(Block::Quote(vec![Block::Para(vec![run(&h.line)])]));
                    }
                    out.push(Block::List {
                        start: None,
                        items: vec![item],
                    });
                }
                out
            }
        }
    }

    /// **The page laid out** at `width` and `size`, kept while neither nor
    /// the page changes.
    fn laid(&self, width: f32, size: f32) -> Rc<Laid> {
        if let Some((page, w, s, laid)) = self.laid.borrow().as_ref()
            && page == self.page()
            && *w == width
            && *s == size
        {
            return laid.clone();
        }
        let laid = Rc::new(lay(&self.blocks(), width, size));
        *self.laid.borrow_mut() = Some((self.page().clone(), width, size, laid.clone()));
        laid
    }

    /// Where the parts are in `rect`, the strip along its top -- or in
    /// `header`, the slot of its window's header, when its window took the
    /// strip as its header ([`crate::host::header`]).
    fn geometry(&self, rect: Rect, scale: f32, m: &Metrics, header: Option<Rect>) -> Geometry {
        let ui = font::DEFAULT_SIZE * scale;
        let size = self.text_size * scale;
        let row = control_box(ui, m);
        let (strip, mut top) = match header {
            Some(slot) => (slot, rect.y),
            None => {
                let strip = Rect::new(rect.x, rect.y, rect.w, row.min(rect.h));
                (strip, strip.y + strip.h)
            }
        };
        // the strip's controls, a line of control centred in it
        let sy = strip.y + ((strip.h - row) * 0.5).max(0.0);
        let mut parts = Vec::new();
        let square = row;
        let mut x = strip.x;
        for part in [Part::Back, Part::Forward, Part::Home] {
            parts.push((part, Rect::new(x, sy, square, row)));
            x += square;
        }
        let find = Rect::new(strip.x + strip.w - square, sy, square, row);
        parts.push((Part::Find, find));
        let (book, _) = self.trail();
        let trail_x = x + m.pad;
        if let Some(b) = &book {
            let w = (font::width(b, ui) + 2.0 * m.pad).min((find.x - trail_x).max(0.0));
            parts.push((Part::Book, Rect::new(trail_x, sy, w, row)));
        }
        let trail = Rect::new(trail_x, sy, (find.x - trail_x - m.pad).max(0.0), row);
        let mut count = None;
        let bar = self.find_open.then(|| {
            let bar = Rect::new(
                rect.x,
                top,
                rect.w,
                row.min((rect.y + rect.h - top).max(0.0)),
            );
            let close = Rect::new(bar.x + bar.w - square, bar.y, square, row);
            let all_w = text_box("All documents", ui, m) + m.pad;
            let page_w = text_box("Page", ui, m) + m.pad;
            let mut right = close.x - all_w;
            parts.push((Part::Everywhere, Rect::new(right, bar.y, all_w, row)));
            right -= page_w;
            parts.push((Part::InPage, Rect::new(right, bar.y, page_w, row)));
            if self.scope == Scope::Page {
                right -= square;
                parts.push((Part::Next, Rect::new(right, bar.y, square, row)));
                right -= square;
                parts.push((Part::Previous, Rect::new(right, bar.y, square, row)));
                let count_w = text_box("000/000", ui, m);
                right -= count_w;
                count = Some(Rect::new(right, bar.y, count_w, row));
            }
            parts.push((Part::Close, close));
            parts.push((
                Part::Field,
                Rect::new(bar.x, bar.y, (right - bar.x).max(1.0), row),
            ));
            top += bar.h;
            bar
        });
        let page = Rect::new(rect.x, top, rect.w, (rect.y + rect.h - top).max(1.0));
        let margin = (font::height(size) / 0.727 * 1.25).round();
        let text = Rect::new(
            page.x + margin,
            page.y + margin * 0.6,
            (page.w - 2.0 * margin).max(1.0),
            (page.h - margin * 0.6).max(1.0),
        );
        Geometry {
            strip,
            bar,
            page,
            text,
            parts,
            trail,
            count,
            ui,
            size,
        }
    }

    /// The scroll kept inside the page, and where it is asked to be: at a
    /// heading or an anchor, or at the current match.
    fn settle(&self, g: &Geometry) -> (f32, Rc<Laid>) {
        let laid = self.laid(g.text.w, g.size);
        let mut s = self.scroll.get();
        if let Some(id) = self.reveal.take()
            && let Some(y) = laid.anchor(&id)
        {
            s = y;
        }
        if self.reveal_match.replace(false)
            && let Some(m) = self.matches(&laid).get(self.current)
        {
            let (top, bottom) = (m.1 - g.text.y, m.1 + m.3);
            if top < s || bottom > s + g.text.h {
                s = (m.1 - g.text.h * 0.4).max(0.0);
            }
        }
        let most = (laid.height - g.text.h * 0.5).max(0.0);
        s = s.clamp(0.0, most);
        self.scroll.set(s);
        (s, laid)
    }

    /// **Where the find bar's text stands on the page**, as rectangles from
    /// the page's top left: every piece of text that holds it, case aside.
    fn matches(&self, laid: &Laid) -> Vec<(f32, f32, f32, f32)> {
        let sought = self.find.trim().to_lowercase();
        if !self.find_open || self.scope != Scope::Page || sought.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for item in &laid.items {
            let Item::Text {
                x,
                y,
                text,
                family,
                size,
                ..
            } = item
            else {
                continue;
            };
            let lower = text.to_lowercase();
            // only where lowering kept the bytes, so a place is the text's
            if lower.len() != text.len() {
                continue;
            }
            for (at, _) in lower.match_indices(&sought) {
                let from = font::width_in(*family, &text[..at], *size);
                let w = font::width_in(*family, &text[at..at + sought.len()], *size);
                out.push((x + from, *y, w, font::height(*size)));
            }
        }
        out
    }

    /// The index of every document, built the first time it is asked for.
    fn index(&self) -> Rc<Index> {
        if let Some(i) = self.index.borrow().as_ref() {
            return i.clone();
        }
        let mut docs: Vec<(&str, &str)> = self
            .documents
            .iter()
            .map(|(p, s)| (p.as_str(), s.as_str()))
            .collect();
        docs.sort();
        let index = Rc::new(Index::build(docs));
        *self.index.borrow_mut() = Some(index.clone());
        index
    }

    /// **A search of every document** for the find bar's text.
    fn search_all(&mut self) {
        let query = self.find.trim().to_string();
        if query.is_empty() {
            return;
        }
        let hits = self.index().search(&query, MOST);
        self.go(Page::Search { query, hits });
    }

    /// The find bar opened, on the page or on every document.
    fn open_find(&mut self, scope: Scope) {
        self.find_open = true;
        self.scope = scope;
        self.find_focus = true;
        textedit::select_all(&self.find.clone(), &mut self.caret);
        self.reveal_match.set(true);
        if scope == Scope::All {
            self.search_all();
        }
    }

    /// One step through the page's matches, round past either end.
    fn step_match(&mut self, by: isize, total: usize) {
        if total > 0 {
            self.current = (self.current as isize + by).rem_euclid(total as isize) as usize;
            self.reveal_match.set(true);
        }
    }

    /// The number of the page's matches, laid out as last drawn.
    fn match_count(&self) -> usize {
        self.laid
            .borrow()
            .as_ref()
            .map_or(0, |(_, _, _, laid)| self.matches(laid).len())
    }

    /// **A link followed**: a page the panel writes, a document of the books
    /// (relative to the one on screen), a place in it -- or, out of the
    /// documentation, reported for the owner to open.
    fn follow(&mut self, href: &str) -> Events {
        if let Some(rest) = href.strip_prefix(INTERNAL) {
            if let Some(prefix) = rest.strip_prefix("book:") {
                self.go(Page::Book(prefix.to_string()));
            } else if let Some(target) = rest.strip_prefix("doc:") {
                let (path, anchor) = match target.split_once('#') {
                    Some((p, a)) => (p, Some(a.to_string())),
                    None => (target, None),
                };
                if let Page::Search { query, .. } = self.page() {
                    // from the results, the word searched is found in the page
                    self.find = query.split_whitespace().next().unwrap_or("").to_string();
                    self.scope = Scope::Page;
                }
                self.go(Page::Doc {
                    path: path.to_string(),
                    anchor,
                });
            }
            return Events::none();
        }
        let scheme = href.split_once(':').is_some_and(|(s, _)| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
        });
        if scheme {
            return Events::message(vec![
                OscType::String("link".into()),
                OscType::String(href.to_string()),
            ]);
        }
        let (path, anchor) = match href.split_once('#') {
            Some((p, a)) => (p, Some(a.to_string())),
            None => (href, None),
        };
        let from = match self.page() {
            Page::Doc { path, .. } => Some(path.clone()),
            _ => None,
        };
        let path = match (path.is_empty(), &from) {
            (true, Some(f)) => f.clone(),
            (false, Some(f)) => resolve(f, path),
            (_, None) => path.to_string(),
        };
        self.go(Page::Doc { path, anchor });
        Events::none()
    }

    /// **Help on a name**: the section that is it, a document named after
    /// it, the section that says it most -- or the text the owner sent.
    fn help(&mut self, v: &Value) {
        let text_of = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let (name, qualname, fullname) =
            (text_of("name"), text_of("qualname"), text_of("fullname"));
        if name.is_empty() {
            return;
        }
        let mut terms: Vec<&str> = Vec::new();
        for t in [qualname.as_str(), name.as_str()] {
            if !t.is_empty() && !terms.contains(&t) {
                terms.push(t);
            }
        }
        let docs: Vec<(&str, &str)> = self
            .documents
            .iter()
            .map(|(p, s)| (p.as_str(), s.as_str()))
            .collect();
        let searches: Vec<_> = terms
            .iter()
            .map(|t| talking_about(docs.iter().copied(), t))
            .collect();
        let open = |hit: &crate::host::graphics::docs::index::Named| Page::Doc {
            path: hit.path.clone(),
            anchor: hit.heading.as_deref().map(slugify),
        };
        let page = if let Some(hits) = searches.iter().find(|h| h.first().is_some_and(|x| x.named))
        {
            open(&hits[0])
        } else {
            let file = format!(
                "{}.md",
                name.rsplit('.').next().unwrap_or(&name).to_lowercase()
            );
            let mut paths: Vec<&String> = self.documents.keys().collect();
            paths.sort();
            match paths
                .into_iter()
                .find(|d| d.to_lowercase().rsplit('/').next() == Some(file.as_str()))
            {
                Some(path) => Page::Doc {
                    path: path.clone(),
                    anchor: None,
                },
                None => match searches.iter().find(|h| !h.is_empty()) {
                    Some(hits) => open(&hits[0]),
                    None => Page::Text {
                        title: if fullname.is_empty() {
                            name.clone()
                        } else {
                            fullname
                        },
                        text: text_of("text"),
                        source: text_of("source"),
                    },
                },
            }
        };
        self.go(page);
    }

    fn paint(&self, d: &mut Draw, ctx: &Ctx) {
        let g = self.geometry(ctx.rect, ctx.scale, ctx.metrics, ctx.header);
        let (scroll, laid) = self.settle(&g);
        let kept = d.mesh.clip();
        let clip = |r: Rect| kept.map_or(r, |k| k.intersect(r));
        // what the strip may draw on: its own rect, and the header's slot
        let strip_clip = ctx.header.map_or(ctx.rect, |h| h.union(ctx.rect));
        {
            let (mesh, m, theme) = d.parts();
            mesh.set_clip(Some(clip(ctx.rect)));
            mesh.rect(ctx.rect, theme.background);
            // the page
            mesh.set_clip(Some(clip(g.page)));
            let (left, top) = (g.text.x, g.text.y - scroll);
            let marks = self.matches(&laid);
            for item in &laid.items {
                if item.bottom() + top < g.page.y || item.top() + top > g.page.y + g.page.h {
                    continue;
                }
                match item {
                    Item::Fill { rect, ink } => mesh.rect(
                        Rect::new(left + rect.x, top + rect.y, rect.w, rect.h),
                        ink_color(theme, *ink),
                    ),
                    Item::Text {
                        x,
                        y,
                        text,
                        family,
                        size,
                        ink,
                        underline,
                        strike,
                    } => {
                        let color = ink_color(theme, *ink);
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
            for (i, (x, y, w, h)) in marks.iter().enumerate() {
                let color = if i == self.current {
                    theme.find_current
                } else {
                    theme.find_match
                };
                mesh.rect(Rect::new(left + x, top + y, *w, *h), color);
            }
            // the scroll bar, where there is more than is seen
            let total = laid.height + g.text.h * 0.5;
            if total > g.text.h + 0.5 {
                let bar = (m.pad * 0.75).max(3.0 * ctx.scale).round();
                let h = (g.page.h * g.text.h / total).max(m.pad * 2.0);
                let y = g.page.y + (g.page.h - h) * (scroll / (total - g.text.h)).min(1.0);
                mesh.rect(
                    Rect::new(g.page.x + g.page.w - bar, y, bar, h),
                    theme.separator,
                );
            }
            // the strip along the top, or in the window's header, whose
            // ground and line the header draws
            mesh.set_clip(Some(clip(strip_clip)));
            if ctx.header.is_none() {
                mesh.rect(g.strip, theme.status_bar);
                mesh.rect(
                    Rect::new(
                        g.strip.x,
                        g.strip.y + g.strip.h - m.divider_w,
                        g.strip.w,
                        m.divider_w,
                    ),
                    theme.separator,
                );
            }
            let (book, label) = self.trail();
            let ty = g.trail.y + (g.trail.h - font::height(g.ui)) * 0.5;
            let mut x = g.trail.x + m.pad;
            if let Some(b) = &book {
                let w = font::width(b, g.ui);
                font::text_ellipsis(
                    mesh,
                    b,
                    x,
                    ty,
                    (g.trail.w - m.pad).max(0.0),
                    g.ui,
                    theme.text,
                );
                x += w + m.pad;
                if !label.is_empty() {
                    let sep = "\u{203A}";
                    font::text(mesh, sep, x, ty, g.ui, theme.text_dim);
                    x += font::width(sep, g.ui) + m.pad;
                }
            }
            let room = (g.trail.x + g.trail.w - x).max(0.0);
            font::text_ellipsis(mesh, &label, x, ty, room, g.ui, theme.text);
            if let Some(bar) = g.bar {
                mesh.rect(bar, theme.status_bar);
                mesh.rect(
                    Rect::new(bar.x, bar.y + bar.h - m.divider_w, bar.w, m.divider_w),
                    theme.separator,
                );
            }
            if let Some(r) = g.count {
                let total = marks.len();
                let text = if total > 0 {
                    format!("{}/{total}", self.current.min(total - 1) + 1)
                } else if self.find.trim().is_empty() {
                    String::new()
                } else {
                    "0/0".to_string()
                };
                let w = font::width(&text, g.ui);
                font::text(
                    mesh,
                    &text,
                    r.x + (r.w - w) * 0.5,
                    r.y + (r.h - font::height(g.ui)) * 0.5,
                    g.ui,
                    theme.text_dim,
                );
            }
        }
        let flat = ButtonLook {
            active: false,
            hovered: false,
            flat: true,
        };
        d.mesh.set_clip(Some(clip(strip_clip)));
        for (part, r) in &g.parts {
            let inset = |r: &Rect| {
                let p = (d.m.pad * 0.5).round();
                Rect::new(
                    r.x + p,
                    r.y + p,
                    (r.w - 2.0 * p).max(1.0),
                    (r.h - 2.0 * p).max(1.0),
                )
            };
            match part {
                Part::Back
                | Part::Find
                | Part::Forward
                | Part::Home
                | Part::Previous
                | Part::Next
                | Part::Close => {
                    let (symbol, live) = match part {
                        Part::Back => (
                            controls::icon("arrow-left", font::symbol::LEFT),
                            self.pos > 0,
                        ),
                        Part::Forward => (
                            controls::icon("arrow-right", font::symbol::RIGHT),
                            self.pos + 1 < self.history.len(),
                        ),
                        Part::Home => (controls::icon("house", '\u{2302}'), true),
                        Part::Find => (controls::icon("search", '?'), true),
                        Part::Previous => (controls::icon("chevron-up", font::symbol::UP), true),
                        Part::Next => (controls::icon("chevron-down", font::symbol::DOWN), true),
                        _ => (controls::icon("x", font::symbol::CLOSE), true),
                    };
                    if live {
                        controls::button(d, None, Some(symbol), inset(r), flat, g.ui);
                    } else {
                        let (mesh, _, theme) = d.parts();
                        font::text_centered(
                            mesh,
                            &symbol.to_string(),
                            inset(r),
                            g.ui,
                            theme.text_disabled,
                        );
                    }
                }
                Part::InPage | Part::Everywhere => {
                    let (on, label) = if *part == Part::InPage {
                        (self.scope == Scope::Page, "Page")
                    } else {
                        (self.scope == Scope::All, "All documents")
                    };
                    controls::toggle(
                        d,
                        on,
                        Some(label),
                        None,
                        inset(r),
                        ToggleView::Button,
                        false,
                        g.ui,
                    );
                }
                Part::Field => {
                    let caret = (ctx.focused && self.find_focus).then_some(self.caret);
                    controls::field(d, &self.find, None, *r, g.ui, false, caret);
                }
                Part::Book => {}
            }
        }
        d.mesh.set_clip(kept);
    }
}

/// The color an ink is drawn in: its theme role.
fn ink_color(theme: &Theme, ink: Ink) -> Color {
    match ink {
        Ink::Text => theme.text,
        Ink::Dim => theme.text_dim,
        Ink::Code(None) => theme.text,
        Ink::Code(Some(kind)) => crate::host::elements::code::role(theme, kind),
        Ink::CodeGround => theme.field,
        Ink::Line => theme.separator,
    }
}

impl Element for DocsView {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "books" => structure(v)
                .and_then(|v| v.as_array().map(|b| b.iter().map(book).collect()))
                .map(|books| {
                    self.books = books;
                    self.laid.replace(None);
                })
                .is_some(),
            "documents" => structure(v)
                .and_then(|v| v.as_object().cloned())
                .map(|docs| {
                    for (path, text) in docs {
                        match text.as_str() {
                            Some(t) => {
                                self.documents.insert(path, t.to_string());
                            }
                            None => {
                                self.documents.remove(&path);
                            }
                        }
                    }
                    self.index.replace(None);
                    self.laid.replace(None);
                })
                .is_some(),
            "open" => v
                .as_str()
                .map(|target| {
                    if target.is_empty() {
                        self.go(Page::Home);
                    } else {
                        let (path, anchor) = match target.split_once('#') {
                            Some((p, a)) => (p.to_string(), Some(a.to_string())),
                            None => (target.to_string(), None),
                        };
                        self.go(Page::Doc { path, anchor });
                    }
                })
                .is_some(),
            "help" => structure(v).map(|v| self.help(&v)).is_some(),
            "text_size" => v
                .as_f64()
                .filter(|n| *n > 0.0)
                .map(|n| self.text_size = n as f32)
                .is_some(),
            "id" | "type" | "name" => true,
            _ => false,
        }
    }

    /// What a set does rather than keeps: going somewhere, asking for help.
    fn writes(&self, key: &str) -> bool {
        matches!(key, "open" | "help")
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
        let page = match self.page() {
            Page::Home => "home".to_string(),
            Page::Book(p) => format!("book:{p}"),
            Page::Doc { path, anchor } => match anchor {
                Some(a) => format!("doc:{path}#{a}"),
                None => format!("doc:{path}"),
            },
            Page::Text { title, .. } => format!("text:{title}"),
            Page::Search { query, .. } => format!("search:{query}"),
        };
        let results = match self.page() {
            Page::Search { hits, .. } => hits.len(),
            _ => 0,
        };
        vec![
            ("page".into(), Value::from(page)),
            ("results".into(), Value::from(results)),
            ("found".into(), Value::from(self.match_count())),
            ("documents".into(), Value::from(self.documents.len())),
        ]
    }

    /// Its way back, its start page, where the page is and find: a strip a
    /// window may take as its header.
    fn strip(&self) -> bool {
        true
    }

    fn accepts_focus(&self) -> bool {
        true
    }

    fn takes_text(&self) -> bool {
        self.find_focus
    }

    fn press(&mut self, at: (f64, f64), input: &Input) -> Claim {
        let g = self.geometry(input.rect, input.scale, input.metrics, input.header);
        if let Some(&(part, r)) = g.parts.iter().find(|(_, r)| r.contains(at.0, at.1)) {
            self.find_focus = part == Part::Field;
            match part {
                Part::Back => {
                    self.step(-1);
                }
                Part::Forward => {
                    self.step(1);
                }
                Part::Home => self.go(Page::Home),
                Part::Book => {
                    if let Page::Doc { path, .. } = self.page()
                        && let Some(prefix) = self.book_of(path).map(|b| b.prefix.clone())
                    {
                        self.go(Page::Book(prefix));
                    }
                }
                Part::Find => self.open_find(Scope::Page),
                Part::Field => {
                    let pos = controls::caret_at(
                        r,
                        &self.find,
                        false,
                        g.ui,
                        false,
                        self.caret,
                        at.0,
                        at.1,
                        input.metrics,
                    );
                    self.caret = Caret { pos, anchor: None };
                }
                Part::Previous => self.step_match(-1, self.match_count()),
                Part::Next => self.step_match(1, self.match_count()),
                Part::InPage => {
                    self.scope = Scope::Page;
                    if matches!(self.page(), Page::Search { .. }) {
                        self.step(-1);
                    }
                    self.reveal_match.set(true);
                }
                Part::Everywhere => {
                    self.scope = Scope::All;
                    self.search_all();
                }
                Part::Close => {
                    self.find_open = false;
                    self.find_focus = false;
                }
            }
            return Claim::take();
        }
        self.find_focus = false;
        if !g.page.contains(at.0, at.1) {
            return Claim::take();
        }
        let (scroll, laid) = self.settle(&g);
        let (px, py) = (at.0 as f32 - g.text.x, at.1 as f32 - g.text.y + scroll);
        if let Some(code) = laid.copy_at(px, py) {
            self.copy = Some(code.to_string());
            return Claim::events(Events::none().and_command("copy"));
        }
        if let Some(href) = laid.link_at(px, py).map(str::to_string) {
            return Claim::events(self.follow(&href));
        }
        Claim::take()
    }

    fn wheel(&mut self, _at: (f64, f64), delta: (f64, f64), input: &Input) -> Option<Events> {
        // Ctrl+wheel is the size of the text, not a scroll: declined, the
        // window's own zoom takes it.
        if input.mods.ctrl {
            return None;
        }
        let g = self.geometry(input.rect, input.scale, input.metrics, input.header);
        let line = font::line_advance(g.size) * 1.4;
        let before = self.scroll.get();
        self.scroll
            .set(before - delta.1 as f32 * WHEEL_LINES * line);
        let (after, _) = self.settle(&g);
        (after != before).then(Events::none)
    }

    fn key(&mut self, key: &Key, input: &mut KeyInput) -> Option<Events> {
        let mods = input.mods;
        match key {
            Key::Char(c) if mods.ctrl && !mods.alt && c.eq_ignore_ascii_case(&'f') => {
                self.open_find(if mods.shift { Scope::All } else { Scope::Page });
                return Some(Events::none());
            }
            Key::Left if mods.alt => {
                self.step(-1);
                return Some(Events::none());
            }
            Key::Right if mods.alt => {
                self.step(1);
                return Some(Events::none());
            }
            Key::Escape if self.find_open => {
                self.find_open = false;
                self.find_focus = false;
                return Some(Events::none());
            }
            _ => {}
        }
        if self.find_focus {
            if *key == Key::Enter {
                match self.scope {
                    Scope::All => self.search_all(),
                    Scope::Page => {
                        self.step_match(if mods.shift { -1 } else { 1 }, self.match_count())
                    }
                }
                return Some(Events::none());
            }
            let changed = edit_key(&mut self.find, &mut self.caret, false, key, input)?;
            if changed {
                self.current = 0;
                self.reveal_match.set(true);
            }
            return Some(Events::none());
        }
        let page = self
            .laid
            .borrow()
            .as_ref()
            .map_or(400.0, |(_, w, _, _)| *w)
            .max(1.0);
        let line = font::line_advance(self.text_size) * 1.4;
        let s = self.scroll.get();
        let to = match key {
            Key::Up => s - line,
            Key::Down => s + line,
            Key::PageUp => s - page * 0.8,
            Key::PageDown => s + page * 0.8,
            Key::Home => 0.0,
            Key::End => f32::MAX,
            _ => return None,
        };
        self.scroll.set(to.max(0.0));
        Some(Events::none())
    }

    /// The code a copy button was pressed on, to the clipboard.
    fn verb(&mut self, verb: Verb, input: &mut KeyInput) -> Option<Events> {
        match verb {
            Verb::Copy => {
                if let Some(code) = self.copy.take() {
                    input.clipboard.set_text(&code);
                }
                Some(Events::none())
            }
            _ => None,
        }
    }

    /// The panel's commands, for a menu or a verb.
    fn command(&mut self, name: &str, _input: &mut KeyInput) -> Option<Events> {
        match name {
            "find" => self.open_find(Scope::Page),
            "search" => self.open_find(Scope::All),
            "back" => {
                self.step(-1);
            }
            "forward" => {
                self.step(1);
            }
            "home" => self.go(Page::Home),
            _ => return None,
        }
        Some(Events::none())
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests;
