//! **The code element's language help**: the completions of what is being
//! typed, the parameters of the call the caret is in, and what a name under
//! the pointer is.
//!
//! The element computes none of it. It asks whoever owns the window (a
//! session's own answers, Jedi's for Python and the TypeScript service's in
//! a page) with an event that carries the text and where in it
//! (`"complete"`, `"signatures"`, `"inspect"`), and the owner answers with a
//! set (`completions`, `signature`, `inspected`). What arrives is held here
//! and drawn in the popup layer, the list walked by the element's own keys.
//!
//! **Completions are asked for once a word**, as the current editor asks:
//! when a name's first character is typed, after a dot, or on Ctrl+Space.
//! While the word goes on being typed the list is filtered here, and an
//! answer that arrives after the caret left the word is let go.

use serde_json::Value;

use crate::host::graphics::code::Code;
use crate::host::graphics::code::find::is_word;
use crate::host::menu::Entry;

/// The most completions a list holds.
const MOST: usize = 100;

/// One completion the owner offered.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Offer {
    pub name: String,
    /// What it is, as the session's kinds say: `function`, `module`, ...
    pub kind: String,
}

/// **Completions asked for, and then arrived.**
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Completing {
    /// Where the caret stood when they were asked for.
    pub asked: usize,
    /// Where the word being completed starts, once they arrived.
    pub from: Option<usize>,
    offers: Vec<Offer>,
    /// The offers that match what is typed, by index.
    pub shown: Vec<usize>,
    /// The row the keys are on.
    pub hover: usize,
    /// How far a hand scrolled the list; `None` while it follows `hover`.
    pub scroll: Option<f32>,
}

/// The call the caret is in, as the owner read it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Signature {
    pub name: String,
    pub params: Vec<String>,
    /// The parameter the caret is at.
    pub index: Option<usize>,
    /// Where the call's bracket is: a line from 1 and a column from 0, in
    /// characters.
    pub bracket: (usize, usize),
    pub doc: String,
}

/// What a name is, as the owner read it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Inspected {
    pub name: String,
    pub kind: String,
    pub signature: String,
    pub doc: String,
}

/// The language help one code element holds.
#[derive(Clone, Debug, Default)]
pub(super) struct Help {
    pub completing: Option<Completing>,
    /// Whether the parameters of the call are followed: from a `(` or a `,`
    /// typed until Escape or an answer of none.
    pub calling: bool,
    pub signature: Option<Signature>,
    /// The name the pointer rests on, its bytes, and what it is once the
    /// owner said.
    pub resting: Option<(usize, usize, Option<Inspected>)>,
}

/// A value that is an array or an object, or the JSON text of one, as a set
/// carries a structure.
fn structure(v: &Value) -> Option<Value> {
    match v {
        Value::String(s) if s.trim().is_empty() => Some(Value::Null),
        Value::String(s) => serde_json::from_str(s).ok(),
        other => Some(other.clone()),
    }
}

