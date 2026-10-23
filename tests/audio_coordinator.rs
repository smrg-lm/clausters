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
//! The take is a file of a mono ramp, so every edit leaves samples that say
//! which frames are where: a cut, a paste, a mix, their undo and redo, a save
//! read back from the WAV it wrote, a write the disk refuses, and strokes
//! whose renders go when no entry names them. What is heard is read from the
//! files the list names, as the engine streams them.

#![cfg(feature = "synth")]

use std::collections::VecDeque;

use clausters::rosc::{OscPacket, OscType};
use clausters::server::nrtsession::{NrtSession, SessionConfig};
use clausters::server::render::{OscScore, RenderConfig, render_discarding, render_to_wav};
use clausters_apps::editing::{Editing, call_json_ids};
use clausters_core::ids::{IdShare, IdSpaces, ServerShape};
use clausters_editing::apply::{Step, steps_from_json};
use clausters_editing::run::{Reply, Runner, Server};
use serde_json::{Value, json};

const SR: f64 = 48_000.0;
const FRAMES: usize = 100;
/// The join the window plays.
const DISPLAY: i64 = 1;
/// The window's picture, as the editor's `window` verb is told.
const PICTURE: i64 = 10;
const WINDOW: i64 = 900;

/// Sample `i` of the take: a ramp, so a frame says where it came from.
fn ramp(i: usize) -> f32 {
    i as f32 / FRAMES as f32
}

/// **A client of the coordinator over a real server**: what a language
/// adapter does, in Rust -- the steps carried out on the server, the renders
/// and the moves on the disk, a failure reported as the run's result, and
/// the renders nothing names deleted.
struct Client {
    server: NrtSession,
    /// The file the take is opened from, and the editor's working folder.
    take: String,
    work: std::path::PathBuf,
    editing: Editing,
    ids: IdSpaces,
    member: Value,
    /// The clipboard document the host would hold.
    clipboard: Option<String>,
    /// The writes the server will refuse, by path.
    refuse: Vec<String>,
}

impl Client {
    fn new(directory: &std::path::Path) -> Client {
        let server = NrtSession::open(&SessionConfig {
            sample_rate: SR,
            channels: 2,
            ..Default::default()
        })
        .expect("open");
        let take = directory.join("ramp.wav").to_string_lossy().into_owned();
        let mut w = clausters::dsp::pcm::Writer::create(&take, 1, SR, "float").unwrap();
        w.write(&(0..FRAMES).map(ramp).collect::<Vec<_>>()).unwrap();
        w.finalize().unwrap();
        let work = directory.join("work");
        std::fs::create_dir(&work).unwrap();
        Client {
            server,
            take,
            work,
            editing: Editing::new(),
            ids: IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE),
            member: Value::Null,
            clipboard: None,
            refuse: Vec::new(),
        }
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

    /// Open the take as a window would.
    fn open(&mut self) {
        let (take, work) = (self.take.clone(), self.work.to_str().unwrap().to_string());
        self.member = self.call(json!({"verb": "openAudio", "key": "take", "path": take,
            "workspace": work, "frames": FRAMES, "channels": 1, "rate": SR,
            "display": DISPLAY}))["member"]
            .clone();
        self.member(json!({"verb": "sync", "window": WINDOW, "rate": SR}));
        self.member(json!({"verb": "window", "widget": PICTURE}));
        let opened = self.run(json!({"verb": "open", "chunk": 4096, "rate": SR,
            "empty": false, "window": true}));
        assert!(opened["error"].is_null(), "{opened}");
    }

