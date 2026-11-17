//! **Finding in the text**: a query, every place it matches, the next and
//! the previous from where the selection is, and what a match is replaced
//! by.
//!
//! A query is the text sought and three switches: whether case counts,
//! whether the text is a regular expression, and whether a match must be a
//! whole word. Whatever the switches, it is searched as one expression: a
//! plain query is escaped into one, so there is one search and one place a
//! word is checked. In a plain query and in a replacement, `\n`, `\r`, `\t`
//! and `\\` stand for a line end, a return, a tab and a backslash, as in the
//! current editor; in a replacement for a regular expression, `$&` is the
//! match, `$1` ... its groups and `$$` a dollar sign.
//!
//! An empty match is no match: an expression that can match nothing (`^`,
//! `x*`) finds only where it matches something, so a search always moves.

use regex::{Regex, RegexBuilder};

/// What is sought, and how.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    pub text: String,
    /// Whether case counts.
    pub case: bool,
    /// Whether the text is a regular expression.
    pub regexp: bool,
    /// Whether a match is a whole word.
    pub word: bool,
}

/// **A query ready to search**: its expression, or why there is none.
#[derive(Clone, Debug)]
pub struct Finder {
    query: Query,
    expr: Result<Regex, String>,
}

impl Finder {
    pub fn new(query: &Query) -> Finder {
        let source = if query.regexp {
            query.text.clone()
        } else {
            regex::escape(&unquote(&query.text))
        };
        let expr = if query.text.is_empty() {
            Err(String::new())
        } else {
            RegexBuilder::new(&source)
                .case_insensitive(!query.case)
                .multi_line(true)
                .build()
                .map_err(|e| first_line(&e.to_string()))
        };
        Finder {
            query: query.clone(),
            expr,
        }
    }

    pub fn query(&self) -> &Query {
        &self.query
    }

    /// Whether there is something to search for.
    pub fn ready(&self) -> bool {
        self.expr.is_ok()
    }

    /// Why a regular expression typed is not one; nothing for an empty
    /// query.
    pub fn error(&self) -> Option<&str> {
        self.expr
            .as_ref()
            .err()
            .filter(|e| !e.is_empty())
            .map(String::as_str)
    }

    /// **The matches that start at byte `from` or after**, in order, as
    /// byte ranges.
    pub fn from<'t>(&'t self, text: &'t str, from: usize) -> Matches<'t> {
        Matches {
            finder: self,
            text,
            at: Some(from.min(text.len())),
        }
    }

    /// Every match that starts in `from..to`.
    pub fn within(&self, text: &str, from: usize, to: usize) -> Vec<(usize, usize)> {
        self.from(text, from).take_while(|m| m.0 < to).collect()
    }

    /// How many matches the text holds, counted up to `cap`.
    pub fn count(&self, text: &str, cap: usize) -> usize {
        self.from(text, 0).take(cap).count()
    }

    /// **The match after a selection from `a` to `b`**: the first that
    /// starts at `b` or after, or, past the end, the first from the
    /// start. None when the only match is the one selected.
    pub fn next(&self, text: &str, a: usize, b: usize) -> Option<(usize, usize)> {
        let m = self
            .from(text, b)
            .next()
            .or_else(|| self.from(text, 0).next())?;
        (m != (a, b)).then_some(m)
    }

    /// **The match before a selection from `a` to `b`**: the last that ends
    /// at `a` or before, or, past the start, the last after `b`.
    pub fn previous(&self, text: &str, a: usize, b: usize) -> Option<(usize, usize)> {
        let mut last = None;
        for m in self.from(text, 0) {
            if m.1 > a {
                break;
            }
            last = Some(m);
        }
        last.or_else(|| self.from(text, b).last())
            .filter(|m| *m != (a, b))
    }

    /// **What match `m` of `text` is replaced by**: the replacement as
    /// typed, its escapes read, and for a regular expression its `$`
    /// references filled from the match.
    pub fn replacement(&self, text: &str, m: (usize, usize), with: &str) -> String {
        let with = unquote(with);
        let Ok(expr) = &self.expr else {
            return with;
        };
        if !self.query.regexp {
            return with;
        }
        let Some(caps) = expr
            .captures_at(text, m.0)
            .filter(|c| c.get(0).is_some_and(|g| g.start() == m.0))
        else {
            return with;
        };
        let group = |n: usize| caps.get(n).map_or("", |g| g.as_str());
        let mut out = String::new();
        let mut rest = with.as_str();
        while let Some(i) = rest.find('$') {
            out.push_str(&rest[..i]);
            let after = &rest[i + 1..];
            if let Some(tail) = after.strip_prefix('$') {
                out.push('$');
                rest = tail;
            } else if let Some(tail) = after.strip_prefix('&') {
                out.push_str(group(0));
                rest = tail;
            } else {
                let digits = after.bytes().take_while(u8::is_ascii_digit).count();
                // the longest number that names a group, the digits after it
                // kept as text
                let named = (1..=digits).rev().find(|&l| {
                    after[..l]
                        .parse::<usize>()
                        .is_ok_and(|n| n > 0 && n < caps.len())
                });
                match named {
                    Some(l) => {
                        out.push_str(group(after[..l].parse().unwrap_or(0)));
                        rest = &after[l..];
                    }
                    None => {
                        out.push('$');
                        rest = after;
                    }
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// Whether `m` stands as a whole word: no word character runs on past
    /// either end of it that is a word character itself.
    fn whole(text: &str, m: (usize, usize)) -> bool {
        let before = text[..m.0].chars().next_back();
        let first = text[m.0..m.1].chars().next();
        let last = text[m.0..m.1].chars().next_back();
        let after = text[m.1..].chars().next();
        let joined =
            |a: Option<char>, b: Option<char>| a.is_some_and(is_word) && b.is_some_and(is_word);
        !joined(before, first) && !joined(last, after)
    }
}

/// The matches of a [`Finder`] from a place on.
pub struct Matches<'t> {
    finder: &'t Finder,
    text: &'t str,
    at: Option<usize>,
}

impl Iterator for Matches<'_> {
    type Item = (usize, usize);

    fn next(&mut self) -> Option<(usize, usize)> {
        let expr = self.finder.expr.as_ref().ok()?;
        loop {
            let at = self.at?;
            let Some(m) = expr.find_at(self.text, at) else {
                self.at = None;
                return None;
            };
            let found = (m.start(), m.end());
            if found.0 < found.1 && (!self.finder.query.word || Finder::whole(self.text, found)) {
                self.at = Some(found.1);
                return Some(found);
            }
            // nothing, or not a word: on from the next character
            self.at = self.text[found.0..]
                .chars()
                .next()
                .map(|c| found.0 + c.len_utf8());
        }
    }
}

/// Whether `c` is part of a word: a letter, a digit or an underscore.
pub fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `s` with `\n`, `\r`, `\t` and `\\` read as what they stand for.
fn unquote(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// The first line of a message: what a bar has room to say.
fn first_line(s: &str) -> String {
    s.lines()
        .rev()
        .find(|l| l.starts_with("error:"))
        .unwrap_or(s.lines().next().unwrap_or(s))
        .trim()
        .to_string()
}
