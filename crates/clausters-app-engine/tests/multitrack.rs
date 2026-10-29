//! **A multitrack's track read by the application is the track the clip
//! readers played**: one region, through the document's reading and the
//! core's arithmetic, against the same region planned as a clip and played
//! by the engine's reader def -- with fades, a playrate, a source at another
//! rate and a loop.

#![cfg(feature = "synth")]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use clausters::dsp::buffer::Buffer;
use clausters::server::engine::{BLOCK_SIZE, Cmd, engine_pair};
use clausters_app_engine::take::{Headers, PlacedRead};
use clausters_app_engine::{AppEngine, EngineMedium};
use clausters_apps::editing::held::{PlacedList, PlacedSource};
use clausters_core::ids::{IdShare, IdSpaces, ServerShape, Space};
use clausters_core::mixer;
use clausters_document::multitrack::nodes::{self, SourceInfo};
use clausters_document::multitrack::read::{SourceShape, track_reads};
use clausters_document::multitrack::{Content, Fade, Multitrack, Region, TakeLane, Track, Window};
use clausters_document::parts::TakeFile;
use clausters_document::timebase::Second;
use clausters_document::{Lifetime, NodeId, Opaque, SourceId, SourceRef};
use clausters_editing::apply::{MULTITRACK_TRANSPORT, Medium};
use clausters_editing::instance::{Op, Port, Ports};

const SR: f64 = 48_000.0;

/// A mono float file at `rate`, a smooth signal so a reading between two
/// frames is a small number.
fn source(name: &str, rate: u32, frames: usize) -> (String, Vec<f32>) {
    let values: Vec<f32> = (0..frames)
        .map(|n| (n as f32 * 0.003).sin() * 0.5)
        .collect();
    let path = std::env::temp_dir().join(format!("mt-read-{name}-{}.wav", std::process::id()));
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(&path, spec).unwrap();
    for &v in &values {
        w.write_sample(v).unwrap();
    }
    w.finalize().unwrap();
    (path.to_string_lossy().into_owned(), values)
}

fn multitrack(region: Region) -> Multitrack {
    let mut track = Track::new(NodeId(1), NodeId(2));
    track.channels = 1;
    track.take_lanes[0] = TakeLane {
        regions: vec![region],
        ..TakeLane::new(NodeId(2))
    };
    Multitrack {
        tracks: vec![track],
        channels: 1,
        ..Multitrack::default()
    }
}

/// **The region, read by the application** at the engine's rate.
fn read(multitrack: &Multitrack, path: &str, rate: f64, frames: usize, n: usize) -> Vec<f32> {
    let shapes = HashMap::from([(
        SourceId(9),
        SourceShape {
            channels: 1,
            frames: frames as u64,
            rate,
        },
    )]);
    let track = track_reads(multitrack, SR, &shapes, &BTreeSet::new()).remove(0);
    let sources = BTreeMap::from([(
        SourceId(9),
        PlacedSource::File(TakeFile {
            path: path.into(),
            peaks: None,
            channels: 1,
            frames: frames as u64,
            rate,
        }),
    )]);
    let list = PlacedList {
        regions: track.regions.into(),
        sources: Arc::new(sources),
        frames: track.frames,
    };
    let placed =
        PlacedRead::over(&list, 1, &mut Headers::default(), &Arc::default()).expect("it reads");
    placed.read(0, n, 0)
}

