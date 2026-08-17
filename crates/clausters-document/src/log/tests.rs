//! O5's acceptance: a run of gestures inverts back to the starting document
//! exactly, a redo re-emits the intent the gesture first sent, and what comes
//! back from an owner never enters the log.

use super::*;
use crate::intent::apply;
use crate::{Beats, Body, Grouping, Member, Node, NodeId};

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
            config: Opaque::none(),
        },
    ))
}

fn place(node: u64, offset: Beats) -> Intent {
    Intent::Place {
        node: NodeId(node),
        offset,
        dur: None,
    }
}

/// Applies whatever an undo handed back.
fn run(document: &mut Document, intents: Vec<Intent>) {
    for intent in intents {
        apply(document, &intent, &Against::unstated(), &Rules::none());
    }
}

#[test]
fn a_run_of_gestures_inverts_back_to_where_it_started() {
    // O5's acceptance, and the reason the vocabulary is absolute: the inverse
    // of an edit that states a value is the edit that states the previous one,
    // so undo needs no second path at all.
    let mut d = doc();
    let start = d.root.clone();
    let mut log = Log::new();

    for (node, offset) in [(2, 1.0), (3, 7.0), (2, 2.5), (3, 0.5)] {
        apply_logged(
            &mut d,
            &place(node, offset),
            &Against::unstated(),
            &Rules::none(),
            &mut log,
            "move",
        );
    }
    assert_ne!(d.root, start);
    assert_eq!(log.len(), 4);

    while let Some(inverse) = log.undo() {
        run(&mut d, inverse);
    }
    assert_eq!(d.root, start, "exactly, not approximately");
    assert!(!log.can_undo() && log.can_redo());
}

#[test]
fn a_redo_re_emits_the_intent_the_gesture_first_sent() {
    let mut d = doc();
    let mut log = Log::new();
    let outcome = apply_logged(
        &mut d,
        &place(2, 3.0),
        &Against::unstated(),
        &Rules::none(),
        &mut log,
        "move",
    );
    let applied = outcome.effective.clone();

    run(&mut d, log.undo().unwrap());
    let redone = log.redo().unwrap();
    assert_eq!(redone, vec![Step::Edit(applied)]);
}

#[test]
fn what_the_owner_transformed_is_what_gets_replayed() {
    // The forward half stores the *effective* intent, not the one proposed. A
    // redo that replayed the proposal would snap again -- harmless here, and
    // wrong the moment a rule is not idempotent.
    let mut d = doc();
    let mut log = Log::new();
    apply_logged(
        &mut d,
        &place(2, 4.3),
        &Against::unstated(),
        &Rules::quantized(1.0),
        &mut log,
        "move",
    );
    run(&mut d, log.undo().unwrap());
    let Some(Step::Edit(Intent::Place { offset, .. })) = log.redo().unwrap().pop() else {
        panic!("a placement");
    };
    assert_eq!(offset, 4.0);
}

#[test]
fn a_refused_edit_leaves_no_entry() {
    // Including a stale one. A refusal is not an edit: it does not move the
    // version, and there is nothing to invert.
    let mut d = doc();
    let mut log = Log::new();
    apply_logged(
        &mut d,
        &place(99, 1.0),
        &Against::unstated(),
        &Rules::none(),
        &mut log,
        "move a node that is not there",
    );
    let ahead = Against::at(d.version + 5);
    apply_logged(
        &mut d,
        &place(2, 1.0),
        &ahead,
        &Rules::none(),
        &mut log,
        "move against a version this document never had",
    );
    assert!(log.is_empty());
}

#[test]
fn an_edit_that_changed_nothing_leaves_no_entry() {
    // A resend over a lossy leg, or a gesture that put a clip back where it
    // was. Recording it would make one undo do nothing, which reads as broken.
    let mut d = doc();
    let mut log = Log::new();
    let same = place(2, 0.0);
    apply_logged(
        &mut d,
        &same,
        &Against::unstated(),
        &Rules::none(),
        &mut log,
        "move",
    );
    assert!(log.is_empty());
}

