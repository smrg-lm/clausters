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
            data: Opaque::default(),
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

mod editing_a_structure_the_crate_can_hold {
    use super::*;
    use crate::points::{Point, PointsIntent};

    fn points(pairs: &[(f64, f64)]) -> Opaque {
        Opaque(
            serde_json::to_value(
                pairs
                    .iter()
                    .map(|(at, value)| Point {
                        at: *at,
                        value: *value,
                        data: Opaque::default(),
                    })
                    .collect::<Vec<_>>(),
            )
            .expect("points serialize"),
        )
    }

    fn set(pairs: &[(f64, f64)]) -> Opaque {
        Opaque(
            serde_json::to_value(PointsIntent::SetPoints {
                points: pairs
                    .iter()
                    .map(|(at, value)| Point {
                        at: *at,
                        value: *value,
                        data: Opaque::default(),
                    })
                    .collect(),
            })
            .expect("intent serializes"),
        )
    }

    #[test]
    fn answers_the_new_state_and_the_payload_that_puts_it_back() {
        let before = points(&[(0.0, 1.0), (1.0, 0.0)]);
        let edited = edit(POINTS, &before, &set(&[(0.0, 0.5)])).expect("a curve is held here");
        assert!(edited.applied);
        assert_eq!(edited.state, points(&[(0.0, 0.5)]));
        // The inverse is read *before* the edit lands, which is the whole reason
        // one call answers both: it states the curve as it stood.
        assert_eq!(edited.current, Some(set(&[(0.0, 1.0), (1.0, 0.0)])));
    }

    #[test]
    fn a_resend_moves_nothing_and_is_still_answered() {
        let before = points(&[(0.0, 1.0)]);
        let edited = edit(POINTS, &before, &set(&[(0.0, 1.0)])).expect("a curve is held here");
        assert!(!edited.applied, "a resend is not an edit");
        assert_eq!(edited.state, before, "and it leaves the state where it was");
        assert!(
            edited.reason.is_none(),
            "refused for nothing: it simply did not move"
        );
    }

    #[test]
    fn a_timeline_of_events_is_held_the_same_way() {
        let before = Opaque(serde_json::json!([{"at": 0.0}]));
        let payload = Opaque(serde_json::json!({
            "intent": "setevents",
            "events": [{"at": 0.0}, {"at": 2.0}],
        }));
        let edited = edit(EVENTS, &before, &payload).expect("a timeline is held here");
        assert!(edited.applied);
        assert_eq!(
            edited.state,
            Opaque(serde_json::json!([{"at": 0.0}, {"at": 2.0}]))
        );
        assert_eq!(
            edited.current,
            Some(Opaque(
                serde_json::json!({"intent": "setevents", "events": [{"at": 0.0}]})
            ))
        );
    }

    #[test]
    fn the_domains_whose_state_is_not_a_value_are_not_held_here() {
        // Not an omission, and the two reasons differ: a tree's edit needs a
        // version and a grid, and a span of samples is a borrowed view whose
        // frames are in a buffer somewhere else.
        let anything = Opaque(serde_json::json!([]));
        let payload = set(&[(0.0, 0.0)]);
        assert!(edit(TREE, &anything, &payload).is_none());
        assert!(edit(SAMPLES, &anything, &payload).is_none());
        assert!(edit("smaples", &anything, &payload).is_none());
    }

    #[test]
    fn a_state_that_is_not_this_vocabulary_is_declined_rather_than_guessed() {
        let not_points = Opaque(serde_json::json!({"curve": "later"}));
        assert!(edit(POINTS, &not_points, &set(&[(0.0, 0.0)])).is_none());
    }
}
