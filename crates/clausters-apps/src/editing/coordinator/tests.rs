//! The coordinator's runs, driven by a client that holds, fails and
//! reorders awaits over an engine that plays nothing.

use super::super::call_json;
use super::super::held::{HeldEngine, HeldTake};
use super::*;
use crate::editing::{Caller, Header, Ran};
use clausters_core::ids::{IdShare, ServerShape, Space};
use clausters_editing::apply::{Applier, Endpoint};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// What the engine below has been told, as a test reads it.
#[derive(Debug, Default)]
struct Told {
    rolling: bool,
    /// The revision it reads of each file.
    revisions: HashMap<u64, u64>,
    /// The files closed, in order.
    closed: Vec<u64>,
}

/// **An engine that plays nothing**: its playback's ops are steps a
/// test reads, its transport rolls as the client says, and a take handed
/// to it is one buffer per channel.
#[derive(Debug)]
struct Engine(Arc<Mutex<Told>>);

impl HeldEngine for Engine {
    fn medium(&mut self) -> Box<dyn Medium + Send> {
        Box::new(Applier::new(Endpoint { chunk: 4096 }))
    }
    fn rate(&self) -> f64 {
        48_000.0
    }
    fn take(
        &mut self,
        _member: MemberId,
        take: &HeldTake,
        ids: &mut IdSpaces,
    ) -> Result<Vec<i32>, String> {
        self.0
            .lock()
            .unwrap()
            .revisions
            .insert(take.reader, take.revision);
        (0..take.channels)
            .map(|_| {
                ids.alloc(Space::Buffers, 1)
                    .map(|b| b as i32)
                    .map_err(|e| e.to_string())
            })
            .collect()
    }
    fn revision(&self, _member: MemberId, file: u64) -> Option<u64> {
        self.0.lock().unwrap().revisions.get(&file).copied()
    }
    fn close(&mut self, _member: MemberId, file: u64, _ids: &mut IdSpaces) {
        let mut told = self.0.lock().unwrap();
        told.revisions.remove(&file);
        told.closed.push(file);
    }
    fn close_member(&mut self, _member: MemberId, _ids: &mut IdSpaces) {
        self.0.lock().unwrap().revisions.clear();
    }
    fn rolling(&self, _transport: i32) -> bool {
        self.0.lock().unwrap().rolling
    }
    fn control(&self, _bus: usize) -> f32 {
        0.0
    }
    fn starved(&self) -> u64 {
        0
    }
}

/// **A client that does what it is asked**: every service succeeds, the
/// engine's transport rolls as `rolling` says, a file read is a
/// two-channel take, and every effect and service is written down. A render
/// is held until the test lands it.
struct Client {
    ids: IdSpaces,
    rolling: bool,
    engine: Arc<Mutex<Told>>,
    /// The service that fails once: `steps`.
    fail: Option<&'static str>,
    trace: Vec<String>,
    effects: Vec<Value>,
    /// What a look on disk finds, by path.
    stamps: Value,
    /// The files the services deleted, in order.
    deleted: Vec<String>,
}

/// **The client's services**, each written down as the await it was:
/// `await:steps` (steps carried out), `await:shape` (a header read),
/// `await:stat` (a look on disk) and `delete`.
struct Disk<'a> {
    trace: &'a mut Vec<String>,
    fail: &'a mut Option<&'static str>,
    stamps: &'a Value,
    deleted: &'a mut Vec<String>,
}

impl Caller for Disk<'_> {
    fn header(&mut self, _path: &str) -> Result<Header, String> {
        self.trace.push("await:shape".into());
        Ok(Header {
            frames: 200,
            channels: 2,
            rate: 48_000.0,
            codec: "wav".into(),
            sample_format: Some("float".into()),
        })
    }
    fn stamps(&mut self, _paths: &[String]) -> Value {
        self.trace.push("await:stat".into());
        self.stamps.clone()
    }
    fn carry(&mut self, _steps: &[clausters_editing::apply::Step]) -> Result<(), String> {
        self.trace.push("await:steps".into());
        if *self.fail == Some("steps") {
            *self.fail = None;
            return Err("refused".into());
        }
        Ok(())
    }
    fn delete(&mut self, files: &[String]) {
        self.trace.push("delete".into());
        self.deleted.extend_from_slice(files);
    }
    fn call_off(&mut self, _member: MemberId) {
        self.trace.push("call_off".into());
    }
}

