//! **The audio editor driven through its coordinator, on a real server.**
//!
//! Both clients drive an audio editor through one door, the editing
//! context's `audio` verb: an action starts a run, the run answers effects
//! and stops on what only a client can do, and the client resumes it with
//! the result. This plays that client against an offline server -- every
//! step carried out, every `/done` and `/fail` the server's own -- and reads
//! the samples back, so what is checked is what is heard and what is written,
//! not the steps that were asked for.
//!
//! The take is a mono ramp, so every edit leaves samples that say which
//! frames are where: a cut, a paste, a mix, their undo and redo, a save read
//! back from the WAV it wrote, a write the server refuses, and a history
//! held on disk and read back for an undo.

#![cfg(feature = "synth")]

use std::collections::VecDeque;

use clausters::rosc::{OscPacket, OscType};
use clausters::server::nrtsession::{NrtSession, SessionConfig};
use clausters_apps::editing::{Editing, call_json_ids};
use clausters_core::ids::{IdShare, IdSpaces, ServerShape};
use clausters_editing::apply::steps_from_json;
use clausters_editing::run::{Reply, Runner, Server};
use serde_json::{Value, json};

const SR: f64 = 48_000.0;
const FRAMES: usize = 100;
/// The take's buffer, its join and the buffers handed over at open.
const TAKE: i64 = 0;
const DISPLAY: i64 = 1;
/// The window's picture, as the editor's `window` verb is told.
const PICTURE: i64 = 10;
const WINDOW: i64 = 900;

/// Sample `i` of the take: a ramp, so a frame says where it came from.
fn ramp(i: usize) -> f32 {
    i as f32 / FRAMES as f32
}

/// **A client of the coordinator over a real server**: what a language
/// adapter does, in Rust -- the steps carried out on the server, a failure
/// reported as the run's result, buffer numbers handed over, freed takes
/// released through their cleanup tickets, a spill and a history step run
/// as the operations they are.
struct Client {
    server: NrtSession,
    editing: Editing,
    ids: IdSpaces,
    member: Value,
    next: i64,
    /// The clipboard document the host would hold.
    clipboard: Option<String>,
    /// The writes the server will refuse, by path.
    refuse: Vec<String>,
}