    /// **One action, carried out to its end**, as `AudioEditor._audio` does.
    fn run(&mut self, action: Value) -> Value {
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
                    for file in freed["files"].as_array().into_iter().flatten() {
                        let _ = std::fs::remove_file(file.as_str().unwrap());
                    }
                }
            }
            "delete" => {
                for file in effect["files"].as_array().into_iter().flatten() {
                    let _ = std::fs::remove_file(file.as_str().unwrap());
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
                message(&mut self.server, &sent.addr, sent.args);
            }
            // a render or a move is the endpoint's, on the disk
            match runner.awaiting() {
                Some((_, Step::Render(render))) => {
                    let render = render.clone();
                    if self.refuse.contains(&render.path) {
                        // what a full disk answers the write
                        return Err("no space left on device".into());
                    }
                    let score = OscScore::from_bytes(&render.score).unwrap();
                    let cfg = RenderConfig {
                        sample_rate: render.rate,
                        channels: render.channels,
                        ..RenderConfig::default()
                    };
                    let done = if render.path.is_empty() {
                        render_discarding(&score, &cfg, |_, _| {}).map(|_| ())
                    } else {
                        render_to_wav(&score, &cfg, &render.path, &render.format).map(|_| ())
                    };
                    runner.rendered(done.err().map(|e| e.to_string()));
                    continue;
                }
                Some((_, Step::Move { from, to })) => {
                    let done = std::fs::rename(from, to);
                    runner.rendered(done.err().map(|e| e.to_string()));
                    continue;
                }
                _ => {}
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

    /// The take as the edits left it: the list the window draws, read from
    /// the files it names.
    fn heard(&mut self) -> Vec<f32> {
        let list = self.member(json!({"verb": "segments"}));
        let mut heard = Vec::new();
        for part in list["parts"].as_array().unwrap() {
            let path = part["path"].as_str().unwrap();
            let layout = clausters::dsp::pcm::Layout::open(path).unwrap();
            let mut file = std::fs::File::open(path).unwrap();
            let (mut bytes, mut out) = (Vec::new(), Vec::new());
            clausters_core::pcm::read_frames(
                &mut file,
                &layout,
                part["start"].as_u64().unwrap(),
                part["frames"].as_u64().unwrap() as usize,
                &mut bytes,
                &mut out,
            )
            .unwrap();
            heard.extend(out);
        }
        heard
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
        self.gesture(seq, tag, &[json!(at), json!("segments"), json!(document)])
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
    let mut client = Client::new(&directory);
    client.open();
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
    let opened: Vec<f32> = hound::WavReader::open(&client.take)
        .unwrap()
        .samples::<f32>()
        .map(Result::unwrap)
        .collect();
    assert_close(
        &opened,
        &original,
        "the file it was opened from is never written",
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
    let mut client = Client::new(&directory);
    client.open();
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
        binding["file"]["path"],
        client.take.as_str(),
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

/// **A stroke is a render, and it goes when no entry names it**: three
/// strokes are three files of the working folder, an undo and a redo name
/// them again, and an edit after an undo lets the stroke it replaced go.
#[test]
fn strokes_are_renders_and_one_no_entry_names_is_deleted() {
    let directory = scratch("strokes");
    let mut client = Client::new(&directory);
    client.open();
    let stroke = |client: &mut Client, seq: i64, value: f64| {
        let drawn = client.gesture(
            seq,
            "draw",
            &[json!(0), json!(10), json!([value]), json!([0])],
        );
        assert!(drawn["error"].is_null(), "{drawn}");
    };
    let renders = |client: &Client| {
        std::fs::read_dir(&client.work)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|x| x == "w64")
            })
            .count()
    };
    stroke(&mut client, 1, 0.5);
    stroke(&mut client, 2, 0.25);
    stroke(&mut client, 3, 0.75);
    assert_eq!(renders(&client), 3, "a file each");
    let at = |client: &mut Client| client.heard()[10];
    assert_eq!(at(&mut client), 0.75);
    client.verb(4, "undo");
    assert_eq!(at(&mut client), 0.25);
    client.verb(5, "undo");
    assert_eq!(at(&mut client), 0.5);
    client.verb(6, "redo");
    assert_eq!(at(&mut client), 0.25, "the render, named again");
    stroke(&mut client, 7, 0.125);
    assert_eq!(at(&mut client), 0.125);
    assert_eq!(renders(&client), 3, "the redo's stroke went, this one came");
    client.verb(8, "undo");
    client.verb(9, "undo");
    client.verb(10, "undo");
    assert_close(
        &client.heard(),
        &(0..FRAMES).map(ramp).collect::<Vec<_>>(),
        "the take",
    );
    assert_eq!(client.pending_cleanup(), 0);
    std::fs::remove_dir_all(directory).unwrap();
}

/// **A copy keeps the render it names**: a span of a stroke copied, the
/// stroke undone and replaced, and its render is still there for the paste,
/// which hears it.
#[test]
fn a_render_the_clipboard_names_outlives_the_entry_that_made_it() {
    let directory = scratch("clipboard");
    let mut client = Client::new(&directory);
    client.open();
    let drawn = client.gesture(1, "draw", &[json!(0), json!(10), json!([0.5]), json!([0])]);
    assert!(drawn["error"].is_null(), "{drawn}");
    let copied = client.gesture(2, "copy", &[json!(10.0), json!(1.0)]);
    assert!(copied["error"].is_null(), "{copied}");
    let named = client.clipboard.clone().expect("a copy");
    assert!(
        named.contains("stroke"),
        "the copy names the stroke's render"
    );
    client.verb(3, "undo");
    let other = client.gesture(4, "draw", &[json!(0), json!(20), json!([0.25]), json!([0])]);
    assert!(other["error"].is_null(), "{other}");
    let strokes = std::fs::read_dir(&client.work)
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with("-stroke.w64")
        })
        .count();
    assert_eq!(strokes, 2, "the undone stroke is kept for the clipboard");
    client.paste(5, "paste", 0.0);
    assert_eq!(client.heard()[0], 0.5, "the copy, heard");
    std::fs::remove_dir_all(directory).unwrap();
}

