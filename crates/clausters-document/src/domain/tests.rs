use super::*;
use crate::events::{Event, EventsIntent};
use crate::points::{Point, PointsIntent};
use crate::samples::SamplesIntent;

#[test]
fn every_domain_the_crate_speaks_answers_its_own_sentence() {
    let tree = crate::log::payload(&crate::Intent::Place {
        node: crate::NodeId(7),
        offset: 1.0,
        dur: None,
    });
    assert_eq!(coalesce_key(TREE, &tree).as_deref(), Some("place:7"));

    let curve = crate::points::payload(&PointsIntent::SetPoints {
        points: vec![Point {
            at: 0.0,
            value: 1.0,
        }],
    });
    assert_eq!(coalesce_key(POINTS, &curve).as_deref(), Some("points"));

    let span = crate::samples::payload(&SamplesIntent::Write {
        channel: 1,
        start: 40,
        values: vec![0.5, 0.5],
    });
    assert_eq!(
        coalesce_key(SAMPLES, &span).as_deref(),
        Some("samples:1:40:2")
    );

    let roll = crate::events::payload(&EventsIntent::SetEvents {
        events: vec![Event {
            at: 0.0,
            data: crate::Opaque::none(),
        }],
    });
    assert_eq!(coalesce_key(EVENTS, &roll).as_deref(), Some("events"));
}

#[test]
fn a_domain_the_crate_does_not_speak_answers_nothing() {
    // Which is also what catches a misspelled domain name: `register` takes any
    // string, so a typo would otherwise mint a structure in a vocabulary nobody
    // reads and go on working until an undo did the wrong thing.
    assert!(!known("smaples"));
    assert!(coalesce_key("smaples", &crate::Opaque::none()).is_none());
}

#[test]
fn a_payload_in_another_vocabulary_answers_nothing() {
    let curve = crate::points::payload(&PointsIntent::SetPoints { points: Vec::new() });
    assert!(
        coalesce_key(SAMPLES, &curve).is_none(),
        "a curve's edit is not a span, whatever the caller says the domain is"
    );
}

#[test]
fn the_table_lists_each_domain_once() {
    let mut seen = DOMAINS.to_vec();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), DOMAINS.len());
}