/// **The same region, planned as a clip and played by its reader def** on
/// an engine, block by block.
fn played(multitrack: &Multitrack, values: &[f32], rate: f64, n: usize) -> Vec<f32> {
    let (mut engine, handle) = engine_pair(SR as f32, 2);
    let app = Arc::new(Mutex::new(AppEngine::over(handle)));
    let mut ids = IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE);
    let bufnum = ids.alloc(Space::Buffers, 1).unwrap() as i32;
    let buffer = Buffer::new(values.to_vec(), 1, values.len(), rate);
    assert!(app.lock().unwrap().push(vec![Cmd::SetBuffer {
        index: bufnum as usize,
        buffer: Some(Arc::new(buffer)),
    }]));
    let sources = HashMap::from([(
        SourceId(9),
        SourceInfo {
            buffer: bufnum,
            channels: 1,
            duration: Some(values.len() as f64 / rate),
        },
    )]);
    let plan = nodes::plan(multitrack, SR, &sources);
    let reader = plan.tracks[0].clips[0].readers[0].clone();
    let ports: Ports = [
        ("out0", 0.0),
        ("out1", 1.0),
        ("buf", f64::from(reader.buffer)),
        ("chan", 0.0),
        ("at", reader.at),
        ("span", reader.span),
        ("start", reader.start),
        ("loop", f64::from(u8::from(reader.looping))),
        ("rate", reader.rate),
        ("fade_in", reader.fade_in),
        ("fade_out", reader.fade_out),
        ("fade_in_shape", f64::from(reader.fade_in_shape.0)),
        ("fade_in_curve", reader.fade_in_shape.1),
        ("fade_out_shape", f64::from(reader.fade_out_shape.0)),
        ("fade_out_curve", reader.fade_out_shape.1),
    ]
    .into_iter()
    .map(|(port, value)| (port.to_string(), Port::Number(value)))
    .collect();
    let mut medium = EngineMedium::new(Arc::clone(&app));
    medium
        .apply(
            vec![
                Op::Def {
                    family: "synth".into(),
                    spec: mixer::reader_def(),
                },
                Op::Follow {
                    handle: "group".into(),
                    transport: MULTITRACK_TRANSPORT,
                },
                Op::Synth {
                    handle: "reader".into(),
                    def: mixer::reader_name(),
                    target: "group".into(),
                    ports,
                },
                Op::TransportRun {
                    transport: MULTITRACK_TRANSPORT,
                    rolling: true,
                },
            ],
            &mut ids,
        )
        .expect("applied");
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    let mut left = Vec::new();
    while left.len() < n {
        engine.process_block(&mut out);
        app.lock().unwrap().pump();
        left.extend(out.as_chunks::<2>().0.iter().map(|f| f[0]));
    }
    left.truncate(n);
    left
}

fn region(at: f64, len: f64, start: f64, playrate: f64, looping: bool) -> Region {
    let content = Content::Window {
        window: Window {
            source: SourceRef {
                source: SourceId(9),
                lifetime: Lifetime::Session,
                generation: 0,
                range: None,
            },
            start,
            duration: len,
        },
        playrate,
        args: Opaque::none(),
        looping,
    };
    let mut region = Region::new(NodeId(3), Second(at), Second(len), content);
    region.fade_in = Some(Fade::of(Second(0.01)));
    region.fade_out = Some(Fade::of(Second(0.02)));
    region
}

