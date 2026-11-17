//! **A Markdown document read once into blocks**: headings, paragraphs,
//! lists, quotes, tables, code blocks, rules and images, their text in runs
//! of one style each -- strong, emphasized, code, a link.
//!
//! The reading is pulldown-cmark's, with the extensions a book writes
//! (tables, strikethrough, task lists). Raw HTML is not drawn, as the
//! current editor's sanitizer would draw little of it; an anchor written as
//! `<a id="...">`, which an API reference puts before each entry, is kept
//! as a place a link can land on.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// A run of text in one style.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Run {
    pub text: String,
    pub strong: bool,
    pub emph: bool,
    pub code: bool,
    pub strike: bool,
    /// Where it links to, as written.
    pub link: Option<String>,
}

/// How a table's column is aligned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// One block of a document.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    /// A heading, its level from 1, and the id a link lands on.
    Heading {
        level: u8,
        runs: Vec<Run>,
        id: String,
    },
    Para(Vec<Run>),
    /// A code block, the language its fence names.
    Code {
        lang: String,
        text: String,
    },
    /// A list, numbered from `start` or not; each item is blocks.
    List {
        start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    Quote(Vec<Block>),
    Table {
        aligns: Vec<Align>,
        head: Vec<Vec<Run>>,
        rows: Vec<Vec<Vec<Run>>>,
    },
    Rule,
    /// An image, which is drawn as its description.
    Image {
        url: String,
        alt: String,
    },
    /// A place a link can land on, with no text of its own.
    Anchor(String),
}

/// **A heading's id**, as GitHub and mdBook make one: lower case, letters,
/// digits, `_` and `-` kept, spaces turned into `-`.
pub fn slugify(s: &str) -> String {
    let kept: String = s
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '_' || *c == '-')
        .collect();
    kept.split_whitespace().collect::<Vec<_>>().join("-")
}

/// The text of `runs`, as plain text.
pub fn plain(runs: &[Run]) -> String {
    runs.iter().map(|r| r.text.as_str()).collect()
}

/// The id an `<a id="...">` names.
fn anchor_of(html: &str) -> Option<String> {
    let rest = html.trim_start().strip_prefix("<a")?;
    let at = rest.find("id=")? + 3;
    let rest = &rest[at..];
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let end = rest[1..].find(quote)?;
    Some(rest[1..1 + end].to_string())
}

/// What is being read: the blocks of each container open, innermost last.
struct Reader {
    /// The containers open: a list's items, a quote, an item.
    stack: Vec<Frame>,
    /// The runs of the paragraph, heading or cell being read.
    runs: Vec<Run>,
    style: Run,
    /// The link open, if any.
    links: Vec<String>,
    /// The table being read.
    table: Option<TableRead>,
    /// The code block being read.
    code: Option<(String, String)>,
    /// The image being read: its address and its description so far.
    image: Option<(String, String)>,
}

enum Frame {
    Root(Vec<Block>),
    List {
        start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    Item(Vec<Block>),
    Quote(Vec<Block>),
}

struct TableRead {
    aligns: Vec<Align>,
    head: Vec<Vec<Run>>,
    rows: Vec<Vec<Vec<Run>>>,
    in_head: bool,
}

impl Reader {
    fn push(&mut self, block: Block) {
        match self.stack.last_mut() {
            Some(Frame::Root(b) | Frame::Item(b) | Frame::Quote(b)) => b.push(block),
            Some(Frame::List { items, .. }) => {
                // text straight in a list (a tight item's) is an item's
                items.push(vec![block]);
            }
            None => {}
        }
    }

    fn text(&mut self, s: &str) {
        if let Some((_, alt)) = self.image.as_mut() {
            alt.push_str(s);
            return;
        }
        if let Some((_, text)) = self.code.as_mut() {
            text.push_str(s);
            return;
        }
        let run = Run {
            text: s.to_string(),
            link: self.links.last().cloned(),
            ..self.style.clone()
        };
        match self.runs.last_mut() {
            Some(last)
                if last.strong == run.strong
                    && last.emph == run.emph
                    && last.code == run.code
                    && last.strike == run.strike
                    && last.link == run.link =>
            {
                last.text.push_str(s);
            }
            _ => self.runs.push(run),
        }
    }

