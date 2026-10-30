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
    fail_repair: bool,
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
    fn carry(&mut self, steps: &[Step]) -> Result<(), String> {
        assert!(steps.is_empty(), "a cache refusal has no file to restore");
        self.repairs += 1;
        if self.fail_repair {
            Err("storage unavailable".into())
        } else {
            Ok(())
        }
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
    let tracks = &f.editing.tracks[&f.member];
    let Some(Member::Multitrack(editor)) = f.editing.member(f.member) else {
        panic!()
    };
    assert_ne!(
        editor.session_snapshot(tracks.session.as_ref().map(|(s, _)| s)),
        *tracks.saved.as_ref().unwrap()
    );
    let read_back: Session = serde_json::from_slice(&std::fs::read(&destination).unwrap()).unwrap();
    assert_eq!(read_back, captured);
    let checkpoint = tracks.saved.clone();
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
    assert_eq!(
        f.editing.tracks[&f.member].session.as_ref().unwrap().1,
        destination.to_string_lossy()
    );
    assert_eq!(f.editing.tracks[&f.member].saved, checkpoint);
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
    assert_eq!(
        f.editing.tracks[&f.member].session.as_ref().unwrap().1,
        retry_path.to_string_lossy()
    );
    assert_eq!(
        f.editing.tracks[&f.member]
            .saved
            .as_ref()
            .unwrap()
            .multitrack
            .channels,
        6
    );
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
fn partial_failure_retains_the_original_lock_through_failed_repair_and_retry() {
    let mut f = Fixture::new();
    let first = f.open("partial", 2);
    let work = f.editing.multitrack_waits[&first.run].clone();
    let second = f.land(first.run, Ok(())).render.unwrap();
    f.disk.fail_repair = true;
    let failed = f.land(second.run, Err("measurement failed".into()));
    assert!(f.peaks(1).is_none() && f.peaks(2).is_none());
    assert!(
        f.editing
            .multitrack_runtime
            .operations
            .busy(work.ticket.target)
    );
    assert!(failed.effects.iter().any(|e| {
        e["reason"]
            .as_str()
            .unwrap_or("")
            .contains("storage unavailable")
    }));
    f.disk.fail_repair = false;
    let retried = f.editing.start(
        f.member,
        &json!({"verb":"retry"}),
        None,
        &mut f.disk,
        &mut f.ids,
    );
    assert!(matches!(retried.end, Some(Ok(_))));
    assert!(
        !f.editing
            .multitrack_runtime
            .operations
            .busy(work.ticket.target)
    );
    assert_eq!(f.disk.repairs, 2);
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