/// The largest difference between the two readings.
fn worst(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

#[test]
fn a_region_read_by_the_application_is_the_clip_it_was() {
    let cases = [
        ("plain", 48_000, region(0.02, 0.2, 0.05, 1.0, false)),
        ("rate", 44_100, region(0.02, 0.2, 0.05, 1.0, false)),
        ("playrate", 48_000, region(0.01, 0.2, 0.1, 0.75, false)),
        ("loop", 48_000, region(0.01, 0.3, 0.05, 1.0, true)),
        ("short", 48_000, region(0.0, 0.5, 0.15, 1.0, false)),
    ];
    for (name, rate, region) in cases {
        let frames = rate as usize / 5;
        let (path, values) = source(name, rate, frames);
        let multitrack = multitrack(region);
        let n = 30_000;
        let new = read(&multitrack, &path, f64::from(rate), frames, n);
        let old = played(&multitrack, &values, f64::from(rate), n);
        assert!(new.iter().any(|&s| s != 0.0), "{name}: the region sounds");
        let error = worst(&new, &old);
        assert!(error < 1e-4, "{name}: the readings differ by {error}");
        let _ = std::fs::remove_file(&path);
    }
}

/// **A multitrack plays on an engine the application holds**: its track's
/// regions read by the application into the track's fed buffers, played by
/// the track's feed reader through its strip and the master, with nothing
/// walked. Past the cue and the play's ramp, the output is the track as
/// the application reads it.
#[test]
fn a_multitrack_plays_its_tracks_on_an_engine_it_holds() {
    use clausters_app_engine::held::Readers;
    use clausters_apps::editing::held::{HeldEngine, HeldList, HeldTake};
    use clausters_document::multitrack::nodes::PlannedFeed;
    use clausters_editing::playback::MultitrackPlayback;

    let (path, values) = source("plays", 48_000, 24_000);
    let mut region = region(0.0, 0.5, 0.0, 1.0, false);
    region.fade_in = Some(Fade::of(Second(0.0)));
    let multitrack = multitrack(region);
    let expected = read(&multitrack, &path, SR, values.len(), 24_000);

    let (mut engine, handle) = engine_pair(SR as f32, 2);
    let app = Arc::new(Mutex::new(AppEngine::over(handle)));
    let mut ids = IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE);
    let mut readers = Readers::new(Arc::clone(&app));
    let shapes = HashMap::from([(
        SourceId(9),
        SourceShape {
            channels: 1,
            frames: values.len() as u64,
            rate: SR,
        },
    )]);
    let track = track_reads(&multitrack, SR, &shapes, &BTreeSet::new()).remove(0);
    let sources = BTreeMap::from([(
        SourceId(9),
        PlacedSource::File(TakeFile {
            path: path.as_str().into(),
            peaks: None,
            channels: 1,
            frames: values.len() as u64,
            rate: SR,
        }),
    )]);
    let take = HeldTake {
        reader: 1,
        revision: 1,
        list: HeldList::Placed(PlacedList {
            regions: track.regions.into(),
            sources: Arc::new(sources),
            frames: track.frames,
        }),
        channels: 1,
        rate: SR,
    };
    let buffers = readers.take(0, &take, &mut ids).expect("the take reads");
    let feeds = HashMap::from([(
        NodeId(1),
        PlannedFeed {
            buffers,
            frames: track.frames as f64,
        },
    )]);
    let mut playback = MultitrackPlayback::in_medium(EngineMedium::new(Arc::clone(&app)));
    let steps = playback
        .sync_fed(&multitrack, SR, &feeds, 1.0, &mut ids)
        .expect("synced");
    assert!(steps.is_empty(), "nothing to walk: {steps:?}");
    assert!(playback.play_pass(None, false).is_empty());
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    let mut left = Vec::new();
    while left.len() < 24_000 {
        engine.process_block(&mut out);
        app.lock().unwrap().pump();
        left.extend(out.as_chunks::<2>().0.iter().map(|f| f[0]));
        std::thread::sleep(std::time::Duration::from_micros(1_333));
    }
    let first = left
        .iter()
        .position(|&s| s != 0.0)
        .expect("the track was heard");
    assert!(first < 12_000, "the cue took {first} samples");
    let rolled = first / BLOCK_SIZE * BLOCK_SIZE;
    for i in rolled + 256..rolled + 8_000 {
        let want = expected[i - rolled];
        assert!(
            (left[i] - want).abs() < 1e-5,
            "sample {i}: {} against {want}",
            left[i]
        );
    }
    let _ = std::fs::remove_file(&path);
}

