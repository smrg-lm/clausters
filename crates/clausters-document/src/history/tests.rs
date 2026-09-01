use super::*;
use serde_json::json;

/// A structure with no document behind it: the notes of a roll the caller
/// built. Its whole vocabulary is one verb — *the notes are now these* — which
/// is what a domain has to bring, and all it has to bring.
#[derive(Debug, Default, PartialEq)]
struct Notes(Vec<i64>);

impl Editable for Notes {
    fn apply(&mut self, payload: &Opaque) -> Applied {
        let Ok(notes) = serde_json::from_value::<Vec<i64>>(payload.0.clone()) else {
            return Applied::refused(self.now(), "not this roll's vocabulary");
        };
        self.0 = notes;
        Applied {
            effective: self.now(),
            applied: true,
            reason: None,
            stale: false,
        }
    }

    fn current(&self, _payload: &Opaque) -> Option<Opaque> {
        Some(self.now())
    }

    fn coalesce_key(&self, _payload: &Opaque) -> Option<String> {
        Some("notes".to_string())
    }
}

impl Notes {
    /// Sets the notes and hands back the payload that would put them back —
    /// the inverse, read *before* the edit lands, which is the rule every
    /// domain follows.
    fn set(&mut self, notes: &[i64]) -> Opaque {
        let before = Opaque(json!(self.0));
        self.0 = notes.to_vec();
        before
    }

    fn adopt(&mut self, payload: &Opaque) {
        self.0 = serde_json::from_value(payload.0.clone()).expect("the notes' own payload");
    }

    fn now(&self) -> Opaque {
        Opaque(json!(self.0))
    }
}

/// One edit over one structure, ready to record.
fn edit(structure: StructureId, label: &str, now: Opaque, before: Opaque) -> Entry {
    Entry::new(label, structure, Step::Edit(now), before)
}

#[test]
fn a_structure_with_no_document_behind_it_has_a_working_history() {
    // The shape the module exists for as much as the arrangement's: a curve, a
    // buffer, a roll the client built, edited in a view and read back. Nothing
    // in this test is a `Document`.
    let mut history = History::new();
    let roll = history.register("notes");
    let mut notes = Notes(vec![60, 64]);

    let before = notes.set(&[60, 65]);
    history.record(edit(roll, "move a note", notes.now(), before));
    assert_eq!(notes.0, vec![60, 65]);

    for (structure, payload) in history.undo().expect("something to undo").legs {
        assert_eq!(structure, roll);
        notes.adopt(&payload);
    }
    assert_eq!(notes.0, vec![60, 64], "back where it started");

    for (structure, payload) in history.redo().expect("something to redo").edits {
        assert_eq!(structure, roll);
        notes.adopt(&payload);
    }
    assert_eq!(notes.0, vec![60, 65]);
}

#[test]
fn two_views_of_one_structure_walk_one_order() {
    // The measured defect, inverted. A and B are two editors over one roll --
    // the multitrack and a dedicated view, say. With a history each, A's undo
    // reverted across B's edit and left a state nobody was in; with one
    // history there is no "A's own history" to step out of order.
    let mut history = History::new();
    let roll = history.register("notes");
    let mut notes = Notes(vec![60, 64]);

    // A edits.
    let before = notes.set(&[60, 65]);
    history.record(edit(roll, "A moves the second", notes.now(), before));
    // B edits the same data, through the same history.
    let before = notes.set(&[62, 65]);
    history.record(edit(roll, "B moves the first", notes.now(), before));

    // B's edit is undoable from A -- which is the half that used to be false:
    // `b.can_undo` was true and `a` could not see B's edit at all.
    assert!(history.can_undo());
    assert_eq!(history.undo_label().as_deref(), Some("B moves the first"));

    for (_, payload) in history.undo().unwrap().legs {
        notes.adopt(&payload);
    }
    assert_eq!(notes.0, vec![60, 65], "the latest edit, whoever made it");
    for (_, payload) in history.undo().unwrap().legs {
        notes.adopt(&payload);
    }
    assert_eq!(notes.0, vec![60, 64], "and then the one before it");
    assert!(!history.can_undo());
}

