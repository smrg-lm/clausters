use super::*;
use crate::editing::held::{HeldEngine, HeldTake};
use clausters_core::ids::{IdShare, IdSpaces, ServerShape};
use clausters_document::{
    Document, Lifetime,
    session::{Session, Source},
};
use clausters_editing::apply::{Applier, Endpoint, Medium, Step};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug)]
struct Engine;
impl HeldEngine for Engine {
    fn medium(&mut self) -> Box<dyn Medium + Send> {
        Box::new(Applier::new(Endpoint::default()))
    }
    fn rate(&self) -> f64 {
        48_000.0
    }
    fn take(&mut self, _: MemberId, _: &HeldTake, _: &mut IdSpaces) -> Result<Vec<i32>, String> {
        panic!("the fixture has no tracks to play")
    }
    fn revision(&self, _: MemberId, _: u64) -> Option<u64> {
        None
    }
    fn close(&mut self, _: MemberId, _: u64, _: &mut IdSpaces) {}
    fn close_member(&mut self, _: MemberId, _: &mut IdSpaces) {}
    fn rolling(&self, _: i32) -> bool {
        false
    }
    fn control(&self, _: usize) -> f32 {
        0.0
    }
    fn starved(&self) -> u64 {
        0
    }
}

#[derive(Default)]
struct Disk {
    repairs: usize,
}
impl Caller for Disk {
    fn header(&mut self, _: &str) -> Result<Header, String> {
        Ok(Header {
            frames: 128,
            channels: 1,
            rate: 48_000.0,
            codec: "wav".into(),
            sample_format: Some("float".into()),
        })
    }
    fn stamps(&mut self, _: &[String]) -> Value {
        Value::Null
    }
    fn carry(&mut self, _: &[Step]) -> Result<(), String> {
        self.repairs += 1;
        Ok(())
    }
    fn delete(&mut self, _: &[String]) {}
    fn call_off(&mut self, _: MemberId) {}
}

struct Fixture {
    editing: Editing,
    member: MemberId,
    ids: IdSpaces,
    disk: Disk,
    folder: std::path::PathBuf,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let folder = std::env::temp_dir().join(format!(
            "multitrack-operation-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&folder).unwrap();
        let mut editing = Editing::new();
        editing.hold_engine(Box::new(Engine));
        let opened: Value = serde_json::from_str(&call_json(
            &mut editing,
            &json!({"verb":"openMultitrack", "key":"session", "multitrack":{}}).to_string(),
        ))
        .unwrap();
        Self {
            editing,
            member: opened["member"].as_u64().unwrap() as u32,
            ids: IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE),
            disk: Disk::default(),
            folder,
        }
    }
    fn open(&mut self, name: &str, sources: u64) -> Render {
        let mut session = Session::new(Document::empty());
        for id in 1..=sources {
            session.sources.insert(
                SourceId(id),
                Source::file(format!("{name}-{id}.wav"), Lifetime::Session),
            );
        }
        let path = self.folder.join(format!("{name}.json"));
        std::fs::write(&path, serde_json::to_vec(&session).unwrap()).unwrap();
        let ran = self.editing.start(
            self.member,
            &json!({"verb":"open", "session":path}),
            None,
            &mut self.disk,
            &mut self.ids,
        );
        assert!(matches!(ran.end, Some(Ok(_))), "{:?}", ran.end);
        ran.render.expect("a measurement is pending")
    }
    fn land(&mut self, run: u64, result: Result<(), String>) -> Ran {
        self.editing.land(
            self.member,
            run,
            result,
            None,
            &mut self.disk,
            &mut self.ids,
        )
    }
    fn peaks(&self, id: u64) -> Option<String> {
        let Some(Member::Multitrack(editor)) = self.editing.member(self.member) else {
            panic!()
        };
        editor
            .take(SourceId(id))
            .unwrap()
            .peaks
            .as_ref()
            .map(ToString::to_string)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.folder).unwrap();
    }
}

#[test]
fn save_confirms_only_the_snapshot_written_and_failure_keeps_the_previous_destination() {
    let mut f = Fixture::new();
    let measured = f.open("save", 1);
    f.land(measured.run, Ok(()));
    let destination = f.folder.join("saved.json");
    let save = f.editing.start(
        f.member,
        &json!({"verb":"save", "path":destination}),
        None,
        &mut f.disk,
        &mut f.ids,
    );
    assert!(save.end.is_none(), "a save is not confirmed before storage");
    let storage = save.render.unwrap();
    let [Step::Store { path, contents }] = &storage.steps[..] else {
        panic!("a storage step")
    };
    let captured: Session = serde_json::from_str(contents).unwrap();
    assert_eq!(
        captured.sources[&SourceId(1)].location,
        clausters_document::session::Location::File {
            path: "save-1.wav".into()
        }
    );
    if let Some(Member::Multitrack(editor)) = f.editing.member_mut(f.member) {
        editor.multitrack().channels = 6;
    }
    f.editing.moved();
    std::fs::write(path, contents).unwrap();
    let saved = f.land(storage.run, Ok(()));
    assert_eq!(saved.end, Some(Ok(json!({"destination":destination}))));
    let Some(Member::Multitrack(editor)) = f.editing.member(f.member) else {
        panic!()
    };
    assert_ne!(
        editor.session_snapshot(),
        *editor.file().saved.as_ref().unwrap()
    );
    let read_back: Session = serde_json::from_slice(&std::fs::read(&destination).unwrap()).unwrap();
    assert_eq!(read_back, captured);
    let checkpoint = editor.file().saved.clone();
    let retry_path = f.folder.join("retry.json");
    let failed = f
        .editing
        .start(
            f.member,
            &json!({"verb":"save", "path":retry_path}),
            None,
            &mut f.disk,
            &mut f.ids,
        )
        .render
        .unwrap();
    let failure = f.land(failed.run, Err("write refused".into()));
    assert!(matches!(failure.end, Some(Err(_))));
    let Member::Multitrack(editor) = f.editing.member(f.member).unwrap() else {
        panic!()
    };
    assert_eq!(
        editor.file().path.as_deref().unwrap(),
        destination.to_string_lossy()
    );
    assert_eq!(editor.file().saved, checkpoint);
    let retry = f
        .editing
        .start(
            f.member,
            &json!({"verb":"save", "path":retry_path}),
            None,
            &mut f.disk,
            &mut f.ids,
        )
        .render
        .unwrap();
    let [Step::Store { path, contents }] = &retry.steps[..] else {
        panic!()
    };
    std::fs::write(path, contents).unwrap();
    f.land(retry.run, Ok(()));
    let Member::Multitrack(editor) = f.editing.member(f.member).unwrap() else {
        panic!()
    };
    assert_eq!(
        editor.file().path.as_deref().unwrap(),
        retry_path.to_string_lossy()
    );
    assert_eq!(editor.file().saved.as_ref().unwrap().multitrack.channels, 6);
}

#[test]
fn measurements_publish_together_and_duplicate_landings_cannot_advance_a_batch() {
    let mut f = Fixture::new();
    let first = f.open("first", 2);
    let work = f.editing.multitrack_waits[&first.run].clone();
    let next = f.land(first.run, Ok(())).render.unwrap();
    assert_ne!(first.run, next.run);
    assert!(f.peaks(1).is_none() && f.peaks(2).is_none());
    let duplicate = f.land(first.run, Ok(()));
    assert!(duplicate.render.is_none() && duplicate.effects.is_empty());
    assert!(
        f.editing
            .multitrack_runtime
            .operations
            .busy(work.ticket.target)
    );
    let done = f.land(next.run, Ok(()));
    assert!(f.peaks(1).is_some() && f.peaks(2).is_some());
    assert_eq!(done.effects, [json!({"kind":"resync"})]);
    assert!(
        !f.editing
            .multitrack_runtime
            .operations
            .busy(work.ticket.target)
    );
    assert!(!f.editing.can_undo(), "a cache is not an edit");
}

#[test]
fn a_reply_of_a_previous_session_cannot_attach_peaks_to_reused_source_ids() {
    let mut f = Fixture::new();
    let old = f.open("old", 1);
    let new = f.open("new", 1);
    let old_reply = f.land(old.run, Ok(()));
    assert!(f.peaks(1).is_none());
    assert!(old_reply.effects.iter().all(|e| e["kind"] != "resync"));
    f.land(new.run, Ok(()));
    assert!(f.peaks(1).unwrap().contains("new-1"));
}

