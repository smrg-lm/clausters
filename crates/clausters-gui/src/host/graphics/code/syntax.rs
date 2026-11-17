//! **The code's syntax, kept as a tree**: what each part of the text is,
//! for the color it is drawn in.
//!
//! The text is parsed once into a tree, and every edit is told to the tree
//! as it is made -- where it was, what it removed, what it inserted -- so
//! the next parse reads again only what the edit touched. The parse itself
//! is asked for after the edits ([`Syntax::parse`]), so an undo of a run of
//! typing, or an indentation of many lines, is one parse.
//!
//! **A parse can be given a time, and continued.** What an edit costs to
//! parse again is not only the lines it touched: the parser walks past every
//! statement at the top of the file to reuse it, and a text left broken by a
//! half-typed line (an open quote, an open bracket) costs it a search for
//! the reading that recovers. In a long file that is several milliseconds,
//! more than a key may take. So a parse stops when its time is up and the
//! next one carries on where it stopped; meanwhile the tree is the one
//! before, its places moved by the edits, and the text keeps the colors it
//! had until the parse catches up.
//!
//! What each part is comes from the grammar's own highlighting query, asked
//! only over the bytes a view shows. The query's names (`keyword`,
//! `function`, `string`, ...) become a [`Kind`] each, and the element draws
//! a kind in its theme role (`syntax_keyword`, ...).
//!
//! **Which name wins.** A query's patterns overlap: one node can be named by
//! several, and a node inside another by its own. The later pattern names a
//! node over an earlier one -- the grammars write their general patterns
//! first and the exceptions after: `(identifier) @variable`, then an
//! identifier that starts with a capital -- and a node inside another is
//! named over it, as an escape inside a string is -- but for a regular
//! expression, which is one color whole.
//!
//! A build without the `highlight` feature keeps the same surface and
//! answers no spans: code is drawn in one color.

use super::Language;

/// **What a part of the code is**, as far as its color goes. Each is a
/// theme role of its own, named `syntax_` and the kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A keyword, and a constant the language names (`None`, `true`, `this`).
    Keyword,
    /// A keyword that moves control or brings a module in (`if`, `return`,
    /// `import`).
    Control,
    /// A name: a variable, a property, a parameter.
    Name,
    /// A function's name, defined or called.
    Function,
    /// A type, and a name that starts with a capital.
    Type,
    Number,
    String,
    /// A regular expression written as a literal.
    Regexp,
    Comment,
    /// An operator, punctuation, an escape inside a string.
    Operator,
}

impl Kind {
    /// The kind a query's capture name stands for; a name with none
    /// (`embedded`) leaves what is under it as it was.
    #[cfg_attr(not(feature = "highlight"), allow(dead_code))]
    fn of_capture(name: &str) -> Option<Kind> {
        Some(match name {
            "string.special" => Kind::Regexp,
            "constant.builtin" | "variable.builtin" => Kind::Keyword,
            _ => match name.split('.').next().unwrap_or(name) {
                "keyword" => Kind::Keyword,
                "variable" | "property" | "constant" => Kind::Name,
                "function" => Kind::Function,
                "type" | "constructor" => Kind::Type,
                "number" => Kind::Number,
                "string" => Kind::String,
                "comment" => Kind::Comment,
                "operator" | "punctuation" | "escape" => Kind::Operator,
                _ => return None,
            },
        })
    }
}

/// **A run of the text of one kind**: bytes `start..end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}

/// **What the tree says of a bracket**: whether it is the code's, and the
/// one it pairs with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bracket {
    /// Punctuation of the code, paired with the bracket at this byte, or
    /// with none.
    Code(Option<usize>),
    /// A character of a string, a comment or a pattern: not a bracket.
    Text,
}

/// Whether `kind`, a node's, is text rather than code: a string, a part of
/// one, a comment.
#[cfg_attr(not(feature = "highlight"), allow(dead_code))]
fn is_text_node(kind: &str) -> bool {
    kind.contains("string") || kind.contains("comment")
}

/// Whether keyword `word` moves control or brings a module in: the keywords
/// the grammars name as one, told apart by what they say.
#[cfg_attr(not(feature = "highlight"), allow(dead_code))]
fn control(language: Language, word: &str) -> bool {
    match language {
        Language::Python => matches!(
            word,
            "if" | "elif"
                | "else"
                | "for"
                | "while"
                | "try"
                | "except"
                | "finally"
                | "return"
                | "raise"
                | "break"
                | "continue"
                | "with"
                | "pass"
                | "assert"
                | "await"
                | "yield"
                | "match"
                | "case"
                | "import"
                | "from"
        ),
        Language::JavaScript => matches!(
            word,
            "if" | "else"
                | "switch"
                | "case"
                | "default"
                | "for"
                | "while"
                | "do"
                | "break"
                | "continue"
                | "return"
                | "throw"
                | "try"
                | "catch"
                | "finally"
                | "yield"
                | "await"
                | "import"
                | "export"
                | "from"
        ),
        Language::Plain => false,
    }
}