/// **Two renders over two files at once, landing in either order**: a
/// stroke in each of two editors stops on its render, both wait together,
/// the second lands first, and each file hears its own stroke.
#[test]
fn two_renders_over_two_files_land_in_either_order() {
    let directory = scratch("two-renders");
    let mut client = Client::new(&directory);
    client.open();
    let first = client.member.clone();
    // a second editor, over a file of its own
    let other = directory.join("other.wav").to_string_lossy().into_owned();
    std::fs::copy(&client.take, &other).unwrap();
    let work = directory.join("other-work");
    std::fs::create_dir(&work).unwrap();
    client.take = other;
    client.work = work;
    client.open();
    let second = client.member.clone();
    assert_ne!(first, second);

    // each stroke carried as far as its render, and held there
    let mut held = Vec::new();
    for (seq, (member, value)) in [(&first, 0.5), (&second, 0.25)].into_iter().enumerate() {
        client.member = member.clone();
        let args = vec![
            json!(PICTURE),
            json!(seq + 1),
            json!(client.editing.version()),
            json!("draw"),
            json!(0),
            json!(10),
            json!([value]),
            json!([0]),
        ];
        let mut answer = client.call(json!({"verb": "audio", "member": member, "do":
            {"verb": "event", "addr": "/gui_event", "args": args, "lease": false,
             "window": true}}));
        loop {
            for effect in answer["effects"].as_array().cloned().unwrap_or_default() {
                client.effect(&effect);
            }
            assert_ne!(
                answer["done"], true,
                "the stroke ended before its render: {answer}"
            );
            let renders = steps_from_json(&answer["await"]["steps"])
                .iter()
                .any(|step| matches!(step, Step::Render(_)));
            if answer["await"]["kind"] == "batch" && renders {
                break;
            }
            let result = client.wait(&answer["await"]);
            answer = client.call(json!({"verb": "audio", "member": member,
                "run": answer["run"], "result": result}));
        }
        held.push((member.clone(), answer));
    }

    // the second lands first, then the first
    for (member, mut answer) in held.into_iter().rev() {
        client.member = member.clone();
        loop {
            let result = client.wait(&answer["await"]);
            answer = client.call(json!({"verb": "audio", "member": member,
                "run": answer["run"], "result": result}));
            assert!(answer["run"].is_u64(), "{answer}");
            for effect in answer["effects"].as_array().cloned().unwrap_or_default() {
                client.effect(&effect);
            }
            if answer["done"] == true {
                assert!(answer["error"].is_null(), "{answer}");
                break;
            }
        }
    }
    for (member, value) in [(&first, 0.5), (&second, 0.25)] {
        client.member = member.clone();
        let heard = client.heard();
        assert_eq!(heard[10], value, "each file hears its own stroke");
        assert_eq!(heard[9], ramp(9));
        assert_eq!(client.binding()["dirty"], true);
    }
    assert_eq!(client.pending_cleanup(), 0);
    std::fs::remove_dir_all(directory).unwrap();
}