#[test]
fn a_partial_failure_publishes_nothing_and_releases_its_lock_at_once() {
    let mut f = Fixture::new();
    let first = f.open("partial", 2);
    let work = f.editing.multitrack_waits[&first.run].clone();
    let second = f.land(first.run, Ok(())).render.unwrap();
    let failed = f.land(second.run, Err("measurement failed".into()));
    assert!(f.peaks(1).is_none() && f.peaks(2).is_none());
    // a session's work has nothing to recover: the refusal is repaired
    // at once, and the session takes edits again
    assert!(
        !f.editing
            .multitrack_runtime
            .operations
            .busy(work.ticket.target)
    );
    assert!(failed.effects.iter().any(|e| {
        e["reason"]
            .as_str()
            .unwrap_or("")
            .contains("measurement failed")
    }));
    assert_eq!(f.disk.repairs, 0, "no recovery was carried");
    assert!(f.land(second.run, Ok(())).effects.is_empty());
    assert!(!f.editing.can_undo());
}

#[test]
fn a_failed_multitrack_walk_keeps_contents_and_order_then_undo_and_redo_work() {
    use crate::application::operations::Result as IoResult;
    use crate::application::services::Application;
    let mut f = Fixture::new();
    let measured = f.open("history", 1);
    f.land(measured.run, Ok(()));
    let edited = f.editing.start(f.member, &json!({"verb":"handle", "label":"marker",
        "call":{"verb":"apply", "intent":{"intent":"setmarker", "marker":7, "at":2.0, "name":"cue"}}}),
        None, &mut f.disk, &mut f.ids);
    assert!(matches!(edited.end, Some(Ok(_))), "{:?}", edited.end);
    let expected = f
        .editing
        .services
        .histories
        .peek_walk(Direction::Undo)
        .unwrap();
    let scope = f.editing.services.histories.selected();
    let seat = &f.editing.seats[f.member as usize];
    let Member::Multitrack(editor) = &seat.member else {
        panic!()
    };
    let request = Request {
        target: Target {
            member: f.member,
            object: editor.session_binding(),
            server: seat.server,
        },
        command: Command::Restore,
    };
    let plan = editor.prepare(request, &f.editing.services).unwrap();
    let ticket = f
        .editing
        .multitrack_runtime
        .prepare_walk(
            &mut f.editing.services.resources,
            scope,
            Direction::Undo,
            expected.clone(),
            vec![(f.member, plan)],
        )
        .unwrap();
    let mut finished = f
        .editing
        .multitrack_runtime
        .operations
        .finish_ready(ticket)
        .unwrap();
    finished.result = IoResult::Failed("participant refused".into());
    let walk = f.editing.multitrack_runtime.take_walk(ticket).unwrap();
    let refused = f.editing.settle_multitrack_walk(finished, walk);
    assert!(!refused.stepped);
    assert_eq!(
        f.editing.services.histories.peek_walk(Direction::Undo),
        Some(expected)
    );
    assert!(!f.editing.multitrack_runtime.operations.busy(ticket.target));
    let audio = f
        .editing
        .open_independent(Member::Audio(Box::new(audio::AudioEditor::empty())));
    f.editing.select(Some(audio));
    assert!(
        !f.editing.can_undo(),
        "the other application's order is independent"
    );
    assert_eq!(f.editing.query(f.member, "history")["canUndo"], true);
    f.editing.select(Some(audio));
    let undone = f.editing.start(
        f.member,
        &json!({"verb":"undo"}),
        None,
        &mut f.disk,
        &mut f.ids,
    );
    assert_eq!(undone.end, Some(Ok(json!(true))));
    let Member::Multitrack(editor) = f.editing.member(f.member).unwrap() else {
        panic!()
    };
    assert!(editor.multitrack().markers.is_empty());
    let redone = f.editing.start(
        f.member,
        &json!({"verb":"redo"}),
        None,
        &mut f.disk,
        &mut f.ids,
    );
    assert_eq!(redone.end, Some(Ok(json!(true))));
    let Member::Multitrack(editor) = f.editing.member(f.member).unwrap() else {
        panic!()
    };
    assert_eq!(editor.multitrack().markers.len(), 1);
}

#[test]
fn a_closed_view_cannot_be_repopulated_by_a_late_measurement() {
    let mut f = Fixture::new();
    let pending = f.open("closed", 1);
    let closed = f.editing.start(
        f.member,
        &json!({"verb":"close"}),
        None,
        &mut f.disk,
        &mut f.ids,
    );
    assert!(
        closed
            .effects
            .iter()
            .any(|effect| effect["kind"] == "closeWindow")
    );
    let late = f.land(pending.run, Ok(()));
    assert!(f.peaks(1).is_none());
    assert!(late.effects.iter().all(|effect| effect["kind"] != "resync"));
}

#[test]
fn the_close_form_waits_for_storage_and_a_failed_save_keeps_the_window() {
    let mut f = Fixture::new();
    let measured = f.open("closing", 1);
    f.land(measured.run, Ok(()));
    let Member::Multitrack(editor) = f.editing.member_mut(f.member).unwrap() else {
        panic!()
    };
    editor.set_close_form(Some(crate::closing::Ids {
        stack: 50,
        dialog: 51,
        save: 52,
        discard: 53,
        cancel: 54,
    }));
    editor.set_asks_to_close(true);
    editor.window(40, 41);
    editor.multitrack().channels = 6;
    let answer = |f: &mut Fixture, seq| {
        f.editing.start(
            f.member,
            &json!({"verb":"event", "addr":"/gui_event", "args":[52, seq, 0, "click"]}),
            None,
            &mut f.disk,
            &mut f.ids,
        )
    };
    let saving = answer(&mut f, 1);
    assert!(saving.end.is_none());
    assert!(
        saving
            .effects
            .iter()
            .all(|effect| effect["kind"] != "closeWindow")
    );
    let failed = f.land(saving.render.unwrap().run, Err("write failed".into()));
    assert!(matches!(failed.end, Some(Err(_))));
    assert!(f.editing.tracks.contains_key(&f.member));
    assert!(
        failed
            .effects
            .iter()
            .all(|effect| effect["kind"] != "closeWindow")
    );
    let retry = answer(&mut f, 2).render.unwrap();
    let [Step::Store { path, contents }] = &retry.steps[..] else {
        panic!()
    };
    std::fs::write(path, contents).unwrap();
    let closed = f.land(retry.run, Ok(()));
    assert!(
        closed
            .effects
            .iter()
            .any(|effect| effect["kind"] == "closeWindow")
    );
    assert!(
        closed
            .effects
            .iter()
            .all(|effect| effect["kind"] != "resync")
    );
    assert!(!f.editing.tracks.contains_key(&f.member));
    assert!(f.land(retry.run, Ok(())).effects.is_empty());
}

#[test]
fn cancellation_drains_only_its_own_measurement_and_invalidation_retires_its_reply() {
    let mut f = Fixture::new();
    let first = f.open("cancelled", 2);
    let work = f.editing.multitrack_waits[&first.run].clone();
    let dispatch = f.editing.multitrack_runtime.control(
        &mut f.editing.services.resources,
        Control::Cancel {
            ticket: work.ticket,
        },
        f.editing.version,
    );
    assert!(matches!(dispatch, Dispatch::Response(protocol) if protocol.draining));
    assert!(f.land(first.run, Ok(())).render.is_none());
    assert!(f.peaks(1).is_none() && f.peaks(2).is_none());
    assert!(
        !f.editing
            .multitrack_runtime
            .operations
            .busy(work.ticket.target)
    );
    let second = f.open("lost", 1);
    let work = f.editing.multitrack_waits[&second.run].clone();
    f.editing.invalidate_server(work.ticket.target.server);
    assert!(f.land(second.run, Ok(())).effects.is_empty());
    assert!(f.peaks(1).is_none());
    assert!(
        !f.editing
            .multitrack_runtime
            .operations
            .busy(work.ticket.target)
    );
}

#[test]
fn malformed_handle_input_is_refused_before_selecting_an_order_or_creating_playback() {
    let mut f = Fixture::new();
    let selected = f.editing.services.histories.selected();
    let version = f.editing.version;
    for action in [
        json!({"verb":"handle", "call":{"verb":"apply", "intent":{"intent":"setmarker", "marker":7}}}),
        json!({"verb":"handle", "call":{"verb":"record", "legs":[{"forward":{"intent":"setmarker", "marker":7,"at":2.0}}]}}),
        json!({"verb":"play", "range":[0.0]}),
    ] {
        let ran = f
            .editing
            .start(f.member, &action, None, &mut f.disk, &mut f.ids);
        assert!(matches!(ran.end, Some(Err(_))));
        assert!(ran.effects.is_empty());
        assert!(ran.render.is_none());
        assert_eq!(f.editing.services.histories.selected(), selected);
        assert_eq!(f.editing.version, version);
        assert!(f.editing.tracks.is_empty());
    }
    assert!(!f.editing.can_undo());
}