#[cfg(feature = "highlight")]
mod tree {
    use std::ops::ControlFlow;
    use std::sync::OnceLock;
    use std::time::Duration;

    use tree_sitter::{
        InputEdit, ParseOptions, ParseState, Parser, Point, Query, QueryCursor, StreamingIterator,
        Tree,
    };
    use web_time::Instant;

    use super::{Bracket, Kind, Language, Span, control, is_text_node};

    /// A grammar's highlighting query and the kind each of its names is.
    struct Highlights {
        query: Query,
        kinds: Vec<Option<Kind>>,
        keyword: Option<u32>,
    }

    impl Highlights {
        fn new(grammar: &tree_sitter::Language, source: &str) -> Option<Highlights> {
            let query = Query::new(grammar, source)
                .map_err(|e| tracing::warn!("code: a highlighting query does not read: {e}"))
                .ok()?;
            let kinds = query
                .capture_names()
                .iter()
                .map(|n| Kind::of_capture(n))
                .collect();
            let keyword = query.capture_index_for_name("keyword");
            Some(Highlights {
                query,
                kinds,
                keyword,
            })
        }
    }

    fn grammar(language: Language) -> Option<tree_sitter::Language> {
        match language {
            Language::Python => Some(tree_sitter_python::LANGUAGE.into()),
            Language::JavaScript => Some(tree_sitter_javascript::LANGUAGE.into()),
            Language::Plain => None,
        }
    }