#[test]
fn one_pile_over_several_structures_undoes_in_the_order_the_edits_were_made() {
    // The combination: an application composing two editable views. The
    // interleaved order is the pile, and there is no second mechanism.
    let mut history = History::new();
    let melody = history.register("notes");
    let bass = history.register("notes");
    let (mut a, mut b) = (Notes(vec![60]), Notes(vec![36]));

    let before = a.set(&[62]);
    history.record(edit(melody, "melody", a.now(), before));
    let before = b.set(&[38]);
    history.record(edit(bass, "bass", b.now(), before));
    let before = a.set(&[64]);
    history.record(edit(melody, "melody again", a.now(), before));

    let walk = |history: &mut History, a: &mut Notes, b: &mut Notes| {
        for (structure, payload) in history.undo().expect("something to undo").legs {
            let target = if structure == melody {
                &mut *a
            } else {
                &mut *b
            };
            target.adopt(&payload);
        }
    };
    walk(&mut history, &mut a, &mut b);
    assert_eq!((&a.0[..], &b.0[..]), (&[62i64][..], &[38i64][..]));
    walk(&mut history, &mut a, &mut b);
    assert_eq!((&a.0[..], &b.0[..]), (&[62i64][..], &[36i64][..]));
    walk(&mut history, &mut a, &mut b);
    assert_eq!((&a.0[..], &b.0[..]), (&[60i64][..], &[36i64][..]));
    assert!(!history.can_undo());
}

#[test]
fn a_structure_belongs_to_exactly_one_history() {
    // The rule, enforced where it can be rather than asked for: an identity one
    // history minted means nothing to another, so an entry naming it is
    // refused whole. This is what stops a composed view from quietly opening a
    // second history over data that already has one.
    let mut one = History::new();
    let mut other = History::new();
    let there = other.register("notes");

    assert!(!one.holds(there));
    assert!(
        !one.record(edit(
            there,
            "elsewhere",
            Opaque(json!([1])),
            Opaque(json!([0]))
        )),
        "an identity this history did not mint"
    );
    assert!(one.is_empty());
}

#[test]
fn a_transaction_with_a_foreign_leg_records_nothing_at_all() {
    // Half a transaction is worse than none: it would undo one structure and
    // leave the other where the gesture put it.
    let mut history = History::new();
    let here = history.register("notes");
    let elsewhere = History::new().register("notes");

    let entry = Entry::new(
        "a drag across two",
        here,
        Step::Edit(Opaque(json!([1]))),
        Opaque(json!([0])),
    )
    .and(
        elsewhere,
        Step::Edit(Opaque(json!([3]))),
        Opaque(json!([2])),
    );
    assert!(!history.record(entry));
    assert!(history.is_empty(), "not even the leg it could have taken");
}

#[test]
fn a_transaction_unwinds_in_the_order_it_was_laid_down() {
    let mut history = History::new();
    let curve = history.register("points");
    let clip = history.register("tree");
    let entry = Entry::new(
        "drag the clip and its curve",
        clip,
        Step::Edit(Opaque(json!({"offset": 4.0}))),
        Opaque(json!({"offset": 0.0})),
    )
    .and(
        curve,
        Step::Edit(Opaque(json!([[0.0, 1.0]]))),
        Opaque(json!([[0.0, 0.0]])),
    );
    assert!(history.record(entry));
    assert_eq!(history.len(), 1, "one transaction, whatever it holds");

    let undone = history.undo().unwrap();
    assert_eq!(
        undone.legs.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
        vec![curve, clip],
        "in reverse order, the way it was laid down"
    );
}

#[test]
fn the_domain_is_carried_and_never_read() {
    // The routing tag: a caller holding several structures asks which reader an
    // entry's payload belongs to. The pile itself never looks.
    let mut history = History::new();
    let tree = history.register("tree");
    let points = history.register("points");
    assert_eq!(history.domain(tree), Some("tree"));
    assert_eq!(history.domain(points), Some("points"));
    assert_eq!(history.structures(), vec![tree, points]);
    assert_eq!(history.domain(StructureId(99)), None);
}