impl Client {
    fn new() -> Client {
        Client {
            ids: IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE),
            rolling: false,
            engine: Arc::default(),
            fail: None,
            trace: Vec::new(),
            effects: Vec::new(),
            stamps: json!({}),
            deleted: Vec::new(),
        }
    }

    fn hold(&mut self, editing: &mut Editing) {
        if editing.engine.is_none() {
            editing.hold_engine(Box::new(Engine(Arc::clone(&self.engine))));
        }
        self.engine.lock().unwrap().rolling = self.rolling;
    }

    fn call(&mut self, editing: &mut Editing, request: Value) -> Value {
        self.hold(editing);
        serde_json::from_str(&call_json(editing, &request.to_string())).unwrap()
    }

    fn query(&mut self, editing: &mut Editing, member: MemberId, query: &str) -> Value {
        self.hold(editing);
        editing.query(member, query)
    }

    /// Run one action to its end, its renders landed as they come.
    fn run(&mut self, editing: &mut Editing, member: MemberId, action: Value) -> Value {
        let first = self.begin(editing, member, action);
        self.finish(editing, member, first)
    }

    /// **Start an action and stop at its first render**, which is held: a
    /// render that has not landed yet. The effects decided before it are
    /// written down.
    fn begin(&mut self, editing: &mut Editing, member: MemberId, action: Value) -> Value {
        self.hold(editing);
        let ran = editing.start(
            member,
            &action,
            None,
            &mut Disk {
                trace: &mut self.trace,
                fail: &mut self.fail,
                stamps: &self.stamps,
                deleted: &mut self.deleted,
            },
            &mut self.ids,
        );
        self.said(ran)
    }

    /// Land the render `pending` waits on, as `result` says (`{}` or
    /// `{"failed": reason}`), and stop at the next one or at the end.
    fn answer(
        &mut self,
        editing: &mut Editing,
        member: MemberId,
        pending: &Value,
        result: Value,
    ) -> Value {
        self.hold(editing);
        let result = match result["failed"].as_str() {
            Some(reason) => Err(reason.to_string()),
            None => Ok(()),
        };
        let ran = editing.land(
            member,
            pending["run"].as_u64().unwrap_or(0),
            result,
            None,
            &mut Disk {
                trace: &mut self.trace,
                fail: &mut self.fail,
                stamps: &self.stamps,
                deleted: &mut self.deleted,
            },
            &mut self.ids,
        );
        self.said(ran)
    }

    /// Carry a run on from `answer` to its end, every render landing.
    fn finish(&mut self, editing: &mut Editing, member: MemberId, mut answer: Value) -> Value {
        while answer["done"] != true {
            answer = self.answer(editing, member, &answer, json!({}));
        }
        answer
    }

    /// What a run came to, written down: its effects, the render it waits
    /// on (`await:batch`), or its end.
    fn said(&mut self, ran: Ran) -> Value {
        for effect in ran.effects {
            self.trace
                .push(effect["kind"].as_str().unwrap().to_string());
            self.effects.push(effect);
        }
        if let Some(render) = ran.render {
            self.trace.push("await:batch".into());
            return json!({"done": false, "run": render.run});
        }
        match ran.end {
            Some(Ok(result)) => json!({"done": true, "result": result}),
            Some(Err(raised)) => json!({"done": true,
                "error": {"type": raised.kind, "message": raised.message}}),
            None => json!({"done": false}),
        }
    }
}

/// The file in focus as the editor binds it.
fn binding(editing: &mut Editing, member: MemberId) -> Value {
    serde_json::from_str(&call_json(
        editing,
        &json!({"verb": "member", "member": member, "call": {"verb": "binding"}}).to_string(),
    ))
    .unwrap()
}

/// A cut of 20 frames from frame 10 of the file in focus, from its picture.
fn cut(seq: i64, editing: &Editing) -> Value {
    json!({"verb": "event", "addr": "/gui_event", "window": true,
        "args": [12, seq, editing.version(), "cut", 10.0, 20.0]})
}

/// The same 20 frames reversed: an edit that renders, so its run waits
/// on a batch.
fn reverse(seq: i64, editing: &Editing) -> Value {
    json!({"verb": "event", "addr": "/gui_event", "window": true,
        "args": [12, seq, editing.version(), "reverse", 10.0, 20.0]})
}

fn save(path: &str) -> Value {
    json!({"verb": "save", "request": {"path": path, "format": "float"}})
}

/// The render `pending` stopped at, held: a run stops at its first.
fn until(
    _client: &mut Client,
    _editing: &mut Editing,
    _member: MemberId,
    pending: Value,
    kind: &str,
    _nth: usize,
) -> Value {
    assert_eq!(kind, "batch");
    assert_ne!(
        pending["done"], true,
        "the run ended before its render: {pending}"
    );
    pending
}

fn opened(client: &mut Client, editing: &mut Editing, empty: bool) -> MemberId {
    let request = if empty {
        json!({"verb": "openAudio", "key": "audio:empty", "empty": true})
    } else {
        json!({"verb": "openAudio", "key": "take:3", "path":"/in/take3.wav","workspace":"/work", "frames": 100})
    };
    let member = client.call(editing, request)["member"].as_u64().unwrap() as MemberId;
    client.call(
        editing,
        json!({"verb": "member", "member": member, "call": {"verb": "sync", "window": 900}}),
    );
    client.call(
        editing,
        json!({"verb": "member", "member": member,
            "call": {"verb": "window", "widget": 12, "meter": 13, "filesWidget": 14}}),
    );
    let answer = client.run(
        editing,
        member,
        json!({"verb": "open", "chunk": 4096, "rate": 48000.0, "empty": empty, "window": true}),
    );
    assert!(answer["error"].is_null(), "{answer}");
    assert!(answer["result"]["transport"].is_i64(), "{answer}");
    member
}

fn position(trace: &[String], kind: &str) -> usize {
    trace
        .iter()
        .position(|k| k == kind)
        .unwrap_or_else(|| panic!("no {kind} in {trace:?}"))
}

/// **The window coming back looks at the files on disk**: the
/// activation asks the client for their stamps, and a file that changed
/// is said in the answer to the window's event.
#[test]
fn a_window_coming_back_is_told_a_file_changed_on_disk() {
    let mut editing = Editing::new();
    let mut client = Client::new();
    let member = client.call(
        &mut editing,
        json!({"verb": "openAudio", "key": "take:3", "path": "/in/take3.wav",
            "workspace": "/work", "frames": 100,
            "stamp": {"modified": 1.0, "size": 10}}),
    )["member"]
        .as_u64()
        .unwrap() as MemberId;
    client.call(
        &mut editing,
        json!({"verb": "member", "member": member, "call": {"verb": "sync", "window": 900}}),
    );
    client.call(
        &mut editing,
        json!({"verb": "member", "member": member, "call": {"verb": "window", "widget": 12}}),
    );
    client.run(
        &mut editing,
        member,
        json!({"verb": "open", "chunk": 4096, "rate": 48000.0, "empty": false, "window": true}),
    );
    client.stamps = json!({"/in/take3.wav": {"modified": 2.0, "size": 10}});
    client.trace.clear();
    client.effects.clear();
    let version = editing.version();
    let back = client.run(
        &mut editing,
        member,
        json!({"verb": "event", "addr": "/gui_event", "window": true,
            "args": [900, 7, version, "activated", 1]}),
    );
    assert!(back["error"].is_null(), "{back}");
    assert!(
        client.trace.contains(&"await:stat".to_string()),
        "{:?}",
        client.trace
    );
    let said = client
        .effects
        .iter()
        .find(|e| e["kind"] == "echo")
        .map(|e| e["answer"].clone())
        .unwrap();
    assert_eq!(said["seq"], 7);
    assert_eq!(said["reason"], "take3.wav changed on disk");
}