/// **A multitrack member, driven by the application that holds its engine**:
/// opened over a take the application resolved, it plays its track on the
/// engine; a region moved by a route that is not a turn (a script's handle)
/// is heard where it now is once the multitrack is sounded again, its track's
/// readers the same nodes.
#[test]
fn a_multitrack_member_plays_and_follows_an_edit_in_its_application() {
    use clausters_app_engine::app::App;
    use clausters_apps::editing::{Editing, Member};

    let (path, values) = source("member", 48_000, 24_000);
    let mut region = region(0.0, 0.5, 0.0, 1.0, false);
    region.fade_in = Some(Fade::of(Second(0.0)));
    let multitrack = multitrack(region);
    let expected = read(&multitrack, &path, SR, values.len(), 24_000);

    let (mut engine, handle) = engine_pair(SR as f32, 2);
    let app_engine = Arc::new(Mutex::new(AppEngine::over(handle)));
    let mut app = App::new(Arc::clone(&app_engine));
    let mut editing = Editing::new();
    app.hold(&mut editing);
    let opened = app.call(
        &mut editing,
        &serde_json::json!({"verb": "openMultitrack", "key": "mt",
            "multitrack": serde_json::to_value(&multitrack).unwrap(), "rate": SR}),
    );
    let member = opened["member"].as_u64().expect("opened") as u32;
    let Some(Member::Multitrack(editor)) = editing.member_mut(member) else {
        panic!("not a multitrack");
    };
    editor.set_takes(HashMap::from([(
        SourceId(9),
        TakeFile {
            path: path.as_str().into(),
            peaks: None,
            channels: 1,
            frames: values.len() as u64,
            rate: SR,
        },
    )]));
    let ran = app.act(
        &mut editing,
        member,
        serde_json::json!({"verb": "open"}),
        1,
        None,
    );
    assert!(matches!(ran.end, Some(Ok(_))), "{:?}", ran.end);
    let ran = app.act(
        &mut editing,
        member,
        serde_json::json!({"verb": "play"}),
        2,
        None,
    );
    assert!(matches!(ran.end, Some(Ok(_))), "{:?}", ran.end);
    let left = blocks(&mut engine, &app_engine, &mut app, &mut editing, 24_000);
    let first = left.iter().position(|&s| s != 0.0).expect("heard");
    let rolled = first / BLOCK_SIZE * BLOCK_SIZE;
    for i in rolled + 256..rolled + 4_000 {
        assert!((left[i] - expected[i - rolled]).abs() < 1e-5, "sample {i}");
    }
    let rolling = app.query(&mut editing, member, "transport");
    assert_eq!(rolling["rolling"], true, "{rolling}");
    let output = app.query(&mut editing, member, "output");
    assert_eq!(output["starved"], 0, "{output}");

    // the region moved a tenth of a second later, by a script's handle
    app.act(
        &mut editing,
        member,
        serde_json::json!({"verb": "stop", "mark": 0.0}),
        3,
        None,
    );
    let Some(Member::Multitrack(editor)) = editing.member(member) else {
        panic!("not a multitrack");
    };
    let shared = editor.shared();
    shared.lock().unwrap().tracks[0].take_lanes[0].regions[0].position = Second(0.1);
    let ran = app.act(
        &mut editing,
        member,
        serde_json::json!({"verb": "sound"}),
        4,
        None,
    );
    assert!(matches!(ran.end, Some(Ok(_))), "{:?}", ran.end);
    let _ = blocks(&mut engine, &app_engine, &mut app, &mut editing, 4_096);
    app.act(
        &mut editing,
        member,
        serde_json::json!({"verb": "play"}),
        5,
        None,
    );
    let left = blocks(&mut engine, &app_engine, &mut app, &mut editing, 24_000);
    // From the top the region is silent for a tenth of a second: what
    // sounds before it is at most the stream's fade out of the frames it
    // kept from before the edit, into the refill of the new take.
    let early = left[..4_700].iter().fold(0.0f32, |a, b| a.max(b.abs()));
    assert!(early < 0.01, "the region sounds before it starts: {early}");
    let late = left[4_800..12_000]
        .iter()
        .fold(0.0f32, |a, b| a.max(b.abs()));
    assert!(late > 0.3, "the region sounds where it is: {late}");
    let _ = std::fs::remove_file(&path);
}