#[test]
fn a_run_over_one_structure_coalesces_and_a_run_over_two_does_not() {
    // Coalescing is "the same thing done the same way", and the pile cannot say
    // what that means -- the key comes from the domain. Two legs that name
    // different structures are never the same thing, whatever their keys say.
    let mut history = History::new();
    let a = history.register("notes");
    let b = history.register("notes");

    history.record(edit(a, "drag", Opaque(json!([1])), Opaque(json!([0]))).keyed("note:1"));
    for step in 2..5 {
        history.record(
            edit(a, "drag", Opaque(json!([step])), Opaque(json!([step - 1])))
                .keyed("note:1")
                .continuing(),
        );
    }
    assert_eq!(history.len(), 1, "one gesture, one undo");
    assert_eq!(
        history.peek_undo().unwrap().legs,
        vec![(a, Opaque(json!([0])))],
        "the oldest inverse: an undo lands where the run started"
    );

    history.record(edit(b, "drag", Opaque(json!([9])), Opaque(json!([8]))).keyed("note:1"));
    history.record(
        edit(a, "drag", Opaque(json!([9])), Opaque(json!([8])))
            .keyed("note:1")
            .continuing(),
    );
    assert_eq!(
        history.len(),
        3,
        "a different structure is a different thing"
    );
}

#[test]
fn an_unkeyed_leg_never_coalesces() {
    // What a `Recompute` wants: its payload is parameters the crate cannot
    // compare, and merging two operations into one is the caller's decision.
    let mut history = History::new();
    let a = history.register("notes");
    let params = Opaque(json!({"op": "normalize"}));
    for _ in 0..3 {
        history.record(
            Entry::new(
                "normalize",
                a,
                Step::Recompute(params.clone()),
                Opaque(json!([0])),
            )
            .continuing(),
        );
    }
    assert_eq!(history.len(), 3);
}

#[test]
fn a_new_edit_after_an_undo_truncates_the_redo() {
    let mut history = History::new();
    let a = history.register("notes");
    history.record(edit(a, "one", Opaque(json!([1])), Opaque(json!([0]))));
    history.record(edit(a, "two", Opaque(json!([2])), Opaque(json!([1]))));
    history.undo().unwrap();
    assert!(history.can_redo());

    history.record(edit(a, "three", Opaque(json!([3])), Opaque(json!([1]))));
    assert!(!history.can_redo(), "linear: the branch is not kept");
    assert_eq!(history.len(), 2);
    assert_eq!(history.redo_label(), None);
    assert_eq!(history.undo_label().as_deref(), Some("three"));
}

#[test]
fn a_big_payload_leaves_the_pile_and_comes_back_whole() {
    // Generic by size rather than by which edit it is: any payload whose bulk
    // follows the data instead of the parameters goes to the store.
    let big: Vec<f32> = (0..1024).map(|i| i as f32 * 0.001).collect();
    let previous = Opaque(json!(big));
    let written = Opaque(json!(vec![0.5f32; 1024]));
    let mut history = History::new();
    let buffer = history.register("samples");
    history.record(edit(buffer, "draw", written.clone(), previous.clone()));

    assert_eq!(history.undo().unwrap().legs, vec![(buffer, previous)]);
    assert_eq!(
        history.redo().unwrap().edits,
        vec![(buffer, written)],
        "and forward again, out of the store"
    );
}

#[test]
fn a_small_payload_stays_in_the_pile() {
    let mut history = History::new().spill_above(1 << 20);
    let a = history.register("notes");
    history.record(edit(a, "nudge", Opaque(json!([1])), Opaque(json!([0]))));
    assert_eq!(history.undo().unwrap().legs, vec![(a, Opaque(json!([0])))]);
}

#[test]
fn the_oldest_entries_fall_off_and_take_their_spilled_bytes_with_them() {
    let mut history = History::new().budget(2);
    let a = history.register("samples");
    for i in 0..5i64 {
        history.record(edit(
            a,
            "draw",
            Opaque(json!(vec![i as f32; 1024])),
            Opaque(json!(vec![-i as f32; 1024])),
        ));
    }
    assert_eq!(history.len(), 2, "the budget holds");
    assert_eq!(
        history.undo().unwrap().legs,
        vec![(a, Opaque(json!(vec![-4.0f32; 1024])))],
        "and the survivors still invert"
    );
}