#[test]
fn typed_handle_edits_can_be_grouped_and_walked_as_one_entry() {
    let mut f = Fixture::new();
    let forward = json!({"intent":"setmarker", "marker":7, "at":2.0, "name":"cue"});
    let edited = f.editing.start(
        f.member,
        &json!({"verb":"handle", "record":false,
        "call":{"verb":"apply", "intent":forward}}),
        None,
        &mut f.disk,
        &mut f.ids,
    );
    let answer = edited.end.unwrap().unwrap();
    assert_eq!(answer["applied"], true);
    assert!(!f.editing.can_undo());
    let recorded = f.editing.start(
        f.member,
        &json!({"verb":"handle", "label":"marker block",
        "call":{"verb":"record", "legs":[{"forward":forward, "backward":answer["current"]}]}}),
        None,
        &mut f.disk,
        &mut f.ids,
    );
    assert_eq!(recorded.end, Some(Ok(json!(true))));
    for verb in ["undo", "redo"] {
        let ran = f.editing.start(
            f.member,
            &json!({"verb":verb}),
            None,
            &mut f.disk,
            &mut f.ids,
        );
        assert_eq!(ran.end, Some(Ok(json!(true))));
        let queried = f.editing.start(
            f.member,
            &json!({"verb":"handle", "call":{"verb":"ids", "of":"markers"}}),
            None,
            &mut f.disk,
            &mut f.ids,
        );
        assert_eq!(
            queried.end,
            Some(Ok(
                json!({"ids": if verb == "undo" { vec![] } else { vec![7] }})
            ))
        );
    }
}

#[test]
fn replacing_a_session_hands_back_the_one_let_go_and_closes_its_order() {
    let mut f = Fixture::new();
    let measured = f.open("first", 1);
    f.land(measured.run, Ok(()));
    f.marker(f.member, 7);
    let first_scope = f.editing.services.histories.selected();
    assert!(f.editing.can_undo());
    let path = f.folder.join("second.json");
    let mut session = Session::new(Document::empty());
    session
        .sources
        .insert(SourceId(1), Source::file("second-1.wav", Lifetime::Session));
    std::fs::write(&path, serde_json::to_vec(&session).unwrap()).unwrap();
    let opened = f.editing.start(
        f.member,
        &json!({"verb":"open", "session":path}),
        None,
        &mut f.disk,
        &mut f.ids,
    );
    assert!(matches!(opened.end, Some(Ok(_))), "{:?}", opened.end);
    // the session let go of is handed back as the window left it
    let released = opened
        .effects
        .iter()
        .find(|effect| effect["kind"] == "released")
        .expect("the first session is released");
    let markers = &released["snapshot"]["session"]["multitrack"]["markers"];
    assert_eq!(markers.as_array().map(Vec::len), Some(1), "{released}");
    assert!(
        released["snapshot"]["path"]
            .as_str()
            .unwrap()
            .ends_with("first.json")
    );
    // and its order went with it: nothing walks a session nobody holds
    assert!(
        !f.editing.can_undo(),
        "the second session's order is its own"
    );
    f.editing.services.histories.select(first_scope);
    assert!(!f.editing.can_undo());
    assert_eq!(f.markers(f.member), Some(Ok(json!({"ids":[]}))));
}

#[test]
fn closing_the_file_keeps_the_window_and_an_empty_window_never_selects_another_order() {
    let mut f = Fixture::new();
    let edited = f.editing.start(
        f.member,
        &json!({"verb":"handle", "label":"marker",
        "call":{"verb":"apply", "intent":{"intent":"setmarker", "marker":7, "at":2.0}}}),
        None,
        &mut f.disk,
        &mut f.ids,
    );
    assert!(matches!(edited.end, Some(Ok(_))));
    let original = match f.editing.member(f.member).unwrap() {
        Member::Multitrack(editor) => editor.shared(),
        _ => panic!(),
    };
    let closed = f.editing.start(
        f.member,
        &json!({"verb":"close_file"}),
        None,
        &mut f.disk,
        &mut f.ids,
    );
    assert_eq!(closed.end, Some(Ok(Value::Null)));
    assert!(
        closed
            .effects
            .iter()
            .any(|effect| effect["kind"] == "publish")
    );
    assert!(
        closed
            .effects
            .iter()
            .all(|effect| effect["kind"] != "closeWindow")
    );
    assert_eq!(
        f.editing.multitrack_query(f.member, "file"),
        json!({"empty":true})
    );
    // the session closed took its order with it; what is selected now is
    // the context, and nothing refused selects another
    let scope = f.editing.services.histories.selected();
    for action in [
        json!({"verb":"undo"}),
        json!({"verb":"play"}),
        json!({"verb":"save", "path":f.folder.join("none.json")}),
        json!({"verb":"handle", "call":{"verb":"state"}}),
    ] {
        let ran = f
            .editing
            .start(f.member, &action, None, &mut f.disk, &mut f.ids);
        assert_eq!(ran.end.unwrap().unwrap_err().message, "no file is open");
        assert!(ran.render.is_none());
        assert_eq!(f.editing.services.histories.selected(), scope);
    }
    assert!(!f.editing.can_undo(), "the closed session's order is gone");
    assert_eq!(original.lock().unwrap().markers.len(), 1);
    let history: Value = serde_json::from_str(&call_json(
        &mut f.editing,
        &json!({"verb":"state", "member":f.member}).to_string(),
    ))
    .unwrap();
    assert_eq!(history["canUndo"], false);
    assert_eq!(f.editing.services.histories.selected(), scope);
    let actions = f.editing.action_states(f.member);
    assert_eq!(actions["undo"]["reason"], "no file is open");
    let new = f.editing.start(
        f.member,
        &json!({"verb":"new_file"}),
        None,
        &mut f.disk,
        &mut f.ids,
    );
    assert_eq!(new.end, Some(Ok(Value::Null)));
    let file = f.editing.multitrack_query(f.member, "file");
    assert_eq!(file["empty"], false);
    assert_eq!(file["path"], Value::Null);
    assert_eq!(file["dirty"], false);
    assert!(!f.editing.can_undo());
    assert_eq!(original.lock().unwrap().markers.len(), 1);
}

#[test]
fn opening_without_a_session_is_distinct_from_an_empty_open_session() {
    let mut f = Fixture::new();
    let empty: Value = serde_json::from_str(&call_json(
        &mut f.editing,
        &json!({"verb":"openMultitrack","key":"no-session","empty":true}).to_string(),
    ))
    .unwrap();
    let empty = empty["member"].as_u64().unwrap() as u32;
    assert_eq!(
        f.editing.multitrack_query(empty, "file"),
        json!({"empty":true})
    );
    assert_eq!(f.editing.multitrack_query(f.member, "file")["empty"], false);
    let before = f.editing.services.histories.selected();
    let denied = f.editing.start(
        empty,
        &json!({"verb":"undo"}),
        None,
        &mut f.disk,
        &mut f.ids,
    );
    assert_eq!(denied.end.unwrap().unwrap_err().message, "no file is open");
    assert_eq!(f.editing.services.histories.selected(), before);
    let shown: Value = serde_json::from_str(&call_json(
        &mut f.editing,
        &json!({"verb":"member","member":empty,"call":{"verb":"window","widget":40,"ruler":41}})
            .to_string(),
    ))
    .unwrap();
    assert!(
        shown["children"].is_array(),
        "chrome remains composable: {shown}"
    );
}

#[test]
fn a_failed_open_keeps_the_binding_and_an_event_from_before_replacement_is_refused() {
    let mut f = Fixture::new();
    let Some(Member::Multitrack(editor)) = f.editing.member_mut(f.member) else {
        panic!()
    };
    editor.window(40, 41);
    editor.set_window(Some(39));
    let version = f.editing.version;
    let replaced = f.editing.start(
        f.member,
        &json!({"verb":"new_file"}),
        None,
        &mut f.disk,
        &mut f.ids,
    );
    assert!(matches!(replaced.end, Some(Ok(_))));
    let before = f.editing.multitrack_query(f.member, "file");
    let failed = f.editing.start(
        f.member,
        &json!({"verb":"open","session":f.folder.join("missing.json")}),
        None,
        &mut f.disk,
        &mut f.ids,
    );
    assert!(matches!(failed.end, Some(Err(_))));
    assert_eq!(f.editing.multitrack_query(f.member, "file"), before);
    let late = f.editing.start(
        f.member,
        &json!({"verb":"event","addr":"/gui_event", "args":[39,1,version,"crossfade"]}),
        None,
        &mut f.disk,
        &mut f.ids,
    );
    assert!(
        late.effects
            .iter()
            .any(|effect| effect["kind"] == "echo" && effect["answer"]["reason"].is_string()),
        "{late:?}"
    );
    assert!(!f.editing.can_undo());
    assert_eq!(f.editing.multitrack_query(f.member, "file"), before);
}

