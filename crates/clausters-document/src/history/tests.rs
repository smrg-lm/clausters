use super::*;
use serde_json::json;

/// A structure with no document behind it: the notes of a roll the caller
/// built. Its whole vocabulary is one verb — *the notes are now these* — which
/// is what a domain has to bring, and all it has to bring.
#[derive(Debug, Default, PartialEq)]
struct Notes(Vec<i64>);

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

    for (structure, payload) in history.undo().expect("something to undo") {
        assert_eq!(structure, roll);
        notes.adopt(&payload);
    }
    assert_eq!(notes.0, vec![60, 64], "back where it started");

    for (structure, step) in history.redo().expect("something to redo") {
        assert_eq!(structure, roll);
        notes.adopt(step.payload().expect("an ordinary edit"));
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
    assert_eq!(history.undo_label(), Some("B moves the first"));

    for (_, payload) in history.undo().unwrap() {
        notes.adopt(&payload);
    }
    assert_eq!(notes.0, vec![60, 65], "the latest edit, whoever made it");
    for (_, payload) in history.undo().unwrap() {
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
        for (structure, payload) in history.undo().expect("something to undo") {
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
        undone.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
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
        history.peek_undo().unwrap(),
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
    assert_eq!(history.undo_label(), Some("three"));
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

    assert_eq!(history.undo().unwrap(), vec![(buffer, previous)]);
    assert_eq!(
        history.redo().unwrap(),
        vec![(buffer, Step::Edit(written))],
        "and forward again, out of the store"
    );
}

#[test]
fn a_small_payload_stays_in_the_pile() {
    let mut history = History::new().spill_above(1 << 20);
    let a = history.register("notes");
    history.record(edit(a, "nudge", Opaque(json!([1])), Opaque(json!([0]))));
    assert_eq!(history.undo().unwrap(), vec![(a, Opaque(json!([0])))]);
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
        history.undo().unwrap(),
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
