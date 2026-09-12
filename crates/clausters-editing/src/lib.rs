//! **What an editable structure owes its three endpoints**, written once.
//!
//! A structure in this system has three endpoints and an edit may start at any
//! of them: the document owns the model, a host draws it, a server sounds it.
//! Each pair of those needs a *projection* — the structure as props, a gesture
//! as payloads, the structure as what is sounding — and a projection with two
//! implementations is how one curve comes to be drawn two ways and one piece
//! comes to sound two. So there is one of each, here, and every client binds
//! it while the host links it.
//!
//! # Why this is not `clausters-document`
//!
//! That crate's own rule: **the wire stays out of it.** It defines intents and
//! outcomes and does not encode them, and a projection's answer is exactly an
//! encoding — the props of a `/gui_*` message, the arguments of an OSC one. So
//! the projections sit *above* the document rather than in it, in a crate that
//! may know both the model and the shape it is being written into.
//!
//! # Why this is not `clausters-core` either
//!
//! The numeric and drawing *rules* are there and stay there — this crate asks
//! [`clausters_core::envshape::curve_axis`] rather than restating it. What is
//! here is the step after: assembling a rule's answer into the payload an
//! endpoint reads. The core cannot hold that for the projections that come
//! next, because those are over the document's types and the core does not
//! depend on the document.
//!
//! # What a projection is, and what it is not
//!
//! A **function**. It takes the structure and whatever the caller is holding,
//! and hands back the payload. It keeps nothing: where a picture's own state
//! lives — the axis a view has settled on, the zoom, the selection — is the
//! endpoint's question, and the answer is that the *host* owns view state. A
//! projection that kept it would be a fourth place for it to live.

pub mod events;
pub mod instance;
pub mod intake;
pub mod multitrack;
pub mod points;
pub mod samples;

use serde_json::Value;

use clausters_document::events::EVENTS;
use clausters_document::multitrack::edit::MULTITRACK;
use clausters_document::points::POINTS;
use clausters_document::samples::SAMPLES;

use crate::intake::Intake;

/// **One door for the edit ingestion**, over every domain there is.
///
/// A host reports a gesture the same way whatever it is over — a tag and a flat
/// list of values — so what reads one is one call, and which structure's
/// vocabulary the answer comes back in is the `domain`. A domain this does not
/// know answers [`Intake::nothing`], which is exactly what a tag a domain does
/// not answer for says: **a client cannot quietly grow a fifth vocabulary**,
/// because there is nowhere for it to grow.
///
/// `request` is one JSON object, and each domain reads the fields it needs:
///
/// - `values` — the report, always.
/// - `state` — the structure as its vocabulary holds it. The piece for
///   `multitrack`, the timeline for `events`; the other two read the gesture
///   alone.
/// - `unitsPerBeat`, `editable` — a roll's axis, and whether what it draws can
///   be written onto at all.
/// - `rate`, `defaultBpm`, `sources` — a piece's axis and its buffer table, the
///   same three [`multitrack::props_json`] takes.
///
/// The answer is [`Intake::to_json`]: `payloads`, `label`, and `inverse` or
/// `refusal` only where there is one.
pub fn intake_json(domain: &str, tag: &str, request: &str) -> String {
    let request: Value = serde_json::from_str(request).unwrap_or(Value::Null);
    let empty: Vec<Value> = Vec::new();
    let values = request
        .get("values")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let number =
        |key: &str, default: f64| request.get(key).and_then(Value::as_f64).unwrap_or(default);
    // **The vocabulary's own name**, not a literal: the document says what each
    // domain is called and a second spelling here is a domain nobody can reach.
    let taken = match domain {
        POINTS => {
            let flat: Vec<f64> = values.iter().map(intake::number).collect();
            points::intake(tag, &flat)
        }
        SAMPLES => samples::intake(tag, values),
        EVENTS => events::intake(
            request
                .get("state")
                .and_then(Value::as_array)
                .unwrap_or(&empty),
            tag,
            values,
            number("unitsPerBeat", 1.0),
            request
                .get("editable")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        ),
        MULTITRACK => multitrack::intake_value(
            request.get("state").unwrap_or(&Value::Null),
            tag,
            values,
            number("rate", 0.0),
            number("defaultBpm", 0.0),
            request.get("sources").unwrap_or(&Value::Null),
        ),
        _ => Intake::nothing(),
    };
    taken.to_json().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Every domain is reached through the one door, and one that does not
    /// exist is nothing rather than an error.
    #[test]
    fn the_door_reaches_each_vocabulary_and_invents_none() {
        let drawn = intake_json(
            "points",
            "points",
            &json!({ "values": [0.0, 0.5, 1, 0.0] }).to_string(),
        );
        let drawn: Value = serde_json::from_str(&drawn).expect("JSON");
        assert_eq!(drawn["payloads"][0]["intent"], json!("setpoints"));
        assert_eq!(drawn["label"], json!("draw the curve"));

        let wrote = intake_json(
            "samples",
            "sample",
            &json!({ "values": [0, 2, 0.5, 0.0] }).to_string(),
        );
        let wrote: Value = serde_json::from_str(&wrote).expect("JSON");
        assert_eq!(wrote["inverse"]["values"], json!([0.0]));

        let unknown: Value =
            serde_json::from_str(&intake_json("clips", "clips", "{}")).expect("JSON");
        assert_eq!(unknown["payloads"], json!([]));
        assert!(unknown.get("refusal").is_none());
    }

    /// A request that is not an object is read as one with nothing in it,
    /// rather than answered with a failure a caller would have to handle.
    #[test]
    fn an_unreadable_request_says_nothing() {
        let quiet: Value =
            serde_json::from_str(&intake_json("points", "points", "not json")).expect("JSON");
        assert_eq!(quiet["payloads"], json!([]));
    }
}