#[test]
fn opening_measures_the_take_and_hands_it_to_the_engine() {
    let mut editing = Editing::new();
    let mut client = Client::new();
    opened(&mut client, &mut editing, false);
    let trace = &client.trace;
    // the peaks are an operation's batch, and the engine reads the take
    assert!(trace.contains(&"await:batch".to_string()), "{trace:?}");
    assert_eq!(client.engine.lock().unwrap().revisions.len(), 1);
    assert!(
        !trace.contains(&"dirty".to_string()),
        "dirty is read from the editor, never projected: {trace:?}"
    );
}

#[test]
fn an_empty_editor_opens_with_nothing_to_sound() {
    let mut editing = Editing::new();
    let mut client = Client::new();
    opened(&mut client, &mut editing, true);
    assert!(
        !client.trace.iter().any(|k| k.starts_with("await:")),
        "{:?}",
        client.trace
    );
    let refused = client.run(&mut editing, 0, json!({"verb": "play"}));
    assert_eq!(refused["error"]["message"], NO_FILE);
}

#[test]
fn an_opened_file_comes_into_focus_and_its_meter_is_told_before_the_redraw() {
    let mut editing = Editing::new();
    let mut client = Client::new();
    let member = opened(&mut client, &mut editing, true);
    client.trace.clear();
    let answer = client.run(
        &mut editing,
        member,
        json!({"verb": "openFile", "paths": ["/takes/a.wav"], "window": true}),
    );
    assert!(answer["error"].is_null(), "{answer}");
    let trace = &client.trace;
    let bind = position(trace, "bind");
    let publish = position(trace, "publish");
    assert!(position(trace, "await:shape") < bind, "{trace:?}");
    assert!(bind < publish, "{trace:?}");
    // the meter is known when the window is drawn again
    let session = editing.sessions.get(&member).unwrap();
    assert!(session.playback.meters().is_some());
    // and nothing is set on a widget the window does not draw
    assert!(file_id(&editing, member).is_some());
}

#[test]
fn the_space_bar_plays_or_stops_as_the_transport_rolls() {
    let mut editing = Editing::new();
    let mut client = Client::new();
    let member = opened(&mut client, &mut editing, false);
    let play = |client: &mut Client, editing: &mut Editing| {
        client.trace.clear();
        client.run(
            editing,
            member,
            json!({"verb": "event", "addr": "/gui_event",
                "args": [900, 1, 0, "play", 0], "window": true}),
        )
    };
    play(&mut client, &mut editing);
    let trace = client.trace.clone();
    assert!(trace.contains(&"await:steps".to_string()), "{trace:?}");
    assert!(
        trace.contains(&"set".to_string()),
        "the play cursor: {trace:?}"
    );
    assert_eq!(
        editing.sessions[&member].playback.playing(),
        editing.sessions[&member].playback.focus()
    );
    client.rolling = true;
    play(&mut client, &mut editing);
    assert_eq!(
        editing.sessions[&member].playback.playing(),
        None,
        "stopped"
    );
}

#[test]
fn a_failed_await_ends_the_run_for_the_client_to_raise() {
    let mut editing = Editing::new();
    let mut client = Client::new();
    let member = opened(&mut client, &mut editing, false);
    client.fail = Some("steps");
    let answer = client.run(
        &mut editing,
        member,
        json!({"verb": "play", "window": true}),
    );
    assert_eq!(answer["done"], true);
    assert_eq!(answer["error"]["message"], "refused", "{answer}");
}

#[test]
fn closing_the_last_file_draws_the_empty_window_and_corrects_nothing_absent() {
    let mut editing = Editing::new();
    let mut client = Client::new();
    let member = opened(&mut client, &mut editing, false);
    client.trace.clear();
    client.effects.clear();
    let answer = client.run(
        &mut editing,
        member,
        json!({"verb": "verb", "call": {"verb": "close_file"}, "window": true}),
    );
    assert!(answer["error"].is_null(), "{answer}");
    let bind = client
        .effects
        .iter()
        .find(|e| e["kind"] == "bind")
        .expect("a bind");
    assert!(bind["binding"].is_null());
    assert!(client.trace.contains(&"publish".to_string()));
    assert!(
        !client
            .effects
            .iter()
            .any(|e| e["kind"] == "set" && e["widget"] == 13),
        "the meter is not drawn: {:?}",
        client.effects
    );
}

#[test]
fn a_script_reads_the_span_and_the_loop_switch_it_set() {
    let mut editing = Editing::new();
    let mut client = Client::new();
    let member = opened(&mut client, &mut editing, false);
    client.run(
        &mut editing,
        member,
        json!({"verb": "span", "span": [0.0, 0.001]}),
    );
    client.run(&mut editing, member, json!({"verb": "looping", "on": true}));
    let state = client.query(&mut editing, member, "transport");
    assert_eq!(state["span"], json!([0.0, 0.001]));
    assert_eq!(state["looping"], true);
}