#[test]
fn what_the_owner_pushes_back_never_enters_the_log() {
    // The acknowledgement's state push is the document describing itself, not
    // an edit -- and the rule is mechanical rather than a habit, because the
    // only door into the log applies and records in one call. Whatever else a
    // caller does to the document is invisible here.
    let mut d = doc();
    let mut log = Log::new();
    apply_logged(
        &mut d,
        &place(2, 2.0),
        &Against::unstated(),
        &Rules::none(),
        &mut log,
        "move",
    );
    // The owner answering: applying the effective value again, the way a
    // re-read or a state push would.
    apply(&mut d, &place(2, 2.0), &Against::unstated(), &Rules::none());
    assert_eq!(log.len(), 1, "one gesture, one entry");
}

#[test]
fn a_new_edit_after_an_undo_drops_what_was_waiting_to_be_redone() {
    let mut d = doc();
    let mut log = Log::new();
    for offset in [1.0, 2.0, 3.0] {
        apply_logged(
            &mut d,
            &place(2, offset),
            &Against::unstated(),
            &Rules::none(),
            &mut log,
            "move",
        );
    }
    run(&mut d, log.undo().unwrap());
    run(&mut d, log.undo().unwrap());
    assert!(log.can_redo());

    apply_logged(
        &mut d,
        &place(3, 9.0),
        &Against::unstated(),
        &Rules::none(),
        &mut log,
        "move the other one",
    );
    assert!(!log.can_redo(), "the fork replaced the branch");
    assert_eq!(log.len(), 2);
}

#[test]
fn a_continuing_run_of_adjustments_is_one_undo() {
    // A hundred small moves of the same clip are one thing the person did. The
    // merged entry keeps the *oldest* inverse, so the undo lands where the run
    // started rather than one step back into it.
    let mut d = doc();
    let mut log = Log::new();
    let start = d.root.clone();
    for (i, offset) in [1.0, 1.5, 2.0, 2.5].into_iter().enumerate() {
        let mut entry = Entry::new(
            "move",
            Step::Edit(place(2, offset)),
            current(&d, &place(2, offset)).unwrap(),
        );
        if i > 0 {
            entry = entry.continuing();
        }
        apply(
            &mut d,
            &place(2, offset),
            &Against::unstated(),
            &Rules::none(),
        );
        log.record(entry);
    }
    assert_eq!(log.len(), 1);
    run(&mut d, log.undo().unwrap());
    assert_eq!(d.root, start);
}

#[test]
fn a_run_only_coalesces_into_the_same_kind_of_edit_on_the_same_node() {
    let mut log = Log::new();
    log.record(Entry::new("move", Step::Edit(place(2, 1.0)), place(2, 0.0)));
    // Same shape, different node: two things were done.
    log.record(Entry::new("move", Step::Edit(place(3, 1.0)), place(3, 4.0)).continuing());
    // Same node, different shape.
    log.record(
        Entry::new(
            "configure",
            Step::Edit(Intent::Configure {
                node: NodeId(3),
                config: Opaque(serde_json::json!({"amp": 0.5})),
            }),
            Intent::Configure {
                node: NodeId(3),
                config: Opaque::none(),
            },
        )
        .continuing(),
    );
    assert_eq!(log.len(), 3);
}

#[test]
fn the_labels_say_what_undo_and_redo_would_do() {
    let mut d = doc();
    let mut log = Log::new();
    apply_logged(
        &mut d,
        &place(2, 1.0),
        &Against::unstated(),
        &Rules::none(),
        &mut log,
        "move the clip",
    );
    assert_eq!(log.undo_label(), Some("move the clip"));
    assert_eq!(log.redo_label(), None);
    run(&mut d, log.undo().unwrap());
    assert_eq!(log.undo_label(), None);
    assert_eq!(log.redo_label(), Some("move the clip"));
}

// ---- the spill store ----

fn write(start: u64, values: Vec<f32>) -> Intent {
    Intent::WriteSamples {
        node: NodeId(1),
        channel: 0,
        start,
        values,
    }
}

#[test]
fn a_big_sample_payload_leaves_the_log_and_comes_back_whole() {
    let previous: Vec<f32> = (0..1024).map(|i| i as f32 * 0.001).collect();
    let written: Vec<f32> = vec![0.5; 1024];
    let mut log = Log::new();
    log.record(Entry::new(
        "draw",
        Step::Edit(write(0, written.clone())),
        write(0, previous.clone()),
    ));

    let undone = log.undo().unwrap();
    assert_eq!(undone, vec![write(0, previous)], "the span, put back whole");
    let redone = log.redo().unwrap();
    assert_eq!(redone, vec![Step::Edit(write(0, written))]);
}

