//! O5's acceptance: a run of gestures inverts back to the starting document
//! exactly, a redo re-emits the intent the gesture first sent, and what comes
//! back from an owner never enters the log.

use super::*;
use crate::intent::apply;
use crate::{Beats, Body, Grouping, Member, Node, NodeId};

fn clang(id: u64) -> Node {
    Node::new(
        NodeId(id),
        Body::Clang {
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
        Body::Aggregate {
            grouping: Grouping::Concrete,
            members: vec![placed(0.0, clang(2)), placed(4.0, clang(3))],
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

    while let Some(undone) = log.undo() {
        run(&mut d, undone.intents);
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

    run(&mut d, log.undo().unwrap().intents);
    let redone = log.redo().unwrap();
    assert_eq!(redone.intents, vec![applied]);
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
    run(&mut d, log.undo().unwrap().intents);
    let Some(Intent::Place { offset, .. }) = log.redo().unwrap().intents.pop() else {
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
    run(&mut d, log.undo().unwrap().intents);
    run(&mut d, log.undo().unwrap().intents);
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
    run(&mut d, log.undo().unwrap().intents);
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
    assert_eq!(log.undo_label().as_deref(), Some("move the clip"));
    assert_eq!(log.redo_label(), None);
    run(&mut d, log.undo().unwrap().intents);
    assert_eq!(log.undo_label(), None);
    assert_eq!(log.redo_label().as_deref(), Some("move the clip"));
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
    assert_eq!(
        undone.intents,
        vec![write(0, previous)],
        "the span, put back whole"
    );
    let redone = log.redo().unwrap();
    assert_eq!(redone.intents, vec![write(0, written)]);
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
    assert_eq!(log.undo().unwrap().intents.len(), 1);
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
    assert_eq!(log.undo().unwrap().intents, vec![write(10, vec![0.3, 0.4])]);
}

#[test]
fn a_deterministic_operation_stores_its_parameters_and_not_its_result() {
    // The asymmetry the placement exists for. Undoing a normalize is the old samples
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

    assert_eq!(log.undo().unwrap().intents, vec![write(0, previous)]);
    let redone = log.redo().unwrap();
    assert!(
        matches!(redone.remaining.as_slice(), [Step::Recompute(_)]),
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

    run(&mut d, log.undo().unwrap().intents);
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
    assert_eq!(undone.intents, vec![write(4, vec![-4.0; 1024])]);
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

// ---- two domains, one pile ----

#[test]
fn a_history_holding_a_document_and_a_curve_undoes_them_in_one_order() {
    // O16's acceptance, and the whole reason the pile carries no vocabulary: an
    // application composing a multitrack and a curve has one history, and the
    // interleaved order is the pile. Nothing here routes by anything but the
    // structure each leg names.
    use crate::history::{Editable, History};
    use crate::points::{POINTS, Point, Points, PointsIntent, payload as points_payload};

    let mut d = doc();
    let start = d.root.clone();
    let mut curve = Points::new(vec![Point {
        at: 0.0,
        value: 0.0,
        data: Opaque::default(),
    }]);

    let mut history = History::new();
    let tree = history.register(TREE);
    let points = history.register(POINTS);

    history.apply(
        tree,
        &mut Tree::new(&mut d),
        &super::payload(&place(2, 5.0)),
        "move the clip",
    );
    history.apply(
        points,
        &mut curve,
        &points_payload(&PointsIntent::SetPoints {
            points: vec![Point {
                at: 0.0,
                value: 1.0,
                data: Opaque::default(),
            }],
        }),
        "draw",
    );
    history.apply(
        tree,
        &mut Tree::new(&mut d),
        &super::payload(&place(3, 7.0)),
        "move the other",
    );
    assert_eq!(history.len(), 3, "one pile over both");

    // Undoing walks that one order, and each leg says which structure it is
    // for -- which is all a caller needs to route it.
    for expected in [tree, points, tree] {
        let undone = history.undo().expect("something to undo").legs;
        for (structure, load) in undone {
            assert_eq!(structure, expected);
            if structure == tree {
                let mut state = Tree::new(&mut d);
                state.apply(&load);
            } else {
                curve.apply(&load);
            }
        }
    }
    assert_eq!(d.root, start, "the document, back where it started");
    assert_eq!(
        curve.0,
        vec![Point {
            at: 0.0,
            value: 0.0,
            data: Opaque::default(),
        }],
        "and the curve with it"
    );
    assert!(!history.can_undo());
}

#[test]
fn the_arrangements_own_door_still_records_through_the_generic_one() {
    // `apply_logged` is `History::apply` wearing the arrangement's vocabulary.
    // What this checks is that nothing about the refusal rules moved: a stale
    // edit is refused and leaves no entry, and the outcome still reads as an
    // intent.
    let mut d = doc();
    let mut log = Log::new();
    let stale = Against {
        version: 99,
        generation: None,
    };
    let outcome = apply_logged(
        &mut d,
        &place(2, 5.0),
        &stale,
        &Rules::none(),
        &mut log,
        "move",
    );
    assert!(!outcome.applied && outcome.stale);
    assert!(log.is_empty(), "a refusal is not an edit");

    let outcome = apply_logged(
        &mut d,
        &place(2, 5.0),
        &Against::unstated(),
        &Rules::none(),
        &mut log,
        "move",
    );
    assert!(outcome.applied);
    assert_eq!(outcome.effective, place(2, 5.0));
    assert_eq!(log.len(), 1);
}

/// The shape `O20` is for: a stroke over a *placed* take is one gesture with a
/// leg in each domain — the tree's, which says the samples moved, and the
/// samples' own, which says what they now hold and what they held. One entry,
/// undone in one step, and consistent at every point in between.
mod a_stroke_over_a_placed_take {
    use super::*;
    use crate::samples::{SAMPLES, Samples, SamplesIntent};
    use crate::{Lifetime, SourceId, SourceRef};

    fn take(id: u64) -> Node {
        Node::new(
            NodeId(id),
            Body::Vector {
                source: SourceRef {
                    source: SourceId(1),
                    lifetime: Lifetime::Session,
                    generation: 0,
                    range: None,
                },
                config: Opaque::none(),
            },
        )
    }

    fn document() -> Document {
        Document::new(Node::new(
            NodeId(1),
            Body::Aggregate {
                grouping: Grouping::Concrete,
                members: vec![placed(0.0, take(2))],
                config: Opaque::none(),
            },
        ))
    }

    fn write(start: u64, values: &[f32]) -> Opaque {
        crate::samples::payload(&SamplesIntent::Write {
            channel: 0,
            start,
            values: values.to_vec(),
        })
    }

    fn generation(document: &Document) -> u64 {
        let Body::Aggregate { members, .. } = &document.root.body else {
            unreachable!("the root is an aggregate")
        };
        let Body::Vector { source, .. } = &members[0].node.body else {
            unreachable!("the member is a take")
        };
        source.generation
    }

    #[test]
    fn is_one_entry_with_a_leg_in_each_domain_and_one_undo() {
        let mut history = History::new();
        let tree_id = history.register("tree");
        let samples_id = history.register(SAMPLES);

        let mut document = document();
        let mut data = vec![0.0, 0.1, 0.2, 0.3];

        let outcomes = {
            let mut tree = Tree::new(&mut document);
            let mut samples = Samples::interleaved(&mut data, 1);
            let stroke = crate::log::payload(&Intent::WriteSamples {
                node: NodeId(2),
                channel: 0,
                start: 1,
                values: vec![-1.0, -2.0],
            });
            history.transact(
                "draw over the take",
                &mut [
                    (tree_id, &mut tree, stroke),
                    (samples_id, &mut samples, write(1, &[-1.0, -2.0])),
                ],
            )
        };
        assert!(outcomes.iter().all(|o| o.applied), "both legs landed");
        assert_eq!(
            generation(&document),
            1,
            "readers are told the samples moved"
        );
        assert_eq!(data, vec![0.0, -1.0, -2.0, 0.3], "and the samples did move");
        assert_eq!(history.len(), 1, "one gesture, one entry");

        // One step, both structures, each leg routed to the domain it was
        // registered under -- which is the whole of what the registry is for.
        let undone = history.undo().expect("something to undo");
        assert_eq!(undone.legs.len(), 2);
        {
            let mut tree = Tree::new(&mut document);
            let mut samples = Samples::interleaved(&mut data, 1);
            for (structure, payload) in undone.legs {
                if structure == tree_id {
                    tree.apply(&payload);
                } else {
                    samples.apply(&payload);
                }
            }
        }
        assert_eq!(data, vec![0.0, 0.1, 0.2, 0.3], "the samples came back");
        assert_eq!(
            generation(&document),
            2,
            "and the generation moved again rather than backwards: a reader's \
             copy is stale either way, and a history is not a clock"
        );
    }

    #[test]
    fn a_refused_leg_leaves_neither_structure_moved() {
        let mut history = History::new();
        let tree_id = history.register("tree");
        let samples_id = history.register(SAMPLES);

        let mut document = document();
        let mut data = vec![0.0, 0.1];

        let outcomes = {
            let mut tree = Tree::new(&mut document);
            let mut samples = Samples::interleaved(&mut data, 1);
            let stroke = crate::log::payload(&Intent::WriteSamples {
                node: NodeId(2),
                channel: 0,
                start: 0,
                values: vec![-1.0],
            });
            // The span runs off the end of the samples: the tree's leg is
            // legal and the samples' is not.
            history.transact(
                "draw over the take",
                &mut [
                    (tree_id, &mut tree, stroke),
                    (samples_id, &mut samples, write(5, &[-1.0])),
                ],
            )
        };
        assert!(outcomes.iter().all(|o| !o.applied));
        assert_eq!(data, vec![0.0, 0.1], "the samples never moved");
        assert_eq!(history.len(), 0, "and nothing was recorded");
        // The tree's leg *was* put back -- its inverse was applied -- and the
        // generation is 2 rather than 0 because the counter is monotonic: it
        // answers "is my copy still good", and a reader that saw generation 1
        // between the two has to be told it is not, whichever way the document
        // then went.
        assert_eq!(generation(&document), 2);
    }
}
