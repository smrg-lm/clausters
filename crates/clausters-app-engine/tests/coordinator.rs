//! **The audio editor driven through its application, heard**: an editing
//! context and the application that holds its engine, as the GUI host runs
//! them, with every render carried out on the application's own threads and
//! every file it writes read back.
//!
//! The take is a file of a mono ramp, so every edit leaves samples that say
//! which frames are where: a cut, a paste, a mix, their undo and redo, a save
//! read back from the WAV it wrote, a write the disk refuses, and strokes
//! whose renders go when no entry names them. What is heard is read from the
//! files the list names, as the engine's reader reads them.

#![cfg(feature = "synth")]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clausters_app_engine::AppEngine;
use clausters_app_engine::app::{App, Ran};
use clausters_apps::editing::{Editing, MemberId};
use serde_json::{Value, json};

const SR: f64 = 48_000.0;
const FRAMES: usize = 100;
/// The window's picture, as the editor's `window` verb is told.
const PICTURE: i64 = 10;
const WINDOW: i64 = 900;

/// Sample `i` of the take: a ramp, so a frame says where it came from.
fn ramp(i: usize) -> f32 {
    i as f32 / FRAMES as f32
}

/// **The application and its context**, as the GUI host's application
/// thread holds them, with what the host would keep: the clipboard's
/// document.
struct Client {
    app: App,
    editing: Editing,
    /// The file the take is opened from, and the editor's working folder.
    take: String,
    work: std::path::PathBuf,
    member: MemberId,
    /// The clipboard document the host would hold.
    clipboard: Option<String>,
    next_token: u64,
    /// The ends that landed before they were waited for, by token.
    ended: std::collections::HashMap<u64, Result<Value, String>>,
}

impl Client {
    fn new(directory: &std::path::Path) -> Client {
        let take = directory.join("ramp.wav").to_string_lossy().into_owned();
        write(&take, &(0..FRAMES).map(ramp).collect::<Vec<_>>());
        let work = directory.join("work");
        std::fs::create_dir(&work).unwrap();
        let (engine, _processed) = AppEngine::detached(SR as f32, 2);
        let app = App::new(Arc::new(Mutex::new(engine)));
        let mut editing = Editing::new();
        app.hold(&mut editing);
        Client {
            app,
            editing,
            take,
            work,
            member: 0,
            clipboard: None,
            next_token: 1,
            ended: Default::default(),
        }
    }

    fn call(&mut self, request: Value) -> Value {
        self.app.call(&mut self.editing, &request)
    }

    fn member(&mut self, call: Value) -> Value {
        let member = self.member;
        self.call(json!({"verb": "member", "member": member, "call": call}))
    }

    /// Open the take as a window would.
    fn open(&mut self) {
        let (take, work) = (self.take.clone(), self.work.to_str().unwrap().to_string());
        let opened = self.call(json!({"verb": "openAudio", "key": take, "path": take,
            "workspace": work, "frames": FRAMES, "channels": 1, "rate": SR}));
        self.member = opened["member"].as_u64().expect("opened") as MemberId;
        self.member(json!({"verb": "sync", "window": WINDOW}));
        self.member(json!({"verb": "attach_view"}));
        self.member(json!({"verb": "window", "widget": PICTURE}));
        let opened = self.run(
            json!({"verb": "open", "empty": false, "window": true}),
            None,
        );
        assert!(opened.is_ok(), "{opened:?}");
    }

    /// **Starts an action** and answers its token: what it said so far is
    /// taken in, and it may still be waiting on a render.
    fn start(&mut self, action: Value, lease: Option<u64>) -> (u64, Ran) {
        let token = self.next_token;
        self.next_token += 1;
        let member = self.member;
        let ran = self
            .app
            .act(&mut self.editing, member, action, token, lease);
        self.take_in(&ran);
        (token, ran)
    }