fn text_of(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

impl Completing {
    pub fn asked_at(at: usize) -> Completing {
        Completing {
            asked: at,
            ..Completing::default()
        }
    }

    /// Whether there is a list to show.
    pub fn showing(&self) -> bool {
        self.from.is_some() && !self.shown.is_empty()
    }

    /// **The offers that match what is typed from the word's start to the
    /// caret**: those that start with it, case aside, in the owner's order.
    pub fn filter(&mut self, code: &Code) {
        let Some(from) = self.from else {
            return;
        };
        let pos = code.caret().pos;
        let typed = code
            .text()
            .get(from..pos)
            .unwrap_or_default()
            .to_lowercase();
        self.shown = self
            .offers
            .iter()
            .enumerate()
            .filter(|(_, o)| o.name.to_lowercase().starts_with(&typed))
            .map(|(i, _)| i)
            .take(MOST)
            .collect();
        self.hover = self.hover.min(self.shown.len().saturating_sub(1));
    }

    /// The offer the keys are on.
    pub fn chosen(&self) -> Option<&Offer> {
        self.shown.get(self.hover).map(|&i| &self.offers[i])
    }

    /// The rows of the list: each name, what it is beside it.
    pub fn entries(&self) -> Vec<Entry> {
        self.shown
            .iter()
            .map(|&i| {
                let o = &self.offers[i];
                let mut e = Entry::action(&o.name, "complete");
                e.key = (!o.kind.is_empty()).then(|| o.kind.clone());
                e
            })
            .collect()
    }

    /// The keys walk the list: `by` rows on, round past either end.
    pub fn walk(&mut self, by: isize) {
        let n = self.shown.len() as isize;
        if n > 0 {
            self.hover = (self.hover as isize + by).rem_euclid(n) as usize;
        }
        // the keys bring the list back to the row they are on
        self.scroll = None;
    }
}

impl Help {
    /// Nothing asked for and nothing shown.
    pub fn clear(&mut self) {
        self.completing = None;
        self.calling = false;
        self.signature = None;
        self.resting = None;
    }

    /// **The owner's completions arrived**: an array of `{name, kind,
    /// typed}`, `typed` how many characters of the name stand before the
    /// caret. Answers whether the list changed.
    pub fn completions(&mut self, code: &Code, v: &Value) -> bool {
        let Some(c) = self.completing.as_mut() else {
            return false;
        };
        let Some(list) = structure(v) else {
            return false;
        };
        let offers: Vec<(Offer, usize)> = list
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|o| {
                        let typed = o.get("typed").and_then(Value::as_u64).unwrap_or(0) as usize;
                        (
                            Offer {
                                name: text_of(o, "name"),
                                kind: text_of(o, "kind"),
                            },
                            typed,
                        )
                    })
                    .filter(|(o, _)| !o.name.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        // the word the answer completes starts `typed` characters before
        // where the caret stood, and the caret is still in it
        let typed = offers.first().map_or(0, |o| o.1);
        let text = code.text();
        let from = text[..c.asked.min(text.len())]
            .char_indices()
            .rev()
            .nth(typed.saturating_sub(1))
            .map_or(c.asked, |(i, _)| if typed == 0 { c.asked } else { i });
        let pos = code.caret().pos;
        let still = pos >= from
            && text
                .get(from..pos)
                .is_some_and(|w| w.chars().all(|ch| is_word(ch) || ch == '$'));
        if offers.is_empty() || !still {
            self.completing = None;
            return true;
        }
        c.offers = offers.into_iter().map(|(o, _)| o).collect();
        c.from = Some(from);
        c.hover = 0;
        c.scroll = None;
        c.filter(code);
        if c.shown.is_empty() {
            self.completing = None;
        }
        true
    }

    /// **The owner's reading of the call** arrived: `{name, params, index,
    /// bracket: [line, column], doc}`, or nothing, which ends following it.
    pub fn signature(&mut self, v: &Value) -> bool {
        if !self.calling {
            return false;
        }
        let read = structure(v).filter(|o| o.get("name").is_some()).map(|o| {
            let bracket = o
                .get("bracket")
                .and_then(Value::as_array)
                .map(|b| {
                    let n = |i: usize| b.get(i).and_then(Value::as_u64).unwrap_or(0) as usize;
                    (n(0), n(1))
                })
                .unwrap_or((1, 0));
            Signature {
                name: text_of(&o, "name"),
                params: o
                    .get("params")
                    .and_then(Value::as_array)
                    .map(|p| {
                        p.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
                index: o.get("index").and_then(Value::as_u64).map(|i| i as usize),
                bracket,
                doc: text_of(&o, "doc"),
            }
        });
        if read.is_none() {
            self.calling = false;
        }
        let changed = self.signature != read;
        self.signature = read;
        changed
    }

    /// **The owner's reading of the name the pointer rests on** arrived:
    /// `{name, kind, signature, doc}`, or nothing.
    pub fn inspected(&mut self, v: &Value) -> bool {
        let Some((_, _, held)) = self.resting.as_mut() else {
            return false;
        };
        let read = structure(v)
            .filter(|o| o.get("name").is_some())
            .map(|o| Inspected {
                name: text_of(&o, "name"),
                kind: text_of(&o, "kind"),
                signature: text_of(&o, "signature"),
                doc: text_of(&o, "doc"),
            });
        if read.is_none() {
            self.resting = None;
            return true;
        }
        *held = read;
        true
    }
}

/// **The head of a call's note**: its name and parameters, the one the
/// caret is at marked.
pub(super) fn call_head(sig: &Signature) -> Vec<(String, bool)> {
    let mut head = vec![(format!("{}(", sig.name), false)];
    for (i, p) in sig.params.iter().enumerate() {
        if i > 0 {
            head.push((", ".into(), false));
        }
        head.push((p.clone(), sig.index == Some(i)));
    }
    head.push((")".into(), false));
    head
}