#[test]
fn clearing_forgets_the_order_and_keeps_the_identities() {
    // Closing an editing context. The structures are still the caller's --
    // it holds their handles -- so what goes is the order, not the registry.
    let mut history = History::new();
    let a = history.register("notes");
    history.record(edit(a, "one", Opaque(json!([1])), Opaque(json!([0]))));
    history.clear();
    assert!(history.is_empty() && !history.can_undo() && !history.can_redo());
    assert!(history.holds(a), "the identity outlives the order");
    assert!(history.record(edit(a, "again", Opaque(json!([1])), Opaque(json!([0])))));
}

// ---- a transaction ----

#[test]
fn a_composite_gesture_undoes_and_redoes_in_one_step() {
    // The drag that moves a clip and rewrites the curve it carries: two
    // structures, one gesture, one undo.
    let mut history = History::new();
    let clip = history.register("notes");
    let curve = history.register("notes");
    let (mut a, mut b) = (Notes(vec![0]), Notes(vec![10]));

    let outcomes = {
        let mut legs: Vec<(StructureId, &mut dyn Editable, Opaque)> = vec![
            (clip, &mut a, Opaque(json!([4]))),
            (curve, &mut b, Opaque(json!([14]))),
        ];
        history.transact("drag the clip and its curve", &mut legs)
    };
    assert!(outcomes.iter().all(|o| o.applied));
    assert_eq!((&a.0[..], &b.0[..]), (&[4i64][..], &[14i64][..]));
    assert_eq!(history.len(), 1, "one gesture, one entry");

    let undone = history.undo().expect("something to undo");
    assert_eq!(
        undone.legs.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
        vec![curve, clip],
        "in reverse order, the way it was laid down"
    );
    for (structure, payload) in undone.legs {
        let target = if structure == clip { &mut a } else { &mut b };
        target.adopt(&payload);
    }
    assert_eq!((&a.0[..], &b.0[..]), (&[0i64][..], &[10i64][..]));

    for (structure, payload) in history.redo().expect("something to redo").edits {
        let target = if structure == clip { &mut a } else { &mut b };
        target.adopt(&payload);
    }
    assert_eq!((&a.0[..], &b.0[..]), (&[4i64][..], &[14i64][..]));
    assert!(!history.can_redo());
}

#[test]
fn a_refused_leg_leaves_no_entry_and_no_half_applied_gesture() {
    // Atomic in both directions: the leg that already landed is put back, so
    // the two structures are consistent at every point a reader could look.
    let mut history = History::new();
    let clip = history.register("notes");
    let elsewhere = History::new().register("notes");
    let (mut a, mut b) = (Notes(vec![0]), Notes(vec![10]));

    let outcomes = {
        let mut legs: Vec<(StructureId, &mut dyn Editable, Opaque)> = vec![
            (clip, &mut a, Opaque(json!([4]))),
            (elsewhere, &mut b, Opaque(json!([14]))),
        ];
        history.transact("drag", &mut legs)
    };
    assert!(outcomes.iter().all(|o| !o.applied));
    assert!(outcomes.iter().all(|o| o.reason.is_some()));
    assert!(history.is_empty(), "no entry anywhere");
    assert_eq!(
        (&a.0[..], &b.0[..]),
        (&[0i64][..], &[10i64][..]),
        "and nothing was left applied"
    );
}

#[test]
fn a_leg_refused_after_one_landed_rolls_the_first_one_back() {
    let mut history = History::new();
    let one = history.register("notes");
    let two = history.register("notes");
    let (mut a, mut b) = (Notes(vec![0]), Notes(vec![10]));

    let outcomes = {
        let mut legs: Vec<(StructureId, &mut dyn Editable, Opaque)> = vec![
            (one, &mut a, Opaque(json!([4]))),
            // `Notes` refuses what it cannot read as its own vocabulary.
            (two, &mut b, Opaque(json!({"intent": "place"}))),
        ];
        history.transact("drag", &mut legs)
    };
    assert!(outcomes.iter().all(|o| !o.applied));
    assert_eq!(a.0, vec![0], "the leg that landed was put back");
    assert_eq!(b.0, vec![10]);
    assert!(history.is_empty());
}

