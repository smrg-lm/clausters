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
