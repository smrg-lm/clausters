//! O2's acceptance: the vocabulary is absolute, applying is idempotent, and
//! every outcome names an effective value — including a refusal, which is the
//! previous value handed back rather than an error.

use super::*;
use crate::{Grouping, Lifetime, SourceId, SourceRef};

fn event(id: u64) -> Node {
    Node::new(
        NodeId(id),
        Body::Event {
            config: Opaque::none(),
            fires: None,
        },
    )
}

fn placed(offset: Beats, node: Node) -> Member {
    Member {
        offset,
        dur: None,
        node,
    }
}

fn doc() -> Document {
    Document::new(Node::new(
        NodeId(1),
        Body::Set {
            grouping: Grouping::Concrete,
            members: vec![placed(0.0, event(2)), placed(4.0, event(3))],
        },
    ))
}

fn config(json: serde_json::Value) -> Opaque {
    Opaque(json)
}

#[test]
fn applying_the_same_intent_twice_leaves_the_same_document() {
    // Idempotence is what makes a resend harmless on a lossy leg, and it is a
    // property of the vocabulary being absolute rather than of any care taken
    // here.
    let mut a = doc();
    let intent = Intent::Place {
        node: NodeId(2),
        offset: 2.5,
        dur: Some(1.0),
    };
    apply(&mut a, &intent, &Against::unstated(), &Rules::none());
    let once = a.clone();
    let second = apply(&mut a, &intent, &Against::unstated(), &Rules::none());

    assert_eq!(a.root, once.root);
    assert!(!second.applied, "nothing changed, so nothing was applied");
    assert_eq!(second.effective, intent);
}

#[test]
fn a_version_moves_only_when_the_document_did() {
    let mut d = doc();
    assert_eq!(
        d.version,
        crate::FIRST_VERSION,
        "one, since zero means unstated"
    );
    apply(
        &mut d,
        &Intent::Place {
            node: NodeId(2),
            offset: 1.0,
            dur: None,
        },
        &Against::unstated(),
        &Rules::none(),
    );
    assert_eq!(d.version, crate::FIRST_VERSION + 1);
    // The same edit again changes nothing, and a version that moved anyway
    // would make every other reader re-sync for nothing.
    apply(
        &mut d,
        &Intent::Place {
            node: NodeId(2),
            offset: 1.0,
            dur: None,
        },
        &Against::unstated(),
        &Rules::none(),
    );
    assert_eq!(d.version, crate::FIRST_VERSION + 1);
}

#[test]
fn a_transformed_edit_reports_the_value_it_became() {
    // The defect this whole design started from: the owner snaps, the view
    // never hears about it, and the two disagree by half a grid step with no
    // message capable of saying so.
    let mut d = doc();
    let outcome = apply(
        &mut d,
        &Intent::Place {
            node: NodeId(2),
            offset: 4.3,
            dur: None,
        },
        &Against::unstated(),
        &Rules::quantized(1.0),
    );
    assert!(outcome.applied);
    assert_eq!(
        outcome.effective,
        Intent::Place {
            node: NodeId(2),
            offset: 4.0,
            dur: None
        }
    );
    assert!(outcome.reason.is_some(), "a transformation says so");
}

#[test]
fn an_edit_that_needed_no_transforming_says_nothing() {
    let mut d = doc();
    let outcome = apply(
        &mut d,
        &Intent::Place {
            node: NodeId(2),
            offset: 3.0,
            dur: None,
        },
        &Against::unstated(),
        &Rules::quantized(1.0),
    );
    assert!(outcome.applied);
    assert_eq!(outcome.reason, None);
}

