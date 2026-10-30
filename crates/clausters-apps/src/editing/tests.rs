use super::*;
use clausters_document::multitrack::Window;
use clausters_document::multitrack::{Content, Multitrack, Region, Track};
use clausters_document::points::{Points, payload as points_payload};
use clausters_document::{Lifetime, NodeId, Second, SourceId, SourceRef};
use std::collections::HashMap;

const SR: f64 = 48_000.0;

fn region(id: u64, at: f64) -> Region {
    let mut region = Region::new(
        NodeId(id),
        Second(at),
        Second(2.0),
        Content::Unknown(Value::Null),
    );
    region.content = Content::window(Window {
        source: SourceRef {
            source: SourceId(1),
            lifetime: Lifetime::Session,
            generation: 0,
            range: None,
        },
        start: 0.0,
        duration: 2.0,
    });
    region
}

/// **A notes editor edits the sequence its holder shares**, and an undo
/// through the context puts it back there.
#[test]
fn a_notes_editor_edits_the_shared_sequence_and_undoes_through_the_context() {
    use clausters_document::EventSequence;
    use clausters_document::events::Event as SeqEvent;
    use std::sync::{Arc, Mutex};

    let sequence = Arc::new(Mutex::new(EventSequence::new(vec![SeqEvent::new(
        0.0,
        json!({"midinote": 60, "sustain": 1.0, "amp": 0.1}),
    )])));
    let mut editing = Editing::new();
    let opened: Value =
        serde_json::from_str(&editing.open_notes("seq", sequence.clone(), r#"{"rate": 100.0}"#))
            .unwrap();
    let member = opened["member"].as_u64().unwrap() as MemberId;
    call_json(
        &mut editing,
        &json!({"verb": "member", "member": member, "call": {"verb": "window", "widget": 40}})
            .to_string(),
    );
    let moved = vec![
        json!(1),
        json!(50.0),
        json!(100.0),
        json!(67.0),
        json!(13.0),
        json!(0.0),
    ];
    let turned = editing
        .event(member, &event(40, 1, "notes", moved))
        .unwrap();
    assert!(turned.outcome.changed());
    assert_eq!(
        sequence.lock().unwrap().get(1).unwrap().data.0["midinote"],
        json!(67.0)
    );
    let stepped = editing.step(Direction::Undo);
    assert!(stepped.stepped);
    assert!(matches!(
        stepped.effects[0],
        Effect::Notes { applied: true, .. }
    ));
    assert_eq!(sequence.lock().unwrap().get(1).unwrap().at.0, 0.0);
    assert_eq!(
        sequence.lock().unwrap().get(1).unwrap().data.0["midinote"],
        json!(60)
    );
    assert!(editing.step(Direction::Redo).stepped);
    assert_eq!(
        sequence.lock().unwrap().get(1).unwrap().data.0["midinote"],
        json!(67.0)
    );
}

/// **A region over a bound sequence draws what a notes editor edits**: the
/// binding answers the multitrack's picture with the note in box 12, and an
/// edit in the roll opened on the same handle corrects that picture.
/// An engraver that keeps the document it is handed and draws nothing.
#[cfg(feature = "notation")]
struct Kept(std::sync::Mutex<String>);

#[cfg(feature = "notation")]
impl clausters_core::notation::Engraver for Kept {
    type Guard = ();
    fn lock(&self) -> Self::Guard {}
    fn load_data(&self, data: &str) -> bool {
        *self.0.lock().unwrap() = data.to_string();
        !data.is_empty()
    }
    fn render_svg(&self, _page: i32) -> String {
        String::new()
    }
    fn mei(&self) -> String {
        self.0.lock().unwrap().clone()
    }
    fn edit(&self, _action: &str) -> bool {
        false
    }
    fn timemap(&self, _options: &str) -> String {
        "[]".into()
    }
    fn midi_values(&self, _xml_id: &str) -> Option<String> {
        None
    }
    fn set_options(&self, _options: &str) -> bool {
        true
    }
}

/// A box of notes is drawn as its page where the window says so and an
/// engraver was handed over: the page, and the time of each of its notes
/// in the box's own frames.
#[cfg(feature = "notation")]
#[test]
fn a_box_of_notes_is_stated_as_its_page_once_an_engraver_is_bound() {
    use clausters_core::notation::{AnyEngraver, Score, sheet_to_mei, voice_to_sheet};
    use clausters_document::EventSequence;
    use clausters_document::events::Event as SeqEvent;
    use std::sync::{Arc, Mutex};

    let sequence = Arc::new(Mutex::new(EventSequence::new(vec![
        SeqEvent::new(0.0, json!({"midinote": 60, "dur": 1.0})),
        SeqEvent::new(1.0, json!({"midinote": 64, "dur": 1.0})),
    ])));
    let mut editing = Editing::new();
    let multitrack = editing.join("multitrack", a_multitrack());
    editing.bind_sequence(multitrack, SourceId(1), sequence.clone());
    let picture = |editing: &mut Editing| match editing.member_mut(multitrack) {
        Some(Member::Multitrack(editor)) => editor.picture(40),
        _ => panic!("a multitrack editor"),
    };
    // as rolls, and with nothing to engrave with, there is no page
    assert_eq!(picture(&mut editing)["notes_view"], "roll");
    assert!(!picture(&mut editing).contains_key("scores"));

    let blank = sheet_to_mei(&voice_to_sheet(&[], "4/4", "G2", "C")).unwrap();
    let engraver = AnyEngraver::new(Kept(Mutex::new(String::new())));
    let score = Arc::new(Mutex::new(Score::open(engraver, &blank).expect("opens")));
    editing.bind_engraver(multitrack, score.clone());
    assert!(
        !picture(&mut editing).contains_key("scores"),
        "still as rolls"
    );
    match editing.member_mut(multitrack) {
        Some(Member::Multitrack(editor)) => assert!(editor.set_notes_view("score")),
        _ => panic!("a multitrack editor"),
    }
    let props = picture(&mut editing);
    assert_eq!(props["notes_view"], "score");
    let pages = props["scores"].as_object().expect("the pages");
    let (_, page) = pages.iter().next().expect("the box's page");
    // the second note stands a beat in: a second, in the axis's frames
    let anchors = page["anchors"].as_array().unwrap();
    assert_eq!(anchors[0], "n1");
    assert_eq!(anchors[1], 0.0);
    assert_eq!(anchors[4], "n2");
    assert_eq!(anchors[5], SR);
    // what was engraved is the sequence, read
    assert!(score.lock().unwrap().mei().contains("pname=\"e\""));
    // and a note moved is the page engraved again
    sequence.lock().unwrap().events[1].at.0 = 2.0;
    let again = picture(&mut editing);
    let (_, page) = again["scores"].as_object().unwrap().iter().next().unwrap();
    assert_eq!(page["anchors"][5], 2.0 * SR);
}

#[test]
fn a_region_over_a_bound_sequence_follows_the_roll() {
    use clausters_document::EventSequence;
    use clausters_document::events::Event as SeqEvent;
    use std::sync::{Arc, Mutex};

    let sequence = Arc::new(Mutex::new(EventSequence::new(vec![SeqEvent::new(
        0.0,
        json!({"midinote": 60, "sustain": 1.0}),
    )])));
    let mut editing = Editing::new();
    let multitrack = editing.join("multitrack", a_multitrack());
    let bound: Value =
        serde_json::from_str(&editing.bind_sequence(multitrack, SourceId(1), sequence.clone()))
            .unwrap();
    let text = bound.to_string();
    assert!(
        text.contains("notes"),
        "the picture carries the notes: {text}"
    );

    let opened: Value =
        serde_json::from_str(&editing.open_notes("seq", sequence.clone(), r#"{"rate": 100.0}"#))
            .unwrap();
    let notes = opened["member"].as_u64().unwrap() as MemberId;
    call_json(
        &mut editing,
        &json!({"verb": "member", "member": notes, "call": {"verb": "window", "widget": 60}})
            .to_string(),
    );
    let moved = vec![
        json!(1),
        json!(0.0),
        json!(100.0),
        json!(67.0),
        json!(100.0),
        json!(0.0),
    ];
    let turned = editing.event(notes, &event(60, 1, "notes", moved)).unwrap();
    let corrected = turned
        .corrections
        .iter()
        .find(|c| c.member == multitrack)
        .expect("the multitrack is corrected");
    let text = serde_json::to_string(&corrected.answer).unwrap();
    assert!(text.contains("67"), "box 12 draws the moved note: {text}");

    // A double click on box 12 asks for the roll over source 1.
    let opened = editing
        .event(multitrack, &event(40, 2, "open", vec![json!("12")]))
        .unwrap();
    let Outcome::Multitrack(outcome) = &opened.outcome else {
        panic!("the multitrack's outcome");
    };
    assert_eq!(outcome.open, Some(1));
    assert!(!opened.outcome.changed(), "and nothing is edited");

    // A member that is not a multitrack binds nothing.
    assert_eq!(editing.bind_sequence(notes, SourceId(1), sequence), "{}");
}

/// A multitrack of one track holding box 12, drawn by widget 40 in window 39.
fn a_multitrack() -> Member {
    let mut track = Track::new(NodeId(10), NodeId(11));
    track.take_lanes[0].regions = vec![region(12, 0.0)];
    let multitrack = Multitrack {
        tracks: vec![track],
        ..Multitrack::default()
    };
    let mut editor = MultitrackEditor::new(multitrack, SR, FIRST_VERSION);
    editor.set_sources(HashMap::from([(SourceId(1), 7)]));
    editor.chrome(
        None,
        crate::multitrack::Transport::Unnumbered,
        "multitrack",
        (1000, 560),
    );
    editor.window(40, 41);
    editor.set_window(Some(39));
    Member::Multitrack(Box::new(editor))
}

/// **A join no region reads and no entry can put back is handed back to
/// free**, under the multitrack that minted it: an undo keeps it for the
/// redo, and the edit after the undo lets it go.
#[test]
fn a_join_undone_and_overtaken_is_freed() {
    let half = |id: u64, at: f64, start: f64| {
        let mut region = region(id, at);
        region.length = Second(1.0);
        region.content = Content::window(Window {
            source: SourceRef {
                source: SourceId(1),
                lifetime: Lifetime::Session,
                generation: 0,
                range: None,
            },
            start,
            duration: 1.0,
        });
        region
    };
    // The take's halves swapped: no window onto the take reads them in
    // this order, so the join mints a source.
    let mut track = Track::new(NodeId(10), NodeId(11));
    track.take_lanes[0].regions = vec![half(12, 0.0, 1.0), half(13, 1.0, 0.0)];
    let mut editor = MultitrackEditor::new(
        Multitrack {
            tracks: vec![track],
            ..Multitrack::default()
        },
        SR,
        FIRST_VERSION,
    );
    editor.set_sources(HashMap::from([(SourceId(1), 7)]));
    editor.chrome(
        None,
        crate::multitrack::Transport::Unnumbered,
        "multitrack",
        (1000, 560),
    );
    editor.window(40, 41);
    editor.set_window(Some(39));
    let mut editing = Editing::default();
    let member = editing.join("multitrack", Member::Multitrack(Box::new(editor)));

    let joined = editing
        .event(
            member,
            &event(40, 1, "join", vec![json!("12"), json!("13")]),
        )
        .unwrap();
    let Outcome::Multitrack(outcome) = &joined.outcome else {
        panic!("a multitrack's outcome");
    };
    let join = SourceId(outcome.minted[0]["id"].as_u64().unwrap());
    assert!(joined.freed.is_empty(), "the joined box reads it");

    let undone = editing.step(Direction::Undo);
    assert!(undone.freed.is_empty(), "a redo can still name it");

    // The two boxes are back as they were; the first moves to 3 s.
    let boxes = vec![
        json!("12"),
        json!("10"),
        json!(3.0 * SR),
        json!(SR),
        json!(SR),
        json!(""),
        json!(7),
        json!("13"),
        json!("10"),
        json!(SR),
        json!(SR),
        json!(0.0),
        json!(""),
        json!(7),
    ];
    let overtaken = editing
        .event(member, &event(40, 2, "clips", boxes))
        .unwrap();
    assert!(overtaken.outcome.changed());
    assert_eq!(
        overtaken.freed,
        [Freed {
            member,
            buffers: Vec::new(),
            sources: vec![join.0],
            files: Vec::new(),
        }],
        "the edit after the undo dropped the only entry that named it"
    );
    let Some(Member::Multitrack(editor)) = editing.member_mut(member) else {
        panic!("the multitrack is still there");
    };
    assert!(!editor.is_join(join), "and the editor forgot it");
}

/// A mono take in buffer 7, a hundred frames, edited by an audio editor
/// drawn by widget 50 in window 49, with buffers for new takes.
fn a_take() -> Member {
    let mut editor = audio::new_json(
        r#"{"path":"/in/take7.wav","workspace":"/work", "frames": 100, "version": 1}"#,
    )
    .unwrap();
    audio::call_json(&mut editor, r#"{"verb": "window", "widget": 50}"#);
    audio::call_json(&mut editor, r#"{"verb": "sync", "window": 49}"#);
    Member::Audio(Box::new(editor))
}

fn event(widget: i64, seq: i64, tag: &str, values: Vec<Value>) -> Event {
    let mut args = vec![json!(widget), json!(seq), json!(0), json!(tag)];
    args.extend(values);
    Event {
        addr: "/gui_event".into(),
        args,
    }
}

fn moved_to(at: f64) -> Vec<Value> {
    vec![
        json!("12"),
        json!("10"),
        json!(at * SR),
        json!(2.0 * SR),
        json!(0.0),
        json!(""),
        json!(7),
    ]
}

fn stroke(value: f64, was: f64) -> Vec<Value> {
    vec![json!(0), json!(2), json!([value]), json!([was])]
}

fn position(editing: &mut Editing, member: MemberId) -> f64 {
    match editing.member_mut(member) {
        Some(Member::Multitrack(editor)) => {
            editor.multitrack().tracks[0].take_lanes[0].regions[0]
                .position
                .0
        }
        _ => f64::NAN,
    }
}

/// The takes an audio member's list reads now, in order.
/// The file each part of a member's list reads, by its stem.
fn reads(editing: &Editing, member: MemberId) -> Vec<String> {
    match editing.member(member) {
        Some(Member::Audio(editor)) => editor.part_files(),
        _ => Vec::new(),
    }
}

/// How many audio members a step handed a list again.
fn relisted(stepped: &Stepped) -> usize {
    stepped
        .effects
        .iter()
        .filter(|e| matches!(e, Effect::Audio { .. }))
        .count()
}

/// **An editor open alone is a context of one**: its stroke is recorded
/// there, and an undo hands back the list that puts the take back.
#[test]
fn an_editor_open_alone_undoes_through_a_context_of_one() {
    let mut editing = Editing::default();
    let take = editing.join("buffer:7", a_take());
    let turned = editing
        .event(take, &event(50, 1, "draw", stroke(0.5, 0.0)))
        .unwrap();
    assert!(turned.outcome.changed());
    assert_eq!(turned.version, 2);
    assert!(editing.can_undo());
    assert_eq!(editing.undo_label().as_deref(), Some("draw the samples"));

    assert_eq!(reads(&editing, take), ["take7", "1-2-stroke", "take7"]);
    let undone = editing.step(Direction::Undo);
    assert!(undone.stepped);
    assert_eq!(undone.version, 3);
    assert_eq!(relisted(&undone), 1);
    assert_eq!(reads(&editing, take), ["take7"]);
    assert_eq!(undone.corrections.len(), 1);
    assert_eq!(undone.corrections[0].member, take);
    let Answer::Push {
        doc_version,
        corrections,
        ..
    } = &undone.corrections[0].answer
    else {
        panic!("a push");
    };
    assert_eq!(*doc_version, 3);
    assert_eq!(corrections[0].widget, 50);
    assert!(
        corrections[0].props["segments"].is_object(),
        "the window is told the list again"
    );
    editing.step(Direction::Redo);
    assert_eq!(reads(&editing, take), ["take7", "1-2-stroke", "take7"]);
}

/// **Two applications in one context walk one order**: a box moved, a
/// stroke drawn, a box moved again, and undo walks the three back in that
/// order, each handed to its own member.
#[test]
fn two_applications_in_one_context_walk_one_order() {
    let mut editing = Editing::default();
    let multitrack = editing.join("multitrack", a_multitrack());
    let take = editing.join("buffer:7", a_take());

    let first = editing
        .event(multitrack, &event(40, 1, "clips", moved_to(2.0)))
        .unwrap();
    assert!(first.outcome.changed());
    assert!(
        first.corrections.iter().any(|c| c.member == take),
        "the other window is corrected"
    );
    editing
        .event(take, &event(50, 1, "draw", stroke(0.5, 0.0)))
        .unwrap();
    editing
        .event(multitrack, &event(40, 2, "clips", moved_to(4.0)))
        .unwrap();
    assert_eq!(position(&mut editing, multitrack), 4.0);

    let back = editing.step(Direction::Undo);
    assert!(matches!(back.effects[..], [Effect::Multitrack { .. }]));
    assert_eq!(position(&mut editing, multitrack), 2.0);

    let back = editing.step(Direction::Undo);
    assert_eq!(relisted(&back), 1, "the stroke, in between");
    assert_eq!(reads(&editing, take), ["take7"]);
    assert_eq!(position(&mut editing, multitrack), 2.0);

    editing.step(Direction::Undo);
    assert_eq!(position(&mut editing, multitrack), 0.0);
    assert!(!editing.can_undo());

    editing.step(Direction::Redo);
    editing.step(Direction::Redo);
    assert_eq!(reads(&editing, take), ["take7", "1-2-stroke", "take7"]);
}

/// **Two windows over one take are one structure**: a stroke in one is
/// one entry, undone once whichever asks, and each window is handed the
/// list the step put back.
#[test]
fn two_windows_over_one_take_are_one_structure() {
    let mut editing = Editing::default();
    let left = editing.join("buffer:7", a_take());
    let right = editing.join("buffer:7", a_take());
    editing
        .event(left, &event(50, 1, "draw", stroke(0.5, 0.0)))
        .unwrap();
    let undone = editing.step(Direction::Undo);
    assert!(undone.stepped);
    assert_eq!(relisted(&undone), 2, "each window draws its own list");
    assert_eq!(
        (reads(&editing, left), reads(&editing, right)),
        (vec!["take7".to_string()], vec!["take7".to_string()])
    );
    assert!(!editing.can_undo(), "one entry");
    assert!(undone.corrections.iter().any(|c| c.member == right));
}

/// **An external member's legs are handed back**, in the same order as the
/// rest.
#[test]
fn an_external_members_legs_are_handed_back() {
    let mut editing = Editing::default();
    let curve = editing.join(
        "curve",
        Member::External {
            domain: "points".into(),
        },
    );
    let take = editing.join("buffer:7", a_take());
    let record = Record {
        label: "draw the curve".into(),
        legs: vec![crate::turn::Leg {
            forward: json!({"edit": {"points": [1.0]}}),
            backward: json!({"points": [0.0]}),
            key: String::new(),
            ..Default::default()
        }],
    };
    assert!(editing.record(curve, &record, false));
    assert_eq!(editing.version(), 2);
    editing
        .event(take, &event(50, 1, "draw", stroke(0.5, 0.0)))
        .unwrap();

    editing.step(Direction::Undo);
    let back = editing.step(Direction::Undo);
    assert_eq!(
        back.effects,
        [Effect::External {
            member: curve,
            payloads: vec![json!({"points": [0.0]})],
        }]
    );
}

/// **An entry an embedder built** is recorded under its structure, moves the
/// version, and walks back like any other.
#[test]
fn an_entry_built_in_rust_joins_the_order() {
    let mut editing = Editing::default();
    let curve = editing.join(
        "curve",
        Member::External {
            domain: "points".into(),
        },
    );
    let structure = editing.structure(curve).unwrap();
    assert!(editing.member(curve).is_some());
    let entry = Entry::new(
        "bend",
        structure,
        Step::Edit(Opaque(json!({"points": [1.0]}))),
        Opaque(json!({"points": [0.0]})),
    );
    assert!(editing.record_entry(entry));
    assert_eq!(editing.version(), 2);
    let back = editing.step(Direction::Undo);
    assert_eq!(
        back.effects,
        [Effect::External {
            member: curve,
            payloads: vec![json!({"points": [0.0]})]
        }]
    );
}

/// **A vocabulary that applies and records in one call** does it through
/// the context's history, and the version moves only when it recorded.
#[test]
fn an_edit_recorded_through_a_door_of_its_own_moves_the_version() {
    let mut editing = Editing::default();
    let tree = editing.join(
        "tree",
        Member::External {
            domain: "tree".into(),
        },
    );
    assert!(!editing.record_with(tree, |_, _| false));
    assert_eq!(
        editing.version(),
        FIRST_VERSION,
        "nothing recorded, nothing moved"
    );
    assert!(editing.record_with(tree, |history, structure| {
        history.record(Entry::new(
            "move",
            structure,
            Step::Edit(Opaque(json!({"x": 1}))),
            Opaque(json!({"x": 0})),
        ))
    }));
    assert_eq!(editing.version(), 2);
    assert!(editing.can_undo());
}

/// **A step nothing could apply is not a step**: the cursor goes back, the
/// entry is still on top, and the reason names it.
#[test]
fn a_step_nothing_could_apply_puts_the_cursor_back() {
    let mut editing = Editing::default();
    let multitrack = editing.join("multitrack", a_multitrack());
    let record = Record {
        label: "move nothing".into(),
        legs: vec![crate::turn::Leg {
            forward: json!({"edit": {"nonsense": true}}),
            backward: json!({"nonsense": true}),
            key: String::new(),
            ..Default::default()
        }],
    };
    editing.record(multitrack, &record, false);
    let version = editing.version();
    let refused = editing.step(Direction::Undo);
    assert!(!refused.stepped);
    assert!(
        refused
            .reason
            .as_deref()
            .unwrap()
            .starts_with("move nothing")
    );
    assert!(editing.can_undo(), "still there");
    assert_eq!(editing.version(), version);
}

/// **A step is whole across structures**: one whose second structure
/// refuses puts back the first, which it had already stepped.
#[test]
fn a_step_refused_on_one_structure_puts_back_the_others() {
    let mut editing = Editing::default();
    let multitrack = editing.join("multitrack", a_multitrack());
    let curve = call(
        &mut editing,
        json!({"verb": "openPoints", "key": "curve", "rate": 100.0,
               "points": [0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 0.0]}),
    )["member"]
        .clone();
    let curve = int(&curve) as MemberId;
    let held = |editing: &Editing| match &editing.seats[curve as usize].member {
        Member::Points(editor) => editor.curve().lock().unwrap().points.clone(),
        _ => unreachable!(),
    };
    let before = held(&editing);
    let mut bent = Points(before.clone());
    bent.0[1].value = 0.5;
    let entry = Entry::new(
        "both",
        editing.structure(multitrack).unwrap(),
        Step::Edit(Opaque(json!({"edit": {"nonsense": true}}))),
        Opaque(json!({"nonsense": true})),
    )
    .and(
        editing.structure(curve).unwrap(),
        Step::Edit(points_payload(&Points(before.clone()).state())),
        points_payload(&bent.state()),
    );
    assert!(editing.record_entry(entry));
    let version = editing.version();
    let refused = editing.step(Direction::Undo);
    assert!(!refused.stepped);
    assert!(refused.effects.is_empty());
    assert_eq!(held(&editing), before, "the curve is put back");
    assert!(editing.can_undo(), "still there");
    assert_eq!(editing.version(), version);
}

/// **An undo asked of a window is the context's step**, answered with the
/// acknowledgement of the key that asked.
#[test]
fn an_undo_from_a_window_steps_the_context() {
    let mut editing = Editing::default();
    let take = editing.join("buffer:7", a_take());
    editing
        .event(take, &event(50, 1, "draw", stroke(0.5, 0.0)))
        .unwrap();
    let undo = Event {
        addr: "/gui_event".into(),
        args: vec![json!(49), json!(2), json!(0), json!("undo")],
    };
    let turned = editing.event(take, &undo).unwrap();
    let stepped = turned.stepped.expect("a step");
    assert!(stepped.stepped);
    let Outcome::Audio(outcome) = &turned.outcome else {
        panic!()
    };
    // a step that renders nothing is taken at once, and answered
    assert!(stepped.operation.is_none());
    let answer = serde_json::to_value(outcome.answer.as_ref().expect("answered")).unwrap();
    assert_eq!(answer["seq"], 2, "{answer}");
    assert_eq!(answer["docVersion"], 3, "{answer}");
}

/// The JSON door opens members, turns, steps and reports the state.
#[test]
fn failed_add_file_keeps_the_open_list_and_returns_only_settled_ids() {
    let mut editing = Editing::new();
    let member = an_audio_take(&mut editing);
    let opening = raw_call(
        &mut editing,
        json!({"verb":"member","member":member,
        "call":{"verb":"add_file","path":"/in/take50.wav","workspace":"/work","frames":100,"untitled":true}}),
    );
    assert!(opening["operation"].is_object());
    let files = raw_call(
        &mut editing,
        json!({"verb":"member","member":member,"call":{"verb":"files"}}),
    );
    assert_eq!(files["files"].as_array().unwrap().len(), 1);
    let prepared = &opening["operation"];
    let failed = raw_call(
        &mut editing,
        json!({"verb":"operation","action":"complete",
        "ticket":prepared["ticket"],"batch":prepared["batch"],"result":{"status":"failed","reason":"allocation refused"}}),
    );
    assert!(failed["repair"].is_object());
    assert!(
        failed.get("returned").is_none(),
        "ids remain reserved during recovery"
    );
    let repaired = raw_call(
        &mut editing,
        json!({"verb":"operation","action":"repaired","ticket":failed["repair"]}),
    );
    assert_eq!(repaired["returned"]["buffers"], json!([]));
    // a file of the disk reserves nothing, so nothing comes back
    assert!(repaired["freed"].as_array().is_none_or(|f| f.is_empty()));
    assert_eq!(
        raw_call(
            &mut editing,
            json!({"verb":"member","member":member,"call":{"verb":"files"}})
        )["files"],
        files["files"]
    );
    assert_eq!(
        raw_call(
            &mut editing,
            json!({"verb":"operation","action":"repaired","ticket":failed["repair"]})
        )["accepted"],
        false
    );
}

#[test]
fn window_save_keeps_its_destination_and_dirty_checkpoint_until_confirmation() {
    let mut editing = Editing::new();
    let member = an_audio_take(&mut editing);
    let opening = raw_call(
        &mut editing,
        json!({"verb":"operation","action":"prepare","member":member,"command":{"kind":"open"}}),
    );
    settle_operation(&mut editing, opening);
    draw(&mut editing, member, 1, 10);
    let window = 49;
    raw_call(
        &mut editing,
        json!({"verb":"member","member":member,"call":{"verb":"sync","window":window}}),
    );
    let version = editing.version();
    let pending = raw_call(
        &mut editing,
        json!({"verb":"event","member":member,"addr":"/gui_event",
        "args":[window,2,version,"save"]}),
    );
    let prepared = pending["outcome"]["operation"].clone();
    assert!(
        prepared.is_object(),
        "window save uses prepared batches: {pending}"
    );
    assert_eq!(
        raw_call(
            &mut editing,
            json!({"verb":"member","member":member,"call":{"verb":"binding"}})
        )["dirty"],
        true
    );
    let (saved, _) = settle_operation(&mut editing, prepared);
    assert_eq!(saved["committed"], true);
    assert_eq!(
        raw_call(
            &mut editing,
            json!({"verb":"member","member":member,"call":{"verb":"binding"}})
        )["dirty"],
        false
    );
}

#[test]
fn saving_one_audio_window_does_not_acknowledge_in_another_context() {
    let mut origin = Editing::new();
    let mut other = Editing::new();
    let member = an_audio_take(&mut origin);
    let unrelated = an_audio_take(&mut other);
    for (context, member, window) in [(&mut origin, member, 49), (&mut other, unrelated, 99)] {
        let opening = raw_call(
            context,
            json!({
                "verb":"operation", "action":"prepare", "member":member,
                "command":{"kind":"open"}
            }),
        );
        settle_operation(context, opening);
        raw_call(
            context,
            json!({"verb":"member", "member":member,
            "call":{"verb":"sync", "window":window}}),
        );
        draw(context, member, 1, 10);
    }
    for args in [json!([49, 2, 0, "save"]), json!([49, 3, 0, "menu", "save"])] {
        let wrong = raw_call(
            &mut other,
            json!({"verb":"event", "member":unrelated,
            "addr":"/gui_event", "args":args}),
        );
        assert!(
            wrong["outcome"]["answer"].is_null(),
            "foreign window must not acknowledge: {wrong}"
        );
        assert!(wrong["outcome"]["operation"].is_null());
        let own = raw_call(
            &mut origin,
            json!({"verb":"event", "member":member,
            "addr":"/gui_event", "args":args}),
        );
        assert!(own["outcome"]["operation"].is_object(), "{own}");
        let (saved, _) = settle_operation(&mut origin, own["outcome"]["operation"].clone());
        assert_eq!(saved["committed"], true);
        assert_eq!(
            raw_call(
                &mut other,
                json!({"verb":"member", "member":unrelated,
            "call":{"verb":"binding"}})
            )["dirty"],
            true
        );
    }
}

#[test]
fn the_door_opens_turns_and_steps() {
    let mut editing = Editing::default();
    let take = call(
        &mut editing,
        json!({"verb": "openAudio", "sharedHistory": true, "key": "buffer:7", "path":"/in/take7.wav","workspace":"/work", "frames": 100}),
    )["member"]
        .clone();
    call(
        &mut editing,
        json!({"verb": "member", "member": take, "call": {"verb": "window", "widget": 50}}),
    );
    let turned = call(
        &mut editing,
        json!({"verb": "event", "member": take, "addr": "/gui_event",
               "args": [50, 1, 0, "draw", 0, 2, [0.5], [0.0]]}),
    );
    assert_eq!(turned["version"], 2);
    assert_eq!(turned["outcome"]["record"]["label"], "draw the samples");
    let state = call(&mut editing, json!({"verb": "state"}));
    assert_eq!(
        call(&mut editing, json!({"verb": "moved"}))["version"],
        3,
        "an edit with no entry moves the version and records nothing"
    );
    assert_eq!(state["undoLabel"], "draw the samples");
    let stepped = call(&mut editing, json!({"verb": "step", "direction": "undo"}));
    assert_eq!(stepped["effects"][0]["kind"], "audio");
    assert_eq!(
        call(
            &mut editing,
            json!({"verb": "openAudio", "sharedHistory": true, "key": "x", "layers": []})
        )["error"]
            .as_str()
            .map(|e| e.contains("measures something")),
        Some(true)
    );
}

fn raw_call(editing: &mut Editing, request: Value) -> Value {
    serde_json::from_str(&call_json(editing, &request.to_string())).unwrap()
}

// A headless executor: acknowledge declared batches, keeping their steps
// for ordering assertions. The document is inspected only after commit.
fn settle_operation(editing: &mut Editing, mut prepared: Value) -> (Value, Vec<Value>) {
    let mut steps = Vec::new();
    while prepared["complete"] != true {
        steps.extend(prepared["steps"].as_array().unwrap().iter().cloned());
        prepared = raw_call(
            editing,
            json!({"verb":"operation", "action":"complete",
            "ticket":prepared["ticket"], "batch":prepared["batch"], "result":{"status":"success"}}),
        );
    }
    (prepared, steps)
}

fn call(editing: &mut Editing, request: Value) -> Value {
    let mut answer = raw_call(editing, request);
    if answer["outcome"]["operation"].is_object() {
        let (completed, _) = settle_operation(editing, answer["outcome"]["operation"].clone());
        for key in ["outcome", "freed", "version"] {
            if let Some(value) = completed.get(key) {
                answer[key] = value.clone();
            }
        }
    } else if answer["operation"].is_object() {
        let (completed, steps) = settle_operation(editing, answer["operation"].clone());
        answer = if completed["outcome"].is_object() {
            completed["outcome"].clone()
        } else {
            completed["stepped"].clone()
        };
        if let Some(effects) = answer["effects"].as_array_mut() {
            for effect in effects {
                if effect["kind"] == "audio" {
                    effect["steps"] = json!(steps);
                }
            }
        }
        for key in ["freed", "version"] {
            if let Some(value) = completed.get(key) {
                answer[key] = value.clone();
            }
        }
    }
    // Keep the original sparse encoding used by the assertions below.
    if answer["freed"].as_array().is_some_and(Vec::is_empty) {
        answer.as_object_mut().unwrap().remove("freed");
    }
    answer
}

/// Two windows over one curve edit one curve, and an undo in either hands
/// the holder the points to write back once.
#[test]
fn two_points_editors_over_one_curve_share_it_and_one_undo() {
    let mut editing = Editing::default();
    let open = |editing: &mut Editing, widget: i64| -> Value {
        let member = call(
            editing,
            json!({"verb": "openPoints", "key": "object:1", "rate": 100.0,
                   "points": [0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 0.0]}),
        )["member"]
            .clone();
        call(
            editing,
            json!({"verb": "member", "member": member, "call": {"verb": "window", "widget": widget}}),
        );
        member
    };
    let first = open(&mut editing, 40);
    let second = open(&mut editing, 41);
    assert_eq!(
        editing.structure(int(&first) as MemberId),
        editing.structure(int(&second) as MemberId)
    );
    let turned = call(
        &mut editing,
        json!({"verb": "event", "member": first, "addr": "/gui_event",
               "args": [40, 1, 0, "points", 0.0, 0.0, 1, 0.0, 1.0, 0.5, 1, 0.0]}),
    );
    assert_eq!(
        turned["outcome"]["points"],
        json!([0.0, 0.0, 1.0, 0.0, 1.0, 0.5, 1.0, 0.0])
    );
    let state = call(
        &mut editing,
        json!({"verb": "member", "member": second, "call": {"verb": "state"}}),
    );
    assert_eq!(
        state["points"][5], 0.5,
        "the other window's curve is the same one"
    );
    let corrected = &turned["corrections"][0];
    assert_eq!(corrected["member"], second);
    let stepped = call(&mut editing, json!({"verb": "step", "direction": "undo"}));
    assert_eq!(stepped["effects"].as_array().unwrap().len(), 1);
    assert_eq!(stepped["effects"][0]["kind"], "points");
    assert_eq!(stepped["effects"][0]["points"][5], 1.0);
    assert_eq!(stepped["corrections"].as_array().unwrap().len(), 2);
}

/// An audio editor over take 3 (100 frames), drawn through join 9 by
/// widget 12, with buffers 20..24 for new takes.
fn an_audio_take(editing: &mut Editing) -> MemberId {
    let opened = call(
        editing,
        json!({"verb": "openAudio", "sharedHistory": true, "key": "take:3", "path":"/in/take3.wav","workspace":"/work", "frames": 100,
               "rate": 48000}),
    );
    let member = opened["member"].as_u64().unwrap() as MemberId;
    call(
        editing,
        json!({"verb": "member", "member": member, "call": {"verb": "window", "widget": 12}}),
    );
    member
}

fn draw(editing: &mut Editing, member: MemberId, seq: i64, at: i64) -> Value {
    let version = editing.version();
    call(
        editing,
        json!({"verb": "event", "member": member, "addr": "/gui_event",
               "args": [12, seq, version, "draw", 0, at, [0.5], [0.0]]}),
    )
}

/// **A take the history can no longer reach is handed back to free**, and
/// one it can is not: an undo keeps the stroke's take for the redo, and
/// the edit after it lets it go.
#[test]
fn a_stroke_s_take_is_freed_when_no_entry_can_reach_it() {
    // the takes an entry names by number, as the history holds them
    let named = |editing: &Editing| -> Vec<u64> {
        let take = |id: u64| id & (1 << 61) != 0 && id & (1 << 62) == 0;
        let held = editing.services.histories.held_sources();
        held.into_iter()
            .map(|s| s.0)
            .filter(|id| take(*id))
            .collect()
    };
    let mut editing = Editing::default();
    let take = an_audio_take(&mut editing);
    let first = draw(&mut editing, take, 1, 10);
    let before = named(&editing);
    assert_eq!(before.len(), 2, "the take before the stroke and after it");
    assert!(before.iter().all(|id| editing.shared.take(*id).is_some()));
    assert!(first.get("freed").is_none());
    let undone = call(&mut editing, json!({"verb": "step", "direction": "undo"}));
    assert_eq!(undone["effects"][0]["kind"], "audio");
    assert!(undone.get("freed").is_none(), "a redo can still find it");
    let second = draw(&mut editing, take, 2, 30);
    assert_eq!(
        second["freed"],
        json!([{"member": take, "files": [
            "/work/1-2-stroke.w64",
            "/work/1-2-stroke.peaks",
            "/work/1-2-stroke.spectra",
        ]}]),
        "the redo is gone, and the stroke's render with it, its summaries too"
    );
    // and so are the takes its entry named, while the new entry's stay
    assert!(before.iter().all(|id| editing.shared.take(*id).is_none()));
    let after = named(&editing);
    assert_eq!(after.len(), 2);
    assert!(after.iter().all(|id| editing.shared.take(*id).is_some()));
}

/// **An empty file in focus has nothing to undo**, whether the edited one
/// was opened before it or after it: two windows, or two files of one
/// window, never fall back to each other's order or the context's.
#[test]
fn an_empty_focused_file_never_undoes_another_whichever_opened_first() {
    let open = |editing: &mut Editing, n: i64| {
        let member = call(
            editing,
            json!({"verb": "openAudio", "key": format!("file:{n}"), "path": format!("/in/take{}.wav", 3 + n), "workspace": "/work",
                   "frames": 100, "rate": 48000}),
        )["member"]
            .as_u64()
            .unwrap() as MemberId;
        call(
            editing,
            json!({"verb": "member", "member": member,
                   "call": {"verb": "window", "widget": 12}}),
        );
        member
    };
    for edited_first in [true, false] {
        // two windows
        let mut editing = Editing::new();
        let members = [open(&mut editing, 0), open(&mut editing, 1)];
        let (edited, empty) = if edited_first {
            (members[0], members[1])
        } else {
            (members[1], members[0])
        };
        draw(&mut editing, edited, 1, 10);
        let before = reads(&editing, edited);
        let state = call(&mut editing, json!({"verb": "actions", "member": empty}));
        assert_eq!(state["undo"]["enabled"], false, "{state}");
        assert_eq!(state["undo"]["target"]["member"], empty);
        for target in [json!(empty), Value::Null] {
            let stepped = call(
                &mut editing,
                json!({"verb": "step", "member": target, "direction": "undo"}),
            );
            assert_ne!(stepped["stepped"], true, "{target}: {stepped}");
        }
        assert_eq!(reads(&editing, edited), before);
        let state = call(&mut editing, json!({"verb": "actions", "member": edited}));
        assert_eq!(state["undo"]["enabled"], true);

        // two files of one window: the added one is edited or left empty
        let mut editing = Editing::new();
        let member = open(&mut editing, 0);
        call(
            &mut editing,
            json!({"verb": "member", "member": member, "call": {"verb": "add_file",
                   "path":"/in/take50.wav","workspace":"/work", "frames": 100, "channels": 1, "rate": 48000,
                   "untitled": true}}),
        );
        let (first, second) = (0, 1);
        let (edited, empty) = if edited_first {
            (first, second)
        } else {
            (second, first)
        };
        let focus = |editing: &mut Editing, index: i64| {
            call(
                editing,
                json!({"verb": "member", "member": member,
                       "call": {"verb": "focus_file", "index": index}}),
            );
        };
        focus(&mut editing, edited);
        draw(&mut editing, member, 1, 10);
        let before = reads(&editing, member);
        focus(&mut editing, empty);
        let state = call(&mut editing, json!({"verb": "actions", "member": member}));
        assert_eq!(state["undo"]["enabled"], false, "{state}");
        let stepped = call(
            &mut editing,
            json!({"verb": "step", "member": member, "direction": "undo"}),
        );
        assert_ne!(stepped["stepped"], true, "{stepped}");
        focus(&mut editing, edited);
        assert_eq!(reads(&editing, member), before, "the edited file is intact");
        let state = call(&mut editing, json!({"verb": "actions", "member": member}));
        assert_eq!(state["undo"]["enabled"], true);
    }
}

#[test]
fn actions_read_current_presentation_and_pending_operations() {
    let mut editing = Editing::new();
    let member = an_audio_take(&mut editing);
    let state = call(&mut editing, json!({"verb":"actions", "member":member}));
    assert_eq!(state["copy"]["enabled"], false);
    assert_eq!(
        state["copy"]["unselected"], true,
        "the host reads its own selection"
    );
    // a process renders the selection: nothing selected, nothing to do
    assert_eq!(state["reverse"]["enabled"], false);
    assert_eq!(state["reverse"]["unselected"], true);
    assert_eq!(state["view_wave"]["checked"], true);
    call(
        &mut editing,
        json!({"verb":"member", "member":member, "call":{"verb":"view_both"}}),
    );
    let state = call(&mut editing, json!({"verb":"actions", "member":member}));
    assert_eq!(state["view_wave"]["checked"], false);
    assert_eq!(state["view_both"]["checked"], true);
    let prepared = raw_call(
        &mut editing,
        json!({"verb":"operation", "action":"prepare", "member":member,
        "command":{"kind":"save", "path":"/tmp/action.wav"}}),
    );
    assert!(prepared["ticket"].is_object(), "{prepared}");
    let state = call(&mut editing, json!({"verb":"actions", "member":member}));
    assert_eq!(state["save"]["pending"], true);
    assert_eq!(state["save"]["enabled"], false);
    // pending work outweighs an empty selection, which a host reads itself
    assert_eq!(state["cut"]["reason"], "the file has pending work");
    assert_eq!(state["cut"].get("unselected"), None);
    assert_eq!(state["view_wave"]["enabled"], true);
    settle_operation(&mut editing, prepared);
    assert_eq!(
        call(&mut editing, json!({"verb":"actions", "member":member}))["save"]["pending"],
        false
    );
}

#[test]
fn context_history_steps_accept_an_omitted_or_null_member() {
    for target in [None, Some(Value::Null)] {
        let mut editing = Editing::new();
        let member = an_audio_take(&mut editing);
        draw(&mut editing, member, 1, 10);
        let mut request = json!({"verb":"step", "direction":"undo"});
        if let Some(target) = target {
            request["member"] = target;
        }
        let stepped = call(&mut editing, request);
        assert_eq!(stepped["stepped"], true, "{stepped}");
        assert_eq!(stepped["effects"][0]["member"], member);
    }
}

#[test]
fn invalid_history_step_members_never_use_another_order() {
    for direction in ["undo", "redo"] {
        let mut editing = Editing::new();
        let member = an_audio_take(&mut editing);
        draw(&mut editing, member, 1, 10);
        call(
            &mut editing,
            json!({"verb":"member", "member":member,
            "call":{"verb":"add_file", "path":"/in/take50.wav","workspace":"/work", "frames":100}}),
        );
        draw(&mut editing, member, 2, 20);
        if direction == "redo" {
            call(
                &mut editing,
                json!({"verb":"step", "member":member,
                "direction":"undo"}),
            );
        }
        let selected = editing.services.histories.selected();
        let version = editing.version();
        let before = reads(&editing, member);
        for target in [
            json!(999),
            json!(1_u64 << 32),
            json!(u64::MAX),
            json!(-1),
            json!(0.5),
            json!("0"),
            json!(false),
        ] {
            let refused = raw_call(
                &mut editing,
                json!({"verb":"step",
                "member":target, "direction":direction}),
            );
            assert_eq!(refused["stepped"], false, "{target}: {refused}");
            assert_eq!(refused["reason"], "the history step member is invalid");
            assert!(refused["operation"].is_null());
            assert_eq!(refused["effects"], json!([]));
            assert_eq!(editing.services.histories.selected(), selected);
            assert_eq!(editing.version(), version);
            assert_eq!(reads(&editing, member), before);
        }
        let valid = call(
            &mut editing,
            json!({"verb":"step", "member":member,
            "direction":direction}),
        );
        assert_eq!(valid["stepped"], true, "{valid}");
    }
}

/// **A window's undo walks the order of its file in focus**, whichever
/// member had the last turn, and the context's order is the other window's.
#[test]
fn a_step_named_for_a_window_walks_its_focused_file_beside_another_window() {
    let mut editing = Editing::new();
    let member = an_audio_take(&mut editing);
    call(
        &mut editing,
        json!({"verb": "member", "member": member, "call": {"verb": "add_file",
        "path":"/in/take50.wav","workspace":"/work", "frames": 200, "channels": 1, "rate": 48000, "untitled": true}}),
    );
    assert_eq!(
        draw(&mut editing, member, 1, 30)["outcome"]["changed"],
        true
    );
    let other = call(
        &mut editing,
        json!({"verb": "openAudio", "sharedHistory": true, "key": "take:4", "path":"/in/take4.wav","workspace":"/work", "frames": 100,
               "rate": 48000}),
    )["member"]
        .as_u64()
        .unwrap() as MemberId;
    call(
        &mut editing,
        json!({"verb": "member", "member": other, "call": {"verb": "window", "widget": 13}}),
    );
    let version = editing.version();
    call(
        &mut editing,
        json!({"verb": "event", "member": other, "addr": "/gui_event",
               "args": [13, 1, version, "draw", 0, 10, [0.5], [0.0]]}),
    );
    let state = |editing: &mut Editing, member: Option<MemberId>| {
        call(editing, json!({"verb": "state", "member": member}))
    };
    assert_eq!(state(&mut editing, Some(member))["canUndo"], true);
    let undone = call(
        &mut editing,
        json!({"verb": "step", "direction": "undo", "member": member}),
    );
    assert_eq!(undone["stepped"], true);
    assert_eq!(undone["effects"][0]["member"], member);
    assert_eq!(state(&mut editing, Some(member))["canUndo"], false);
    assert_eq!(
        state(&mut editing, Some(other))["canUndo"],
        true,
        "the other window's edit stays"
    );
    let context = call(&mut editing, json!({"verb": "step", "direction": "undo"}));
    assert_eq!(context["effects"][0]["member"], other);
}

/// **Closing the file an editor was opened over** forgets its edits in
/// the context's order, and the context's order is left whole when the
/// editor's last file closes.
#[test]
fn closing_the_opened_file_forgets_its_edits_and_keeps_the_context_order() {
    let mut editing = Editing::new();
    let member = an_audio_take(&mut editing);
    draw(&mut editing, member, 1, 10);
    call(
        &mut editing,
        json!({"verb": "member", "member": member, "call": {"verb": "add_file",
        "path":"/in/take50.wav","workspace":"/work", "frames": 200, "channels": 1, "rate": 48000, "untitled": true}}),
    );
    call(
        &mut editing,
        json!({"verb": "member", "member": member, "call": {"verb": "close_file", "file": 1}}),
    );
    let context = call(&mut editing, json!({"verb": "step", "direction": "undo"}));
    assert_ne!(
        context["stepped"], true,
        "nothing applies a closed file's edit"
    );
    let closed = call(
        &mut editing,
        json!({"verb": "member", "member": member, "call": {"verb": "close_file"}}),
    );
    assert_eq!(closed["empty"], true, "the editor stays, with no file");
    assert!(closed["binding"].is_null());
    assert_eq!(editing.services.histories.orders_len(), 1);
    assert_eq!(editing.services.histories.selected(), ScopeId(0));
}

/// **File > Close file closes the file in focus and no other**: with a
/// second file open, the window stays, showing the file left.
#[test]
fn the_close_file_entry_closes_only_the_focused_file() {
    let mut editing = Editing::new();
    let member = an_audio_take(&mut editing);
    call(
        &mut editing,
        json!({"verb": "member", "member": member, "call": {"verb": "sync", "window": 900}}),
    );
    call(
        &mut editing,
        json!({"verb": "member", "member": member, "call": {"verb": "add_file",
        "path":"/in/take50.wav","workspace":"/work", "frames": 200, "channels": 1, "rate": 48000, "untitled": true}}),
    );
    let version = editing.version();
    let asked = call(
        &mut editing,
        json!({"verb": "event", "member": member, "addr": "/gui_event",
               "args": [900, 1, version, "close_file"]}),
    );
    let action = asked["outcome"]["fileAction"].clone();
    let closed = call(
        &mut editing,
        json!({"verb": "member", "member": member, "call": {"verb": action["verb"], "file": action["file"]}}),
    );
    assert!(closed.get("error").is_none(), "{closed}");
    assert_ne!(closed["empty"], true, "one file is left");
    let files = call(
        &mut editing,
        json!({"verb": "member", "member": member, "call": {"verb": "files"}}),
    );
    assert_eq!(files["files"].as_array().unwrap().len(), 1, "{files}");
}

/// **An audio editor opened with no file** has no order of its own and
/// nothing to undo; its first file comes into focus with an order of its
/// own, and closing it leaves the editor empty again with no order kept.
#[test]
fn an_audio_editor_opens_empty_and_returns_to_empty() {
    let mut editing = Editing::new();
    let opened = call(
        &mut editing,
        json!({"verb": "openAudio", "key": "audio:empty", "empty": true}),
    );
    let member = opened["member"].as_u64().unwrap() as MemberId;
    let member_call = |editing: &mut Editing, inner: Value| {
        call(
            editing,
            json!({"verb": "member", "member": member, "call": inner}),
        )
    };
    member_call(&mut editing, json!({"verb": "window", "widget": 12}));
    let orders = editing.services.histories.orders_len();
    let bound = member_call(&mut editing, json!({"verb": "binding"}));
    assert_eq!(bound["empty"], true, "{bound}");
    assert_eq!(
        call(&mut editing, json!({"verb": "state", "member": member}))["canUndo"],
        false
    );
    let added = member_call(
        &mut editing,
        json!({"verb": "add_file", "path":"/in/take50.wav","workspace":"/work", "frames": 200,
        "channels": 1, "rate": 48000, "untitled": true}),
    );
    assert_eq!(added["binding"]["id"], 2, "{added}");
    let version = editing.version();
    let drawn = call(
        &mut editing,
        json!({"verb": "event", "member": member, "addr": "/gui_event",
               "args": [12, 1, version, "draw", 0, 10, [0.5], [0.0]]}),
    );
    assert_eq!(drawn["outcome"]["changed"], true, "{drawn}");
    assert!(editing.can_undo());
    let closed = member_call(&mut editing, json!({"verb": "close_file"}));
    assert_eq!(closed["empty"], true);
    assert!(!editing.can_undo(), "nothing of the closed file is left");
    assert_eq!(editing.services.histories.orders_len(), orders);
    assert_eq!(
        member_call(&mut editing, json!({"verb": "parts"}))["error"],
        "no file is open"
    );
}

#[test]
fn audio_files_keep_independent_histories_and_restore_their_state() {
    let mut editing = Editing::new();
    let member = an_audio_take(&mut editing);
    let first = draw(&mut editing, member, 1, 10);
    assert!(first["outcome"]["changed"].as_bool().unwrap());
    let member_call = |editing: &mut Editing, inner: Value| {
        call(
            editing,
            json!({"verb": "member", "member": member, "call": inner}),
        )
    };
    member_call(&mut editing, json!({"verb": "locate", "at": 0.0005}));
    member_call(
        &mut editing,
        json!({"verb": "span", "span": [0.0002, 0.0008]}),
    );
    let first_parts = member_call(&mut editing, json!({"verb": "parts"}));
    let added = member_call(
        &mut editing,
        json!({"verb": "add_file", "path":"/in/take50.wav","workspace":"/work", "frames": 200,
        "channels": 2, "rate": 48000, "untitled": true}),
    );
    assert_eq!(added["binding"]["id"], 2);
    assert!(!editing.can_undo(), "a new file has no edits");
    let second = draw(&mut editing, member, 2, 30);
    assert_eq!(second["outcome"]["changed"], true);
    assert!(editing.step(Direction::Undo).stepped);
    assert!(!editing.can_undo());
    assert!(editing.can_redo());
    let restored = member_call(&mut editing, json!({"verb": "focus_file", "file": 1}));
    assert_eq!(restored["binding"]["cursor"], 24);
    // the host learns the version the switch moved to, with the cursor
    // and selection the incoming file left
    assert_eq!(restored["answer"]["answer"], "push");
    assert_eq!(restored["answer"]["docVersion"], editing.version());
    assert_eq!(
        restored["answer"]["corrections"][0]["props"]["sel_start"],
        10
    );
    assert_eq!(restored["answer"]["corrections"][0]["props"]["sel_len"], 28);
    assert_eq!(
        restored["binding"]["span"],
        json!([10.0 / 48000.0, 38.0 / 48000.0])
    );
    assert!(editing.can_undo());
    assert!(!editing.can_redo());
    assert_eq!(
        member_call(&mut editing, json!({"verb": "parts"})),
        first_parts
    );
    assert!(editing.step(Direction::Undo).stepped);
    assert!(!editing.can_undo());
    member_call(&mut editing, json!({"verb": "focus_file", "file": 2}));
    assert!(editing.can_redo(), "the other file retains its redo");
    assert!(editing.step(Direction::Redo).stepped);
    let closed = member_call(&mut editing, json!({"verb": "close_file", "file": 1}));
    assert_eq!(
        closed["binding"]["id"], 2,
        "closing a background file keeps focus"
    );
    assert_eq!(
        member_call(&mut editing, json!({"verb": "files"}))["files"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        editing.can_undo(),
        "closing another file keeps this history"
    );
}
#[test]
fn a_gesture_from_the_previous_file_cannot_edit_the_new_binding() {
    let mut editing = Editing::new();
    editing.moved();
    let member = an_audio_take(&mut editing);
    let old_version = editing.version();
    call(
        &mut editing,
        json!({"verb":"member", "member":member, "call":{
        "verb":"add_file", "path":"/in/take50.wav","workspace":"/work", "frames":200, "channels":2,
        "rate":48000}}),
    );
    let before = call(
        &mut editing,
        json!({"verb":"member", "member":member, "call":{"verb":"parts"}}),
    );
    let late = call(
        &mut editing,
        json!({"verb":"event", "member":member,
        "addr":"/gui_event", "args":[12, 9, old_version, "draw", 0, 10, [0.5], [0.0]]}),
    );
    assert_eq!(late["outcome"]["turn"], "stale");
    assert_eq!(
        call(
            &mut editing,
            json!({"verb":"member", "member":member, "call":{"verb":"parts"}})
        ),
        before
    );
    assert!(!editing.can_undo());
}
#[test]
fn shared_undo_routes_to_an_unfocused_object_without_rebinding_the_window() {
    let mut editing = Editing::new();
    let member = an_audio_take(&mut editing);
    draw(&mut editing, member, 1, 10);
    call(
        &mut editing,
        json!({"verb":"member", "member":member, "call":{"verb":"add_file", "path":"/in/take50.wav","workspace":"/work","frames":200}}),
    );
    let binding = call(
        &mut editing,
        json!({"verb":"member", "member":member, "call":{"verb":"binding"}}),
    );
    let stepped = call(&mut editing, json!({"verb":"step", "direction":"undo"}));
    assert_eq!(stepped["stepped"], true);
    assert_eq!(stepped["effects"][0]["file"]["id"], 1);
    let after = call(
        &mut editing,
        json!({"verb":"member", "member":member, "call":{"verb":"binding"}}),
    );
    assert_eq!(after["id"], binding["id"]);
    assert_eq!(after["generation"], binding["generation"]);
    call(
        &mut editing,
        json!({"verb":"member", "member":member, "call":{"verb":"focus_file", "file":1}}),
    );
    let parts = call(
        &mut editing,
        json!({"verb":"member", "member":member, "call":{"verb":"parts"}}),
    );
    assert_eq!(parts["parts"].as_array().unwrap().len(), 1);
}

#[test]
fn a_lost_endpoint_refuses_its_step_without_blocking_unrelated_context_edits() {
    let mut editing = Editing::new();
    let member = an_audio_take(&mut editing);
    draw(&mut editing, member, 1, 10);
    let external = editing.join(
        "local",
        Member::External {
            domain: "local".into(),
        },
    );
    call(
        &mut editing,
        json!({"verb":"record", "member":external, "label":"local edit",
        "legs":[{"forward":{"edit":{"x":1}},"backward":{"x":0}}]}),
    );
    call(
        &mut editing,
        json!({"verb":"invalidate", "server":{"connection":0,"generation":0}}),
    );
    assert_eq!(
        call(&mut editing, json!({"verb":"step","direction":"undo"}))["stepped"],
        true
    );
    let refused = call(&mut editing, json!({"verb":"step","direction":"undo"}));
    assert_eq!(refused["stepped"], false);
    assert!(
        refused["reason"]
            .as_str()
            .unwrap()
            .contains("previous server instance")
    );
    assert!(editing.can_undo());
}

/// **A window gone takes its member's history with it**: the takes its
/// entries named are let go, the renders only it wrote are handed back to
/// delete, and what stays of the member holds no file.
#[test]
fn a_forgotten_audio_member_lets_go_of_what_only_it_named() {
    let mut editing = Editing::default();
    let opened = call(
        &mut editing,
        json!({"verb": "openAudio", "key": "take:3", "path":"/in/take3.wav",
               "workspace":"/work", "frames": 100, "rate": 48000}),
    );
    let take = opened["member"].as_u64().unwrap() as MemberId;
    call(
        &mut editing,
        json!({"verb": "member", "member": take, "call": {"verb": "window", "widget": 12}}),
    );
    draw(&mut editing, take, 1, 10);
    draw(&mut editing, take, 2, 30);
    let named: Vec<u64> = editing
        .services
        .histories
        .held_sources()
        .into_iter()
        .map(|s| s.0)
        .filter(|id| id & (1 << 61) != 0 && id & (1 << 62) == 0)
        .collect();
    assert!(!named.is_empty());
    assert!(named.iter().all(|id| editing.shared.take(*id).is_some()));
    let freed = editing.forget_audio(take);
    assert!(
        named.iter().all(|id| editing.shared.take(*id).is_none()),
        "the takes its entries named are let go"
    );
    let files: Vec<&String> = freed.iter().flat_map(|f| &f.files).collect();
    assert!(
        files.iter().any(|f| f.ends_with("-stroke.w64")),
        "its renders are handed back: {files:?}"
    );
    assert!(editing.services.histories.held_sources().is_empty());
    assert!(matches!(
        editing.member(take),
        Some(Member::Audio(editor)) if !editor.has_file()
    ));
}
