//! **What a gesture over a buffer's samples means.**
//!
//! The smallest of the ingestions and the only one whose inverse arrives with
//! the gesture: the `samples` vocabulary states the run a stroke *wrote* and
//! has no field for the run it replaced, so what the host sends beside it is
//! the inverse, and it is only knowable while the host still holds it.
//!
//! # What does not cross, and why the line is there
//!
//! The wire's own framing. A `draw` carries its run as a little-endian `f32`
//! blob, which is a `memoryview` in one client and an `ArrayBuffer` in the
//! other and cannot be either of those in a JSON request — so a client decodes
//! the blob and this reads the numbers. That is the crate's standing rule
//! rather than an exception: it defines edits and does not encode them.

use serde_json::{Value, json};

use crate::intake::{Intake, number};

/// A reported field as a run of samples: an array as itself, a lone number as a
/// run of one, which is the difference between a `draw` and a `sample`.
fn run(value: &Value) -> Vec<f64> {
    match value {
        Value::Array(items) => items.iter().map(number).collect(),
        Value::Null => Vec::new(),
        other => vec![number(other)],
    }
}

/// **The words this domain answers for**, and the only place they are listed.
///
/// Two tags, one verb: a stroke and a single sample differ in how much they
/// carry and not in what they mean.
pub fn answers(tag: &str) -> bool {
    matches!(tag, "draw" | "sample")
}

/// **What a report over samples came to**: the write, and the run it replaced
/// when that run can undo it.
///
/// The label is here rather than at each caller for the reason every other
/// label in this crate is: a verb named two ways in two endpoints is the same
/// divergence as a verb applied two ways, only quieter.
#[derive(Debug, Clone, PartialEq)]
pub struct Write {
    /// Which channel of the samples the span belongs to.
    pub channel: u32,
    /// First frame of the span, in that channel.
    pub start: u64,
    /// The values the span now holds.
    pub values: Vec<f64>,
    /// The run it replaced, when that is an inverse the pile can apply.
    pub previous: Option<Vec<f64>>,
    /// What the undo stack shows for it.
    pub label: &'static str,
}

/// **The gesture as the write it is**, from the numbers rather than from a
/// payload.
///
/// The typed door, for a caller that is already holding the run: a host has
/// decoded the blob the wire carried, and turning it back into JSON so that
/// this crate could read it would be a conversion in each direction for a rule
/// fifteen lines long. [`intake`] is the same function with the JSON reading in
/// front of it, which is what a client needs.
///
/// `f64` and not `f32` because that is what a report's numbers are once they
/// have been read: the samples on the wire are `f32` and every value here comes
/// from one, so both conversions are exact and neither door has to know which
/// the other used.
///
/// Two rules, and both of them are *what is not an edit*:
///
/// - **A write of nothing is not one.** A stroke that covered no frame changed
///   no frame, and recording it would put an entry in the pile that undoes to
///   itself.
/// - **The inverse is stated only when it covers the same span.** An inverse
///   shorter or longer than the write it undoes would leave part of the edit
///   standing, and an entry the pile cannot invert is better recorded as one
///   than pretended.
pub fn write(
    tag: &str,
    channel: i64,
    start: i64,
    wrote: &[f64],
    previous: &[f64],
) -> Option<Write> {
    if !answers(tag) || wrote.is_empty() {
        return None;
    }
    Some(Write {
        channel: channel.max(0) as u32,
        start: start.max(0) as u64,
        values: wrote.to_vec(),
        previous: (previous.len() == wrote.len()).then(|| previous.to_vec()),
        label: "draw the samples",
    })
}

/// The gesture as the write it is, with the run it replaced as its inverse —
/// [`write()`] behind the JSON reading a client's report arrives as.
pub fn intake(tag: &str, values: &[Value]) -> Intake {
    if !answers(tag) || values.len() < 4 {
        return Intake::nothing();
    }
    let (channel, start) = (number(&values[0]) as i64, number(&values[1]) as i64);
    let (wrote, previous) = (run(&values[2]), run(&values[3]));
    let Some(written) = write(tag, channel, start, &wrote, &previous) else {
        return Intake::nothing();
    };
    let payload = |values: &[f64]| json!({ "intent": "write", "channel": written.channel, "start": written.start, "values": values });
    let mut intake = Intake::edit(payload(&written.values), written.label);
    if let Some(previous) = &written.previous {
        intake.inverse = Some(payload(previous));
    }
    intake
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stroke and a single sample are one verb said two ways.
    #[test]
    fn a_stroke_and_a_sample_are_the_same_write() {
        let stroke = intake(
            "draw",
            &[json!(0), json!(4), json!([0.5, 0.25]), json!([0.0, 0.0])],
        );
        assert_eq!(stroke.payloads.len(), 1);
        assert_eq!(stroke.payloads[0]["values"], json!([0.5, 0.25]));
        assert_eq!(stroke.inverse.as_ref().expect("inverse")["start"], json!(4));

        let one = intake("sample", &[json!(1), json!(9), json!(0.5), json!(-0.5)]);
        assert_eq!(one.payloads[0]["values"], json!([0.5]));
        assert_eq!(one.payloads[0]["channel"], json!(1));
    }

    /// A stroke that wrote nothing is not an entry in the pile.
    #[test]
    fn a_write_of_nothing_is_not_an_edit() {
        let empty = intake("draw", &[json!(0), json!(0), json!([]), json!([])]);
        assert!(empty.payloads.is_empty());
        assert!(empty.refusal.is_none());
    }

    /// An inverse that does not cover the write is not stated at all.
    #[test]
    fn an_inverse_that_does_not_cover_the_write_is_not_stated() {
        let ragged = intake(
            "draw",
            &[json!(0), json!(0), json!([0.1, 0.2]), json!([0.0])],
        );
        assert_eq!(ragged.payloads.len(), 1);
        assert!(ragged.inverse.is_none());
    }

    /// A tag no hand over samples makes is nothing, not a refusal.
    #[test]
    fn another_domain_s_tag_says_nothing() {
        assert_eq!(intake("points", &[json!(0.0)]), Intake::nothing());
    }
}