/// **The loop switch and the span are the file's**, one state a script,
/// a hand and the window's menu all read: a script's switch is the menu's
/// check, and a file brought back into focus brings back its own.
#[test]
fn a_script_s_loop_switch_is_the_menu_s_check_and_each_file_keeps_its_own() {
    let mut editing = Editing::new();
    let mut client = Client::new();
    let member = opened(&mut client, &mut editing, false);
    let checked = |client: &mut Client, editing: &mut Editing| {
        client.call(editing, json!({"verb": "actions", "member": member}))["loop"]["checked"]
            .clone()
    };
    let transport =
        |client: &mut Client, editing: &mut Editing| client.query(editing, member, "transport");
    client.run(&mut editing, member, json!({"verb": "looping", "on": true}));
    assert_eq!(
        checked(&mut client, &mut editing),
        true,
        "the menu reads the switch"
    );
    client.run(
        &mut editing,
        member,
        json!({"verb": "span", "span": [0.0, 0.001]}),
    );
    client.run(
        &mut editing,
        member,
        json!({"verb": "openFile", "paths": ["/takes/b.wav"], "window": true}),
    );
    let other = transport(&mut client, &mut editing);
    assert_eq!(other["looping"], false, "the new file's own switch");
    assert!(other["span"].is_null(), "and its own span");
    assert_eq!(checked(&mut client, &mut editing), false);
    client.run(
        &mut editing,
        member,
        json!({"verb": "verb", "call": {"verb": "focus_file", "index": 0}, "window": true}),
    );
    let first = transport(&mut client, &mut editing);
    assert_eq!(first["looping"], true);
    assert_eq!(first["span"], json!([0.0, 0.001]));
    assert_eq!(checked(&mut client, &mut editing), true);
}

/// **An edit while the write is out**: the save releases the take once
/// its snapshot is copied, so a cut lands while the file is written; the
/// write completing afterwards cleans only the revision it wrote, and an
/// undo back to that revision is clean.
#[test]
fn an_edit_during_a_save_stays_dirty_and_its_undo_is_clean() {
    let mut editing = Editing::new();
    let mut client = Client::new();
    let member = opened(&mut client, &mut editing, false);
    let first = client.begin(&mut editing, member, save("/takes/a.wav"));
    let write = until(&mut client, &mut editing, member, first, "batch", 1);
    let action = cut(1, &editing);

    let edit = client.run(&mut editing, member, action);
    assert!(
        edit["error"].is_null(),
        "an edit is taken while the write is out: {edit}"
    );
    assert_eq!(edit["result"], true, "the cut changed the take: {edit}");
    let saved = client.finish(&mut editing, member, write);
    assert!(saved["error"].is_null(), "{saved}");
    assert_eq!(saved["result"]["destination"]["path"], "/takes/a.wav");
    let now = binding(&mut editing, member);
    assert_eq!(now["dirty"], true, "the cut is newer than what was written");
    assert_eq!(
        now["file"]["path"], "/takes/a.wav",
        "the target moved on success"
    );
    assert_eq!(now["frames"], 80);
    let version = editing.version();
    let undo = client.run(
        &mut editing,
        member,
        json!({"verb": "event", "addr": "/gui_event", "window": true,
            "args": [900, 2, version, "undo"]}),
    );
    assert!(undo["error"].is_null(), "{undo}");
    let back = binding(&mut editing, member);
    assert_eq!(back["frames"], 100);
    assert_eq!(back["dirty"], false, "the saved revision again");
}

/// **A write the server refuses** keeps the file dirty and its target
/// where it was; the same save, retried, succeeds.
#[test]
fn a_failed_save_keeps_the_target_and_a_retry_writes_it() {
    let mut editing = Editing::new();
    let mut client = Client::new();
    let member = opened(&mut client, &mut editing, false);
    let action = cut(1, &editing);

    client.run(&mut editing, member, action);
    let before = binding(&mut editing, member);
    let first = client.begin(&mut editing, member, save("/takes/refused.wav"));
    let write = until(&mut client, &mut editing, member, first, "batch", 1);
    let failed = client.answer(
        &mut editing,
        member,
        &write,
        json!({"failed": "quota exceeded"}),
    );
    let failed = client.finish(&mut editing, member, failed);
    assert_eq!(failed["done"], true);
    assert!(
        failed["error"].is_object(),
        "the client raises the refusal: {failed}"
    );
    let after = binding(&mut editing, member);
    assert_eq!(after["dirty"], true);
    assert_eq!(after["file"]["path"], before["file"]["path"]);
    let saved = client.run(&mut editing, member, save("/takes/refused.wav"));
    assert!(saved["error"].is_null(), "{saved}");
    let clean = binding(&mut editing, member);
    assert_eq!(clean["dirty"], false);
    assert_eq!(clean["file"]["path"], "/takes/refused.wav");
}

/// **The window's work called off**: its `"call_off"` has the caller stop
/// its renders, and nothing of the take moves.
#[test]
fn a_window_call_off_has_the_client_stop_its_renders() {
    let mut editing = Editing::new();
    let mut client = Client::new();
    let member = opened(&mut client, &mut editing, false);
    let before = binding(&mut editing, member);
    client.effects.clear();
    let call_off = json!({"verb": "event", "addr": "/gui_event", "window": true,
        "args": [900, 3, editing.version(), "call_off"]});
    let answer = client.run(&mut editing, member, call_off);
    assert!(answer["error"].is_null(), "{answer}");
    assert!(
        client.trace.iter().any(|k| k == "call_off"),
        "{:?}",
        client.trace
    );
    assert_eq!(binding(&mut editing, member), before);
}

/// **A failed render leaves nothing in the working folder**: a reverse
/// whose render the disk refuses deletes what it wrote there, its peaks
/// with it, and a save that fails deletes nothing the user named.
#[test]
fn a_failed_render_deletes_its_scratch_and_nothing_else() {
    let mut editing = Editing::new();
    let mut client = Client::new();
    let member = opened(&mut client, &mut editing, false);
    let reverse = json!({"verb": "event", "addr": "/gui_event", "window": true,
        "args": [12, 1, editing.version(), "reverse", 10.0, 20.0]});
    let first = client.begin(&mut editing, member, reverse);
    let render = until(&mut client, &mut editing, member, first, "batch", 1);
    client.deleted.clear();
    let failed = client.answer(
        &mut editing,
        member,
        &render,
        json!({"failed": "Disk quota exceeded"}),
    );
    let files = std::mem::take(&mut client.deleted);
    assert!(!files.is_empty(), "{failed}");
    assert!(files.iter().all(|f| f.starts_with("/work/")), "{files:?}");
    assert!(files.iter().any(|f| f.ends_with(".w64")), "{files:?}");
    assert!(files.iter().any(|f| f.ends_with(".peaks")), "{files:?}");
    client.finish(&mut editing, member, failed);

    let first = client.begin(&mut editing, member, save("/takes/refused.wav"));
    let write = until(&mut client, &mut editing, member, first, "batch", 1);
    client.deleted.clear();
    let failed = client.answer(
        &mut editing,
        member,
        &write,
        json!({"failed": "quota exceeded"}),
    );
    assert!(
        client.deleted.iter().all(|f| !f.starts_with("/takes/")),
        "{failed}"
    );
}

