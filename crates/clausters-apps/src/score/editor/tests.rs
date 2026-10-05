use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use clausters_core::notation::{
    AnyEngraver, Engraver, Item, Marks, Pitch, Score, Sheet, Staff, Step, Voice, sheet_to_mei,
};
use clausters_core::ratio::Ratio;

use super::*;

/// An engraver that keeps the document it was handed and draws nothing: the
/// editor's turns are about the model and the history, and none of them needs
/// a page to be right about.
struct Kept {
    mei: Mutex<String>,
    /// The options it was last laid out under.
    options: Arc<Mutex<String>>,
}

impl Engraver for Kept {
    type Guard = ();
    fn lock(&self) -> Self::Guard {}
    fn load_data(&self, data: &str) -> bool {
        *self.mei.lock().unwrap() = data.to_string();
        !data.is_empty()
    }
    fn render_svg(&self, _page: i32) -> String {
        String::new()
    }
    fn mei(&self) -> String {
        self.mei.lock().unwrap().clone()
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
    fn set_options(&self, options: &str) -> bool {
        *self.options.lock().unwrap() = options.to_string();
        true
    }
}

fn note(id: u64) -> Item {
    Item::Note {
        id,
        pitches: vec![Pitch {
            step: Step::E,
            alter: 0,
            octave: 4,
            forced: false,
        }],
        dur: Ratio::new(1, 4),
        tie: false,
        marks: Marks::default(),
    }
}

/// A bar of four quarters, ids 1 to 4.
fn shared() -> Shared {
    let sheet = Sheet {
        next_id: 5,
        staves: vec![Staff {
            clef: "G2".into(),
            voices: vec![Voice {
                items: (1..=4).map(note).collect(),
            }],
        }],
        ..Sheet::default()
    };
    let mei = sheet_to_mei(&sheet).expect("writes");
    let engraver = AnyEngraver::new(Kept {
        mei: Mutex::new(String::new()),
        options: OPTIONS.with(Arc::clone),
    });
    Arc::new(Mutex::new(Score::open(engraver, &mei).expect("opens")))
}

thread_local! {
    /// What this test's engraver was last asked to lay out under.
    static OPTIONS: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
}

fn laid() -> Value {
    serde_json::from_str(&OPTIONS.with(|o| o.lock().unwrap().clone())).unwrap_or_default()
}

const IDS: Ids = Ids {
    page: 10,
    scroll: Some(11),
    status: Some(12),
};

fn opened() -> ScoreEditor {
    let mut editor = ScoreEditor::new(shared(), 1);
    editor.window(IDS, Chrome::default());
    editor
}

fn gesture(tag: &str, payload: &[Value]) -> Event {
    let mut args = vec![json!(10), json!(1), json!(1), json!(tag)];
    args.extend_from_slice(payload);
    Event {
        addr: "/gui_event".into(),
        args,
    }
}

fn first_marks(editor: &ScoreEditor) -> Marks {
    let held = editor.held();
    let sheet = held.sheet().expect("a model");
    sheet.staves[0].voices[0].items[0]
        .marks()
        .cloned()
        .unwrap_or_default()
}

#[test]
fn the_window_holds_the_page_in_a_scroll_over_a_status_line() {
    let mut editor = ScoreEditor::new(shared(), 1);
    let window = editor.window(IDS, Chrome::default());
    let scroll = &window["children"][0];
    assert_eq!(scroll["type"], "plane");
    assert_eq!(scroll["id"], 11);
    let page = &scroll["children"][0];
    assert_eq!(page["type"], "score");
    assert_eq!(page["id"], 10);
    assert_eq!(page["editable"], true);
    // the window opens outside note entry, with no edit cursor, and the
    // score's own keys in force
    assert_eq!(page["entry"], false);
    assert_eq!(page["edit_cursor"], "");
    assert_eq!(window["keys"], json!(["score"]));
    assert!(
        page.get("notes").is_none(),
        "the notes are the caller's layer"
    );
    let status = &window["children"][1];
    assert_eq!(status["id"], 12);
    assert_eq!(status["text"], HINT);
}

#[test]
fn a_press_selects_and_the_status_line_says_what() {
    let mut editor = opened();
    let out = editor.event(&gesture("element", &[json!("n2")]), 1);
    assert_eq!(out.turn, Kind::Route);
    assert_eq!(out.selected, Some(vec!["n2".to_string()]));
    assert!(!out.changed, "a selection is no edit");
    assert_eq!(editor.items(), vec![2]);
    assert!(
        editor.describe().starts_with("item 2: 1/4"),
        "{}",
        editor.describe()
    );
    // blank paper clears it
    editor.event(&gesture("element", &[json!("")]), 1);
    assert!(editor.items().is_empty());
    assert_eq!(editor.describe(), HINT);
}

#[test]
fn a_verb_over_the_selection_is_one_entry_and_puts_back() {
    let mut editor = opened();
    editor.event(&gesture("element", &[json!("n1")]), 1);
    let before = editor.held().mei();
    let out = editor.act(&json!({"action": "articulation", "name": "stacc"}), 1);
    assert!(out.changed);
    assert_eq!(out.version, 2);
    assert_eq!(
        first_marks(&editor).articulations,
        vec!["stacc".to_string()]
    );
    let record = out.record.expect("an entry");
    assert_eq!(record.label, "articulation stacc");
    assert_eq!(record.legs.len(), 1);
    assert_eq!(record.legs[0].backward, json!({"mei": before}));
    // the history walks it back through the editor, by the MEI it names
    assert!(editor.apply(&record.legs[0].backward));
    assert!(first_marks(&editor).articulations.is_empty());
    // and forward again
    assert!(editor.apply(&record.legs[0].forward));
    assert_eq!(
        first_marks(&editor).articulations,
        vec!["stacc".to_string()]
    );
}

#[test]
fn a_verb_with_nothing_selected_is_refused_out_loud() {
    let mut editor = opened();
    let out = editor.act(&json!({"action": "delete"}), 1);
    assert!(!out.changed);
    assert!(out.record.is_none());
    let why = match out.answer.clone() {
        Some(Answer::Ack {
            reason: Some(why), ..
        })
        | Some(Answer::Push {
            reason: Some(why), ..
        }) => why,
        other => panic!("a reason: {other:?}"),
    };
    assert!(why.contains("select"), "{why}");
}

#[test]
fn an_unknown_verb_is_refused_rather_than_guessed() {
    let mut editor = opened();
    let out = editor.act(&json!({"action": "levitate"}), 1);
    assert!(!out.changed);
}

/// A press on the page, stamped with the version the page saw.
fn pressed(tag: &str, payload: &[Value], version: i64) -> Event {
    let mut event = gesture(tag, payload);
    event.args[2] = json!(version);
    event
}

/// `event`, stamped with the version its widget saw.
fn stamped(mut event: Event, version: i64) -> Event {
    event.args[2] = json!(version);
    event
}

/// A key of the window's: a verb the key table reports to the owner.
fn key(verb: &str) -> Event {
    Event {
        addr: "/gui_event".into(),
        args: vec![json!(1), json!(9), json!(1), json!(verb)],
    }
}

/// The first voice's items, as `(value, sounds)`.
fn shape(editor: &ScoreEditor) -> Vec<(Ratio, bool)> {
    let held = editor.held();
    held.sheet().unwrap().staves[0].voices[0]
        .items
        .iter()
        .map(|i| (i.dur(), i.sounds()))
        .collect()
}

#[test]
fn note_entry_writes_at_the_cursor_and_nothing_moves() {
    let mut editor = with_tools();
    call_json(&mut editor, r#"{"verb": "sync", "value": [1, 8]}"#);
    // outside the mode the letters are nobody's
    assert!(editor.event(&key("pitch_g"), 1).answer.is_none());
    // N: the cursor goes to the first beat, with nothing selected
    let out = editor.event(&key("entry"), 1);
    let Some(Answer::Push { corrections, .. }) = out.answer else {
        panic!("corrections")
    };
    let page = corrections.iter().find(|c| c.widget == 10).unwrap();
    assert_eq!(page.props["entry"], true);
    assert_eq!(page.props["edit_cursor"]["at"], "n1");
    assert!(
        corrections
            .iter()
            .any(|c| c.widget == 1 && c.props["keys"] == json!(["score", "note_entry"]))
    );
    // a G over the first quarter: an eighth and the rest of it silence, and
    // the bar is as long as it was
    let out = editor.event(&key("pitch_g"), 1);
    assert_eq!(out.record.expect("an entry").label, "write a note");
    let q = Ratio::new(1, 4);
    let e = Ratio::new(1, 8);
    assert_eq!(
        shape(&editor),
        vec![(e, true), (e, false), (q, true), (q, true), (q, true)]
    );
    let first = |editor: &ScoreEditor| {
        let held = editor.held();
        held.sheet().unwrap().staves[0].voices[0].items[0].clone()
    };
    // the G nearest the middle of the staff, then a B above it in its chord
    assert_eq!(first(&editor).pitches()[0].midi(), 67);
    editor.event(&key("chord_b"), 2);
    assert_eq!(
        first(&editor)
            .pitches()
            .iter()
            .map(Pitch::midi)
            .collect::<Vec<_>>(),
        vec![67, 71]
    );
    // the cursor stood past the G: a rest goes over the silence, then a
    // step back and up moves the note just written... none here, so the
    // note the cursor is on
    editor.event(&key("enter_rest"), 3);
    assert_eq!(shape(&editor).len(), 5);
    editor.event(&key("cursor_left"), 3);
    editor.event(&key("cursor_left"), 3);
    editor.event(&key("step_up"), 3);
    assert_eq!(first(&editor).pitches()[0].midi(), 69, "a step up is an A");
    // the digits pick the value in hand
    editor.event(&key("value_half"), 4);
    assert!(call_json(&mut editor, r#"{"verb": "input"}"#).contains("[1,2]"));
    // Escape leaves the mode, and the cursor goes
    let out = editor.event(&key("entry_off"), 4);
    let Some(Answer::Push { corrections, .. }) = out.answer else {
        panic!("corrections")
    };
    let page = corrections.iter().find(|c| c.widget == 10).unwrap();
    assert_eq!(page.props["edit_cursor"], "", "the empty string is none");
    // and coming back with nothing selected, it is where it was left
    editor.event(&pressed("element", &[json!("")], 5), 5);
    editor.event(&key("entry"), 5);
    assert!(
        editor
            .describe()
            .starts_with("note entry: staff 1 voice 1, bar 1 + 0"),
        "{}",
        editor.describe()
    );
}

#[test]
fn a_press_in_note_entry_writes_over_a_rest_and_chords_a_note() {
    let mut editor = with_tools();
    editor.event(&gesture("element", &[json!("n3")]), 1);
    editor.act(&json!({"action": "silence"}), 1);
    // the mode opens on what is selected: the third beat
    editor.event(&key("entry"), 2);
    assert!(
        editor.describe().contains("bar 1 + 1/2"),
        "{}",
        editor.describe()
    );
    // a press on the rest's column writes the value in hand there
    let out = editor.event(&pressed("enter", &[json!("n3"), json!(-3), json!(0)], 2), 2);
    assert_eq!(
        out.record
            .unwrap_or_else(|| panic!("an entry: {:?}", out.answer))
            .label,
        "write a note"
    );
    let held = editor.held();
    let third = held.sheet().unwrap().staves[0].voices[0].items[2].clone();
    drop(held);
    assert!(third.sounds());
    assert_eq!(third.pitches()[0].midi(), 72, "three steps under F5 is C5");
    // a press on a note's column builds its chord
    let out = editor.event(&pressed("enter", &[json!("n4"), json!(-5), json!(0)], 3), 3);
    assert_eq!(out.record.expect("an entry").label, "add to the chord");
    let held = editor.held();
    let fourth = &held.sheet().unwrap().staves[0].voices[0].items[3];
    assert_eq!(fourth.pitches().len(), 2);
    drop(held);
    assert_eq!(shape(&editor).len(), 4, "nothing moved");
    // and playing leaves the mode
    let out = editor.event(&key("play"), 4);
    assert!(out.play.is_some());
    assert!(!editor.describe().starts_with("note entry"));
}

#[test]
fn a_spanner_runs_between_the_selected_notes() {
    let mut editor = opened();
    call_json(
        &mut editor,
        r#"{"verb": "select", "elements": ["n3", "n1"]}"#,
    );
    let out = editor.act(&json!({"action": "spanner", "kind": "slur"}), 1);
    assert!(out.changed, "{:?}", out.answer);
    let held = editor.held();
    let spanners = &held.sheet().unwrap().spanners;
    assert_eq!(spanners.len(), 1);
    assert_eq!((spanners[0].from, spanners[0].to), (1, 3));
}

#[test]
fn the_door_reads_and_sets_the_selection() {
    let mut editor = opened();
    call_json(
        &mut editor,
        r#"{"verb": "select", "elements": ["n2", "n2-1"]}"#,
    );
    let selected: Value =
        serde_json::from_str(&call_json(&mut editor, r#"{"verb": "selected"}"#)).unwrap();
    assert_eq!(
        selected["items"],
        json!([2]),
        "two parts of one item are one"
    );
    let mei: Value = serde_json::from_str(&call_json(&mut editor, r#"{"verb": "mei"}"#)).unwrap();
    assert!(mei["mei"].as_str().is_some_and(|m| m.contains("<mei")));
}

#[test]
fn a_step_corrects_the_whole_window() {
    let mut editor = opened();
    let Answer::Push { corrections, .. } = editor.resync_all(3) else {
        panic!("corrections")
    };
    let widgets: Vec<i64> = corrections.iter().map(|c| c.widget).collect();
    assert_eq!(widgets, vec![10, 11, 12]);
    assert!(corrections[0].props.get("display_list").is_some());
}

#[test]
fn the_context_records_a_verb_and_walks_it_back_on_the_holders_score() {
    use crate::editing::{self, Editing};
    use clausters_document::history::Direction;

    let mut context = Editing::new();
    let held = shared();
    let opened: Value =
        serde_json::from_str(&context.open_score("score:1", held.clone(), "{}")).unwrap();
    let member = opened["member"].as_u64().expect("a member") as editing::MemberId;
    let selected = editing::call_json(
        &mut context,
        &json!({"verb": "member", "member": member,
                "call": {"verb": "select", "elements": ["n1"]}})
        .to_string(),
    );
    assert_eq!(selected, "{}");
    let turned = context
        .act(member, &json!({"action": "tie"}))
        .expect("a score member acts");
    assert_eq!(turned.version, 2, "one entry moved the version");
    let tied = |held: &Shared| {
        matches!(
            held.lock().unwrap().sheet().unwrap().staves[0].voices[0].items[0],
            Item::Note { tie: true, .. }
        )
    };
    assert!(tied(&held), "the holder's own score was edited");
    assert!(context.can_undo());
    let stepped = context.step(Direction::Undo);
    assert!(stepped.stepped, "{:?}", stepped.reason);
    assert!(!tied(&held), "and the step put it back there");
    assert!(context.step(Direction::Redo).stepped);
    assert!(tied(&held));
}

#[test]
fn ctrl_adds_and_removes_and_shift_reaches_across() {
    let mut editor = opened();
    editor.event(&gesture("element", &[json!("n1")]), 1);
    // Ctrl: the note joins, and the same again takes it out
    editor.event(&gesture("element", &[json!("n3"), json!("toggle")]), 1);
    assert_eq!(editor.items(), vec![1, 3]);
    editor.event(&gesture("element", &[json!("n3"), json!("toggle")]), 1);
    assert_eq!(editor.items(), vec![1]);
    // Shift: everything from the first selected to the one pressed, in time
    let out = editor.event(&gesture("element", &[json!("n4"), json!("extend")]), 1);
    assert_eq!(editor.items(), vec![1, 2, 3, 4]);
    // the page is told the whole range, which the press alone could not know
    let Some(Answer::Push { corrections, .. }) = out.answer else {
        panic!("corrections")
    };
    assert_eq!(
        corrections[0].props["selected"],
        json!(["n1", "n2", "n3", "n4"])
    );
    assert_eq!(editor.describe(), "4 items selected");
    // a plain press on paper clears; a modified one does not
    editor.event(&gesture("element", &[json!(""), json!("toggle")]), 1);
    assert_eq!(editor.items().len(), 4);
    editor.event(&gesture("element", &[json!("")]), 1);
    assert!(editor.items().is_empty());
}

#[test]
fn a_press_on_a_staff_selects_its_measure_where_entry_is_off() {
    let mut editor = opened();
    assert_eq!(
        call_json(&mut editor, r#"{"verb": "entry"}"#),
        r#"{"entry":false}"#
    );
    call_json(&mut editor, r#"{"verb": "sync", "entry": true}"#);
    // the page learns it with the next correction
    let Answer::Push { corrections, .. } = editor.resync_all(1) else {
        panic!("corrections")
    };
    assert_eq!(corrections[0].props["entry"], json!(true));
    call_json(&mut editor, r#"{"verb": "sync", "entry": false}"#);
    // a staff's own lines are named by measure and staff
    editor.event(&gesture("element", &[json!("m1s1")]), 1);
    assert_eq!(editor.items(), vec![1, 2, 3, 4]);
}

#[test]
fn a_transformation_runs_over_what_is_selected() {
    let mut editor = opened();
    editor.event(&gesture("element", &[json!("n2")]), 1);
    let out = editor.act(
        &json!({"action": "transform", "name": "transpose", "semitones": 12}),
        1,
    );
    assert!(out.changed, "{:?}", out.answer);
    assert_eq!(out.record.expect("an entry").label, "transpose");
    let held = editor.held();
    let first = &held.sheet().unwrap().staves[0].voices[0].items[0];
    assert_eq!(
        first.pitches()[0].octave,
        5,
        "the bar it is in moved up an octave"
    );
}

#[test]
fn the_window_lays_the_score_out_on_its_paper_and_the_view_is_the_windows() {
    let mut editor = ScoreEditor::new(shared(), 1);
    let window = editor.window(IDS, Chrome::default());
    // a page view fixes the page, at the default nobody chose: A4
    assert_eq!(laid()["pageWidth"], 2100);
    assert_eq!(laid()["breaks"], "smart");
    let scroll = &window["children"][0];
    assert_eq!(
        scroll["axis"], "both",
        "a drag on blank paper pans either way"
    );
    assert_eq!(scroll["bars"], true);
    // the view is switched without an entry: nothing was edited
    call_json(&mut editor, r#"{"verb": "sync", "layout": "continuous"}"#);
    assert_eq!(
        call_json(&mut editor, r#"{"verb": "layout"}"#),
        r#"{"layout":"continuous"}"#
    );
    editor.resync_all(1);
    assert_eq!(laid()["breaks"], "none");
}

#[test]
fn the_page_setup_is_the_documents_and_an_edit_like_any_other() {
    let mut editor = opened();
    let out = editor.act(
        &json!({"action": "page", "paper": "letter", "landscape": true, "staff": 800}),
        1,
    );
    assert!(out.changed, "{:?}", out.answer);
    let record = out.record.expect("an entry");
    assert_eq!(record.label, "page setup");
    let page: Value = serde_json::from_str(&call_json(&mut editor, r#"{"verb": "page"}"#)).unwrap();
    assert_eq!(page["paper"], "Letter");
    assert_eq!(page["landscape"], true);
    assert_eq!(page["page"]["width"], 2794);
    assert_eq!(page["page"]["staff"], 800);
    assert!(page["papers"].as_array().unwrap().contains(&json!("A4")));
    // the engraver is laid out again on the new paper
    assert_eq!(laid()["pageWidth"], 2794);
    assert_eq!(laid()["unit"], 10.0);
    // and the history puts the old one back
    assert!(editor.apply(&record.legs[0].backward));
    editor.resync_all(2);
    assert_eq!(laid()["pageWidth"], 2100);
    // a paper nobody makes is refused out loud
    let out = editor.act(&json!({"action": "page", "paper": "foolscap"}), 2);
    assert!(!out.changed);
}

#[test]
fn a_text_of_the_page_is_written_moved_and_described() {
    let mut editor = opened();
    let out = editor.act(
        &json!({"action": "text", "field": "title", "text": "A title"}),
        1,
    );
    assert!(out.changed, "{:?}", out.answer);
    assert_eq!(out.record.expect("an entry").label, "page text: title");
    editor.act(
        &json!({"action": "text", "field": "note", "text": "* a footnote"}),
        2,
    );
    // moved: the title to the left, on every page
    editor.act(
        &json!({"action": "text", "field": "title", "halign": "left", "pages": "all"}),
        3,
    );
    {
        let held = editor.held();
        let header = &held.sheet().unwrap().header;
        assert_eq!(header.title, "A title");
        assert_eq!(header.notes, vec!["* a footnote".to_string()]);
        assert_eq!(
            header.place("title").halign,
            clausters_core::notation::Halign::Left
        );
    }
    // a press on its block says which field it is and where it sits
    editor.event(&gesture("element", &[json!("t-title")]), 1);
    assert_eq!(
        editor.describe(),
        "title: \"A title\" -- head left middle, every page"
    );
    // typed over where the page draws it: the element's id and what it says,
    // from a page that saw the score as it stands
    let typed = |element: &str, text: &str, seen: i64| {
        let mut event = gesture("text", &[json!(element), json!(text)]);
        event.args[2] = json!(seen);
        event
    };
    let out = editor.event(&typed("t-title", "Another", 4), 4);
    assert!(out.changed, "{:?}", out.answer);
    assert_eq!(out.record.expect("an entry").label, "page text: title");
    assert_eq!(editor.held().sheet().unwrap().header.title, "Another");
    let seen = out.version;
    let out = editor.event(&typed("t-note-1", "* changed", seen), seen);
    assert_eq!(
        editor.held().sheet().unwrap().header.notes,
        vec!["* changed".to_string()]
    );
    // an element that is no page text writes nothing
    let seen = out.version;
    assert!(!editor.event(&typed("n1", "x", seen), seen).changed);
    editor.act(
        &json!({"action": "text", "field": "title", "text": "A title"}),
        seen,
    );
    // back where the convention puts it, the override goes
    editor.act(
        &json!({"action": "text", "field": "title", "halign": "center", "pages": "first"}),
        4,
    );
    assert!(editor.held().sheet().unwrap().header.places.is_empty());
    // an empty text takes a footnote away, and an unknown field is refused
    editor.act(
        &json!({"action": "text", "field": "note", "index": 0, "text": ""}),
        5,
    );
    assert!(editor.held().sheet().unwrap().header.notes.is_empty());
    assert!(
        !editor
            .act(&json!({"action": "text", "field": "motto", "text": "x"}), 6)
            .changed
    );
}

/// A pick of the menu bar: the event the host sends, addressed to the window.
fn pick(verb: &str, state: Option<i64>) -> Event {
    let mut args = vec![json!(1), json!(7), json!(1), json!("menu"), json!(verb)];
    args.extend(state.map(|s| json!(s)));
    Event {
        addr: "/gui_event".into(),
        args,
    }
}

#[test]
fn the_window_carries_the_menu_bar_and_a_pick_is_the_verb_it_wrote() {
    let mut editor = ScoreEditor::new(shared(), 1);
    let window = editor.window(IDS, Chrome::default());
    let titles: Vec<&str> = window["menu"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["label"].as_str())
        .collect();
    assert_eq!(
        titles,
        vec![
            "File",
            "Edit",
            "View",
            "Play",
            "Notes",
            "Notation",
            "Measures",
            "Transform"
        ]
    );
    call_json(&mut editor, r#"{"verb": "sync", "window": 1}"#);
    editor.event(&gesture("element", &[json!("n1")]), 1);
    // an entry that edits is the editor's verb, and one entry of the history
    let verb = json!({"action": "articulation", "name": "stacc"}).to_string();
    let out = editor.event(&pick(&verb, None), 1);
    assert!(out.changed, "{:?}", out.answer);
    assert_eq!(out.record.expect("an entry").label, "articulation stacc");
    assert_eq!(
        first_marks(&editor).articulations,
        vec!["stacc".to_string()]
    );
    // undo is the context's to walk: the editor says a step was asked for
    let out = editor.event(&pick("undo", None), 2);
    assert_eq!(out.turn, Kind::Step);
    assert!(!out.redo);
    assert_eq!(out.seq, 7);
}

#[test]
fn what_is_the_windows_own_moves_the_editor_and_the_bar_says_so() {
    let mut editor = ScoreEditor::new(shared(), 1);
    editor.window(IDS, Chrome::default());
    call_json(&mut editor, r#"{"verb": "sync", "window": 1}"#);
    let out = editor.event(&pick("layout:continuous", Some(1)), 1);
    assert!(!out.changed && out.record.is_none(), "a layout is no edit");
    assert_eq!(
        call_json(&mut editor, r#"{"verb": "layout"}"#),
        r#"{"layout":"continuous"}"#
    );
    // the answer carries the bar again, with the layout that is now in force
    let Some(Answer::Push { corrections, .. }) = out.answer else {
        panic!("corrections")
    };
    let bar = &corrections.last().expect("the window's").props["menu"];
    assert_eq!(
        bar[2]["menu"][1]["checked"], true,
        "Continuous is the one on"
    );
    editor.event(&pick("value:1/8", Some(1)), 1);
    assert_eq!(
        call_json(&mut editor, r#"{"verb": "value"}"#),
        r#"{"value":[1,8]}"#
    );
    editor.event(&pick("entry", Some(0)), 1);
    assert_eq!(
        call_json(&mut editor, r#"{"verb": "entry"}"#),
        r#"{"entry":false}"#
    );
    editor.event(&pick("select_all", None), 1);
    assert_eq!(editor.items(), vec![1, 2, 3, 4]);
}

/// The toolbar's tools, numbered from 100 in their order.
fn numbered() -> tools::Ids {
    tools::TOOLS
        .iter()
        .enumerate()
        .map(|(i, name)| ((*name).to_string(), 100 + i as i32))
        .collect()
}

/// A tool's report: a state tool's value, or a button's click, where a tag is.
fn tool(name: &str, report: Value) -> Event {
    let id = numbered()[name];
    Event {
        addr: "/gui_event".into(),
        args: vec![json!(id), json!(3), json!(1), report],
    }
}

fn with_tools() -> ScoreEditor {
    let mut editor = ScoreEditor::new(shared(), 1);
    editor.window(
        IDS,
        Chrome {
            tools: numbered(),
            ..Chrome::default()
        },
    );
    call_json(&mut editor, r#"{"verb": "sync", "window": 1}"#);
    editor
}

#[test]
fn the_window_has_the_toolbar_when_its_tools_are_numbered() {
    let mut editor = ScoreEditor::new(shared(), 1);
    let bare = editor.window(IDS, Chrome::default());
    assert_eq!(bare["children"].as_array().unwrap().len(), 2);
    let tree = editor.window(
        IDS,
        Chrome {
            tools: numbered(),
            ..Chrome::default()
        },
    );
    let children = tree["children"].as_array().unwrap();
    assert_eq!(
        children.len(),
        3,
        "the toolbar, the scroll, the status line"
    );
    assert_eq!(children[0]["flow"], "row");
    assert_eq!(children[1]["type"], "plane");
    // the door numbers them by the names it hands out
    let names: Value =
        serde_json::from_str(&call_json(&mut editor, r#"{"verb": "tools"}"#)).unwrap();
    assert_eq!(names["tools"].as_array().unwrap().len(), tools::TOOLS.len());
    let tree: Value = serde_json::from_str(&call_json(
        &mut editor,
        r#"{"verb": "window", "widget": 10, "tools": {"value": 100, "fold": 7}}"#,
    ))
    .unwrap();
    assert_eq!(tree["children"][0]["children"][0]["id"], 100);
    assert_eq!(tree["children"][0]["children"].as_array().unwrap().len(), 1);
}

#[test]
fn the_input_state_is_what_the_next_press_writes() {
    let mut editor = with_tools();
    // an eighth, dotted: the chrome is corrected and no page is engraved
    let out = editor.event(&tool("value", json!(3)), 1);
    assert!(
        !out.changed && out.record.is_none(),
        "the value in hand is no edit"
    );
    let Some(Answer::Push { corrections, .. }) = out.answer else {
        panic!("corrections")
    };
    assert!(
        corrections.iter().all(|c| c.widget != 10),
        "the page is left"
    );
    assert!(
        corrections
            .iter()
            .any(|c| c.widget == i64::from(numbered()["value"]) && c.props == json!({"index": 3}))
    );
    editor.event(&tool("dot", json!(1)), 1);
    assert_eq!(
        call_json(&mut editor, r#"{"verb": "input"}"#),
        r#"{"accidental":null,"dotted":true,"rest":false,"value":[1,8]}"#
    );
    // a press past the bar, in note entry, writes a dotted eighth there
    editor.event(&tool("entry", json!(1)), 1);
    call_json(&mut editor, r#"{"verb": "sync", "entry": false}"#);
    call_json(&mut editor, r#"{"verb": "sync", "entry": true}"#);
    for _ in 0..4 {
        editor.event(&key("cursor_right"), 1);
    }
    let out = editor.event(&key("pitch_c"), 1);
    assert_eq!(out.record.expect("an entry").label, "write a note");
    let written = |editor: &ScoreEditor| {
        let held = editor.held();
        held.sheet().unwrap().staves[0].voices[0].items[4].clone()
    };
    assert_eq!(written(&editor).dur(), Ratio::new(3, 16));
    assert!(written(&editor).sounds());
    // and with rest on, a press writes a rest of that value
    editor.event(&stamped(tool("rest", json!(1)), 2), 2);
    let silenced = editor.act(&json!({"action": "silence"}), 2);
    assert!(silenced.changed);
    let out = editor.event(&pressed("enter", &[json!("n5"), json!(-3), json!(0)], 3), 3);
    assert_eq!(out.record.expect("an entry").label, "write a rest");
    assert!(!written(&editor).sounds());
    assert_eq!(written(&editor).dur(), Ratio::new(3, 16));
}

#[test]
fn an_accidental_is_the_selections_or_armed_for_the_next_note() {
    let mut editor = with_tools();
    // nothing selected: a sharp is armed, and the note written takes it
    let out = editor.event(&tool("accidental", json!(4)), 1);
    assert!(!out.changed);
    assert_eq!(
        call_json(&mut editor, r#"{"verb": "input"}"#),
        r#"{"accidental":1,"dotted":false,"rest":false,"value":[1,4]}"#
    );
    call_json(&mut editor, r#"{"verb": "sync", "entry": true}"#);
    for _ in 0..4 {
        editor.event(&key("cursor_right"), 1);
    }
    let out = editor.event(&key("pitch_c"), 1);
    assert!(out.changed, "{:?}", out.answer);
    let alter_of = |editor: &ScoreEditor, at: usize| {
        let held = editor.held();
        held.sheet().unwrap().staves[0].voices[0].items[at].pitches()[0].alter
    };
    assert_eq!(
        alter_of(&editor, 4),
        1,
        "one entry wrote the note and its sharp"
    );
    assert!(
        call_json(&mut editor, r#"{"verb": "input"}"#).contains(r#""accidental":null"#),
        "it was for that note"
    );
    // the note written is selected, so a flat now is its own
    let out = editor.event(&stamped(tool("accidental", json!(2)), 2), 2);
    assert_eq!(out.record.expect("an entry").label, "accidental");
    assert_eq!(alter_of(&editor, 4), -1);
    assert!(call_json(&mut editor, r#"{"verb": "input"}"#).contains(r#""accidental":null"#));
}

#[test]
fn a_tool_that_acts_is_a_verb_over_the_selection() {
    let mut editor = with_tools();
    editor.event(&gesture("element", &[json!("n1")]), 1);
    // the button's value rises with the hand and asks nothing
    let out = editor.event(&tool("stacc", json!(1)), 1);
    assert!(!out.changed);
    let out = editor.event(&tool("stacc", json!("click")), 1);
    assert_eq!(out.record.expect("an entry").label, "articulation stacc");
    assert_eq!(
        first_marks(&editor).articulations,
        vec!["stacc".to_string()]
    );
    // the voice tool sends the selection there, and then shows it
    let out = editor.event(&tool("voice", json!(1)), 2);
    assert!(out.changed, "{:?}", out.answer);
    let Some(Answer::Push { corrections, .. }) = out.answer else {
        panic!("corrections")
    };
    let voice = numbered()["voice"];
    assert!(
        corrections
            .iter()
            .any(|c| c.widget == i64::from(voice) && c.props == json!({"index": 1}))
    );
    // and the layout tool is the window's, entering no history
    let out = editor.event(&tool("layout", json!(1)), 3);
    assert!(out.record.is_none());
    assert_eq!(
        call_json(&mut editor, r#"{"verb": "layout"}"#),
        r#"{"layout":"continuous"}"#
    );
}

/// The dialogs' widgets, numbered from 200 in the order the crate names them.
fn named() -> dialogs::Ids {
    dialogs::names()
        .into_iter()
        .enumerate()
        .map(|(i, name)| (name, 200 + i as i32))
        .collect()
}

/// A report from the dialogs' widget `name`: a field's text, or a button's
/// click, where a tag is.
fn said(name: &str, report: &str) -> Event {
    Event {
        addr: "/gui_event".into(),
        args: vec![json!(named()[name]), json!(5), json!(1), json!(report)],
    }
}

fn with_dialogs() -> ScoreEditor {
    let mut editor = ScoreEditor::new(shared(), 1);
    editor.window(
        IDS,
        Chrome {
            dialogs: named(),
            ..Chrome::default()
        },
    );
    call_json(&mut editor, r#"{"verb": "sync", "window": 1}"#);
    editor
}

fn corrections_of(out: &Outcome) -> Vec<Correction> {
    match &out.answer {
        Some(Answer::Push { corrections, .. }) => corrections.clone(),
        _ => Vec::new(),
    }
}

#[test]
fn the_window_holds_its_dialogs_and_the_bar_opens_them_only_where_there_are() {
    let mut editor = ScoreEditor::new(shared(), 1);
    let bare = editor.window(IDS, Chrome::default());
    assert_eq!(bare["children"].as_array().unwrap().len(), 2);
    let entry = |tree: &Value, label: &str| {
        tree["menu"][0]["menu"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["label"] == label)
            .cloned()
            .unwrap()
    };
    assert_eq!(entry(&bare, "Page text...")["enabled"], false);
    let tree = editor.window(
        IDS,
        Chrome {
            dialogs: named(),
            ..Chrome::default()
        },
    );
    let children = tree["children"].as_array().unwrap();
    assert_eq!(
        children.len(),
        3,
        "the scroll, the status line, the dialogs"
    );
    assert_eq!(children[2]["flow"], "stack");
    assert_eq!(entry(&tree, "Page text...")["enabled"], true);
    // the door names them for a caller to number
    let names: Value =
        serde_json::from_str(&call_json(&mut editor, r#"{"verb": "dialogs"}"#)).unwrap();
    assert_eq!(names["dialogs"].as_array().unwrap().len(), named().len());
}

#[test]
fn the_page_text_form_opens_holding_the_header_and_writes_one_entry() {
    let mut editor = with_dialogs();
    {
        // a score that came with a title: the form opens holding it
        let mut held = editor.held();
        let mut header = held.sheet().unwrap().header.clone();
        header.title = "A title".into();
        assert!(held.apply(&Op::SetHeader { header }));
    }
    let out = editor.event(&pick("dialog:text", None), 1);
    assert!(!out.changed, "opening a form is no edit");
    let shown = corrections_of(&out);
    let ids = named();
    assert!(shown.iter().any(
        |c| c.widget == i64::from(ids["text:title"]) && c.props == json!({"value": "A title"})
    ));
    assert_eq!(
        shown.last().map(|c| (c.widget, c.props.clone())),
        Some((i64::from(ids["stack"]), json!({"index": 1})))
    );
    assert!(shown.iter().all(|c| c.widget != 10), "no page is engraved");
    // two fields typed, as the host reports them: each change, whole
    editor.event(&said("text:composer", "A. Comp"), 1);
    editor.event(&said("text:composer", "A. Composer"), 1);
    let typed = editor.event(&said("text:notes", "* a footnote"), 1);
    assert!(!typed.changed && typed.record.is_none(), "nothing until OK");
    let out = editor.event(&said("text:ok", "click"), 1);
    assert_eq!(out.record.as_ref().expect("one entry").label, "page text");
    {
        let held = editor.held();
        let header = &held.sheet().unwrap().header;
        assert_eq!(
            (header.title.as_str(), header.composer.as_str()),
            ("A title", "A. Composer")
        );
        assert_eq!(header.notes, vec!["* a footnote".to_string()]);
    }
    // and the form is down
    assert_eq!(
        corrections_of(&out)
            .last()
            .map(|c| (c.widget, c.props.clone())),
        Some((i64::from(ids["stack"]), json!({"index": 0})))
    );
    let after = editor.event(&said("text:title", "ignored"), 2);
    assert!(corrections_of(&after).is_empty() && !after.changed);
}

#[test]
fn a_form_is_cancelled_by_its_button_and_by_the_dialog_itself() {
    let mut editor = with_dialogs();
    let ids = named();
    for (widget, report) in [("page:cancel", "click"), ("page", "cancel")] {
        editor.event(&pick("dialog:page", None), 1);
        editor.event(&said("page:top", "30"), 1);
        let out = editor.event(&said(widget, report), 1);
        assert!(!out.changed && out.record.is_none());
        assert_eq!(
            corrections_of(&out),
            vec![Correction {
                widget: i64::from(ids["stack"]),
                props: json!({"index": 0}),
            }]
        );
    }
    assert_eq!(editor.setup().margins, [127; 4], "nothing was written");
}

#[test]
fn a_form_that_cannot_be_read_stays_up_and_says_why() {
    let mut editor = with_dialogs();
    editor.event(&pick("dialog:page", None), 1);
    editor.event(&said("page:left", "wide"), 1);
    let out = editor.event(&said("page:ok", "click"), 1);
    let answered = serde_json::to_value(&out.answer).unwrap();
    let reason = answered["reason"].as_str().unwrap_or_default();
    assert!(reason.contains("millimetres"), "{answered}");
    assert!(!out.changed);
    // corrected, it is written: the margins, in tenths of a millimetre
    editor.event(&said("page:left", "20"), 1);
    let out = editor.event(&said("page:ok", "click"), 1);
    assert_eq!(out.record.expect("an entry").label, "page setup");
    assert_eq!(editor.setup().margins, [127, 127, 127, 200]);
}

#[test]
fn a_transformation_asks_for_its_parameter() {
    let mut editor = with_dialogs();
    let ids = named();
    let out = editor.event(&pick("dialog:transpose", None), 1);
    let shown = corrections_of(&out);
    assert!(
        shown
            .iter()
            .any(|c| c.widget == i64::from(ids["param:label"])
                && c.props == json!({"text": "Semitones"}))
    );
    editor.event(&said("param:value", "12"), 1);
    let before = {
        let held = editor.held();
        held.sheet().unwrap().staves[0].voices[0].items[0].pitches()[0].octave
    };
    let out = editor.event(&said("param:ok", "click"), 1);
    assert_eq!(out.record.expect("an entry").label, "transpose");
    let held = editor.held();
    let after = held.sheet().unwrap().staves[0].voices[0].items[0].pitches()[0].octave;
    assert_eq!(after, before + 1, "an octave up");
}

/// The palettes' entries, numbered from 400 in the order the crate names them.
fn entries() -> palettes::Ids {
    palettes::names()
        .into_iter()
        .enumerate()
        .map(|(i, name)| (name, 400 + i as i32))
        .collect()
}

#[test]
fn the_palettes_stand_beside_the_page_and_an_entry_is_a_verb_over_the_selection() {
    let mut editor = ScoreEditor::new(shared(), 1);
    let tree = editor.window(
        IDS,
        Chrome {
            palettes: entries(),
            ..Chrome::default()
        },
    );
    // a row split between the column of palettes and the page's scroll
    let work = &tree["children"][0];
    assert_eq!(
        (work["flow"].as_str(), work["split"].as_bool()),
        (Some("row"), Some(true))
    );
    assert!(work["children"][0]["children"][0]["title"].is_string());
    assert_eq!(work["children"][1]["type"], "plane");
    // the door names the entries for a caller to number
    let names: Value =
        serde_json::from_str(&call_json(&mut editor, r#"{"verb": "palettes"}"#)).unwrap();
    assert_eq!(names["palettes"].as_array().unwrap().len(), entries().len());

    call_json(&mut editor, r#"{"verb": "sync", "window": 1}"#);
    editor.event(&gesture("element", &[json!("n1")]), 1);
    let press = |name: &str, report: &str| Event {
        addr: "/gui_event".into(),
        args: vec![json!(entries()[name]), json!(6), json!(1), json!(report)],
    };
    // the button's value is no command; its click is the entry's verb
    let out = editor.event(&press("articulations:ten", "1"), 1);
    assert!(!out.changed);
    let out = editor.event(&press("articulations:ten", "click"), 1);
    assert_eq!(out.record.expect("an entry").label, "articulation ten");
    assert_eq!(first_marks(&editor).articulations, vec!["ten".to_string()]);
    let out = editor.event(&press("notes:unacc", "click"), 2);
    assert_eq!(out.record.expect("an entry").label, "grace note");
    assert_eq!(first_marks(&editor).grace.as_deref(), Some("unacc"));
}

/// The window's own save, as Ctrl+S sends it.
fn window_save() -> Event {
    Event {
        addr: "/gui_event".into(),
        args: vec![json!(1), json!(8), json!(1), json!("save")],
    }
}

#[test]
fn a_save_names_the_scores_file_and_asks_for_one_where_it_has_none() {
    let mut editor = with_dialogs();
    let ids = named();
    // no file yet: the save is asked where to, by the menu and by Ctrl+S alike
    for ask in [pick("save", None), window_save()] {
        let out = editor.event(&ask, 1);
        assert_eq!(out.save, None);
        let shown = corrections_of(&out);
        assert!(
            shown
                .iter()
                .any(|c| c.widget == i64::from(ids["file"])
                    && c.props == json!({"title": "Save as"}))
        );
        assert_eq!(
            shown.last().map(|c| (c.widget, c.props.clone())),
            Some((i64::from(ids["stack"]), json!({"index": 4})))
        );
        editor.event(&said("file:cancel", "click"), 1);
    }
    // a path typed and accepted is the file, written now and from now on
    editor.event(&pick("save", None), 1);
    editor.event(&said("file:path", "/tmp/a score.mei"), 1);
    let out = editor.event(&said("file:ok", "click"), 1);
    assert_eq!(out.save.as_deref(), Some("/tmp/a score.mei"));
    assert!(!out.changed && out.record.is_none(), "a save is no edit");
    let out = editor.event(&window_save(), 1);
    assert_eq!(out.save.as_deref(), Some("/tmp/a score.mei"));
    // and a script that read the score from a file says so
    call_json(&mut editor, r#"{"verb": "sync", "path": "/tmp/other.mei"}"#);
    assert_eq!(
        editor.event(&pick("save", None), 1).save.as_deref(),
        Some("/tmp/other.mei")
    );
    // an empty path names nothing
    editor.event(&pick("dialog:save", None), 1);
    editor.event(&said("file:path", "  "), 1);
    let out = editor.event(&said("file:ok", "click"), 1);
    assert!(out.save.is_none());
    assert!(serde_json::to_value(&out.answer).unwrap()["reason"].is_string());
}

#[test]
fn a_file_is_opened_by_its_holder_and_the_document_it_read_is_one_entry() {
    let mut editor = with_dialogs();
    // the form names the file; reading it is whoever drives the editor's
    editor.event(&pick("dialog:open", None), 1);
    editor.event(&said("file:path", "/tmp/two.mei"), 1);
    let out = editor.event(&said("file:ok", "click"), 1);
    assert_eq!(out.open.as_deref(), Some("/tmp/two.mei"));
    assert!(!out.changed);

    // what it read comes back as the `open` verb: the score, replaced whole
    let two = Sheet {
        next_id: 3,
        staves: vec![Staff {
            clef: "F4".into(),
            voices: vec![Voice {
                items: (1..=2).map(note).collect(),
            }],
        }],
        ..Sheet::default()
    };
    let data = sheet_to_mei(&two).unwrap();
    let before = editor.held().mei();
    let out = editor.act(&json!({"action": "open", "data": data}), 1);
    assert_eq!(out.record.as_ref().expect("an entry").label, "open");
    assert_eq!(
        editor.items(),
        Vec::<u64>::new(),
        "nothing of the old score is selected"
    );
    {
        let held = editor.held();
        let sheet = held.sheet().unwrap();
        assert_eq!(sheet.staves[0].clef, "F4");
        assert_eq!(sheet.staves[0].voices[0].items.len(), 2);
    }
    // and the score that was there is a step back
    let leg = &out.record.unwrap().legs[0];
    assert_eq!(leg.backward["mei"], json!(before));
    // a document that is none is refused, and the score stays
    let held = editor.held().mei();
    let out = editor.act(&json!({"action": "open", "data": ""}), 2);
    assert!(!out.changed);
    assert_eq!(editor.held().mei(), held);
}

/// The window's own verb, as the host sends the space bar and `L`.
fn window(tag: &str, looping: i64) -> Event {
    Event {
        addr: "/gui_event".into(),
        args: vec![json!(1), json!(9), json!(1), json!(tag), json!(looping)],
    }
}

#[test]
fn a_play_asks_for_a_pass_from_where_the_selection_starts() {
    let mut editor = with_tools();
    // nothing selected: from the cursor, which is the start
    let out = editor.event(&window("play", 0), 1);
    assert_eq!(
        out.play,
        Some(json!({"looping": false, "range": null, "from": 0.0}))
    );
    assert!(!out.changed && out.record.is_none(), "a play is no edit");
    // one note selected: from it, and no range to repeat
    editor.event(&gesture("element", &[json!("n3")]), 1);
    let out = editor.event(&tool("play", json!("click")), 1);
    assert_eq!(
        out.play,
        Some(json!({"looping": false, "range": null, "from": 2.0}))
    );
    // several: the stretch they cover, a quarter to the beat
    editor.event(&gesture("element", &[json!("n2")]), 1);
    editor.event(&gesture("element", &[json!("n3"), json!("extend")]), 1);
    let out = editor.event(&pick("play", None), 1);
    assert_eq!(
        out.play,
        Some(json!({"looping": false, "range": [1.0, 3.0], "from": 1.0}))
    );
}

#[test]
fn the_loop_switch_is_one_whoever_turns_it_and_a_rewind_cues_the_start() {
    let mut editor = with_tools();
    let switch = numbered()["loop"];
    // `L` over the window: the pass in progress is told, and the toolbar's
    // switch and the menu's check show it
    let out = editor.event(&window("loop", 1), 1);
    assert_eq!(out.relooped.as_ref().unwrap()["looping"], true);
    let shown = corrections_of(&out);
    assert!(
        shown
            .iter()
            .any(|c| c.widget == i64::from(switch) && c.props == json!({"value": 1}))
    );
    let bar = &shown.last().expect("the window's").props["menu"];
    let play = bar
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["label"] == "Play")
        .unwrap();
    assert_eq!(play["menu"][3]["checked"], true);
    // the toolbar turns it off, and the next play does not loop
    let out = editor.event(&tool("loop", json!(0)), 1);
    assert_eq!(out.relooped.as_ref().unwrap()["looping"], false);
    assert_eq!(
        editor.event(&window("play", 1), 1).play.unwrap()["looping"],
        false
    );
    // a rewind is the cursor at the first beat
    let out = editor.event(&tool("rewind", json!("click")), 1);
    assert_eq!(out.locate, Some(0.0));
    assert_eq!(editor.event(&pick("rewind", None), 1).locate, Some(0.0));
}

#[test]
fn the_score_plays_as_its_render_on_the_engravers_time() {
    let mut editor = opened();
    let rendered: Value =
        serde_json::from_str(&call_json(&mut editor, r#"{"verb": "render"}"#)).unwrap();
    let sequence = &rendered["sequence"];
    assert_eq!(sequence["events"].as_array().unwrap().len(), 4);
    assert_eq!(sequence["events"][1]["at"], 1.0);
    // 120 quarters a minute, which is where the page's cursor is drawn
    let tempo: clausters_core::tempomap::TempoMap =
        serde_json::from_value(sequence["tempo_map"].clone()).unwrap();
    assert_eq!(tempo.secs_at(2.0), 1.0);
    // and an edit is in the next render
    editor.event(&gesture("element", &[json!("n1")]), 1);
    editor.act(&json!({"action": "delete"}), 1);
    let again: Value =
        serde_json::from_str(&call_json(&mut editor, r#"{"verb": "render"}"#)).unwrap();
    assert_eq!(again["sequence"]["events"].as_array().unwrap().len(), 3);
    // the window says its owner plays it
    assert_eq!(editor.window(IDS, Chrome::default())["plays"], true);
}

#[test]
fn an_export_names_its_file_and_its_format_and_edits_nothing() {
    let mut editor = with_dialogs();
    let ids = named();
    for (entry, title, format) in [
        ("dialog:export_midi", "Export MIDI", "smf"),
        ("dialog:export_clip", "Export clip", "clip"),
    ] {
        let out = editor.event(&pick(entry, None), 1);
        assert!(
            corrections_of(&out)
                .iter()
                .any(|c| c.widget == i64::from(ids["file"]) && c.props == json!({"title": title}))
        );
        editor.event(&said("file:path", "/tmp/a.mid"), 1);
        let out = editor.event(&said("file:ok", "click"), 1);
        assert_eq!(
            out.export,
            Some(json!({"path": "/tmp/a.mid", "format": format}))
        );
        assert!(
            !out.changed && out.save.is_none(),
            "the score and its file are as they were"
        );
    }
}