    /// The language's query, read once for the process.
    fn highlights(language: Language) -> Option<&'static Highlights> {
        static PYTHON: OnceLock<Option<Highlights>> = OnceLock::new();
        static JAVASCRIPT: OnceLock<Option<Highlights>> = OnceLock::new();
        let (cell, source) = match language {
            Language::Python => (&PYTHON, tree_sitter_python::HIGHLIGHTS_QUERY),
            Language::JavaScript => (&JAVASCRIPT, tree_sitter_javascript::HIGHLIGHT_QUERY),
            Language::Plain => return None,
        };
        cell.get_or_init(|| Highlights::new(&grammar(language)?, source))
            .as_ref()
    }

    /// Where byte `at` of `text` is, as a row and a byte column.
    fn point(text: &str, at: usize) -> Point {
        let before = &text.as_bytes()[..at];
        let row = before.iter().filter(|b| **b == b'\n').count();
        let column = at
            - before
                .iter()
                .rposition(|b| *b == b'\n')
                .map_or(0, |i| i + 1);
        Point { row, column }
    }

    /// `from` moved past `s`.
    fn advance(from: Point, s: &str) -> Point {
        match s.rfind('\n') {
            Some(i) => Point {
                row: from.row + s.bytes().filter(|b| *b == b'\n').count(),
                column: s.len() - i - 1,
            },
            None => Point {
                row: from.row,
                column: from.column + s.len(),
            },
        }
    }

    /// The tree of a text in one language, and the parser that keeps it.
    pub struct Syntax {
        language: Language,
        parser: Option<Parser>,
        tree: Option<Tree>,
        /// Whether an edit was told to the tree since it was last parsed.
        stale: bool,
        /// Whether a parse stopped at its time and the parser holds where.
        stopped: bool,
    }

    impl Syntax {
        pub fn new(language: Language) -> Syntax {
            let parser = grammar(language).and_then(|g| {
                let mut parser = Parser::new();
                parser.set_language(&g).ok()?;
                Some(parser)
            });
            Syntax {
                language,
                parser,
                tree: None,
                stale: true,
                stopped: false,
            }
        }

        pub fn language(&self) -> Language {
            self.language
        }

        /// The text replaced whole: the next parse reads it afresh.
        pub fn reset(&mut self) {
            self.tree = None;
            self.stale = true;
            self.restart();
        }

        /// A parse that stopped is not carried on: the text it read changed.
        fn restart(&mut self) {
            if std::mem::take(&mut self.stopped)
                && let Some(parser) = self.parser.as_mut()
            {
                parser.reset();
            }
        }

        /// Whether the tree is behind the text: a parse is still to do.
        pub fn parsing(&self) -> bool {
            self.stale && self.parser.is_some()
        }

        /// **An edit, told to the tree**: at byte `at` of `before` (the text
        /// as it was), `removed` became `inserted`.
        pub fn edit(&mut self, before: &str, at: usize, removed: &str, inserted: &str) {
            self.stale = true;
            self.restart();
            let Some(tree) = self.tree.as_mut() else {
                return;
            };
            let start = point(before, at);
            tree.edit(&InputEdit {
                start_byte: at,
                old_end_byte: at + removed.len(),
                new_end_byte: at + inserted.len(),
                start_position: start,
                old_end_position: advance(start, removed),
                new_end_position: advance(start, inserted),
            });
        }

        /// **The tree brought up to `text`**, within `budget` if one is
        /// given: answers whether it is, or whether the parse stopped at its
        /// time, to be carried on by the next call.
        pub fn parse(&mut self, text: &str, budget: Option<Duration>) -> bool {
            if !self.stale {
                return true;
            }
            let Some(parser) = self.parser.as_mut() else {
                self.stale = false;
                return true;
            };
            let started = Instant::now();
            let mut time = |_: &ParseState| match budget {
                Some(b) if started.elapsed() >= b => ControlFlow::Break(()),
                _ => ControlFlow::Continue(()),
            };
            let bytes = text.as_bytes();
            let mut read = |at: usize, _: Point| bytes.get(at..).unwrap_or_default();
            let options = ParseOptions::new().progress_callback(&mut time);
            match parser.parse_with_options(&mut read, self.tree.as_ref(), Some(options)) {
                Some(tree) => {
                    self.tree = Some(tree);
                    self.stale = false;
                    self.stopped = false;
                    true
                }
                None => {
                    self.stopped = true;
                    false
                }
            }
        }

        /// **What each part of `text` from byte `from` to `to` is**, by the
        /// tree as it stands: runs of one kind each, in order and apart, the
        /// bytes nothing names left out.
        pub fn spans(&self, text: &str, from: usize, to: usize) -> Vec<Span> {
            let (Some(tree), Some(h)) = (self.tree.as_ref(), highlights(self.language)) else {
                return Vec::new();
            };
            let to = to.min(text.len());
            if from >= to {
                return Vec::new();
            }
            let mut named = Vec::new();
            let mut cursor = QueryCursor::new();
            cursor.set_byte_range(from..to);
            let mut matches = cursor.matches(&h.query, tree.root_node(), text.as_bytes());
            while let Some(m) = matches.next() {
                for c in m.captures() {
                    let Some(mut kind) = h.kinds[c.index as usize] else {
                        continue;
                    };
                    let (a, b) = (c.node.start_byte(), c.node.end_byte());
                    // a tree behind the text may name a place an edit split
                    if Some(c.index) == h.keyword
                        && text.get(a..b).is_some_and(|w| control(self.language, w))
                    {
                        kind = Kind::Control;
                    }
                    named.push((a, b, m.pattern_index, kind));
                }
            }
            // outer before inner, and of one node the later pattern last:
            // what is painted last is what the bytes are. A regular
            // expression is one color whole, its slashes too, which the
            // grammar also names as operators.
            named.sort_by(|x, y| {
                (x.3 == Kind::Regexp)
                    .cmp(&(y.3 == Kind::Regexp))
                    .then(x.0.cmp(&y.0))
                    .then(y.1.cmp(&x.1))
                    .then(x.2.cmp(&y.2))
            });
            let mut by_byte = vec![None; to - from];
            for (a, b, _, kind) in named {
                let (a, b) = (a.max(from), b.min(to));
                if a < b {
                    by_byte[a - from..b - from].fill(Some(kind));
                }
            }
            let mut out = Vec::new();
            let mut i = 0;
            while i < by_byte.len() {
                let kind = by_byte[i];
                let mut j = i + 1;
                while j < by_byte.len() && by_byte[j] == kind {
                    j += 1;
                }
                if let Some(kind) = kind {
                    out.push(Span {
                        start: from + i,
                        end: from + j,
                        kind,
                    });
                }
                i = j;
            }
            out
        }

        /// The tree, when it is the text's: none while an edit is still to
        /// be parsed, since what was just typed is not in it.
        fn fresh(&self) -> Option<&Tree> {
            if self.stale {
                return None;
            }
            self.tree.as_ref()
        }

        /// **The bracket at byte `at`, by the tree**: the code's, with the
        /// one it pairs with, or a character of a string or a comment. None
        /// when the tree cannot say: there is none, or it is behind the text.
        ///
        /// A bracket pairs with the first of its closing kind among the
        /// nodes beside it, the nested pairs counted: a pair is the start
        /// and the end of one node's children.
        pub fn bracket(&self, text: &str, at: usize) -> Option<Bracket> {
            let tree = self.fresh()?;
            let c = *text.as_bytes().get(at)?;
            let node = tree.root_node().descendant_for_byte_range(at, at + 1)?;
            if node.is_named() || node.start_byte() != at || node.end_byte() != at + 1 {
                return Some(Bracket::Text);
            }
            let (open, close, forward) = match c {
                b'(' => (b'(', b')', true),
                b'[' => (b'[', b']', true),
                b'{' => (b'{', b'}', true),
                b')' => (b'(', b')', false),
                b']' => (b'[', b']', false),
                b'}' => (b'{', b'}', false),
                _ => return Some(Bracket::Text),
            };
            let Some(parent) = node.parent() else {
                return Some(Bracket::Code(None));
            };
            let mut cursor = parent.walk();
            let children: Vec<_> = parent.children(&mut cursor).collect();
            let Some(i) = children.iter().position(|n| n.id() == node.id()) else {
                return Some(Bracket::Code(None));
            };
            let kind_of = |n: &tree_sitter::Node| {
                (!n.is_named() && !n.is_missing() && n.end_byte() == n.start_byte() + 1)
                    .then(|| text.as_bytes()[n.start_byte()])
            };
            let (same, other) = if forward {
                (open, close)
            } else {
                (close, open)
            };
            let mut depth = 0usize;
            let mut step = |n: &tree_sitter::Node| -> Option<Option<usize>> {
                match kind_of(n) {
                    Some(k) if k == same => depth += 1,
                    Some(k) if k == other => {
                        if depth == 0 {
                            return Some(Some(n.start_byte()));
                        }
                        depth -= 1;
                    }
                    _ => {}
                }
                None
            };
            let found = if forward {
                children[i + 1..].iter().find_map(&mut step)
            } else {
                children[..i].iter().rev().find_map(&mut step)
            };
            Some(Bracket::Code(found.flatten()))
        }

        /// **Whether byte `at` is inside a string or a comment**, by the
        /// tree; none when it cannot say. A place just after a string's
        /// closing quote is outside it.
        pub fn in_text(&self, text: &str, at: usize) -> Option<bool> {
            let tree = self.fresh()?;
            let at = at.min(text.len());
            let mut node = tree
                .root_node()
                .descendant_for_byte_range(at.saturating_sub(1), at);
            while let Some(n) = node {
                if is_text_node(n.kind()) && n.start_byte() < at && at < n.end_byte() {
                    return Some(true);
                }
                node = n.parent();
            }
            Some(false)
        }
    }

    /// A copy holds the same tree, and a parser of its own.
    impl Clone for Syntax {
        fn clone(&self) -> Self {
            // a stopped parse is the parser's, so the copy starts its own
            let mut syntax = Syntax::new(self.language);
            syntax.tree = self.tree.clone();
            syntax.stale = self.stale;
            syntax
        }
    }

    impl std::fmt::Debug for Syntax {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Syntax")
                .field("language", &self.language)
                .field("parsed", &self.tree.is_some())
                .field("stale", &self.stale)
                .finish()
        }
    }
}

#[cfg(not(feature = "highlight"))]
mod tree {
    use super::{Bracket, Language, Span};

    /// Without the parser, a language and nothing more.
    #[derive(Clone, Debug)]
    pub struct Syntax {
        language: Language,
    }

    impl Syntax {
        pub fn new(language: Language) -> Syntax {
            Syntax { language }
        }

        pub fn language(&self) -> Language {
            self.language
        }

        pub fn reset(&mut self) {}

        pub fn parsing(&self) -> bool {
            false
        }

        pub fn edit(&mut self, _before: &str, _at: usize, _removed: &str, _inserted: &str) {}

        pub fn parse(&mut self, _text: &str, _budget: Option<std::time::Duration>) -> bool {
            true
        }

        pub fn spans(&self, _text: &str, _from: usize, _to: usize) -> Vec<Span> {
            Vec::new()
        }

        pub fn bracket(&self, _text: &str, _at: usize) -> Option<Bracket> {
            None
        }

        pub fn in_text(&self, _text: &str, _at: usize) -> Option<bool> {
            None
        }
    }
}

pub use tree::Syntax;
