//! **What a gesture means**, in one structure's own vocabulary.
//!
//! The second of the four projections. A host reports a gesture as a *tag* and
//! a flat list of values -- `points`, `clips`, `lanes`, `draw` -- and what an
//! editor needs from that is payloads in the structure's vocabulary, which is
//! the only thing [`clausters_document::intent`] will apply. Between those two
//! sits a mapping that was written once per client, in two languages, per
//! domain: sixteen small readers, each of which could disagree with its twin
//! about what a septuple means.
//!
//! # Why the answer is a struct and not a list
//!
//! Because three different things come back from one reading and separating
//! them is what makes the middle one possible:
//!
//! - The **payloads**, however many the gesture took. A report states the whole
//!   structure -- a multitrack's boxes after a block drag says a move, a trim
//!   and a lane's new contents at once -- and those are one entry in the
//!   history, because they are one thing a hand did.
//! - A **refusal**: why a gesture this domain *does* understand cannot be
//!   written. It is not the same as no payloads. A tag that is not this
//!   domain's is nothing at all and the host goes on drawing what it drew; a
//!   tag that is this domain's and cannot be honoured has to be *said*, or the
//!   picture and the data disagree in silence.
//! - The **label**, which is what an undo menu calls the gesture. It comes back
//!   with the payloads because for one domain it is not a function of the
//!   payload at all: both of a roll's lanes state the same whole-list intent,
//!   so only the gesture knows whether a hand edited the notes or the markers.

use serde_json::Value;

/// What a report of a gesture came to, in a structure's vocabulary.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Intake {
    /// The edits, in the order they are applied. Empty for a tag this domain
    /// does not answer for, which is the ordinary case rather than a failure.
    pub payloads: Vec<Value>,
    /// The inverse, when the **gesture** carried it rather than the structure.
    ///
    /// One domain does: a stroke over samples arrives with what it replaced
    /// beside what it wrote, because the vocabulary has no field for "what this
    /// was" and the previous run is only knowable while the host still holds
    /// it. Everywhere else the inverse is read off the structure before the
    /// edit lands, which is `domain::edit`'s `current`.
    pub inverse: Option<Value>,
    /// Why a gesture this domain understands cannot be written, as the sentence
    /// the user is shown.
    pub refusal: Option<String>,
    /// What an undo menu calls this gesture.
    pub label: String,
}

impl Intake {
    /// Not this domain's tag: no edit, no refusal, nothing to say.
    pub fn nothing() -> Self {
        Self::default()
    }

    /// A gesture this domain understands and cannot write.
    pub fn refused(why: impl Into<String>) -> Self {
        Self {
            refusal: Some(why.into()),
            ..Self::default()
        }
    }

    /// The edits a gesture came to, under one label.
    pub fn edits(payloads: Vec<Value>, label: impl Into<String>) -> Self {
        Self {
            payloads,
            label: label.into(),
            ..Self::default()
        }
    }

    /// One edit under one label.
    pub fn edit(payload: Value, label: impl Into<String>) -> Self {
        Self::edits(vec![payload], label)
    }

    /// The answer as the one JSON object both doors carry.
    ///
    /// `refusal` and `inverse` are written only when there is one, so a reader
    /// tells "nothing to say" from "this cannot be done" by whether the key is
    /// there rather than by comparing against an empty string.
    pub fn to_json(&self) -> Value {
        let mut out = serde_json::Map::new();
        out.insert("payloads".into(), Value::Array(self.payloads.clone()));
        out.insert("label".into(), Value::String(self.label.clone()));
        if let Some(inverse) = &self.inverse {
            out.insert("inverse".into(), inverse.clone());
        }
        if let Some(refusal) = &self.refusal {
            out.insert("refusal".into(), Value::String(refusal.clone()));
        }
        Value::Object(out)
    }
}

/// A flat payload as groups of `n`.
///
/// A trailing partial group is **dropped** rather than half-read, which is the
/// rule every flat payload in this system follows: half a septuple says nothing
/// about a box, and guessing at the missing fields invents one.
pub fn groups(values: &[Value], n: usize) -> impl Iterator<Item = &[Value]> {
    values.chunks_exact(n.max(1))
}

/// A wire value as a number, whichever way it was written.
///
/// The GUI protocol carries a flat list of mixed atoms, and whether a `1` came
/// across as an integer, a float or the string a name is spelled with depends
/// on the transport rather than on what it means.
pub fn number(value: &Value) -> f64 {
    match value {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => s.parse().unwrap_or(0.0),
        Value::Bool(b) => f64::from(u8::from(*b)),
        _ => 0.0,
    }
}

/// A wire value as the text a name is.
pub fn text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_partial_group_is_dropped_rather_than_guessed_at() {
        let values = vec![json!("a"), json!(1), json!("b")];
        let read: Vec<_> = groups(&values, 2).collect();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0][0], json!("a"));
    }

    /// Nothing to say and cannot be done are different answers.
    #[test]
    fn a_refusal_is_not_an_empty_edit() {
        let quiet = Intake::nothing().to_json();
        assert!(quiet.get("refusal").is_none());
        let refused = Intake::refused("not here").to_json();
        assert_eq!(refused["payloads"], json!([]));
        assert_eq!(refused["refusal"], json!("not here"));
    }

    /// A number is a number however the wire spelled it.
    #[test]
    fn a_wire_value_is_read_whichever_way_it_came() {
        assert_eq!(number(&json!("2.5")), 2.5);
        assert_eq!(number(&json!(2.5)), 2.5);
        assert_eq!(text(&json!(7)), "7");
        assert_eq!(text(&json!("n1")), "n1");
    }
}