/// Blocks of the engine, paced, the application's turn after each: the
/// left side.
fn blocks(
    engine: &mut clausters::server::engine::Engine,
    app_engine: &Mutex<AppEngine>,
    app: &mut clausters_app_engine::app::App,
    editing: &mut clausters_apps::editing::Editing,
    n: usize,
) -> Vec<f32> {
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    let mut left = Vec::new();
    while left.len() < n {
        engine.process_block(&mut out);
        let _ = app.landed(editing);
        let _ = app_engine;
        left.extend(out.as_chunks::<2>().0.iter().map(|f| f[0]));
        std::thread::sleep(std::time::Duration::from_micros(1_333));
    }
    left
}

/// **A session opens from its folder**: its take's header read, its peaks
/// measured into the session's `peaks/` while the window is already open,
/// its track heard; and a save writes the take relative to the folder.
#[test]
fn a_session_opens_from_its_files_and_saves_relative_paths() {
    use clausters_app_engine::app::App;
    use clausters_apps::editing::Editing;
    use clausters_document::session::{Session, Source};
    use clausters_document::{Document, Lifetime};

    let folder = std::env::temp_dir().join(format!("mt-session-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    let (made, values) = source("session", 48_000, 24_000);
    std::fs::rename(&made, folder.join("take.wav")).unwrap();
    let mut region = region(0.0, 0.5, 0.0, 1.0, false);
    region.fade_in = Some(Fade::of(Second(0.0)));
    let session = Session::new(Document::empty())
        .with_multitrack(multitrack(region))
        .with_source(SourceId(9), Source::file("take.wav", Lifetime::Session));
    let path = folder.join("song.json");
    std::fs::write(&path, serde_json::to_string(&session).unwrap()).unwrap();

    let (mut engine, handle) = engine_pair(SR as f32, 2);
    let app_engine = Arc::new(Mutex::new(AppEngine::over(handle)));
    let mut app = App::new(Arc::clone(&app_engine));
    let mut editing = Editing::new();
    app.hold(&mut editing);
    let opened = app.call(
        &mut editing,
        &serde_json::json!({"verb": "openMultitrack", "key": "mt",
            "multitrack": {}, "rate": SR}),
    );
    let member = opened["member"].as_u64().expect("opened") as u32;
    let ran = app.act(
        &mut editing,
        member,
        serde_json::json!({"verb": "open", "session": path.to_str().unwrap()}),
        1,
        None,
    );
    let Some(Ok(answer)) = &ran.end else {
        panic!("{:?}", ran.end);
    };
    assert_eq!(answer["unresolved"], serde_json::json!([]));
    // the peaks land while the window is open
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let peaks = folder.join("peaks");
    loop {
        let landed = app.landed(&mut editing);
        if landed
            .iter()
            .any(|ran| ran.effects.iter().any(|e| e["kind"] == "resync"))
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the peaks never landed"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let measured: Vec<_> = std::fs::read_dir(&peaks).unwrap().collect();
    assert!(
        !measured.is_empty(),
        "the peaks are in the session's folder"
    );

    app.act(
        &mut editing,
        member,
        serde_json::json!({"verb": "play"}),
        2,
        None,
    );
    let left = blocks(&mut engine, &app_engine, &mut app, &mut editing, 12_000);
    let heard = left.iter().fold(0.0f32, |a, b| a.max(b.abs()));
    assert!(heard > 0.3, "the session's track is heard: {heard}");
    let _ = values;

    let saved_to = folder.join("again.json");
    let ran = app.act(
        &mut editing,
        member,
        serde_json::json!({"verb": "save", "path": saved_to.to_str().unwrap()}),
        3,
        None,
    );
    assert!(matches!(ran.end, Some(Ok(_))), "{:?}", ran.end);
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&saved_to).unwrap()).unwrap();
    assert_eq!(saved["sources"]["9"]["location"]["path"], "take.wav");
    let _ = std::fs::remove_dir_all(&folder);
}