impl Client {
    fn new() -> Client {
        let mut server = NrtSession::open(&SessionConfig {
            sample_rate: SR,
            channels: 2,
            ..Default::default()
        })
        .expect("open");
        let ramp: Vec<f32> = (0..FRAMES).map(ramp).collect();
        message(
            &mut server,
            "/buffer_alloc",
            vec![
                OscType::Int(TAKE as i32),
                OscType::Int(FRAMES as i32),
                OscType::Int(1),
            ],
        );
        server.settle_for(4);
        let blob: Vec<u8> = ramp.iter().flat_map(|v| v.to_le_bytes()).collect();
        message(
            &mut server,
            "/buffer_setRange",
            vec![
                OscType::Int(TAKE as i32),
                OscType::Int(0),
                OscType::Blob(blob),
            ],
        );
        server.settle_for(4);
        Client {
            server,
            editing: Editing::new(),
            ids: IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE),
            member: Value::Null,
            next: 100,
            clipboard: None,
            refuse: Vec::new(),
        }
    }

    fn buffer(&mut self) -> i64 {
        self.next += 1;
        self.next
    }

    fn call(&mut self, request: Value) -> Value {
        serde_json::from_str(&call_json_ids(
            &mut self.editing,
            &request.to_string(),
            &mut self.ids,
        ))
        .unwrap()
    }

    fn member(&mut self, call: Value) -> Value {
        let member = self.member.clone();
        self.call(json!({"verb": "member", "member": member, "call": call}))
    }

    /// Open the take as a window would, with the history kept on disk in
    /// `scratch` past `resident` bytes.
    fn open(&mut self, scratch: &std::path::Path, resident: Option<u64>) {
        let buffers: Vec<i64> = (0..4).map(|_| self.buffer()).collect();
        self.member = self.call(json!({"verb": "openAudio", "key": "take", "take": TAKE,
            "frames": FRAMES, "channels": 1, "rate": SR, "display": DISPLAY,
            "buffers": buffers, "path": "/takes/ramp.wav"}))["member"]
            .clone();
        self.member(json!({"verb": "sync", "window": WINDOW, "scratch": scratch.to_str()}));
        self.member(json!({"verb": "window", "widget": PICTURE}));
        if resident.is_some() {
            self.call(json!({"verb": "resident", "bytes": resident}));
        }
        let opened = self.run(json!({"verb": "open", "chunk": 4096, "rate": SR,
            "empty": false, "window": true}));
        assert!(opened["error"].is_null(), "{opened}");
    }

    /// **One action, carried out to its end**, as `AudioEditor._audio` does.
    fn run(&mut self, action: Value) -> Value {
        self.top_up();
        let member = self.member.clone();
        let mut answer = self.call(json!({"verb": "audio", "member": member, "do": action}));
        loop {
            assert!(answer["run"].is_u64(), "{answer}");
            for effect in answer["effects"].as_array().cloned().unwrap_or_default() {
                self.effect(&effect);
            }
            if answer["done"] == true {
                return answer;
            }
            let result = self.wait(&answer["await"]);
            answer = self.call(json!({"verb": "audio", "member": member,
                "run": answer["run"], "result": result}));
        }
    }

    fn top_up(&mut self) {
        let spare = self.member(json!({"verb": "sync"}))["spare"]
            .as_u64()
            .unwrap_or(0);
        if spare < 2 {
            let more: Vec<i64> = (spare..2).map(|_| self.buffer()).collect();
            self.member(json!({"verb": "sync", "buffers": more}));
        }
    }

    /// What a run waits on, done against the server.
    fn wait(&mut self, wait: &Value) -> Value {
        let done = |result: Result<(), String>| match result {
            Ok(()) => json!({}),
            Err(reason) => json!({"failed": reason}),
        };
        match wait["kind"].as_str().unwrap() {
            "steps" | "batch" => {
                let steps = wait["steps"].clone();
                done(self.steps(&steps))
            }
            "rolling" => json!({"rolling": false}),
            "stepped" => {
                let prepared = wait["stepped"]["operation"].clone();
                if prepared.is_object() {
                    self.run(json!({"verb": "operation", "prepared": prepared}));
                }
                json!({"changed": true})
            }
            "clipboard" => {
                if wait["ready"] != true {
                    let published = self.call(json!({"verb": "exchange", "action": "publish",
                        "document": wait["document"]}));
                    self.clipboard = published["document"].as_str().map(str::to_string);
                } else {
                    self.clipboard = wait["document"].as_str().map(str::to_string);
                }
                json!({})
            }
            "topUp" => {
                self.top_up();
                json!({})
            }
            other => panic!("this client cannot wait on {other}"),
        }
    }

    /// One effect: the ones that touch the server or the resources.
    fn effect(&mut self, effect: &Value) {
        match effect["kind"].as_str().unwrap() {
            "release" => {
                for freed in effect["freed"].as_array().into_iter().flatten() {
                    for buffer in freed["buffers"].as_array().into_iter().flatten() {
                        self.release(json!({"server": {"connection": 0, "generation": 0},
                            "buffer": buffer}));
                    }
                }
                for stored in effect["stored"].as_array().into_iter().flatten() {
                    // a spill the server refuses keeps the take resident
                    let prepared = stored["operation"].clone();
                    self.run(json!({"verb": "operation", "prepared": prepared}));
                }
            }
            "bufferFree" => {
                let buffer = effect["buffer"].as_i64().unwrap() as i32;
                message(&mut self.server, "/buffer_free", vec![OscType::Int(buffer)]);
                self.server.settle_for(4);
            }
            _ => {}
        }
    }

    /// A take nothing reaches, freed through its exact cleanup ticket.
    fn release(&mut self, resource: Value) {
        let release = self.call(json!({"verb": "cleanup", "action": "prepare",
            "resource": resource}));
        if !release["ticket"].is_object() {
            return;
        }
        let steps = release["steps"].clone();
        let success = self.steps(&steps).is_ok();
        self.call(json!({"verb": "cleanup", "action": "complete",
            "ticket": release["ticket"], "success": success}));
    }

    /// Carry `steps` out, waiting on each reply; a `/fail` ends them.
    fn steps(&mut self, steps: &Value) -> Result<(), String> {
        let mut runner = Runner::new();
        runner.push(Server::Sound, steps_from_json(steps));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut bytes = vec![0; 1 << 16];
        let mut ready = VecDeque::new();
        while !runner.is_idle() {
            ready.extend(runner.ready().into_iter().map(|(_, m)| m));
            while let Some(sent) = ready.pop_front() {
                let refused = sent.addr == "/buffer_write"
                    && matches!(sent.args.get(1), Some(OscType::String(path))
                        if self.refuse.contains(path));
                if refused {
                    // what a full disk answers the write
                    return Err("no space left on device".into());
                }
                message(&mut self.server, &sent.addr, sent.args);
            }
            self.server.settle_for(4);
            while let Some(len) = self.server.poll_into(&mut bytes) {
                if let Ok(OscPacket::Message(reply)) = clausters::osc::decode_packet(&bytes[..len])
                {
                    match runner.reply(Server::Sound, &reply) {
                        Reply::Refused(reason) => return Err(format!("{reason:?}")),
                        _ if reply.addr == "/fail" => return Err(format!("{:?}", reply.args)),
                        _ => {}
                    }
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "unsettled steps: {:?}",
                runner.awaiting()
            );
        }
        Ok(())
    }

    /// The join the window draws, read back: the take as the edits left it.
    fn heard(&mut self) -> Vec<f32> {
        let frames = self.member(json!({"verb": "parts"}))["frames"]
            .as_u64()
            .unwrap() as usize;
        samples(&mut self.server, DISPLAY as i32, frames)
    }

    fn binding(&mut self) -> Value {
        self.member(json!({"verb": "binding"}))
    }

    /// A gesture on the window's picture.
    fn gesture(&mut self, seq: i64, tag: &str, values: &[Value]) -> Value {
        let mut args = vec![
            json!(PICTURE),
            json!(seq),
            json!(self.editing.version()),
            json!(tag),
        ];
        args.extend(values.iter().cloned());
        self.deliver(args)
    }

    /// A message from the window, by its verb.
    fn verb(&mut self, seq: i64, verb: &str) -> Value {
        let args = vec![
            json!(WINDOW),
            json!(seq),
            json!(self.editing.version()),
            json!(verb),
        ];
        self.deliver(args)
    }

    /// One `/gui_event`, through the clipboard exchange first, as
    /// `AudioEditor._deliver` does.
    fn deliver(&mut self, args: Vec<Value>) -> Value {
        let prepared = self.call(json!({"verb": "exchange", "action": "event", "args": args}));
        let lease = prepared["lease"].clone();
        let answer = self.run(json!({"verb": "event", "addr": "/gui_event",
            "args": prepared["args"], "lease": !lease.is_null(), "window": true}));
        if !lease.is_null() {
            self.call(json!({"verb": "exchange", "action": "release", "lease": lease}));
        }
        answer
    }

    fn paste(&mut self, seq: i64, tag: &str, at: f64) -> Value {
        let document = self.clipboard.clone().expect("something was copied");
        self.gesture(seq, tag, &[json!(at), json!("buffer"), json!(document)])
    }

    fn save(&mut self, path: &str) -> Value {
        self.run(json!({"verb": "save", "request": {"path": path, "format": "float"}}))
    }

    fn pending_cleanup(&mut self) -> usize {
        self.call(json!({"verb": "cleanup", "action": "pending"}))["tickets"]
            .as_array()
            .map_or(0, Vec::len)
    }
}