#[test]
fn a_transaction_and_a_merge_are_kept_apart() {
    // Coalescing merges *successive* entries over one structure; a transaction
    // is one entry with several legs. A continuing entry over one structure
    // must not merge into a transaction that happens to start with it.
    let mut history = History::new();
    let one = history.register("notes");
    let two = history.register("notes");
    let (mut a, mut b) = (Notes(vec![0]), Notes(vec![10]));
    {
        let mut legs: Vec<(StructureId, &mut dyn Editable, Opaque)> = vec![
            (one, &mut a, Opaque(json!([4]))),
            (two, &mut b, Opaque(json!([14]))),
        ];
        history.transact("drag both", &mut legs);
    }
    history.record(
        edit(one, "drag one", Opaque(json!([5])), Opaque(json!([4])))
            .keyed("notes")
            .continuing(),
    );
    assert_eq!(history.len(), 2, "different shapes never merge");
}

// ---- what the history refuses to promise ----

#[test]
fn an_entry_with_no_inverse_is_walked_past_once_and_the_walk_says_so() {
    // Recording it beats dropping it: a hole in the history that announces
    // itself is what lets a person understand why an undo did not go where they
    // expected.
    let mut history = History::new();
    let a = history.register("notes");
    let mut notes = Notes(vec![0]);

    history.record(edit(a, "first", Opaque(json!([1])), Opaque(json!([0]))));
    history.record(Entry::uninvertible(
        "normalize",
        a,
        Step::Recompute(Opaque(json!({"op": "normalize"}))),
    ));
    history.record(edit(a, "third", Opaque(json!([3])), Opaque(json!([2]))));
    notes.0 = vec![3];

    // The entry on top inverts as usual.
    let undone = history.undo().unwrap();
    assert_eq!(undone.label, "third");
    assert!(undone.skipped.is_empty());
    for (_, payload) in undone.legs {
        notes.adopt(&payload);
    }
    assert_eq!(notes.0, vec![2]);

    // The next one cannot, so the walk goes past it and names it.
    let undone = history.undo().unwrap();
    assert_eq!(undone.skipped, vec!["normalize".to_string()]);
    assert_eq!(undone.label, "first");
    for (_, payload) in undone.legs {
        notes.adopt(&payload);
    }
    assert_eq!(notes.0, vec![0], "back to before the first edit");
    assert!(!history.can_undo());

    // And forward it is skipped too: a state that was never reverted must not
    // be applied twice.
    let redone = history.redo().unwrap();
    assert_eq!(redone.label, "first");
    assert!(redone.skipped.is_empty());
    let redone = history.redo().unwrap();
    assert_eq!(redone.skipped, vec!["normalize".to_string()]);
    assert_eq!(redone.label, "third");
}

#[test]
fn a_history_of_nothing_but_the_non_invertible_still_answers() {
    let mut history = History::new();
    let a = history.register("notes");
    history.record(Entry::uninvertible(
        "normalize",
        a,
        Step::Recompute(Opaque(json!({"op": "normalize"}))),
    ));
    let undone = history.undo().expect("it answers rather than refusing");
    assert!(undone.legs.is_empty());
    assert_eq!(undone.skipped, vec!["normalize".to_string()]);
    assert!(!history.can_undo(), "and the walk still moved");
}

#[test]
fn a_transaction_with_one_uninvertible_leg_is_non_invertible_whole() {
    // Half a transaction that unwinds is the failure atomicity exists to
    // prevent, so one leg with no inverse marks the entry.
    let mut history = History::new();
    let a = history.register("notes");
    let b = history.register("notes");
    let entry = Entry::new(
        "drag",
        a,
        Step::Edit(Opaque(json!([1]))),
        Opaque(json!([0])),
    )
    .and_uninvertible(b, Step::Recompute(Opaque(json!({"op": "normalize"}))));
    assert!(!entry.invertible());
    history.record(entry);

    let undone = history.undo().unwrap();
    assert!(undone.legs.is_empty());
    assert_eq!(undone.skipped, vec!["drag".to_string()]);
}

#[test]
fn deleting_a_structure_invalidates_the_entries_that_name_it() {
    // The case that makes the first rule pay for itself: those entries cannot
    // be applied to data that is gone, so they become non-invertible rather
    // than failing at apply time.
    let mut history = History::new();
    let kept = history.register("notes");
    let gone = history.register("notes");
    history.record(edit(kept, "keep", Opaque(json!([1])), Opaque(json!([0]))));
    history.record(edit(gone, "doomed", Opaque(json!([1])), Opaque(json!([0]))));

    assert!(
        !history.forget(gone),
        "an entry still names it, so its data has to stay alive"
    );
    assert!(
        !history.holds(gone),
        "and nothing new may be recorded there"
    );
    assert!(!history.record(edit(gone, "after", Opaque(json!([2])), Opaque(json!([1])))));

    let undone = history.undo().unwrap();
    assert_eq!(undone.skipped, vec!["doomed".to_string()]);
    assert_eq!(undone.label, "keep");
}