#[test]
fn a_snapshot_reopens_unsaved_contents_sources_and_checkpoint() {
    let mut f = Fixture::new();
    let measured = f.open("kept", 1);
    f.land(measured.run, Ok(()));
    let original = f.editing.multitrack_query(f.member, "file")["object"].clone();
    let applied = f.editing.start(
        f.member,
        &json!({"verb":"handle",
        "call":{"verb":"apply","intent":{"intent":"setmarker","marker":7,"at":3.0}}}),
        None,
        &mut f.disk,
        &mut f.ids,
    );
    assert_eq!(applied.end.unwrap().unwrap()["applied"], true);
    let captured = f.editing.multitrack_query(f.member, "session");
    assert!(captured["path"].as_str().unwrap().ends_with("kept.json"));
    assert!(captured["session"]["sources"]["1"].is_object());
    let closed = f.editing.closing(f.member, "closeWindow");
    assert_eq!(closed["snapshot"], captured);
    let opened = f.editing.start(
        f.member,
        &json!({"verb":"open", "snapshot":captured}),
        None,
        &mut f.disk,
        &mut f.ids,
    );
    assert!(matches!(opened.end, Some(Ok(_))), "{opened:?}");
    let file = f.editing.multitrack_query(f.member, "file");
    assert_ne!(file["object"], original);
    assert_eq!(
        file["dirty"], true,
        "reopening does not confirm unsaved contents"
    );
    assert_eq!(f.editing.multitrack_query(f.member, "session"), captured);
}

impl Fixture {
    fn second(&mut self) -> MemberId {
        let opened: Value = serde_json::from_str(&call_json(
            &mut self.editing,
            &json!({"verb":"openMultitrack", "key":"other", "multitrack":{}}).to_string(),
        ))
        .unwrap();
        opened["member"].as_u64().unwrap() as u32
    }
    fn marker(&mut self, member: MemberId, marker: i64) {
        let edited = self.editing.start(
            member,
            &json!({"verb":"handle", "call":{"verb":"apply",
                "intent":{"intent":"setmarker", "marker":marker, "at":2.0, "name":"cue"}}}),
            None,
            &mut self.disk,
            &mut self.ids,
        );
        assert_eq!(edited.end.unwrap().unwrap()["applied"], true);
    }
    fn markers(&mut self, member: MemberId) -> Option<Result<Value, Raised>> {
        self.editing
            .start(
                member,
                &json!({"verb":"handle", "call":{"verb":"ids", "of":"markers"}}),
                None,
                &mut self.disk,
                &mut self.ids,
            )
            .end
    }
}

#[test]
fn two_sessions_in_one_application_keep_their_histories_apart() {
    let mut f = Fixture::new();
    let other = f.second();
    f.marker(f.member, 7);
    let undone = f.editing.start(
        other,
        &json!({"verb":"undo"}),
        None,
        &mut f.disk,
        &mut f.ids,
    );
    assert_eq!(undone.end, Some(Ok(json!(false))), "nothing to undo there");
    assert_eq!(f.markers(f.member), Some(Ok(json!({"ids":[7]}))));
    f.marker(other, 8);
    let undone = f.editing.start(
        f.member,
        &json!({"verb":"undo"}),
        None,
        &mut f.disk,
        &mut f.ids,
    );
    assert_eq!(undone.end, Some(Ok(json!(true))));
    assert_eq!(f.markers(f.member), Some(Ok(json!({"ids":[]}))));
    assert_eq!(f.markers(other), Some(Ok(json!({"ids":[8]}))));
}

#[test]
fn a_roll_joins_its_own_multitracks_order_whatever_its_key() {
    let mut f = Fixture::new();
    let other = f.second();
    let sequence = || -> crate::notes::Shared { Default::default() };
    let roll = |f: &mut Fixture, parent| {
        let opened: Value =
            serde_json::from_str(
                &f.editing
                    .open_notes_in(parent, "sequence:1", sequence(), "{}"),
            )
            .unwrap();
        opened["member"].as_u64().unwrap() as u32
    };
    let first = f.member;
    let first_roll = roll(&mut f, first);
    let other_roll = roll(&mut f, other);
    assert_ne!(
        f.editing.structure(first_roll),
        f.editing.structure(other_roll),
        "one key under two editors is two structures"
    );
    f.marker(first, 7);
    f.editing.select(Some(first_roll));
    assert!(
        f.editing.can_undo(),
        "the roll walks its multitrack's order"
    );
    f.editing.select(Some(other_roll));
    assert!(!f.editing.can_undo(), "and not the other multitrack's");
}

#[test]
fn a_session_open_in_one_window_brings_it_forward_instead_of_opening_twice() {
    let mut f = Fixture::new();
    let other = f.second();
    for (member, window) in [(f.member, 39), (other, 49)] {
        let Some(Member::Multitrack(editor)) = f.editing.member_mut(member) else {
            panic!()
        };
        editor.set_window(Some(window));
    }
    let measured = f.open("held", 1);
    f.land(measured.run, Ok(()));
    let path = f.folder.join("held.json");
    // written another way, it is the same file
    let written = f.folder.join(".").join("held.json");
    assert_eq!(
        f.editing.window_holding_file(written.to_str().unwrap()),
        Some(39)
    );
    let before = f.editing.multitrack_query(other, "file");
    let again = f.editing.start(
        other,
        &json!({"verb":"open", "session":written}),
        None,
        &mut f.disk,
        &mut f.ids,
    );
    assert!(
        again
            .effects
            .iter()
            .any(|effect| effect["kind"] == "front" && effect["window"] == 39),
        "{again:?}"
    );
    assert!(matches!(again.end, Some(Err(_))), "{:?}", again.end);
    assert_eq!(f.editing.multitrack_query(other, "file"), before);
    // the window that holds it opens it again from its file
    let reread = f.editing.start(
        f.member,
        &json!({"verb":"open", "session":path}),
        None,
        &mut f.disk,
        &mut f.ids,
    );
    assert!(
        !reread
            .effects
            .iter()
            .any(|effect| effect["kind"] == "front"),
        "{reread:?}"
    );
    // a window that went holds nothing
    f.editing.forget_multitrack(f.member, &mut f.ids);
    assert_eq!(f.editing.window_holding_file(path.to_str().unwrap()), None);
}

impl Fixture {
    /// The window as a standalone host holds it: numbered, with its close
    /// form and its dialogs, asking before it loses work when `asks`.
    fn windowed(&mut self, asks: bool) -> crate::forms::Ids {
        let ids: crate::forms::Ids = crate::multitrack::dialogs::names()
            .into_iter()
            .enumerate()
            .map(|(k, name)| (name, 60 + k as i32))
            .collect();
        let Some(Member::Multitrack(editor)) = self.editing.member_mut(self.member) else {
            panic!()
        };
        editor.set_close_form(Some(crate::closing::Ids {
            stack: 50,
            dialog: 51,
            save: 52,
            discard: 53,
            cancel: 54,
        }));
        editor.set_asks_to_close(asks);
        editor.set_saves(true);
        editor.set_dialogs(ids.clone());
        editor.window(40, 41);
        editor.set_window(Some(39));
        ids
    }
    fn gesture(&mut self, widget: i32, seq: i64, tag: &str, values: Value) -> Ran {
        let mut args = vec![
            json!(widget),
            json!(seq),
            json!(self.editing.version),
            json!(tag),
        ];
        args.extend(values.as_array().cloned().unwrap_or_default());
        self.editing.start(
            self.member,
            &json!({"verb":"event", "addr":"/gui_event", "args":args}),
            None,
            &mut self.disk,
            &mut self.ids,
        )
    }
    fn path(&mut self) -> Value {
        self.editing.multitrack_query(self.member, "file")["path"].clone()
    }
}