#[test]
fn a_refusal_hands_back_the_previous_value_rather_than_an_error() {
    // The generator-note case, in the crate that answers it. The host drew the
    // note where the hand put it; what comes back is where it actually is, so
    // the picture corrects itself with no branch anywhere for "refused".
    let mut d = Document::new(Node::new(
        NodeId(1),
        Body::Generator {
            config: Opaque::none(),
            rendered: None,
        },
    ));
    let outcome = apply(
        &mut d,
        &Intent::SetMembers {
            node: NodeId(1),
            members: vec![placed(2.0, event(9))],
        },
        &Against::unstated(),
        &Rules::none(),
    );
    assert!(!outcome.applied);
    assert_eq!(
        outcome.effective,
        Intent::SetMembers {
            node: NodeId(1),
            members: vec![]
        }
    );
    assert_eq!(
        outcome.reason.as_deref(),
        Some("this body holds no members")
    );
    assert_eq!(d.version, crate::FIRST_VERSION, "a refusal is not an edit");
}

#[test]
fn an_edit_naming_a_node_that_is_gone_is_refused_and_says_so() {
    let mut d = doc();
    let intent = Intent::Place {
        node: NodeId(99),
        offset: 1.0,
        dur: None,
    };
    let outcome = apply(&mut d, &intent, &Against::unstated(), &Rules::none());
    assert!(!outcome.applied);
    // There is no previous value to hand back for a node the document does not
    // hold, so the caller's own is what comes back -- and the reason is what
    // carries the information.
    assert_eq!(outcome.effective, intent);
    assert_eq!(outcome.reason.as_deref(), Some("no such node"));
}

#[test]
fn every_intent_is_absolute_and_reports_an_effective_value() {
    // The guard for the vocabulary itself: a new intent cannot be added without
    // an outcome that names a value, and none of them may be a delta. If this
    // list stops matching the enum, the match below fails to compile.
    let mut d = doc();
    let intents = [
        Intent::Place {
            node: NodeId(2),
            offset: 1.0,
            dur: Some(2.0),
        },
        Intent::Configure {
            node: NodeId(2),
            config: config(serde_json::json!({"amp": 0.5})),
        },
        Intent::SetMembers {
            node: NodeId(1),
            members: vec![placed(0.0, event(2))],
        },
        Intent::WriteSamples {
            node: NodeId(2),
            channel: 0,
            start: 0,
            values: vec![0.1],
        },
    ];
    for intent in &intents {
        let outcome = apply(&mut d, intent, &Against::unstated(), &Rules::none());
        assert_eq!(
            std::mem::discriminant(&outcome.effective),
            std::mem::discriminant(intent),
            "an outcome describes the same kind of edit it answers"
        );
        // Exhaustive on purpose: adding a variant fails here until someone says
        // what its absolute form is.
        match intent {
            Intent::Place { .. }
            | Intent::Configure { .. }
            | Intent::SetMembers { .. }
            | Intent::WriteSamples { .. } => {}
        }
    }
}

#[test]
fn a_configuration_is_replaced_whole_rather_than_patched() {
    // A patch is a delta by another name: two overlapping patches applied out
    // of order give two different documents, which is what the absolute rule
    // exists to prevent.
    let mut d = doc();
    apply(
        &mut d,
        &Intent::Configure {
            node: NodeId(2),
            config: config(serde_json::json!({"amp": 0.5, "pan": -1})),
        },
        &Against::unstated(),
        &Rules::none(),
    );
    let outcome = apply(
        &mut d,
        &Intent::Configure {
            node: NodeId(2),
            config: config(serde_json::json!({"amp": 0.9})),
        },
        &Against::unstated(),
        &Rules::none(),
    );
    assert!(outcome.applied);
    let Body::Event { config: now, .. } = &d.find(NodeId(2)).unwrap().body else {
        panic!("not an event")
    };
    assert_eq!(
        now.0,
        serde_json::json!({"amp": 0.9}),
        "pan is gone, not merged"
    );
}

