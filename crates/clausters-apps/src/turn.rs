//! **What a turn of any application's editor is**: the message a host sends,
//! what kind of turn it came to, and the entry a gesture leaves for the history.
//!
//! Every application answers a host the same way — a message read by the
//! conversation, a gesture read in the structure's own vocabulary, an entry for
//! the history the caller keeps — so the words for those are here once rather
//! than once per application.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What kind of turn a message came to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// Not this editor's, or not an event at all.
    #[default]
    Nothing,
    /// This editor's window closed.
    Closed,
    /// A walk through the history, which the caller takes and then answers.
    Step,
    /// An edit made against a picture that is gone: refused, and the picture
    /// handed back.
    Stale,
    /// A gesture, read and answered.
    Route,
}

/// One structure's share of an entry to record: how to redo it, how to put it
/// back, and what makes two of them the same thing done the same way.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Leg {
    /// `{"edit": <payload>}`.
    pub forward: Value,
    /// The payload that puts the structure back, read before the edit landed.
    pub backward: Value,
    /// The coalesce key, empty for an edit that never coalesces.
    pub key: String,
}

/// An entry for the history the caller keeps: one gesture, however many edits
/// it took.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Record {
    /// What an undo menu calls it.
    pub label: String,
    /// The structure's legs, in the order they were applied.
    pub legs: Vec<Leg>,
}

/// One message from the host: its address and its arguments,
/// `<id> <seq> <version> <tag> <payload…>`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Event {
    /// `"/gui_event"` or `"/gui_closed"`.
    pub addr: String,
    /// The arguments, in order.
    #[serde(default)]
    pub args: Vec<Value>,
}

/// A report's number as an integer, whichever way JSON spelled it.
pub(crate) fn int(value: &Value) -> i64 {
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|f| f as i64))
        .unwrap_or(0)
}

/// A report's number.
pub(crate) fn number(value: &Value) -> f64 {
    value.as_f64().unwrap_or(0.0)
}

/// A report's word.
pub(crate) fn text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