#[test]
fn the_data_of_a_deleted_structure_is_freed_when_its_last_entry_retires() {
    // The budget already decides when an entry stops existing; this is the hook
    // that says the last one holding a deleted structure has gone.
    let mut history = History::new().budget(2);
    let a = history.register("notes");
    let gone = history.register("notes");
    history.record(edit(gone, "doomed", Opaque(json!([1])), Opaque(json!([0]))));
    assert!(!history.forget(gone));
    assert!(history.released().is_empty(), "not yet");

    for i in 0..2 {
        history.record(edit(a, "later", Opaque(json!([i])), Opaque(json!([i - 1]))));
    }
    assert_eq!(
        history.released(),
        vec![gone],
        "the entry fell off the budget, so the data may go"
    );
    assert!(history.released().is_empty(), "reported once");
}

#[test]
fn forgetting_a_structure_nothing_names_frees_it_at_once() {
    let mut history = History::new();
    let a = history.register("notes");
    assert!(history.forget(a), "nothing to wait for");
    assert!(history.released().is_empty());
}

#[test]
fn crossing_the_save_mark_backwards_is_allowed_and_announced() {
    let mut history = History::new();
    let a = history.register("notes");
    assert!(
        !history.dirty(),
        "a history that has done nothing is at rest"
    );

    history.record(edit(a, "one", Opaque(json!([1])), Opaque(json!([0]))));
    assert!(history.dirty());
    history.mark_saved();
    assert!(!history.dirty(), "this is what is on disk");

    history.record(edit(a, "two", Opaque(json!([2])), Opaque(json!([1]))));
    assert!(history.dirty());
    history.undo();
    assert!(!history.dirty(), "back at the mark");

    history.undo();
    assert!(
        history.dirty(),
        "past it: nothing on disk changed, and the file still holds that edit"
    );
    assert!(history.saved_reachable());
    history.redo();
    assert!(!history.dirty(), "and forward again returns to clean");
}

#[test]
fn editing_from_before_the_mark_makes_the_saved_state_unreachable() {
    // The third case, and the reason the warning earns its place: undo past the
    // mark and then edit, and the redo is truncated -- so the saved state stops
    // being reachable through the history at all.
    let mut history = History::new();
    let a = history.register("notes");
    history.record(edit(a, "one", Opaque(json!([1])), Opaque(json!([0]))));
    history.mark_saved();
    history.undo();
    assert!(history.dirty() && history.saved_reachable());

    history.record(edit(a, "another", Opaque(json!([9])), Opaque(json!([0]))));
    assert!(
        !history.saved_reachable(),
        "the mark was in what was truncated"
    );
    assert!(history.dirty());
    history.undo();
    assert!(history.dirty(), "and it will not go quiet on its own again");
}

#[test]
fn the_mark_falls_off_with_the_entry_it_stood_behind() {
    let mut history = History::new().budget(2);
    let a = history.register("notes");
    history.record(edit(a, "one", Opaque(json!([1])), Opaque(json!([0]))));
    history.mark_saved();
    for i in 2..5 {
        history.record(edit(a, "more", Opaque(json!([i])), Opaque(json!([i - 1]))));
    }
    assert!(
        !history.saved_reachable(),
        "the history no longer reaches what was saved"
    );
}

#[test]
fn clearing_releases_what_was_waiting_rather_than_losing_it() {
    // Clearing is exactly what makes a forgotten structure releasable: nothing
    // names it any more. Dropping the pending list here would lose the one
    // report the caller frees on.
    let mut history = History::new();
    let gone = history.register("notes");
    history.record(edit(gone, "doomed", Opaque(json!([1])), Opaque(json!([0]))));
    assert!(!history.forget(gone));
    history.clear();
    assert_eq!(history.released(), vec![gone]);
}
