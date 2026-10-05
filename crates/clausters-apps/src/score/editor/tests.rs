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
    editor.window(IDS);
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
    let window = editor.window(IDS);
    let scroll = &window["children"][0];
    assert_eq!(scroll["type"], "scroll");
    assert_eq!(scroll["id"], 11);
    let page = &scroll["children"][0];
    assert_eq!(page["type"], "score");
    assert_eq!(page["id"], 10);
    assert_eq!(page["editable"], true);
    assert_eq!(page["entry"], true);
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

#[test]
fn a_press_on_empty_staff_writes_the_value_in_hand_and_selects_it() {
    let mut editor = opened();
    call_json(&mut editor, r#"{"verb": "sync", "value": [1, 8]}"#);
    let out = editor.event(&gesture("insert", &[json!("n4"), json!(-4), json!(0)]), 1);
    assert!(out.changed);
    let selected = out.selected.expect("the new note is selected");
    let id = item_id(&selected[0]).expect("an item");
    let held = editor.held();
    let at = verbs::locate(held.sheet().unwrap(), id).expect("written");
    assert_eq!(at.item.dur(), Ratio::new(1, 8));
    assert_eq!(at.onset, Ratio::ONE, "after the fourth quarter");
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
        r#"{"entry":true}"#
    );
    call_json(&mut editor, r#"{"verb": "sync", "entry": false}"#);
    // the page learns it with the next correction
    let Answer::Push { corrections, .. } = editor.resync_all(1) else {
        panic!("corrections")
    };
    assert_eq!(corrections[0].props["entry"], json!(false));
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
    let window = editor.window(IDS);
    // a page view fixes the page, at the default nobody chose: A4
    assert_eq!(laid()["pageWidth"], 2100);
    assert_eq!(laid()["breaks"], "auto");
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