    /// **The end of the action started under `token`**, the renders it
    /// handed to the application's threads waited for.
    fn end(&mut self, token: u64) -> Result<Value, String> {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(end) = self.ended.remove(&token) {
                return end;
            }
            for ran in self.app.landed(&mut self.editing) {
                self.take_in(&ran);
                if let Some(end) = ran.end {
                    self.ended
                        .insert(ran.token, end.map_err(|raised| raised.message));
                }
            }
            assert!(Instant::now() < deadline, "the action never ended");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// **One action, carried out to its end.**
    fn run(&mut self, action: Value, lease: Option<u64>) -> Result<Value, String> {
        let (token, ran) = self.start(action, lease);
        match ran.end {
            Some(end) => end.map_err(|raised| raised.message),
            None => self.end(token),
        }
    }

    /// What a run said that the host keeps: the clipboard's document.
    fn take_in(&mut self, ran: &Ran) {
        for effect in &ran.effects {
            if effect["kind"] == "clipboard" {
                self.clipboard = effect["document"].as_str().map(str::to_string);
            }
        }
    }

    /// The take as the edits left it: the list the window draws, read from
    /// the files it names.
    fn heard(&mut self) -> Vec<f32> {
        let list = self.member(json!({"verb": "segments"}));
        let mut heard = Vec::new();
        for part in list["parts"].as_array().unwrap() {
            let all = read(part["path"].as_str().unwrap());
            let start = part["start"].as_u64().unwrap() as usize;
            let frames = part["frames"].as_u64().unwrap() as usize;
            heard.extend_from_slice(&all[start..start + frames]);
        }
        heard
    }

    fn binding(&mut self) -> Value {
        self.member(json!({"verb": "binding"}))
    }

    /// A gesture on the window's picture.
    fn gesture(&mut self, seq: i64, tag: &str, values: &[Value]) -> Result<Value, String> {
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
    fn verb(&mut self, seq: i64, verb: &str) -> Result<Value, String> {
        let args = vec![
            json!(WINDOW),
            json!(seq),
            json!(self.editing.version()),
            json!(verb),
        ];
        self.deliver(args)
    }

    /// **The action of one `/gui_event`**, its clipboard lease taken first,
    /// as the host's application thread does.
    fn event(&mut self, args: Vec<Value>) -> (Value, Option<u64>) {
        let (args, lease) = self.editing.exchange_event(args, Default::default());
        let action = json!({"verb": "event", "addr": "/gui_event",
            "args": args, "window": true});
        (action, lease)
    }

    fn deliver(&mut self, args: Vec<Value>) -> Result<Value, String> {
        let (action, lease) = self.event(args);
        self.run(action, lease)
    }

    fn paste(&mut self, seq: i64, tag: &str, at: f64) -> Result<Value, String> {
        let document = self.clipboard.clone().expect("something was copied");
        self.gesture(seq, tag, &[json!(at), json!("segments"), json!(document)])
    }

    fn save(&mut self, path: &str) -> Result<Value, String> {
        self.run(
            json!({"verb": "save", "request": {"path": path, "format": "float"}}),
            None,
        )
    }

    fn pending_cleanup(&mut self) -> usize {
        self.call(json!({"verb": "cleanup", "action": "pending"}))["tickets"]
            .as_array()
            .map_or(0, Vec::len)
    }

    /// How many renders of `what` the working folder holds.
    fn renders(&self, what: &str) -> usize {
        std::fs::read_dir(&self.work)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(&format!("{what}.w64"))
            })
            .count()
    }
}

/// A mono float WAV of `samples`.
fn write(path: &str, samples: &[f32]) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SR as u32,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for &s in samples {
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
}

/// Every sample of the mono file at `path`, whatever its container.
fn read(path: &str) -> Vec<f32> {
    let layout = clausters_core::pcm::Layout::open(path).unwrap();
    let mut file = std::fs::File::open(path).unwrap();
    let (mut bytes, mut out) = (Vec::new(), Vec::new());
    clausters_core::pcm::read_frames(
        &mut file,
        &layout,
        0,
        layout.frames as usize,
        &mut bytes,
        &mut out,
    )
    .unwrap();
    out
}

fn assert_close(heard: &[f32], expected: &[f32], what: &str) {
    assert_eq!(heard.len(), expected.len(), "{what}: length");
    for (i, (a, b)) in heard.iter().zip(expected).enumerate() {
        assert!((a - b).abs() < 1e-6, "{what}: frame {i} is {a}, not {b}");
    }
}

fn scratch(name: &str) -> std::path::PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "clausters-app-{name}-{}-{}",
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

/// A stroke of one sample, `value`, at frame 10.
fn stroke(client: &mut Client, seq: i64, value: f64) {
    let drawn = client.gesture(
        seq,
        "draw",
        &[json!(0), json!(10), json!([value]), json!([0])],
    );
    assert!(drawn.is_ok(), "{drawn:?}");
}