#[test]
fn an_undo_redo_pair_naming_the_same_bytes_holds_one_copy() {
    // What content addressing is for: a stroke that writes silence over silence
    // has the same span on both sides, and storing it twice would double the
    // cost of every uniform edit.
    let same: Vec<f32> = vec![0.0; 1024];
    let mut store = MemorySpill::new();
    let a = store.put(&[0u8; 4096]);
    let b = store.put(&[0u8; 4096]);
    assert_eq!(a, b);
    assert_eq!(store.len(), 1);
    store.release(a);
    assert_eq!(store.len(), 1, "the other reference still holds it");
    store.release(b);
    assert!(store.is_empty());

    // And through the log, which is where it actually happens.
    let mut log = Log::new();
    log.record(Entry::new(
        "draw",
        Step::Edit(write(0, same.clone())),
        write(0, same),
    ));
    assert_eq!(log.undo().unwrap().len(), 1);
}

#[test]
fn a_small_payload_stays_in_the_log() {
    // The store exists for the case whose size follows the audio. Sending four
    // samples through a file would cost more than it saves.
    let mut log = Log::new();
    log.record(Entry::new(
        "nudge",
        Step::Edit(write(10, vec![0.1, 0.2])),
        write(10, vec![0.3, 0.4]),
    ));
    assert_eq!(log.undo().unwrap(), vec![write(10, vec![0.3, 0.4])]);
}

#[test]
fn a_deterministic_operation_stores_its_parameters_and_not_its_result() {
    // The asymmetry the placement buys. Undoing a normalize is the old samples
    // and nothing else can be; redoing it is the word "normalize" plus its
    // parameters, which the owner re-runs. A log held by the host could not do
    // this, because the host has no algorithms.
    let previous: Vec<f32> = vec![0.25; 4096];
    let mut log = Log::new();
    log.record(Entry::new(
        "normalize",
        Step::Recompute(Opaque(serde_json::json!({"op": "normalize", "peak": 1.0}))),
        write(0, previous.clone()),
    ));

    assert_eq!(log.undo().unwrap(), vec![write(0, previous)]);
    let redone = log.redo().unwrap();
    assert!(
        matches!(redone.as_slice(), [Step::Recompute(_)]),
        "the owner re-runs it rather than replaying four megabytes"
    );
}

#[test]
fn a_transaction_unwinds_in_the_order_it_was_laid_down() {
    let mut d = doc();
    let start = d.root.clone();
    let mut log = Log::new();
    let entry = Entry::new(
        "move both",
        Step::Edit(place(2, 5.0)),
        current(&d, &place(2, 5.0)).unwrap(),
    )
    .and(
        Step::Edit(place(3, 6.0)),
        current(&d, &place(3, 6.0)).unwrap(),
    );
    apply(&mut d, &place(2, 5.0), &Against::unstated(), &Rules::none());
    apply(&mut d, &place(3, 6.0), &Against::unstated(), &Rules::none());
    log.record(entry);
    assert_eq!(log.len(), 1, "one transaction, whatever it holds");

    run(&mut d, log.undo().unwrap());
    assert_eq!(d.root, start);
}

// ---- the budget ----

#[test]
fn the_oldest_entries_fall_off_and_take_their_spilled_bytes_with_them() {
    let mut log = Log::new().budget(2);
    for i in 0..5u64 {
        log.record(Entry::new(
            "draw",
            Step::Edit(write(i, vec![i as f32; 1024])),
            write(i, vec![-(i as f32); 1024]),
        ));
    }
    assert_eq!(log.len(), 2, "the budget holds");
    // The two that survived are the last two, and they still invert.
    let undone = log.undo().unwrap();
    assert_eq!(undone, vec![write(4, vec![-4.0; 1024])]);
}

#[test]
fn clearing_forgets_everything_and_releases_what_was_spilled() {
    // What loading another document leaves behind: a history of edits to a
    // document that is not open inverts nothing.
    let mut log = Log::new();
    log.record(Entry::new(
        "draw",
        Step::Edit(write(0, vec![0.5; 1024])),
        write(0, vec![0.25; 1024]),
    ));
    assert!(log.can_undo());
    log.clear();
    assert!(!log.can_undo() && !log.can_redo() && log.is_empty());
    assert_eq!(log.undo(), None);
}