#[test]
fn a_transposition_travels_as_the_pitch_it_became() {
    // There is no `transpose` intent, and that is the rule rather than an
    // omission: a relative edit would have to be rebased against a corrected
    // state, and rebasing is exactly the replay the host cannot do.
    let mut d = doc();
    let outcome = apply(
        &mut d,
        &Intent::Configure {
            node: NodeId(2),
            config: config(serde_json::json!({"midinote": 64})),
        },
        &Against::unstated(),
        &Rules::none(),
    );
    assert!(outcome.applied);
    let Intent::Configure { config, .. } = &outcome.effective else {
        panic!()
    };
    assert_eq!(config.0["midinote"], 64);
}

#[test]
fn a_set_keeps_the_ids_of_the_members_that_survived_an_edit() {
    // A roll edit arrives as the resulting list. What was already there has to
    // come out the same node, or a log would invert the wrong one and a view
    // would redraw something it never had.
    let mut d = doc();
    apply(
        &mut d,
        &Intent::SetMembers {
            node: NodeId(1),
            members: vec![placed(1.0, event(3)), placed(2.0, event(7))],
        },
        &Against::unstated(),
        &Rules::none(),
    );
    let ids: Vec<NodeId> = d.root.members().iter().map(|m| m.node.id).collect();
    assert_eq!(ids, vec![NodeId(3), NodeId(7)]);
    assert_eq!(d.find(NodeId(2)), None, "the removed note is gone");
}

#[test]
fn writing_material_bumps_the_sources_generation_and_not_the_samples() {
    // The document describes where material is, never what it holds. What
    // applying does is move the counter every reader of that material watches;
    // writing the samples is the owner's next step, against the working buffer.
    let mut d = Document::new(Node::new(
        NodeId(1),
        Body::Buffer {
            source: SourceRef {
                source: SourceId(4),
                lifetime: Lifetime::Temporary,
                generation: 2,
                range: None,
            },
            config: Opaque::none(),
        },
    ));
    let outcome = apply(
        &mut d,
        &Intent::WriteSamples {
            node: NodeId(1),
            channel: 0,
            start: 100,
            values: vec![0.0, 0.5],
        },
        &Against::unstated(),
        &Rules::none(),
    );
    assert!(outcome.applied);
    let Body::Buffer { source, .. } = &d.root.body else {
        panic!()
    };
    assert_eq!(source.generation, 3);
}

#[test]
fn only_material_can_be_written() {
    let mut d = doc();
    let outcome = apply(
        &mut d,
        &Intent::WriteSamples {
            node: NodeId(2),
            channel: 0,
            start: 0,
            values: vec![0.1],
        },
        &Against::unstated(),
        &Rules::none(),
    );
    assert!(!outcome.applied);
    assert_eq!(
        outcome.reason.as_deref(),
        Some("only material can be written")
    );
}

#[test]
fn an_intent_round_trips_through_the_wire_form() {
    // Intents cross a socket to reach the owner, so the vocabulary is part of
    // the format and not only of the API.
    let intent = Intent::Place {
        node: NodeId(12),
        offset: 3.5,
        dur: Some(2.0),
    };
    let json = serde_json::to_string(&intent).unwrap();
    assert_eq!(serde_json::from_str::<Intent>(&json).unwrap(), intent);
    assert!(json.contains("\"intent\":\"place\""));
}

#[test]
fn a_nested_node_is_reached_wherever_it_sits() {
    let mut d = Document::new(Node::new(
        NodeId(1),
        Body::Set {
            grouping: Grouping::Concrete,
            members: vec![placed(
                0.0,
                Node::new(
                    NodeId(2),
                    Body::Set {
                        grouping: Grouping::Concrete,
                        members: vec![placed(0.0, event(3))],
                    },
                ),
            )],
        },
    ));
    let outcome = apply(
        &mut d,
        &Intent::Place {
            node: NodeId(3),
            offset: 7.0,
            dur: None,
        },
        &Against::unstated(),
        &Rules::none(),
    );
    assert!(outcome.applied);
    assert_eq!(d.root.members()[0].node.members()[0].offset, 7.0);
}

// ---- O4: the version, and staleness ----