/// **Completions out of order, each to its own file**: a save of A is
/// out while B comes into focus and is reversed; B's batch answers first and
/// A's last. Each lands on the file it was started on, and the late one
/// moves nothing in the window, which is showing B.
#[test]
fn completions_in_another_order_land_on_the_file_each_was_started_on() {
    let mut editing = Editing::new();
    let mut client = Client::new();
    let member = opened(&mut client, &mut editing, false);
    let action = cut(1, &editing);

    client.run(&mut editing, member, action);
    let a = binding(&mut editing, member)["id"].clone();
    let first = client.begin(&mut editing, member, save("/takes/a.wav"));
    let write_a = until(&mut client, &mut editing, member, first, "batch", 1);
    client.run(
        &mut editing,
        member,
        json!({"verb": "openFile", "paths": ["/takes/b.wav"], "window": true}),
    );
    let b = binding(&mut editing, member)["id"].clone();
    assert_ne!(a, b);
    let action = reverse(2, &editing);

    let started = client.begin(&mut editing, member, action);
    let reversed = until(&mut client, &mut editing, member, started, "batch", 1);
    let reversed = client.finish(&mut editing, member, reversed);
    assert!(reversed["error"].is_null(), "{reversed}");
    client.effects.clear();
    let saved = client.finish(&mut editing, member, write_a);
    assert!(saved["error"].is_null(), "{saved}");
    let shown = binding(&mut editing, member);
    assert_eq!(shown["id"], b, "the window still shows B");
    assert_eq!(shown["dirty"], true, "B's edit is B's");
    assert_eq!(shown["frames"], 200);
    assert!(
        !client
            .effects
            .iter()
            .any(|e| e["kind"] == "bind" || e["kind"] == "publish"),
        "A's late save moves nothing in the window: {:?}",
        client.effects
    );
    client.run(
        &mut editing,
        member,
        json!({"verb": "verb", "call": {"verb": "focus_file", "index": 0}, "window": true}),
    );
    let a_now = binding(&mut editing, member);
    assert_eq!(a_now["id"], a);
    assert_eq!(a_now["dirty"], false, "A's save cleaned A");
    assert_eq!(a_now["file"]["path"], "/takes/a.wav");
    assert_eq!(a_now["frames"], 80);
}

/// **A result answered twice** is taken once: the second answer finds
/// no run waiting and changes nothing.
#[test]
fn a_duplicate_completion_is_refused_and_changes_nothing() {
    let mut editing = Editing::new();
    let mut client = Client::new();
    let member = opened(&mut client, &mut editing, false);
    let action = reverse(1, &editing);

    let started = client.begin(&mut editing, member, action);
    let batch = until(&mut client, &mut editing, member, started, "batch", 1);
    let done = client.answer(&mut editing, member, &batch, json!({}));
    let done = client.finish(&mut editing, member, done);
    assert!(done["error"].is_null(), "{done}");
    let version = editing.version();
    let again = client.answer(&mut editing, member, &batch, json!({}));
    assert!(again["error"].is_object(), "{again}");
    assert_eq!(editing.version(), version);
    let state = client.call(&mut editing, json!({"verb": "state", "member": member}));
    assert_eq!(state["undoLabel"], "reverse", "one entry: {state}");
}

/// **The window closed while a save is out**: the view goes and the
/// playback with it; the write completing afterwards still saves the
/// file the client holds, and does not draw or sound the closed window
/// again.
#[test]
fn a_save_completing_after_its_window_closed_saves_and_sounds_nothing() {
    let mut editing = Editing::new();
    let mut client = Client::new();
    let member = opened(&mut client, &mut editing, false);
    let action = cut(1, &editing);

    client.run(&mut editing, member, action);
    let first = client.begin(&mut editing, member, save("/takes/a.wav"));
    let write = until(&mut client, &mut editing, member, first, "batch", 1);
    let closed = client.run(
        &mut editing,
        member,
        json!({"verb": "close", "window": true}),
    );
    assert!(closed["error"].is_null(), "{closed}");
    client.trace.clear();
    client.effects.clear();
    let saved = client.finish(&mut editing, member, write);
    assert!(saved["error"].is_null(), "{saved}");
    assert_eq!(binding(&mut editing, member)["dirty"], false);
    assert!(
        !client
            .effects
            .iter()
            .any(|e| matches!(e["kind"].as_str(), Some("publish" | "set" | "bind"))),
        "nothing is drawn: {:?}",
        client.effects
    );
    assert!(
        !client.trace.iter().any(|k| k == "await:steps"),
        "nothing sounds: {:?}",
        client.trace
    );
}

/// **An edit whose render fails** changes nothing: the take stays as
/// it was, clean, and nothing is recorded.
#[test]
fn an_edit_whose_render_fails_leaves_the_take_whole() {
    let mut editing = Editing::new();
    let mut client = Client::new();
    let member = opened(&mut client, &mut editing, false);
    let action = reverse(1, &editing);

    let started = client.begin(&mut editing, member, action);
    let render = until(&mut client, &mut editing, member, started, "batch", 1);
    let failed = client.answer(
        &mut editing,
        member,
        &render,
        json!({"failed": "render refused"}),
    );
    let failed = client.finish(&mut editing, member, failed);
    assert_eq!(failed["done"], true, "{failed}");
    assert_eq!(binding(&mut editing, member)["frames"], 100);
    assert_eq!(binding(&mut editing, member)["dirty"], false);
    let undo = client.call(&mut editing, json!({"verb": "state", "member": member}));
    assert_eq!(undo["canUndo"], false, "and nothing was recorded");
}