fn message(server: &mut NrtSession, addr: &str, args: Vec<OscType>) {
    assert!(server.send_msg(addr, args).expect("encode"), "ring full");
}

fn samples(server: &mut NrtSession, buffer: i32, frames: usize) -> Vec<f32> {
    message(
        server,
        "/buffer_getRange",
        vec![
            OscType::Int(buffer),
            OscType::Int(0),
            OscType::Int(frames as i32),
        ],
    );
    server.settle_for(4);
    let mut bytes = vec![0; 1 << 16];
    while let Some(len) = server.poll_into(&mut bytes) {
        if let Ok(OscPacket::Message(reply)) = clausters::osc::decode_packet(&bytes[..len])
            && reply.addr == "/buffer_getRange.reply"
            && let Some(OscType::Blob(values)) = reply.args.last()
        {
            return values
                .as_chunks::<4>()
                .0
                .iter()
                .map(|v| f32::from_le_bytes(*v))
                .collect();
        }
    }
    panic!("no samples came back")
}

fn assert_close(heard: &[f32], expected: &[f32], what: &str) {
    assert_eq!(heard.len(), expected.len(), "{what}: length");
    for (i, (a, b)) in heard.iter().zip(expected).enumerate() {
        assert!((a - b).abs() < 1e-6, "{what}: frame {i} is {a}, not {b}");
    }
}