/// A buffer node at a known generation, for the destructive-edit cases.
fn material(generation: u64) -> Document {
    Document::new(Node::new(
        NodeId(1),
        Body::Buffer {
            source: SourceRef {
                source: SourceId(4),
                lifetime: Lifetime::Temporary,
                generation,
                range: None,
            },
            config: Opaque::none(),
        },
    ))
}

#[test]
fn an_edit_made_against_the_current_version_applies() {
    let mut d = doc();
    let now = Against::at(d.version);
    let outcome = apply(
        &mut d,
        &Intent::Place {
            node: NodeId(2),
            offset: 3.0,
            dur: None,
        },
        &now,
        &Rules::none(),
    );
    assert!(outcome.applied && !outcome.stale);
}

#[test]
fn an_edit_made_against_a_superseded_version_is_refused_and_hands_back_the_present_value() {
    // O4's acceptance. The editor saw version 0; something else moved the
    // document; the edit is reported as stale rather than applied blind, and
    // what comes back is what the document says now -- which is all the caller
    // needs to re-sync, since adopting the effective value is what it does with
    // every other outcome too.
    let mut d = doc();
    let seen = d.version;
    apply(
        &mut d,
        &Intent::Place {
            node: NodeId(3),
            offset: 8.0,
            dur: None,
        },
        &Against::unstated(),
        &Rules::none(),
    );
    assert_ne!(d.version, seen, "something else edited in between");

    let moved = d.version;
    let outcome = apply(
        &mut d,
        &Intent::Place {
            node: NodeId(2),
            offset: 3.0,
            dur: None,
        },
        &Against::at(seen),
        &Rules::none(),
    );
    assert!(outcome.stale && !outcome.applied);
    assert_eq!(
        outcome.effective,
        Intent::Place {
            node: NodeId(2),
            offset: 0.0,
            dur: None,
        },
        "the value the document holds now, not the one that was proposed"
    );
    assert_eq!(d.version, moved, "a refusal is not an edit");
}

#[test]
fn a_stale_rewrite_does_not_delete_what_arrived_in_between() {
    // The reason the check exists at all. `SetMembers` is absolute *and*
    // whole, so an edit made against an old picture states a list that never
    // had the new note in it -- applying it would be a silent deletion, which
    // is the one failure the whole mechanism is built to make impossible.
    let mut d = doc();
    let seen = d.version;
    let mut grown: Vec<Member> = d.root.members().to_vec();
    grown.push(placed(9.0, event(7)));
    apply(
        &mut d,
        &Intent::SetMembers {
            node: NodeId(1),
            members: grown,
        },
        &Against::unstated(),
        &Rules::none(),
    );

    // The editor's own list, made before the seventh note existed.
    let outcome = apply(
        &mut d,
        &Intent::SetMembers {
            node: NodeId(1),
            members: vec![placed(1.0, event(2)), placed(4.0, event(3))],
        },
        &Against::at(seen),
        &Rules::none(),
    );
    assert!(outcome.stale);
    let ids: Vec<NodeId> = d.root.members().iter().map(|m| m.node.id).collect();
    assert_eq!(ids, vec![NodeId(2), NodeId(3), NodeId(7)]);
}

#[test]
fn an_edit_from_ahead_of_the_document_is_stale_too() {
    // And it is the worse case rather than a harmless one: a version the
    // document has never reached means the two are not talking about the same
    // piece, so applying would write an edit meant for another one.
    let mut d = doc();
    let ahead = Against::at(d.version + 5);
    let outcome = apply(
        &mut d,
        &Intent::Place {
            node: NodeId(2),
            offset: 3.0,
            dur: None,
        },
        &ahead,
        &Rules::none(),
    );
    assert!(outcome.stale && !outcome.applied);
}