fn shows_close_form(ran: &Ran) -> bool {
    ran.effects.iter().any(|effect| {
        effect["kind"] == "echo"
            && effect["answer"].to_string().contains(r#""widget":50"#)
            && effect["answer"].to_string().contains(r#""index":1"#)
    })
}

#[test]
fn file_new_with_unsaved_work_asks_and_dont_save_replaces_the_session() {
    let mut f = Fixture::new();
    let measured = f.open("asked", 1);
    f.land(measured.run, Ok(()));
    f.windowed(true);
    f.marker(f.member, 7);
    let asked = f.gesture(39, 1, "new_file", json!([]));
    assert!(shows_close_form(&asked), "{asked:?}");
    assert!(f.path().as_str().unwrap().ends_with("asked.json"));
    // Cancel keeps the session as it is
    f.gesture(54, 2, "click", json!([]));
    assert!(f.path().as_str().unwrap().ends_with("asked.json"));
    assert_eq!(f.markers(f.member), Some(Ok(json!({"ids":[7]}))));
    // asked again, Don't save lets it go
    assert!(shows_close_form(&f.gesture(39, 3, "new_file", json!([]))));
    let replaced = f.gesture(53, 4, "click", json!([]));
    assert!(
        replaced
            .effects
            .iter()
            .any(|effect| effect["kind"] == "released"),
        "{replaced:?}"
    );
    assert_eq!(f.path(), Value::Null);
    assert_eq!(f.markers(f.member), Some(Ok(json!({"ids":[]}))));
    assert!(
        !f.editing.can_undo(),
        "the new session has a history of its own"
    );
}

#[test]
fn file_close_after_save_waits_for_the_write_and_an_edit_meanwhile_keeps_it() {
    let mut f = Fixture::new();
    let measured = f.open("saving", 1);
    f.land(measured.run, Ok(()));
    f.windowed(true);
    f.marker(f.member, 7);
    assert!(shows_close_form(&f.gesture(39, 1, "close_file", json!([]))));
    let saving = f.gesture(52, 2, "click", json!([]));
    let render = saving.render.expect("the save waits on storage");
    assert!(f.path().as_str().unwrap().ends_with("saving.json"));
    // an edit while the save is written is work it does not hold
    f.marker(f.member, 8);
    let [Step::Store { path, contents }] = &render.steps[..] else {
        panic!()
    };
    std::fs::write(path, contents).unwrap();
    let landed = f.land(render.run, Ok(()));
    assert!(
        landed
            .effects
            .iter()
            .any(|effect| effect["kind"] == "say" && effect["reason"].is_string()),
        "{landed:?}"
    );
    assert!(f.path().as_str().unwrap().ends_with("saving.json"));
    // saved again with nothing moving, the close follows the write
    assert!(shows_close_form(&f.gesture(39, 3, "close_file", json!([]))));
    let render = f.gesture(52, 4, "click", json!([])).render.unwrap();
    let [Step::Store { path, contents }] = &render.steps[..] else {
        panic!()
    };
    std::fs::write(path, contents).unwrap();
    let closed = f.land(render.run, Ok(()));
    assert!(matches!(closed.end, Some(Ok(_))), "{closed:?}");
    assert_eq!(
        f.editing.multitrack_query(f.member, "file"),
        json!({"empty": true})
    );
}

#[test]
fn a_window_a_client_holds_changes_its_session_without_asking() {
    let mut f = Fixture::new();
    let measured = f.open("held", 1);
    f.land(measured.run, Ok(()));
    f.windowed(false);
    f.marker(f.member, 7);
    let closed = f.gesture(39, 1, "close_file", json!([]));
    assert!(!shows_close_form(&closed));
    assert!(
        closed
            .effects
            .iter()
            .any(|effect| effect["kind"] == "released"),
        "{closed:?}"
    );
    assert_eq!(
        f.editing.multitrack_query(f.member, "file"),
        json!({"empty": true})
    );
}

#[test]
fn file_open_chooses_a_session_in_the_form_and_opens_it_in_the_window() {
    let mut f = Fixture::new();
    let mut other = Session::new(Document::empty());
    other
        .sources
        .insert(SourceId(1), Source::file("chosen-1.wav", Lifetime::Session));
    let path = f.folder.join("chosen.json");
    std::fs::write(&path, serde_json::to_vec(&other).unwrap()).unwrap();
    let ids = f.windowed(true);
    let shown = f.gesture(39, 1, "open_file", json!([]));
    assert!(
        shown.effects.iter().any(|effect| effect["kind"] == "echo"
            && effect["answer"]
                .to_string()
                .contains(&format!(r#""widget":{}"#, ids["stack"]))),
        "{shown:?}"
    );
    // nothing chosen: the form stays up and says so
    let refused = f.gesture(ids["open:ok"], 2, "click", json!([]));
    assert!(
        refused
            .effects
            .iter()
            .any(|effect| effect["answer"].to_string().contains("select a session")),
        "{refused:?}"
    );
    let picked = f.gesture(ids["open:path"], 3, "pick", json!([[path]]));
    assert!(
        picked
            .effects
            .iter()
            .any(|effect| effect["kind"] == "publish"),
        "{picked:?}"
    );
    assert_eq!(f.path(), json!(path));
}

#[test]
fn a_session_opened_read_only_is_so_alone_and_the_one_in_its_place_saves() {
    let mut f = Fixture::new();
    let mut session = Session::new(Document::empty());
    session
        .sources
        .insert(SourceId(1), Source::file("kept-1.wav", Lifetime::Session));
    let path = f.folder.join("kept.json");
    std::fs::write(&path, serde_json::to_vec(&session).unwrap()).unwrap();
    f.windowed(false);
    let opened = f.editing.start(
        f.member,
        &json!({"verb":"open", "session":path, "saveTo":null}),
        None,
        &mut f.disk,
        &mut f.ids,
    );
    assert!(matches!(opened.end, Some(Ok(_))), "{opened:?}");
    let save = |f: &mut Fixture| f.editing.action_states(f.member)["save"].clone();
    assert!(save(&mut f)["reason"].is_string(), "{}", save(&mut f));
    let saving = f.gesture(39, 1, "save", json!([]));
    assert!(saving.render.is_none(), "nothing written over the file");
    f.gesture(39, 2, "new_file", json!([]));
    assert!(save(&mut f)["reason"].is_null(), "{}", save(&mut f));
    // the new session has no file: its Save asks where, in Save as
    let asked = f.gesture(39, 3, "save", json!([]));
    assert!(asked.render.is_none(), "{asked:?}");
}

fn a_roll(f: &mut Fixture, parent: MemberId, key: &str) -> MemberId {
    let sequence: crate::notes::Shared = Default::default();
    let opened: Value =
        serde_json::from_str(
            &f.editing
                .open_notes_in(parent, key, sequence, r#"{"rate": 100.0}"#),
        )
        .unwrap();
    let member = opened["member"].as_u64().unwrap() as MemberId;
    for call in [
        json!({"verb": "window", "widget": 80}),
        json!({"verb": "sync", "window": 79}),
    ] {
        call_json(
            &mut f.editing,
            &json!({"verb": "member", "member": member, "call": call}).to_string(),
        );
    }
    member
}

#[test]
fn a_session_let_go_takes_the_order_its_rolls_are_in() {
    let mut f = Fixture::new();
    let first = f.member;
    let roll = a_roll(&mut f, first, "sequence:5");
    f.marker(first, 7);
    f.editing.select(Some(roll));
    assert!(
        f.editing.can_undo(),
        "the roll walks its multitrack's order"
    );
    let replaced = f.editing.start(
        first,
        &json!({"verb":"new_file"}),
        None,
        &mut f.disk,
        &mut f.ids,
    );
    assert!(matches!(replaced.end, Some(Ok(_))), "{replaced:?}");
    f.editing.select(Some(roll));
    assert!(!f.editing.can_undo(), "nothing walks a session let go of");
    f.editing.select(Some(first));
    assert!(
        !f.editing.can_undo(),
        "and the new session has its own order"
    );
}

#[test]
fn a_rolls_file_close_closes_it_and_lets_its_transport_go() {
    let mut f = Fixture::new();
    let first = f.member;
    let roll = a_roll(&mut f, first, "sequence:5");
    let opened = f
        .editing
        .start(roll, &json!({"verb":"open"}), None, &mut f.disk, &mut f.ids);
    assert!(matches!(opened.end, Some(Ok(_))), "{opened:?}");
    assert!(f.editing.rolls.contains_key(&roll));
    let closed = f.editing.start(
        roll,
        &json!({"verb":"event", "addr":"/gui_event", "args":[79, 1, f.editing.version, "menu", "close"]}),
        None,
        &mut f.disk,
        &mut f.ids,
    );
    assert!(
        closed
            .effects
            .iter()
            .any(|effect| effect["kind"] == "closeWindow"),
        "{closed:?}"
    );
    assert!(!f.editing.rolls.contains_key(&roll));
}

/// **Two views of one sequence are one history and two conversations**: an
/// edit in one is corrected in the other and undone from either, a gesture
/// the other made against the picture before it is refused as overtaken
/// there alone, and the grid each snaps to stays its own.
#[test]
fn two_rolls_share_edits_and_history_and_keep_their_conversations() {
    use clausters_document::EventSequence;
    use clausters_document::events::Event as SeqEvent;
    use std::sync::{Arc, Mutex};

    let sequence = Arc::new(Mutex::new(EventSequence::new(vec![SeqEvent::new(
        0.0,
        json!({"midinote": 60, "sustain": 1.0, "amp": 0.1}),
    )])));
    let mut editing = Editing::new();
    let mut open = |widget: i64, window: i64| {
        let opened: Value = serde_json::from_str(&editing.open_notes(
            "seq",
            sequence.clone(),
            r#"{"rate": 100.0}"#,
        ))
        .unwrap();
        let member = opened["member"].as_u64().unwrap() as MemberId;
        for call in [
            json!({"verb": "window", "widget": widget}),
            json!({"verb": "sync", "window": window}),
        ] {
            call_json(
                &mut editing,
                &json!({"verb": "member", "member": member, "call": call}).to_string(),
            );
        }
        member
    };
    let (left, right) = (open(40, 39), open(50, 49));
    assert_eq!(editing.structure(left), editing.structure(right));
    let gesture = |widget: i64, against: i64, tag: &str, values: Vec<Value>| {
        let mut args = vec![json!(widget), json!(1), json!(against), json!(tag)];
        args.extend(values);
        crate::turn::Event {
            addr: "/gui_event".into(),
            args,
        }
    };
    let moved = |pitch: f64| {
        vec![
            json!(1),
            json!(50.0),
            json!(100.0),
            json!(pitch),
            json!(13.0),
            json!(0.0),
        ]
    };
    let against = editing.version();
    let turned = editing
        .event(left, &gesture(40, against, "notes", moved(67.0)))
        .unwrap();
    assert!(turned.outcome.changed());
    assert!(
        turned.corrections.iter().any(|c| c.member == right),
        "the other window is corrected"
    );
    let stale = editing
        .event(right, &gesture(50, against, "notes", moved(72.0)))
        .unwrap();
    assert!(
        !stale.outcome.changed(),
        "overtaken in the window that drew the old picture"
    );
    assert_eq!(
        sequence.lock().unwrap().get(1).unwrap().data.0["midinote"],
        json!(67.0)
    );
    let now = editing.version();
    let left_turn = editing
        .event(left, &gesture(40, now, "notes", moved(69.0)))
        .unwrap();
    assert!(left_turn.outcome.changed(), "and only there");
    let at = editing.version();
    editing
        .event(left, &gesture(39, at, "snap", vec![]))
        .unwrap();
    let (snap_left, snap_right) = match (editing.member(left), editing.member(right)) {
        (Some(Member::Notes(a)), Some(Member::Notes(b))) => (a.snap(), b.snap()),
        _ => panic!(),
    };
    assert_ne!(snap_left, snap_right, "the view's own");
    editing.select(Some(right));
    assert!(
        editing.step(Direction::Undo).stepped,
        "undone from the other"
    );
    assert_eq!(
        sequence.lock().unwrap().get(1).unwrap().data.0["midinote"],
        json!(67.0)
    );
}

/// **A roll closed and opened again is the same member over the same
/// object**, and its entries are still walked.
#[test]
fn a_roll_closed_and_reopened_keeps_its_object_and_its_entries() {
    let mut f = Fixture::new();
    let first = f.member;
    let sequence: crate::notes::Shared = Default::default();
    let open = |f: &mut Fixture, sequence: &crate::notes::Shared| -> MemberId {
        let opened: Value = serde_json::from_str(&f.editing.open_notes_in(
            first,
            "sequence:5",
            sequence.clone(),
            r#"{"rate": 100.0}"#,
        ))
        .unwrap();
        opened["member"].as_u64().unwrap() as MemberId
    };
    let roll = open(&mut f, &sequence);
    f.marker(first, 7);
    f.editing.forget_roll(roll, &mut f.ids);
    let again = open(&mut f, &sequence);
    assert_eq!(again, roll);
    let Some(Member::Notes(editor)) = f.editing.member(again) else {
        panic!()
    };
    assert!(std::sync::Arc::ptr_eq(editor.sequence(), &sequence));
    f.editing.select(Some(again));
    assert!(f.editing.can_undo());
}

/// **A score is an application's member**: opened in the context that holds
/// the engine it takes a transport from, a handler's verb and a gesture are
/// its turns, its file goes through the application's storage, a read that
/// fails leaves it, and File > Close closes it and lets the transport go.
#[cfg(feature = "notation")]
#[test]
fn a_score_runs_through_the_applications_door() {
    use clausters_core::notation::{AnyEngraver, Score, sheet_to_mei, voice_to_sheet};
    use std::sync::{Arc, Mutex};

    let mut f = Fixture::new();
    let mei = sheet_to_mei(&voice_to_sheet(&[], "4/4", "G2", "C")).unwrap();
    let engraver = AnyEngraver::new(crate::editing::tests::Kept(Mutex::new(String::new())));
    let score = Arc::new(Mutex::new(Score::open(engraver, &mei).unwrap()));
    let opened: Value =
        serde_json::from_str(&f.editing.open_score("score:1", score.clone(), "{}")).unwrap();
    let member = opened["member"].as_u64().unwrap() as MemberId;
    for call in [
        json!({"verb": "window", "widget": 80, "scroll": 81, "status": 82}),
        json!({"verb": "sync", "window": 79}),
    ] {
        call_json(
            &mut f.editing,
            &json!({"verb": "member", "member": member, "call": call}).to_string(),
        );
    }
    let start = |f: &mut Fixture, action: Value| {
        f.editing
            .start(member, &action, None, &mut f.disk, &mut f.ids)
    };
    let transport = start(&mut f, json!({"verb": "open"}));
    assert!(
        transport
            .effects
            .iter()
            .any(|effect| effect["kind"] == "transport"),
        "{transport:?}"
    );
    assert!(f.editing.rolls.contains_key(&member));
    assert_eq!(
        f.editing.query(member, "transport")["rolling"],
        false,
        "it answers as a roll does"
    );
    // nothing selected: refused, with why, and still answered
    let refused = start(
        &mut f,
        json!({"verb": "act", "call": {"action": "articulation", "name": "stacc"}}),
    );
    let end = refused.end.unwrap().unwrap();
    assert_eq!(end["changed"], false);
    assert!(end["refused"].is_string(), "{end}");
    // the file: no path yet, then written and its own
    let no_file = start(&mut f, json!({"verb": "save"}));
    assert!(matches!(no_file.end, Some(Err(_))), "{no_file:?}");
    let path = f.folder.join("score.mei");
    let saved = start(&mut f, json!({"verb": "save", "path": path}));
    assert!(
        saved.end.is_none(),
        "a save is not confirmed before storage"
    );
    let render = saved.render.expect("the save waits on storage");
    let [Step::Store { path: to, contents }] = &render.steps[..] else {
        panic!("a storage step")
    };
    std::fs::write(to, contents).unwrap();
    let landed = land(&mut f, member, render.run, Ok(()));
    assert_eq!(
        landed.end.unwrap().unwrap()["destination"],
        json!(path),
        "answered where"
    );
    let mei = score.lock().unwrap().mei();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), mei);
    let again = start(&mut f, json!({"verb": "save"}));
    assert!(again.render.is_some(), "{again:?}");
    let render = again.render.unwrap();
    land(&mut f, member, render.run, Ok(()));
    // a read that fails leaves the score
    let missing = f.folder.join("missing.mei");
    let failed = start(&mut f, json!({"verb": "open_file", "path": missing}));
    assert!(matches!(failed.end, Some(Err(_))), "{failed:?}");
    assert_eq!(score.lock().unwrap().mei(), mei);
    // File > Close, nothing to lose
    let at = f.editing.version;
    let closed = start(
        &mut f,
        json!({"verb": "event", "addr": "/gui_event", "args": [79, 1, at, "menu", "close"]}),
    );
    assert!(
        closed
            .effects
            .iter()
            .any(|effect| effect["kind"] == "closeWindow"),
        "{closed:?}"
    );
    assert!(!f.editing.rolls.contains_key(&member));
}

/// A store of a score member landed.
#[cfg(feature = "notation")]
fn land(f: &mut Fixture, member: MemberId, run: u64, result: Result<(), String>) -> Ran {
    f.editing
        .land(member, run, result, None, &mut f.disk, &mut f.ids)
}

/// A score member of `f`'s, its window numbered, holding `f`'s one blank.
#[cfg(feature = "notation")]
fn score_member(f: &mut Fixture, request: Value) -> (MemberId, crate::score::Shared) {
    use clausters_core::notation::{AnyEngraver, Score, sheet_to_mei, voice_to_sheet};
    use std::sync::{Arc, Mutex};
    let mei = sheet_to_mei(&voice_to_sheet(&[], "4/4", "G2", "C")).unwrap();
    let engraver = AnyEngraver::new(crate::editing::tests::Kept(Mutex::new(String::new())));
    let score = Arc::new(Mutex::new(Score::open(engraver, &mei).unwrap()));
    let opened: Value = serde_json::from_str(&f.editing.open_score_at(
        crate::editing::Place::Apart,
        "score:file",
        score.clone(),
        &request.to_string(),
    ))
    .unwrap();
    let member = opened["member"].as_u64().unwrap() as MemberId;
    let ids: Vec<(String, i64)> = crate::score::dialogs::names()
        .into_iter()
        .zip(200..)
        .collect();
    let numbered: serde_json::Map<String, Value> =
        ids.iter().map(|(n, i)| (n.clone(), json!(i))).collect();
    for call in [
        json!({"verb": "window", "widget": 80, "scroll": 81, "status": 82,
            "dialogs": numbered}),
        json!({"verb": "sync", "window": 79, "asks": true}),
    ] {
        call_json(
            &mut f.editing,
            &json!({"verb": "member", "member": member, "call": call}).to_string(),
        );
    }
    f.editing.start(
        member,
        &json!({"verb": "open"}),
        None,
        &mut f.disk,
        &mut f.ids,
    );
    (member, score)
}

/// The file a score member's window has: its path and whether it is dirty.
#[cfg(feature = "notation")]
fn score_file(f: &mut Fixture, member: MemberId) -> Value {
    f.editing.query(member, "file")
}

/// **A save that fails changes nothing**, and what waited for it stays: the
/// close form's Save with a read-only target leaves the window, its document
/// and the file it had; a save that lands confirms the revision it prepared,
/// so an edit made meanwhile keeps the window open and the score unsaved.
#[cfg(feature = "notation")]
#[test]
fn a_score_s_save_is_confirmed_before_what_waits_for_it() {
    let mut f = Fixture::new();
    let (member, score) = score_member(&mut f, json!({}));
    let path = f.folder.join("kept.mei");
    let start = |f: &mut Fixture, action: Value| {
        f.editing
            .start(member, &action, None, &mut f.disk, &mut f.ids)
    };
    let edit = json!({"verb": "act", "call": {"action": "page", "landscape": true}});
    let saving = start(&mut f, json!({"verb": "save", "path": path}));
    let render = saving.render.unwrap();
    assert_eq!(score_file(&mut f, member)["path"], Value::Null, "not yet");
    let failed = land(&mut f, member, render.run, Err("the disk is full".into()));
    assert!(matches!(failed.end, Some(Err(_))), "{failed:?}");
    assert_eq!(score_file(&mut f, member)["path"], Value::Null);
    // a landing again is nobody's
    assert!(land(&mut f, member, render.run, Ok(())).end.is_none());
    // the prepared revision is what is written, whatever is typed meanwhile
    let saving = start(&mut f, json!({"verb": "save", "path": path}));
    let render = saving.render.unwrap();
    let prepared = score.lock().unwrap().mei();
    start(&mut f, edit.clone());
    let [Step::Store { path: to, contents }] = &render.steps[..] else {
        panic!("a storage step")
    };
    assert_eq!(contents, &prepared);
    std::fs::write(to, contents).unwrap();
    let landed = land(&mut f, member, render.run, Ok(()));
    assert!(matches!(landed.end, Some(Ok(_))), "{landed:?}");
    let file = score_file(&mut f, member);
    assert_eq!(file["path"], json!(path));
    assert_eq!(
        file["dirty"], true,
        "the edit is work the save does not hold"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), prepared);
    // another document meanwhile: the file is not taken for it
    let saving = start(&mut f, json!({"verb": "save"}));
    start(&mut f, json!({"verb": "new_file"}));
    let landed = land(&mut f, member, saving.render.unwrap().run, Ok(()));
    assert!(matches!(landed.end, Some(Err(_))), "{landed:?}");
    assert_eq!(score_file(&mut f, member)["path"], Value::Null);
}

/// **Close file unbinds**: the window stays with no document, its order
/// gone and its verbs refused, what it left handed over; New gives it another
/// with a new order, and Open a file another window holds is refused.
#[cfg(feature = "notation")]
#[test]
fn close_file_lets_the_document_go_and_the_window_stays() {
    let mut f = Fixture::new();
    let (member, _) = score_member(&mut f, json!({}));
    let start = |f: &mut Fixture, action: Value| {
        f.editing
            .start(member, &action, None, &mut f.disk, &mut f.ids)
    };
    let edit = json!({"verb": "act", "call": {"action": "page", "landscape": true}});
    start(&mut f, edit.clone());
    f.editing.select(Some(member));
    assert!(f.editing.can_undo());
    let closed = start(&mut f, json!({"verb": "close_file"}));
    assert!(matches!(closed.end, Some(Ok(_))), "{closed:?}");
    let released = closed
        .effects
        .iter()
        .find(|e| e["kind"] == "released")
        .expect("handed over");
    assert!(released["snapshot"]["mei"].is_string());
    assert!(closed.effects.iter().any(|e| e["kind"] == "publish"));
    assert_eq!(score_file(&mut f, member)["empty"], true);
    f.editing.select(Some(member));
    assert!(!f.editing.can_undo(), "the order went with it");
    assert_eq!(f.editing.query(member, "history")["canUndo"], false);
    // a verb with no document is refused, with why
    let refused = start(&mut f, edit.clone());
    let Some(Err(raised)) = refused.end else {
        panic!("{refused:?}")
    };
    assert_eq!(raised.message, "no file is open");
    let refused = start(&mut f, json!({"verb": "save", "path": "/tmp/nothing.mei"}));
    assert!(refused.render.is_none(), "{refused:?}");
    // closing the file again is nothing
    assert!(matches!(
        start(&mut f, json!({"verb": "close_file"})).end,
        Some(Ok(_))
    ));
    // a read that fails leaves it with none
    let missing = f.folder.join("missing.mei");
    let failed = start(&mut f, json!({"verb": "open_file", "path": missing}));
    assert!(matches!(failed.end, Some(Err(_))), "{failed:?}");
    assert_eq!(score_file(&mut f, member)["empty"], true);
    // New binds a new document, which has an order of its own
    let made = start(&mut f, json!({"verb": "new_file"}));
    assert!(
        made.effects.iter().any(|e| e["kind"] == "publish"),
        "{made:?}"
    );
    assert_eq!(score_file(&mut f, member)["empty"], false);
    f.editing.select(Some(member));
    assert!(!f.editing.can_undo(), "the first document is no entry");
    start(&mut f, edit);
    f.editing.select(Some(member));
    assert!(f.editing.can_undo());
    // a file is read back as what was saved
    let path = f.folder.join("back.mei");
    let saved = start(&mut f, json!({"verb": "save", "path": path}));
    let render = saved.render.unwrap();
    let [Step::Store { path: to, contents }] = &render.steps[..] else {
        panic!("a storage step")
    };
    std::fs::write(to, contents).unwrap();
    land(&mut f, member, render.run, Ok(()));
    let written = std::fs::read_to_string(&path).unwrap();
    start(&mut f, json!({"verb": "close_file"}));
    let opened = start(&mut f, json!({"verb": "open_file", "path": path}));
    assert!(matches!(opened.end, Some(Ok(_))), "{opened:?}");
    let file = score_file(&mut f, member);
    assert_eq!(
        (file["path"].clone(), file["dirty"].clone()),
        (json!(path), json!(false))
    );
    let mei = match f.editing.member(member) {
        Some(Member::Score(editor)) => editor.score().lock().unwrap().mei(),
        _ => panic!(),
    };
    assert_eq!(mei, written);
    // **which document the window holds is no entry**: Open and New over one
    // walk nothing back, what they replaced handed over and its order gone
    start(&mut f, turned(false));
    f.editing.select(Some(member));
    assert!(f.editing.can_undo());
    let made = start(&mut f, json!({"verb": "new_file"}));
    assert!(matches!(made.end, Some(Ok(_))), "{made:?}");
    let left = made
        .effects
        .iter()
        .find(|e| e["kind"] == "released")
        .expect("what it replaced is handed over");
    assert_eq!(left["snapshot"]["path"], json!(path));
    f.editing.select(Some(member));
    assert!(!f.editing.can_undo(), "no step back across the change");
    assert_eq!(score_file(&mut f, member)["path"], Value::Null);
    // a read that fails leaves the document and its order
    start(&mut f, turned(true));
    let held = match f.editing.member(member) {
        Some(Member::Score(editor)) => editor.score().lock().unwrap().mei(),
        _ => panic!(),
    };
    let bad = f.folder.join("missing-too.mei");
    let failed = start(&mut f, json!({"verb": "open_file", "path": bad}));
    assert!(matches!(failed.end, Some(Err(_))), "{failed:?}");
    match f.editing.member(member) {
        Some(Member::Score(editor)) => assert_eq!(editor.score().lock().unwrap().mei(), held),
        _ => panic!(),
    }
    f.editing.select(Some(member));
    assert!(f.editing.can_undo());
}

/// The page turned, an edit of the document that has a history.
#[cfg(feature = "notation")]
fn turned(landscape: bool) -> Value {
    json!({"verb": "act", "call": {"action": "page", "landscape": landscape}})
}

/// **A window opened with nothing has no document**, and is not a new score.
#[cfg(feature = "notation")]
#[test]
fn a_score_window_opened_over_nothing_has_no_document() {
    let mut f = Fixture::new();
    let (member, _) = score_member(&mut f, json!({"empty": true}));
    assert_eq!(score_file(&mut f, member)["empty"], true);
    let at = f.editing.version;
    let start = |f: &mut Fixture, action: Value| {
        f.editing
            .start(member, &action, None, &mut f.disk, &mut f.ids)
    };
    let refused = start(
        &mut f,
        json!({"verb": "event", "addr": "/gui_event", "args": [79, 1, at, "menu", "save"]}),
    );
    assert!(matches!(refused.end, Some(Err(_))), "{refused:?}");
    let made = start(
        &mut f,
        json!({"verb": "event", "addr": "/gui_event", "args": [79, 1, at, "menu", "new_file"]}),
    );
    assert!(
        made.effects.iter().any(|e| e["kind"] == "publish"),
        "{made:?}"
    );
    assert_eq!(score_file(&mut f, member)["empty"], false);
    assert_eq!(score_file(&mut f, member)["dirty"], false);
}

/// A report from a widget of a score window's dialogs, at the version it draws.
#[cfg(feature = "notation")]
fn score_said(f: &mut Fixture, member: MemberId, name: &str, seq: i64) -> Ran {
    let widget = 200
        + crate::score::dialogs::names()
            .iter()
            .position(|n| n == name)
            .unwrap() as i32;
    let at = f.editing.version;
    f.editing.start(
        member,
        &json!({"verb": "event", "addr": "/gui_event",
            "args": [widget, seq, at, "click"]}),
        None,
        &mut f.disk,
        &mut f.ids,
    )
}

/// A pick of a score window's File menu, at the version it draws.
#[cfg(feature = "notation")]
fn score_menu(f: &mut Fixture, member: MemberId, verb: &str, seq: i64) -> Ran {
    let at = f.editing.version;
    f.editing.start(
        member,
        &json!({"verb": "event", "addr": "/gui_event",
            "args": [79, seq, at, "menu", verb]}),
        None,
        &mut f.disk,
        &mut f.ids,
    )
}

#[cfg(feature = "notation")]
fn closes(ran: &Ran) -> bool {
    ran.effects
        .iter()
        .any(|effect| effect["kind"] == "closeWindow")
}

/// **The close form's Save waits for the write**, as the multitrack's does: a
/// failure keeps the window, its document and what it had to lose; an edit made
/// while it is written keeps the window too; a write with nothing moving closes.
#[cfg(feature = "notation")]
#[test]
fn a_score_s_close_form_waits_for_its_write() {
    let mut f = Fixture::new();
    let (member, _) = score_member(&mut f, json!({}));
    let path = f.folder.join("asked.mei");
    let render = f
        .editing
        .start(
            member,
            &json!({"verb": "save", "path": path}),
            None,
            &mut f.disk,
            &mut f.ids,
        )
        .render
        .unwrap();
    land(&mut f, member, render.run, Ok(()));
    let edit = turned(true);
    f.editing
        .start(member, &edit, None, &mut f.disk, &mut f.ids);
    // the form is up, and Cancel keeps everything
    let asked = score_menu(&mut f, member, "close", 1);
    assert!(!closes(&asked), "{asked:?}");
    assert!(!closes(&score_said(&mut f, member, "close:cancel", 2)));
    assert_eq!(score_file(&mut f, member)["dirty"], true);
    // Save, and the write fails
    score_menu(&mut f, member, "close", 3);
    let saving = score_said(&mut f, member, "close:ok", 4);
    assert!(!closes(&saving), "{saving:?}");
    let failed = land(
        &mut f,
        member,
        saving.render.expect("the save waits on storage").run,
        Err("the disk is full".into()),
    );
    assert!(!closes(&failed), "{failed:?}");
    assert!(matches!(failed.end, Some(Err(_))), "{failed:?}");
    assert_eq!(score_file(&mut f, member)["dirty"], true);
    assert!(f.editing.rolls.contains_key(&member));
    // again, and work the save does not hold arrives meanwhile
    score_menu(&mut f, member, "close", 5);
    let saving = score_said(&mut f, member, "close:ok", 6);
    let render = saving.render.expect("the save waits on storage");
    f.editing
        .start(member, &turned(false), None, &mut f.disk, &mut f.ids);
    let landed = land(&mut f, member, render.run, Ok(()));
    assert!(!closes(&landed), "{landed:?}");
    assert!(
        landed.effects.iter().any(|e| e["kind"] == "say"),
        "{landed:?}"
    );
    assert_eq!(score_file(&mut f, member)["dirty"], true);
    // with nothing moving, the close follows the write
    score_menu(&mut f, member, "close", 7);
    let saving = score_said(&mut f, member, "close:ok", 8);
    assert!(!closes(&saving), "not before the write");
    let render = saving.render.expect("the save waits on storage");
    let landed = land(&mut f, member, render.run, Ok(()));
    assert!(closes(&landed), "{landed:?}");
    assert!(!f.editing.rolls.contains_key(&member));
    // the landing again is nobody's
    let again = land(&mut f, member, render.run, Ok(()));
    assert!(again.effects.is_empty() && again.end.is_none(), "{again:?}");
}

/// **A store that takes its time**: a second save is refused while it is out,
/// the order can be walked meanwhile, and what is confirmed is what was written.
/// A walk back to what the file holds is nothing left to lose.
#[cfg(feature = "notation")]
#[test]
fn a_score_s_slow_store_keeps_its_revision_and_its_order() {
    let mut f = Fixture::new();
    let (member, score) = score_member(&mut f, json!({}));
    let path = f.folder.join("slow.mei");
    let start = |f: &mut Fixture, action: Value| {
        f.editing
            .start(member, &action, None, &mut f.disk, &mut f.ids)
    };
    let saving = start(&mut f, json!({"verb": "save", "path": path}));
    let run = saving.render.unwrap().run;
    let written = score.lock().unwrap().mei();
    let refused = start(&mut f, json!({"verb": "save", "path": path}));
    assert!(refused.render.is_none(), "{refused:?}");
    assert!(matches!(refused.end, Some(Err(_))), "{refused:?}");
    start(&mut f, turned(true));
    assert_ne!(score.lock().unwrap().mei(), written);
    let landed = land(&mut f, member, run, Ok(()));
    assert!(matches!(landed.end, Some(Ok(_))), "{landed:?}");
    assert_eq!(score_file(&mut f, member)["dirty"], true);
    let walked = start(&mut f, json!({"verb": "undo"}));
    assert!(matches!(walked.end, Some(Ok(_))), "{walked:?}");
    assert_eq!(score.lock().unwrap().mei(), written);
    assert_eq!(score_file(&mut f, member)["dirty"], false);
    let walked = start(&mut f, json!({"verb": "redo"}));
    assert!(matches!(walked.end, Some(Ok(_))), "{walked:?}");
    assert_eq!(score_file(&mut f, member)["dirty"], true);
    // and the file is free to be written again
    let again = start(&mut f, json!({"verb": "save"}));
    assert!(again.render.is_some(), "{again:?}");
}

/// **The window closed while a save is out**: the landing saves the file the
/// holder has and draws nothing of what closed; the operation is settled and
/// holds nothing. A server lost before the reply retires it the same way.
#[cfg(feature = "notation")]
#[test]
fn a_score_s_store_landing_after_its_window_or_its_server_went() {
    let mut f = Fixture::new();
    let (member, _) = score_member(&mut f, json!({}));
    let path = f.folder.join("late.mei");
    let saving = f.editing.start(
        member,
        &json!({"verb": "save", "path": path}),
        None,
        &mut f.disk,
        &mut f.ids,
    );
    let run = saving.render.unwrap().run;
    let target = f.editing.score_waits[&run].ticket.target;
    // the window's close, as the host's goes, over work the save does not hold
    f.editing
        .start(member, &turned(true), None, &mut f.disk, &mut f.ids);
    let mark = score_menu(&mut f, member, "close", 1);
    assert!(!closes(&mark), "work unsaved: the form asks: {mark:?}");
    assert!(closes(&score_said(&mut f, member, "close:discard", 2)));
    f.editing.forget_roll(member, &mut f.ids);
    f.editing.close_order_of(member);
    let landed = land(&mut f, member, run, Ok(()));
    assert!(matches!(landed.end, Some(Ok(_))), "{landed:?}");
    assert_eq!(f.editing.window_holding_file(path.to_str().unwrap()), None);
    assert!(
        !landed
            .effects
            .iter()
            .any(|e| matches!(e["kind"].as_str(), Some("publish" | "closeWindow"))),
        "{landed:?}"
    );
    assert!(f.editing.score_waits.is_empty());
    assert!(!f.editing.score_runtime.operations.busy(target));

    // a server lost
    let mut f = Fixture::new();
    let (member, _) = score_member(&mut f, json!({}));
    let saving = f.editing.start(
        member,
        &json!({"verb": "save", "path": path}),
        None,
        &mut f.disk,
        &mut f.ids,
    );
    let run = saving.render.unwrap().run;
    let target = f.editing.score_waits[&run].ticket.target;
    f.editing.invalidate_server(target.server);
    let landed = land(&mut f, member, run, Ok(()));
    assert!(landed.effects.is_empty() && landed.end.is_none());
    assert_eq!(score_file(&mut f, member)["path"], Value::Null);
    assert!(!f.editing.score_runtime.operations.busy(target));
}