fn last(trace: &[String], kind: &str) -> usize {
    trace
        .iter()
        .rposition(|k| k == kind)
        .unwrap_or_else(|| panic!("no {kind} in {trace:?}"))
}

/// **The answer that has the window read the list again** follows the
/// renders of the files it names.
#[test]
fn the_answer_that_reloads_the_list_follows_the_renders_that_make_it() {
    let mut editing = Editing::new();
    let mut client = Client::new();
    let member = opened(&mut client, &mut editing, false);
    client.trace.clear();
    let action = reverse(1, &editing);
    client.run(&mut editing, member, action);
    let trace = &client.trace;
    assert!(
        last(trace, "await:batch") < last(trace, "echo"),
        "{trace:?}"
    );
}

/// **A file is not edited while it plays**: a cut and an undo over the
/// playing file are refused, the menu reads them refused for it, and once
/// it stops the cut goes through.
#[test]
fn a_file_is_not_edited_while_it_plays() {
    let mut editing = Editing::new();
    let mut client = Client::new();
    let member = opened(&mut client, &mut editing, false);
    let cut = |client: &mut Client, editing: &mut Editing, seq: i64| {
        client.trace.clear();
        client.effects.clear();
        let version = editing.version();
        let answer = client.run(
            editing,
            member,
            json!({"verb": "event", "addr": "/gui_event",
                "args": [12, seq, version, "cut", 10.0, 20.0], "window": true}),
        );
        answer["result"] == true
    };
    client.rolling = true;
    client.run(
        &mut editing,
        member,
        json!({"verb": "play", "window": true}),
    );
    let version = editing.version();
    assert!(!cut(&mut client, &mut editing, 1), "nothing is cut");
    assert_eq!(editing.version(), version, "and nothing changed");
    let said = client
        .effects
        .iter()
        .any(|e| e.to_string().contains(crate::audio::editor::PLAYING));
    assert!(said, "the window is told why: {:?}", client.effects);
    let states = editing.action_states(member);
    for verb in ["cut", "paste", "undo", "redo"] {
        assert_eq!(states[verb]["enabled"], false, "{verb}: {states}");
    }
    assert_ne!(
        states["copy"]["reason"],
        json!(crate::audio::editor::PLAYING),
        "a copy is no edit"
    );
    client.rolling = false;
    client.run(
        &mut editing,
        member,
        json!({"verb": "stop", "window": true}),
    );
    assert!(cut(&mut client, &mut editing, 2), "stopped, it is edited");
}

#[test]
fn a_closed_file_stops_being_read_before_what_it_measured_is_deleted() {
    let mut editing = Editing::new();
    let mut client = Client::new();
    let member = opened(&mut client, &mut editing, false);
    client.run(
        &mut editing,
        member,
        json!({"verb": "openFile", "paths": ["/takes/b.wav"], "window": true}),
    );
    client.trace.clear();
    client.run(
        &mut editing,
        member,
        json!({"verb": "verb", "call": {"verb": "close_file"}, "window": true}),
    );
    let trace = &client.trace;
    // the engine lets the file go before its peaks are deleted
    assert_eq!(client.engine.lock().unwrap().closed.len(), 1);
    assert!(
        position(trace, "await:steps") < position(trace, "delete"),
        "{trace:?}"
    );
}

/// An audio window with no client beside it: its dialogs numbered from 400,
/// and it asks before unsaved work goes.
fn asking(client: &mut Client, editing: &mut Editing) -> (MemberId, BTreeMap<String, i64>) {
    let member = opened(client, editing, false);
    let names = client.call(
        editing,
        json!({"verb": "member", "member": member, "call": {"verb": "dialogs"}}),
    )["dialogs"]
        .clone();
    let ids: BTreeMap<String, i64> = names
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str().unwrap().to_string(), 400 + i as i64))
        .collect();
    client.call(
        editing,
        json!({"verb": "member", "member": member,
            "call": {"verb": "window", "widget": 12, "meter": 13, "filesWidget": 14, "dialogs": ids}}),
    );
    client.call(
        editing,
        json!({"verb": "member", "member": member, "call": {"verb": "sync", "asks": true}}),
    );
    (member, ids)
}

/// One of the window's own verbs.
fn verb_of_window(seq: i64, editing: &Editing, verb: &str) -> Value {
    json!({"verb": "event", "addr": "/gui_event", "window": true,
        "args": [900, seq, editing.version(), verb]})
}

/// A button of the close form.
fn answered(seq: i64, editing: &Editing, ids: &BTreeMap<String, i64>, button: &str) -> Value {
    json!({"verb": "event", "addr": "/gui_event", "window": true,
        "args": [ids[&format!("close:{button}")], seq, editing.version(), "click"]})
}

fn closes(client: &Client) -> bool {
    client.effects.iter().any(|e| e["kind"] == "closeWindow")
}

/// **A window the work's only holder asks before it goes**: nothing to lose
/// closes at once, Cancel keeps everything, and Don't save lets the work go.
#[test]
fn a_close_asks_when_it_would_lose_work_and_a_client_s_does_not() {
    let mut editing = Editing::new();
    let mut client = Client::new();
    let (member, ids) = asking(&mut client, &mut editing);
    let action = cut(1, &editing);
    client.run(&mut editing, member, action);
    assert_eq!(binding(&mut editing, member)["dirty"], true);
    let action = verb_of_window(2, &editing, "close");
    client.run(&mut editing, member, action);
    assert!(!closes(&client), "it asks: {:?}", client.effects);
    let action = answered(3, &editing, &ids, "cancel");
    client.run(&mut editing, member, action);
    assert!(!closes(&client));
    assert_eq!(
        binding(&mut editing, member)["dirty"],
        true,
        "the work stays"
    );
    let action = verb_of_window(4, &editing, "close");
    client.run(&mut editing, member, action);
    let action = answered(5, &editing, &ids, "discard");
    client.run(&mut editing, member, action);
    assert!(closes(&client), "{:?}", client.effects);

    // a client holds its file, and its window goes at once
    let mut editing = Editing::new();
    let mut client = Client::new();
    let member = opened(&mut client, &mut editing, false);
    let action = cut(1, &editing);
    client.run(&mut editing, member, action);
    let action = verb_of_window(2, &editing, "close");
    client.run(&mut editing, member, action);
    assert!(closes(&client));
}