    /// The runs read so far, as a paragraph, when there are any: text a tight
    /// list item holds with no paragraph around it.
    fn flush(&mut self) {
        if !self.runs.is_empty() && self.table.is_none() {
            let runs = std::mem::take(&mut self.runs);
            self.push(Block::Para(runs));
        }
    }
}

/// **`source` read into blocks.**
pub fn parse(source: &str) -> Vec<Block> {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut r = Reader {
        stack: vec![Frame::Root(Vec::new())],
        runs: Vec::new(),
        style: Run::default(),
        links: Vec::new(),
        table: None,
        code: None,
        image: None,
    };
    for event in Parser::new_ext(source, options) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph | Tag::Heading { .. } => r.flush(),
                Tag::BlockQuote(_) => {
                    r.flush();
                    r.stack.push(Frame::Quote(Vec::new()));
                }
                Tag::CodeBlock(kind) => {
                    r.flush();
                    let lang = match kind {
                        CodeBlockKind::Fenced(info) => {
                            info.split_whitespace().next().unwrap_or("").to_lowercase()
                        }
                        CodeBlockKind::Indented => String::new(),
                    };
                    r.code = Some((lang, String::new()));
                }
                Tag::List(start) => {
                    r.flush();
                    r.stack.push(Frame::List {
                        start,
                        items: Vec::new(),
                    });
                }
                Tag::Item => {
                    r.flush();
                    r.stack.push(Frame::Item(Vec::new()));
                }
                Tag::Table(aligns) => {
                    r.flush();
                    r.table = Some(TableRead {
                        aligns: aligns
                            .iter()
                            .map(|a| match a {
                                Alignment::Center => Align::Center,
                                Alignment::Right => Align::Right,
                                _ => Align::Left,
                            })
                            .collect(),
                        head: Vec::new(),
                        rows: Vec::new(),
                        in_head: false,
                    });
                }
                Tag::TableHead => {
                    if let Some(t) = r.table.as_mut() {
                        t.in_head = true;
                    }
                }
                Tag::TableRow => {
                    if let Some(t) = r.table.as_mut() {
                        t.rows.push(Vec::new());
                    }
                }
                Tag::TableCell => r.runs.clear(),
                Tag::Emphasis => r.style.emph = true,
                Tag::Strong => r.style.strong = true,
                Tag::Strikethrough => r.style.strike = true,
                Tag::Link { dest_url, .. } => r.links.push(dest_url.to_string()),
                Tag::Image { dest_url, .. } => {
                    r.flush();
                    r.image = Some((dest_url.to_string(), String::new()));
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => r.flush(),
                TagEnd::Heading(level) => {
                    let runs = std::mem::take(&mut r.runs);
                    let id = slugify(&plain(&runs));
                    let level = match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        HeadingLevel::H3 => 3,
                        HeadingLevel::H4 => 4,
                        HeadingLevel::H5 => 5,
                        HeadingLevel::H6 => 6,
                    };
                    r.push(Block::Heading { level, runs, id });
                }
                TagEnd::BlockQuote(_) => {
                    r.flush();
                    if let Some(Frame::Quote(blocks)) = r.stack.pop() {
                        r.push(Block::Quote(blocks));
                    }
                }
                TagEnd::CodeBlock => {
                    if let Some((lang, mut text)) = r.code.take() {
                        if text.ends_with('\n') {
                            text.pop();
                        }
                        r.push(Block::Code { lang, text });
                    }
                }
                TagEnd::List(_) => {
                    r.flush();
                    if let Some(Frame::List { start, items }) = r.stack.pop() {
                        r.push(Block::List { start, items });
                    }
                }
                TagEnd::Item => {
                    r.flush();
                    if let Some(Frame::Item(blocks)) = r.stack.pop()
                        && let Some(Frame::List { items, .. }) = r.stack.last_mut()
                    {
                        items.push(blocks);
                    }
                }
                TagEnd::TableHead => {
                    if let Some(t) = r.table.as_mut() {
                        t.in_head = false;
                    }
                }
                TagEnd::TableCell => {
                    let runs = std::mem::take(&mut r.runs);
                    if let Some(t) = r.table.as_mut() {
                        if t.in_head {
                            t.head.push(runs);
                        } else if let Some(row) = t.rows.last_mut() {
                            row.push(runs);
                        }
                    }
                }
                TagEnd::Table => {
                    if let Some(t) = r.table.take() {
                        r.push(Block::Table {
                            aligns: t.aligns,
                            head: t.head,
                            rows: t.rows.into_iter().filter(|row| !row.is_empty()).collect(),
                        });
                    }
                }
                TagEnd::Emphasis => r.style.emph = false,
                TagEnd::Strong => r.style.strong = false,
                TagEnd::Strikethrough => r.style.strike = false,
                TagEnd::Link => {
                    r.links.pop();
                }
                TagEnd::Image => {
                    if let Some((url, alt)) = r.image.take() {
                        r.push(Block::Image { url, alt });
                    }
                }
                _ => {}
            },
            Event::Text(t) => r.text(&t),
            Event::Code(t) => {
                r.style.code = true;
                r.text(&t);
                r.style.code = false;
            }
            Event::SoftBreak => r.text(" "),
            Event::HardBreak => r.text("\n"),
            Event::Rule => {
                r.flush();
                r.push(Block::Rule);
            }
            Event::TaskListMarker(done) => r.text(if done { "[x] " } else { "[ ] " }),
            Event::Html(html) | Event::InlineHtml(html) => {
                if let Some(id) = anchor_of(&html) {
                    r.flush();
                    r.push(Block::Anchor(id));
                }
            }
            _ => {}
        }
    }
    r.flush();
    match r.stack.into_iter().next() {
        Some(Frame::Root(blocks)) => blocks,
        _ => Vec::new(),
    }
}