/// **Cut, paste, mix, undo and redo, heard**: each edit's samples, the
/// history walking them back and forward, and a save read back from the
/// file it wrote.
#[test]
fn edited_samples_undo_redo_and_save_through_the_application() {
    let directory = scratch("coordinator");
    let mut client = Client::new(&directory);
    client.open();
    let original: Vec<f32> = (0..FRAMES).map(ramp).collect();
    assert_close(&client.heard(), &original, "opened");

    // cut 20 frames from 10: the clipboard holds them, the take loses them
    let cut = client.gesture(1, "copy_cut", &[json!(10.0), json!(20.0)]);
    assert!(cut.is_ok(), "{cut:?}");
    let after_cut = without(10, 20);
    assert_close(&client.heard(), &after_cut, "cut");
    assert_eq!(client.binding()["dirty"], true);

    // paste them back at frame 50 of what is left
    client.paste(2, "paste", 50.0).unwrap();
    let mut pasted = after_cut.clone();
    let block: Vec<f32> = (10..30).map(ramp).collect();
    pasted.splice(50..50, block.iter().copied());
    assert_close(&client.heard(), &pasted, "paste");

    // mix them over frames 0..20
    client.paste(3, "mix", 0.0).unwrap();
    let mut mixed = pasted.clone();
    for (i, v) in block.iter().enumerate() {
        mixed[i] += v;
    }
    assert_close(&client.heard(), &mixed, "mix");

    // undo walks the three back, redo forward again
    for expected in [&pasted, &after_cut, &original] {
        let undo = client.verb(4, "undo");
        assert!(undo.is_ok(), "{undo:?}");
        assert_close(&client.heard(), expected, "undo");
    }
    assert_eq!(client.binding()["dirty"], false, "back at what was opened");
    for expected in [&after_cut, &pasted, &mixed] {
        let redo = client.verb(5, "redo");
        assert!(redo.is_ok(), "{redo:?}");
        assert_close(&client.heard(), expected, "redo");
    }

    // a save writes what is heard, and the file holds it
    let path = directory.join("saved.wav");
    let saved = client.save(path.to_str().unwrap());
    assert!(saved.is_ok(), "{saved:?}");
    assert_eq!(client.binding()["dirty"], false);
    assert_close(&read(path.to_str().unwrap()), &mixed, "the saved file");
    let take = client.take.clone();
    assert_close(
        &read(&take),
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
    client
        .gesture(1, "delete", &[json!(10.0), json!(20.0)])
        .unwrap();
    // a folder that is not there refuses the write
    let folder = directory.join("not-yet");
    let path = folder.join("full.wav").to_str().unwrap().to_string();
    assert!(client.save(&path).is_err(), "the refusal is raised");
    let binding = client.binding();
    assert_eq!(binding["dirty"], true);
    assert_eq!(
        binding["file"]["path"],
        client.take.as_str(),
        "the target did not move"
    );
    assert!(!std::path::Path::new(&path).exists());
    assert_close(&client.heard(), &without(10, 20), "nothing was lost");

    std::fs::create_dir(&folder).unwrap();
    let saved = client.save(&path);
    assert!(saved.is_ok(), "{saved:?}");
    assert_eq!(client.binding()["dirty"], false);
    assert_eq!(client.binding()["file"]["path"], path.as_str());

    client
        .gesture(2, "delete", &[json!(0.0), json!(10.0)])
        .unwrap();
    assert_eq!(client.binding()["dirty"], true);
    client.verb(3, "undo").unwrap();
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
    stroke(&mut client, 1, 0.5);
    stroke(&mut client, 2, 0.25);
    stroke(&mut client, 3, 0.75);
    assert_eq!(client.renders("stroke"), 3, "a file each");
    let at = |client: &mut Client| client.heard()[10];
    assert_eq!(at(&mut client), 0.75);
    client.verb(4, "undo").unwrap();
    assert_eq!(at(&mut client), 0.25);
    client.verb(5, "undo").unwrap();
    assert_eq!(at(&mut client), 0.5);
    client.verb(6, "redo").unwrap();
    assert_eq!(at(&mut client), 0.25, "the render, named again");
    stroke(&mut client, 7, 0.125);
    assert_eq!(at(&mut client), 0.125);
    assert_eq!(
        client.renders("stroke"),
        3,
        "the redo's stroke went, this one came"
    );
    for seq in 8..11 {
        client.verb(seq, "undo").unwrap();
    }
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
    stroke(&mut client, 1, 0.5);
    let copied = client.gesture(2, "copy", &[json!(10.0), json!(1.0)]);
    assert!(copied.is_ok(), "{copied:?}");
    assert!(client.clipboard.is_some(), "a copy");
    client.verb(3, "undo").unwrap();
    let other = client.gesture(4, "draw", &[json!(0), json!(20), json!([0.25]), json!([0])]);
    assert!(other.is_ok(), "{other:?}");
    assert_eq!(
        client.renders("stroke"),
        2,
        "the undone stroke is kept for the clipboard"
    );
    client.paste(5, "paste", 0.0).unwrap();
    assert_eq!(client.heard()[0], 0.5, "the copy, heard");
    std::fs::remove_dir_all(directory).unwrap();
}

/// **Two renders over two files at once, landing in either order**: a
/// stroke in each of two editors hands its render to a thread of its own,
/// both run together, and each file hears its own stroke.
#[test]
fn two_renders_over_two_files_land_in_either_order() {
    let directory = scratch("two-renders");
    let mut client = Client::new(&directory);
    client.open();
    let first = client.member;
    // a second editor, over a file of its own
    let other = directory.join("other.wav").to_string_lossy().into_owned();
    std::fs::copy(&client.take, &other).unwrap();
    let work = directory.join("other-work");
    std::fs::create_dir(&work).unwrap();
    client.take = other;
    client.work = work;
    client.open();
    let second = client.member;
    assert_ne!(first, second);

    // each stroke handed to its render, neither waited for yet
    let mut held = Vec::new();
    for (seq, (member, value)) in [(first, 0.5), (second, 0.25)].into_iter().enumerate() {
        client.member = member;
        let version = client.editing.version();
        let (action, lease) = client.event(vec![
            json!(PICTURE),
            json!(seq + 1),
            json!(version),
            json!("draw"),
            json!(0),
            json!(10),
            json!([value]),
            json!([0]),
        ]);
        let (token, ran) = client.start(action, lease);
        assert!(ran.end.is_none(), "the stroke waits on its render");
        held.push((member, token));
    }
    for (member, token) in held {
        client.member = member;
        let ended = client.end(token);
        assert!(ended.is_ok(), "{ended:?}");
    }
    for (member, value) in [(first, 0.5), (second, 0.25)] {
        client.member = member;
        let heard = client.heard();
        assert_eq!(heard[10], value, "each file hears its own stroke");
        assert_eq!(heard[9], ramp(9));
        assert_eq!(client.binding()["dirty"], true);
    }
    assert_eq!(client.pending_cleanup(), 0);
    std::fs::remove_dir_all(directory).unwrap();
}

/// **A stereo copy pasted into a mono take is mixed**, rendered by the
/// application's threads: each frame of what is put in is the average of
/// the two channels it was copied from.
#[test]
fn a_stereo_copy_pasted_into_a_mono_take_is_its_average() {
    use clausters_document::clipboard::{Clipboard, SegmentPart};
    let directory = scratch("fitted");
    let mut client = Client::new(&directory);
    client.open();
    let stereo = directory.join("stereo.wav").to_string_lossy().into_owned();
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SR as u32,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(&stereo, spec).unwrap();
    for i in 0..FRAMES {
        w.write_sample(ramp(i)).unwrap();
        w.write_sample(0.5f32).unwrap();
    }
    w.finalize().unwrap();
    let part = SegmentPart {
        path: stereo,
        start: 0,
        frames: 10,
        fade_in: 0,
        fade_out: 0,
        fade_in_shape: Default::default(),
        fade_out_shape: Default::default(),
        channels: None,
        source_channels: 2,
        source_frames: FRAMES as u64,
        source_rate: SR,
    };
    client.clipboard = Some(Clipboard::segments(2, SR, vec![part]).to_json());
    client.paste(1, "paste", 50.0).unwrap();
    let mut expected: Vec<f32> = (0..50).map(ramp).collect();
    expected.extend((0..10).map(|i| (ramp(i) + 0.5) / 2.0));
    expected.extend((50..FRAMES).map(ramp));
    assert_close(&client.heard(), &expected, "mixed in");
    let _ = std::fs::remove_dir_all(&directory);
}

/// A mono 16-bit AIFF of `samples`: a container a part does not read in
/// place, decoded the way a compressed file is.
fn write_aiff(path: &str, samples: &[f32]) {
    let data: Vec<u8> = samples
        .iter()
        .flat_map(|&x| ((x * 32767.0).round() as i16).to_be_bytes())
        .collect();
    let mut comm = Vec::new();
    comm.extend_from_slice(&1u16.to_be_bytes());
    comm.extend_from_slice(&(samples.len() as u32).to_be_bytes());
    comm.extend_from_slice(&16u16.to_be_bytes());
    // 48000 as an 80-bit extended float
    comm.extend_from_slice(&[0x40, 0x0E, 0xBB, 0x80, 0, 0, 0, 0, 0, 0]);
    let mut body = b"AIFF".to_vec();
    body.extend_from_slice(b"COMM");
    body.extend_from_slice(&(comm.len() as u32).to_be_bytes());
    body.extend_from_slice(&comm);
    body.extend_from_slice(b"SSND");
    body.extend_from_slice(&(data.len() as u32 + 8).to_be_bytes());
    body.extend_from_slice(&[0; 8]);
    body.extend_from_slice(&data);
    let mut file = b"FORM".to_vec();
    file.extend_from_slice(&(body.len() as u32).to_be_bytes());
    file.extend_from_slice(&body);
    std::fs::write(path, file).unwrap();
}

/// **A file a part cannot read in place is decoded when it opens**, by the
/// application's threads into its working folder, and the take is heard
/// from the decoded copy; the file opened is left as it was.
#[test]
fn a_file_not_read_in_place_is_decoded_and_heard() {
    let directory = scratch("decoded");
    let mut client = Client::new(&directory);
    let aiff = directory.join("ramp.aiff").to_string_lossy().into_owned();
    write_aiff(&aiff, &(0..FRAMES).map(ramp).collect::<Vec<_>>());
    let before = std::fs::read(&aiff).unwrap();
    let info = clausters_app_engine::probe_audio(&aiff).unwrap();
    assert_eq!((info.frames, info.channels), (FRAMES as u64, 1));
    let work = client.work.to_str().unwrap().to_string();
    let opened = client.call(json!({"verb": "openAudio", "key": aiff, "path": aiff,
        "workspace": work, "frames": info.frames, "channels": info.channels,
        "rate": info.sample_rate, "codec": info.codec}));
    client.member = opened["member"].as_u64().expect("opened") as MemberId;
    client.member(json!({"verb": "sync", "window": WINDOW}));
    client.member(json!({"verb": "attach_view"}));
    client.member(json!({"verb": "window", "widget": PICTURE}));
    let ran = client.run(
        json!({"verb": "open", "empty": false, "window": true}),
        None,
    );
    assert!(ran.is_ok(), "{ran:?}");
    let heard = client.heard();
    assert_eq!(heard.len(), FRAMES);
    for (i, h) in heard.iter().enumerate() {
        assert!(
            (h - ramp(i)).abs() < 1e-4,
            "frame {i} is {h}, not {}",
            ramp(i)
        );
    }
    assert_eq!(
        client.renders("decoded"),
        1,
        "the decoded copy is the working folder's"
    );
    assert_eq!(
        std::fs::read(&aiff).unwrap(),
        before,
        "the file opened is untouched"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **A save is called off like any render**: the window's call-off while
/// the take is being written stops the render, the file is as it was and
/// still dirty in the window, and nothing of the render is left behind.
#[test]
fn a_save_called_off_leaves_the_file_as_it_was() {
    let directory = scratch("calledoff");
    let mut client = Client::new(&directory);
    client.open();
    client
        .gesture(1, "delete", &[json!(10.0), json!(20.0)])
        .unwrap();
    let before = std::fs::read(&client.take).unwrap();
    let (token, ran) = client.start(
        json!({"verb": "save", "request": {"path": null, "format": "float"}}),
        None,
    );
    assert!(ran.end.is_none(), "the save is a render on its way");
    client.verb(2, "call_off").unwrap();
    assert_eq!(client.end(token), Err("called off".into()));
    assert_eq!(
        std::fs::read(&client.take).unwrap(),
        before,
        "the file as it was"
    );
    assert_eq!(client.binding()["dirty"], true);
    let partial = std::fs::read_dir(&client.work)
        .unwrap()
        .filter_map(Result::ok)
        .any(|e| e.file_name().to_string_lossy().ends_with(".saving"));
    assert!(!partial, "nothing of the render left");
    std::fs::remove_dir_all(directory).unwrap();
}