/// **The form's Save closes once the write is confirmed**, and not before.
#[test]
fn what_waits_for_a_save_goes_over_what_it_wrote() {
    let mut editing = Editing::new();
    let mut client = Client::new();
    let (member, ids) = asking(&mut client, &mut editing);
    let action = cut(1, &editing);
    client.run(&mut editing, member, action);
    let action = verb_of_window(2, &editing, "close");
    client.run(&mut editing, member, action);
    let action = answered(3, &editing, &ids, "ok");
    let first = client.begin(&mut editing, member, action);
    let write = until(&mut client, &mut editing, member, first, "batch", 1);
    assert!(!closes(&client), "not before the write is confirmed");
    let saved = client.finish(&mut editing, member, write);
    assert!(saved["error"].is_null(), "{saved}");
    assert!(closes(&client), "{:?}", client.effects);
}

/// **Close file asks about the file in view only**: a clean one goes though
/// another has changes, and Don't save lets a changed one go with the window
/// staying.
#[test]
fn close_file_asks_about_the_file_it_closes() {
    let mut editing = Editing::new();
    let mut client = Client::new();
    let (member, ids) = asking(&mut client, &mut editing);
    let action = cut(1, &editing);
    client.run(&mut editing, member, action);
    let action = verb_of_window(2, &editing, "close_file");
    client.run(&mut editing, member, action);
    assert_eq!(
        binding(&mut editing, member)["dirty"],
        true,
        "it asks first"
    );
    let action = answered(3, &editing, &ids, "discard");
    client.run(&mut editing, member, action);
    assert_eq!(
        binding(&mut editing, member)["empty"],
        true,
        "the file went"
    );
    assert!(!closes(&client), "the window stays");
}

/// **A close goes through the files with changes in turn**, bringing each
/// into view to ask about it, and a clean file asks nothing.
#[test]
fn a_close_brings_each_file_with_changes_into_view() {
    let mut editing = Editing::new();
    let mut client = Client::new();
    let (member, ids) = asking(&mut client, &mut editing);
    let first = binding(&mut editing, member)["id"].clone();
    let action = cut(1, &editing);
    client.run(&mut editing, member, action);
    client.run(
        &mut editing,
        member,
        json!({"verb": "openFile", "paths": ["/takes/b.wav"], "window": true}),
    );
    assert_ne!(binding(&mut editing, member)["id"], first);
    let action = verb_of_window(2, &editing, "close");
    client.run(&mut editing, member, action);
    assert_eq!(
        binding(&mut editing, member)["id"],
        first,
        "the one with changes"
    );
    assert!(!closes(&client));
    let action = answered(3, &editing, &ids, "discard");
    client.run(&mut editing, member, action);
    assert!(closes(&client), "{:?}", client.effects);
}