#[test]
fn an_unstated_claim_skips_the_check() {
    // What a script looks like: it read the document a line ago, so there is no
    // stale picture to protect, and an older client cannot say either way.
    let mut d = doc();
    apply(
        &mut d,
        &Intent::Place {
            node: NodeId(3),
            offset: 8.0,
            dur: None,
        },
        &Against::unstated(),
        &Rules::none(),
    );
    let outcome = apply(
        &mut d,
        &Intent::Place {
            node: NodeId(2),
            offset: 3.0,
            dur: None,
        },
        &Against::unstated(),
        &Rules::none(),
    );
    assert!(outcome.applied && !outcome.stale);
}

/// A write says **which channel** it covers, and the field is optional on the
/// wire: a document written before it — and every mono edit — means channel 0.
#[test]
fn a_write_names_its_channel_and_an_absent_one_is_the_first() {
    let mut d = material(2);
    let write = Intent::WriteSamples {
        node: NodeId(1),
        channel: 1,
        start: 4,
        values: vec![0.5, 0.25],
    };
    let outcome = apply(&mut d, &write, &Against::unstated(), &Rules::none());
    assert!(outcome.applied);
    assert_eq!(
        outcome.effective, write,
        "the channel is part of what the edit was"
    );

    let json = serde_json::to_value(&write).expect("an intent serializes");
    assert_eq!(json["channel"], 1);
    let older = serde_json::json!({
        "intent": "writesamples",
        "node": 1,
        "start": 4,
        "values": [0.5, 0.25],
    });
    let read: Intent = serde_json::from_value(older).expect("an intent without a channel reads");
    assert_eq!(
        read,
        Intent::WriteSamples {
            node: NodeId(1),
            channel: 0,
            start: 4,
            values: vec![0.5, 0.25],
        }
    );
}

#[test]
fn material_rewritten_underneath_makes_a_write_stale() {
    let mut d = material(2);
    let write = Intent::WriteSamples {
        node: NodeId(1),
        channel: 0,
        start: 100,
        values: vec![0.25],
    };
    apply(&mut d, &write, &Against::unstated(), &Rules::none());

    // The editor drew over generation 2; the material is at 3 now.
    let drew_over = Against::at(d.version).with_generation(2);
    let outcome = apply(&mut d, &write, &drew_over, &Rules::none());
    assert!(outcome.stale && !outcome.applied);
    assert_eq!(
        outcome.effective,
        Intent::WriteSamples {
            node: NodeId(1),
            channel: 0,
            start: 100,
            values: Vec::new(),
        },
        "the samples are not in the document, so there is nothing to hand back"
    );
}

#[test]
fn a_generation_can_be_claimed_without_a_document_version() {
    // A waveform view over one source holds no document at all. It can still
    // say which generation of the material it drew, which is the only claim
    // that means anything to it -- and it is enough to catch the conflict.
    let mut d = material(2);
    let write = Intent::WriteSamples {
        node: NodeId(1),
        channel: 0,
        start: 0,
        values: vec![0.5],
    };
    let fresh = apply(
        &mut d,
        &write,
        &Against::unstated().with_generation(2),
        &Rules::none(),
    );
    assert!(fresh.applied && !fresh.stale);

    let again = apply(
        &mut d,
        &write,
        &Against::unstated().with_generation(2),
        &Rules::none(),
    );
    assert!(again.stale, "the first write moved the generation past it");
}

#[test]
fn staleness_never_shadows_a_better_reason() {
    // A node the document does not hold has an answer of its own, and it is
    // more useful than "stale" -- so the gate declines rather than reporting
    // the version it could have.
    let mut d = doc();
    apply(
        &mut d,
        &Intent::Place {
            node: NodeId(3),
            offset: 8.0,
            dur: None,
        },
        &Against::unstated(),
        &Rules::none(),
    );
    let outcome = apply(
        &mut d,
        &Intent::Place {
            node: NodeId(99),
            offset: 1.0,
            dur: None,
        },
        &Against::at(0),
        &Rules::none(),
    );
    assert!(!outcome.applied);
    assert!(!outcome.stale);
    assert_eq!(outcome.reason.as_deref(), Some("no such node"));
}