fn scratch(name: &str) -> std::path::PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "clausters-{name}-{}-{}",
        std::process::id(),
        clausters_core::rng::entropy_seed()
    ));
    std::fs::create_dir(&directory).unwrap();
    directory
}

/// The take with frames `start..start + len` taken out.
fn without(start: usize, len: usize) -> Vec<f32> {
    (0..FRAMES)
        .filter(|i| !(start..start + len).contains(i))
        .map(ramp)
        .collect()
}

/// **Cut, paste, mix, undo and redo, heard**: each edit's samples, the
/// history walking them back and forward, and a save read back from the
/// file it wrote.
#[test]
fn edited_samples_undo_redo_and_save_through_the_coordinator() {
    let directory = scratch("coordinator");
    let mut client = Client::new();
    client.open(&directory, None);
    let original: Vec<f32> = (0..FRAMES).map(ramp).collect();
    assert_close(&client.heard(), &original, "opened");

    // cut 20 frames from 10: the clipboard holds them, the take loses them
    let cut = client.gesture(1, "cut", &[json!(10.0), json!(20.0)]);
    assert!(cut["error"].is_null(), "{cut}");
    let after_cut = without(10, 20);
    assert_close(&client.heard(), &after_cut, "cut");
    assert_eq!(client.binding()["dirty"], true);

    // paste them back at frame 50 of what is left
    client.paste(2, "paste", 50.0);
    let mut pasted = after_cut.clone();
    let block: Vec<f32> = (10..30).map(ramp).collect();
    pasted.splice(50..50, block.iter().copied());
    assert_close(&client.heard(), &pasted, "paste");

    // mix them over frames 0..20
    client.paste(3, "mix", 0.0);
    let mut mixed = pasted.clone();
    for (i, v) in block.iter().enumerate() {
        mixed[i] += v;
    }
    assert_close(&client.heard(), &mixed, "mix");

    // undo walks the three back, redo forward again
    for expected in [&pasted, &after_cut, &original] {
        let undo = client.verb(4, "undo");
        assert!(undo["error"].is_null(), "{undo}");
        assert_close(&client.heard(), expected, "undo");
    }
    assert_eq!(client.binding()["dirty"], false, "back at what was opened");
    for expected in [&after_cut, &pasted, &mixed] {
        let redo = client.verb(5, "redo");
        assert!(redo["error"].is_null(), "{redo}");
        assert_close(&client.heard(), expected, "redo");
    }

    // a save writes what is heard, and the file holds it
    let path = directory.join("saved.wav");
    let saved = client.save(path.to_str().unwrap());
    assert!(saved["error"].is_null(), "{saved}");
    assert_eq!(client.binding()["dirty"], false);
    let written: Vec<f32> = hound::WavReader::open(&path)
        .unwrap()
        .samples::<f32>()
        .map(Result::unwrap)
        .collect();
    assert_close(&written, &mixed, "the saved file");
    assert_close(
        &samples(&mut client.server, TAKE as i32, FRAMES),
        &original,
        "the take it was opened over is never written",
    );
    assert_eq!(
        client.pending_cleanup(),
        0,
        "every release was acknowledged"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

/// **A write the disk refuses** keeps the file dirty and where it was; the
/// samples are untouched, the next save writes them, and an edit after the
/// save leaves it dirty until its undo brings back the written revision.
#[test]
fn a_refused_save_keeps_the_file_dirty_and_a_retry_writes_it() {
    let directory = scratch("refused");
    let mut client = Client::new();
    client.open(&directory, None);
    client.gesture(1, "cut", &[json!(10.0), json!(20.0)]);
    let path = directory.join("full.wav");
    let path = path.to_str().unwrap().to_string();
    client.refuse.push(path.clone());
    let refused = client.save(&path);
    assert!(
        refused["rethrow"] == true || refused["error"].is_object(),
        "the client raises the refusal: {refused}"
    );
    let binding = client.binding();
    assert_eq!(binding["dirty"], true);
    assert_eq!(
        binding["file"]["path"], "/takes/ramp.wav",
        "the target did not move"
    );
    assert!(!std::path::Path::new(&path).exists());
    assert_close(&client.heard(), &without(10, 20), "nothing was lost");

    client.refuse.clear();
    let saved = client.save(&path);
    assert!(saved["error"].is_null(), "{saved}");
    assert_eq!(client.binding()["dirty"], false);
    assert_eq!(client.binding()["file"]["path"], path.as_str());

    client.gesture(2, "cut", &[json!(0.0), json!(10.0)]);
    assert_eq!(client.binding()["dirty"], true);
    client.verb(3, "undo");
    assert_eq!(client.binding()["dirty"], false, "the written revision");
    assert_close(&client.heard(), &without(10, 20), "undo to the saved take");
    std::fs::remove_dir_all(directory).unwrap();
}

/// **The history on disk**: with nothing resident, every take only the
/// history holds is written out as the next edit lands, and an undo reads
/// it back before the join is stitched over it.
#[test]
fn a_history_on_disk_is_read_back_for_its_undo_and_redo() {
    let directory = scratch("spill");
    let mut client = Client::new();
    client.open(&directory, Some(0));
    let stroke = |client: &mut Client, seq: i64, value: f64| {
        let drawn = client.gesture(
            seq,
            "draw",
            &[json!(0), json!(10), json!([value]), json!([0])],
        );
        assert!(drawn["error"].is_null(), "{drawn}");
    };
    stroke(&mut client, 1, 0.5);
    stroke(&mut client, 2, 0.25);
    stroke(&mut client, 3, 0.75);
    let spilled = std::fs::read_dir(&directory).unwrap().count();
    assert!(spilled > 0, "the history went to disk");
    let at = |client: &mut Client| client.heard()[10];
    assert_eq!(at(&mut client), 0.75);
    client.verb(4, "undo");
    assert_eq!(at(&mut client), 0.25, "read back from disk");
    client.verb(5, "undo");
    assert_eq!(at(&mut client), 0.5);
    client.verb(6, "undo");
    assert_close(
        &client.heard(),
        &(0..FRAMES).map(ramp).collect::<Vec<_>>(),
        "the take",
    );
    client.verb(7, "redo");
    client.verb(8, "redo");
    assert_eq!(at(&mut client), 0.25, "redone from disk");
    assert_eq!(client.pending_cleanup(), 0);
    std::fs::remove_dir_all(directory).unwrap();
}