/// **Three applications over one set of services, their replies out of order**:
/// an audio file, a multitrack's session and a symbolic score each have a save
/// out; they land in the opposite order, one of them failing, and each settles
/// its own contents, file and order, none of them moved by another's reply.
#[test]
fn saves_of_different_applications_land_each_on_its_own() {
    let mut editing = Editing::new();
    let mut client = Client::new();
    let folder = std::env::temp_dir().join(format!("three-saves-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let audio = opened(&mut client, &mut editing, false);
    let action = cut(1, &editing);
    client.run(&mut editing, audio, action);
    let multitrack = client.call(
        &mut editing,
        json!({"verb": "openMultitrack", "key": "session", "multitrack": {}}),
    )["member"]
        .as_u64()
        .unwrap() as MemberId;
    let mark = json!({"verb": "handle", "call": {"verb": "apply",
        "intent": {"intent": "setmarker", "marker": 7, "at": 2.0, "name": "cue"}}});
    assert_eq!(
        client.run(&mut editing, multitrack, mark)["result"]["applied"],
        true
    );

    let wrote = folder.join("session.json");
    let audio_out = client.begin(&mut editing, audio, save("/takes/a.wav"));
    let multitrack_out = client.begin(
        &mut editing,
        multitrack,
        json!({"verb": "save", "path": wrote}),
    );
    assert_eq!(audio_out["done"], false, "{audio_out}");
    assert_eq!(multitrack_out["done"], false, "{multitrack_out}");

    #[cfg(feature = "notation")]
    let (score, score_at) = {
        use clausters_core::notation::{AnyEngraver, Score, sheet_to_mei, voice_to_sheet};
        let mei = sheet_to_mei(&voice_to_sheet(&[], "4/4", "G2", "C")).unwrap();
        let engraver = AnyEngraver::new(crate::editing::tests::Kept(Mutex::new(String::new())));
        let score = Arc::new(Mutex::new(Score::open(engraver, &mei).unwrap()));
        let opened: Value =
            serde_json::from_str(&editing.open_score("score:one", score.clone(), "{}")).unwrap();
        let member = opened["member"].as_u64().unwrap() as MemberId;
        client.run(&mut editing, member, json!({"verb": "open"}));
        let turned = json!({"verb": "act", "call": {"action": "page", "landscape": true}});
        client.run(&mut editing, member, turned);
        let at = folder.join("score.mei");
        let out = client.begin(&mut editing, member, json!({"verb": "save", "path": at}));
        assert_eq!(out["done"], false, "{out}");
        // the score's fails, first
        let failed = client.answer(&mut editing, member, &out, json!({"failed": "disk full"}));
        assert!(failed["error"].is_object(), "{failed}");
        (member, at)
    };

    let landed = client.answer(&mut editing, multitrack, &multitrack_out, json!({}));
    assert_eq!(landed["result"]["destination"], json!(wrote), "{landed}");
    let landed = client.finish(&mut editing, audio, audio_out);
    assert!(landed["error"].is_null(), "{landed}");

    assert_eq!(binding(&mut editing, audio)["dirty"], false);
    assert_eq!(editing.query(multitrack, "file")["path"], json!(wrote));
    assert_eq!(editing.query(multitrack, "file")["dirty"], false);
    #[cfg(feature = "notation")]
    {
        let file = editing.query(score, "file");
        assert_eq!(file["path"], Value::Null, "{score_at:?}: {file}");
        assert_eq!(file["dirty"], true, "{file}");
    }

    // each order walks only its own
    let version = editing.version();
    let walked = client.run(
        &mut editing,
        audio,
        json!({"verb": "event", "addr": "/gui_event", "window": true,
            "args": [900, 2, version, "undo"]}),
    );
    assert!(walked["error"].is_null(), "{walked}");
    assert_eq!(binding(&mut editing, audio)["dirty"], true);
    assert_eq!(editing.query(multitrack, "file")["dirty"], false);
    let state = |client: &mut Client, editing: &mut Editing, member: MemberId| {
        client.call(editing, json!({"verb": "state", "member": member}))["canUndo"].clone()
    };
    #[cfg(feature = "notation")]
    {
        assert_eq!(state(&mut client, &mut editing, score), true);
        client.run(&mut editing, score, json!({"verb": "undo"}));
        assert_eq!(state(&mut client, &mut editing, score), false);
        assert_eq!(editing.query(score, "file")["dirty"], false);
    }
    assert_eq!(binding(&mut editing, audio)["dirty"], true);
    assert_eq!(state(&mut client, &mut editing, multitrack), true);
    std::fs::remove_dir_all(&folder).unwrap();
}

/// **The close form's Save goes down at the answer**, as the other
/// applications' does: the write runs with the window usable, a failure keeps
/// the window with its work, and the form can be asked for again. An edit made
/// meanwhile is work the save does not hold, and the window stays.
#[test]
fn a_close_form_answered_with_save_is_down_while_the_file_is_written() {
    let mut editing = Editing::new();
    let mut client = Client::new();
    let (member, ids) = asking(&mut client, &mut editing);
    let action = cut(1, &editing);
    client.run(&mut editing, member, action);
    let form_down = |client: &Client| {
        client.effects.iter().any(|e| {
            e["kind"] == "echo"
                && e["answer"]
                    .to_string()
                    .contains(&format!(r#""widget":{}"#, ids["stack"]))
                && e["answer"].to_string().contains(r#""index":0"#)
        })
    };
    let action = verb_of_window(2, &editing, "close");
    client.run(&mut editing, member, action);
    client.effects.clear();
    let action = answered(3, &editing, &ids, "ok");
    let first = client.begin(&mut editing, member, action);
    assert!(form_down(&client), "{:?}", client.effects);
    let write = until(&mut client, &mut editing, member, first, "batch", 1);
    assert!(!closes(&client));
    let failed = client.answer(&mut editing, member, &write, json!({"failed": "disk full"}));
    assert_eq!(failed["error"]["message"], "disk full", "{failed}");
    assert!(!closes(&client), "{:?}", client.effects);
    assert_eq!(binding(&mut editing, member)["dirty"], true);

    // the form taken down by Cancel this time, a save elsewhere
    let action = verb_of_window(4, &editing, "close");
    client.run(&mut editing, member, action);
    let action = answered(5, &editing, &ids, "cancel");
    client.run(&mut editing, member, action);
    let first = client.begin(&mut editing, member, save("/takes/a.wav"));
    client.finish(&mut editing, member, first);

    // writing elsewhere, an edit reaches the window while it is written
    let action = cut(6, &editing);
    client.run(&mut editing, member, action);
    let action = verb_of_window(7, &editing, "close");
    client.run(&mut editing, member, action);
    let action = answered(8, &editing, &ids, "ok");
    let first = client.begin(&mut editing, member, action);
    let write = until(&mut client, &mut editing, member, first, "batch", 1);
    let action = cut(9, &editing);
    assert_eq!(client.run(&mut editing, member, action)["result"], true);
    client.effects.clear();
    let saved = client.finish(&mut editing, member, write);
    assert!(saved["error"].is_null(), "{saved}");
    assert!(!closes(&client), "{:?}", client.effects);
    assert_eq!(binding(&mut editing, member)["dirty"], true);
    assert!(
        client
            .effects
            .iter()
            .any(|e| e["answer"].to_string().contains("it stays open")),
        "{:?}",
        client.effects
    );
}

/// **A refused operation corrects its window**: what the abort read back reaches
/// the gesture's answer, beside why, so the picture returns to the take.
#[test]
fn a_refused_edit_s_correction_reaches_the_window() {
    let mut editing = Editing::new();
    let mut client = Client::new();
    let member = opened(&mut client, &mut editing, false);
    let action = reverse(1, &editing);
    let started = client.begin(&mut editing, member, action);
    let batch = until(&mut client, &mut editing, member, started, "batch", 1);
    client.effects.clear();
    let failed = client.answer(&mut editing, member, &batch, json!({"failed": "no space"}));
    assert_eq!(failed["error"]["message"], "no space", "{failed}");
    let echo = client
        .effects
        .iter()
        .find(|e| e["kind"] == "echo")
        .expect("the gesture is answered");
    assert_eq!(echo["answer"]["answer"], "push", "{echo}");
    assert_eq!(echo["answer"]["reason"], "no space", "{echo}");
    assert!(
        echo["answer"]["corrections"]
            .as_array()
            .is_some_and(|c| !c.is_empty())
    );
}
