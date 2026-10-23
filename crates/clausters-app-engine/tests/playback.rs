//! **The audio editor's playback, carried out on an engine the application
//! holds**: the same playback a client walks as OSC steps answers no steps
//! here, its ops built into the engine's commands at once, and the take is
//! heard frame for frame.

#![cfg(feature = "synth")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use clausters::dsp::buffer::Buffer;
use clausters::rosc::OscType;
use clausters::server::engine::{BLOCK_SIZE, Cmd, engine_pair};
use clausters::server::nrt::{NrtAction, run_job};
use clausters_app_engine::{AppEngine, EngineMedium};
use clausters_core::ids::{IdShare, IdSpaces, ServerShape, Space};
use clausters_editing::audio_playback::{AudioEditorPlayback, Pass};

const SR: f64 = 48_000.0;
const FRAMES: usize = 24_000;

/// A mono float file whose every frame is told apart from every other.
fn take(name: &str) -> (String, Vec<f32>) {
    let values: Vec<f32> = (0..FRAMES)
        .map(|n| 0.25 + n as f32 / (1 << 20) as f32)
        .collect();
    let path = std::env::temp_dir().join(format!("app-engine-{name}-{}.wav", std::process::id()));
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SR as u32,
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

/// The join of one part over the whole file.
fn join(path: &str) -> Arc<Buffer> {
    let args = vec![
        OscType::Int(9),
        OscType::Int(1),
        OscType::Float(SR as f32),
        OscType::String(path.into()),
        OscType::Int(0),
        OscType::Int(FRAMES as i32),
        OscType::Int(0),
        OscType::Int(0),
        OscType::Int(1),
        OscType::Float(0.0),
        OscType::Int(1),
        OscType::Float(0.0),
        OscType::Int(0),
    ];
    let mirror = vec![None; 16];
    let (_, job) =
        clausters::osc::translate::parse_buffer_msg("/buffer_stitch", &args, &mirror, SR)
            .expect("parsed");
    match run_job(job).expect("stitched") {
        NrtAction::Install(b) => b,
        _ => panic!("a stitch installs a buffer"),
    }
}

/// **The take is heard as it is, with nothing sent but commands**: every
/// call of the playback answers no steps, and once the reader's first refill
/// and the play's ramp are behind it, both sides play the file's frames.
#[test]
fn the_audio_editor_plays_on_an_engine_it_holds() {
    let (path, values) = take("plays");
    let (mut engine, handle) = engine_pair(SR as f32, 2);
    let app = Arc::new(Mutex::new(AppEngine::over(handle)));
    let mut ids = IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE);
    let bufnum = ids.alloc(Space::Buffers, 1).unwrap() as i32;
    assert!(app.lock().unwrap().push(vec![Cmd::SetBuffer {
        index: bufnum as usize,
        buffer: Some(join(&path)),
    }]));
    let mut playback = AudioEditorPlayback::in_medium(EngineMedium::new(Arc::clone(&app)));
    let steps = playback
        .sync(1, bufnum, 1, FRAMES as u64, SR, SR, &mut ids)
        .expect("synced");
    assert!(steps.is_empty(), "nothing to walk: {steps:?}");
    let steps = playback.play(
        1,
        0,
        Pass::Until {
            end: FRAMES as u64,
            back: 0,
        },
    );
    assert!(steps.is_empty(), "nothing to walk: {steps:?}");
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    let (mut left, mut right) = (Vec::new(), Vec::new());
    for _ in 0..(FRAMES / BLOCK_SIZE) {
        engine.process_block(&mut out);
        app.lock().unwrap().pump();
        for f in out.as_chunks::<2>().0 {
            left.push(f[0]);
            right.push(f[1]);
        }
        std::thread::sleep(Duration::from_micros(1_333));
    }
    assert!(left.iter().any(|&s| s != 0.0), "the take was heard");
    // The play's ramp is 5 ms; past the first refill the output is the file.
    for i in 4_000..12_000 {
        assert_eq!(left[i], values[i], "left, sample {i}");
        assert_eq!(right[i], values[i], "right, sample {i}");
    }
    let _ = std::fs::remove_file(&path);
}

/// Blocks of the engine, paced, with the application's turn after each:
/// the left side.
fn blocks(
    engine: &mut clausters::server::engine::Engine,
    app: &Mutex<AppEngine>,
    n: usize,
) -> Vec<f32> {
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    let mut left = Vec::new();
    for _ in 0..n {
        engine.process_block(&mut out);
        app.lock().unwrap().pump();
        left.extend(out.as_chunks::<2>().0.iter().map(|f| f[0]));
        std::thread::sleep(Duration::from_micros(1_333));
    }
    left
}

/// **A loop, a stop and a locate are the engine's commands too**: a pass
/// over a loop wraps on its span, a stop falls silent, and a play from a
/// located frame plays from there.
#[test]
fn loops_stops_and_locates_on_an_engine_it_holds() {
    let (path, values) = take("loops");
    let (mut engine, handle) = engine_pair(SR as f32, 2);
    let app = Arc::new(Mutex::new(AppEngine::over(handle)));
    let mut ids = IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE);
    let bufnum = ids.alloc(Space::Buffers, 1).unwrap() as i32;
    app.lock().unwrap().push(vec![Cmd::SetBuffer {
        index: bufnum as usize,
        buffer: Some(join(&path)),
    }]);
    let mut playback = AudioEditorPlayback::in_medium(EngineMedium::new(Arc::clone(&app)));
    playback
        .sync(1, bufnum, 1, FRAMES as u64, SR, SR, &mut ids)
        .unwrap();
    let (from, to) = (2_000u64, 6_000u64);
    playback.play(1, from, Pass::Loop { from, to });
    let heard = blocks(&mut engine, &app, 300);
    // Past the first refill and the ramp, every sample is the loop's frame.
    let span = (to - from) as usize;
    for (i, &s) in heard.iter().enumerate().skip(1_000) {
        let frame = from as usize + i % span;
        assert_eq!(s, values[frame], "sample {i}, frame {frame}");
    }
    // A stop rolls out its ramp and is silent after it.
    playback.stop(0);
    let after = blocks(&mut engine, &app, 20);
    assert!(
        after[after.len() - BLOCK_SIZE..].iter().all(|&s| s == 0.0),
        "silent once stopped"
    );
    // A play from a located frame plays from there.
    let at = 15_000u64;
    playback.play(
        1,
        at,
        Pass::Until {
            end: FRAMES as u64,
            back: 0,
        },
    );
    let heard = blocks(&mut engine, &app, 60);
    for (i, &s) in heard.iter().enumerate().skip(1_000) {
        assert_eq!(s, values[at as usize + i], "sample {i} after the locate");
    }
    let _ = std::fs::remove_file(&path);
}
