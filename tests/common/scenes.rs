//! Golden-test scenes, shared between `tests/golden.rs` and the
//! `render_golden` example that regenerates the reference files. Everything
//! here must be **deterministic** (no noise UGens): the same scene renders
//! the same samples on every run.

use std::path::Path;
use std::sync::Arc;

use clausters::dsp::buffer::Buffer;
use clausters::rosc::{OscMessage, OscType};
use clausters::server::nrt::{NrtJob, run_job};
use clausters::server::render::{RenderConfig, Score};

pub const SAMPLE_RATE: f64 = 48000.0;
/// The source file for the `playbuf` scene is at 44100 Hz on purpose: the
/// scene must compensate with PlayBuf's rate input.
pub const SOURCE_RATE: f64 = 44100.0;

pub fn config() -> RenderConfig {
    RenderConfig {
        sample_rate: SAMPLE_RATE,
        channels: 1,
    }
}

/// Event time at an exact output sample.
fn t(samples: u64) -> f64 {
    samples as f64 / SAMPLE_RATE
}

fn msg(addr: &str, args: Vec<OscType>) -> OscMessage {
    OscMessage {
        addr: addr.into(),
        args,
    }
}

fn s(v: &str) -> OscType {
    OscType::String(v.into())
}

fn i(v: i32) -> OscType {
    OscType::Int(v)
}

fn f(v: f32) -> OscType {
    OscType::Float(v)
}

/// Two voices of the built-in "default" def overlapping, with mid-block
/// entries, an `/n_set` retune and staggered frees: exercises the node tree,
/// named controls and the sample-accurate scheduler end to end. 0.3 s mono.
pub fn arpeggio() -> Score {
    Score::new([
        (
            t(0),
            vec![msg(
                "/s_new",
                vec![
                    s("default"),
                    i(1000),
                    i(0),
                    i(0),
                    s("freq"),
                    f(330.0),
                    s("amp"),
                    f(0.3),
                ],
            )],
        ),
        (
            // Mid-block (5000 = 78·64 + 8): the engine must split the block.
            t(5000),
            vec![
                msg(
                    "/s_new",
                    vec![
                        s("default"),
                        i(1001),
                        i(0),
                        i(0),
                        s("freq"),
                        f(440.0),
                        s("amp"),
                        f(0.2),
                    ],
                ),
                msg("/n_set", vec![i(1000), s("freq"), f(220.0)]),
            ],
        ),
        (
            t(9603),
            vec![
                msg("/n_free", vec![i(1000)]),
                msg(
                    "/s_new",
                    vec![
                        s("default"),
                        i(1002),
                        i(0),
                        i(0),
                        s("freq"),
                        f(550.0),
                        s("amp"),
                        f(0.1),
                    ],
                ),
            ],
        ),
        // Final bundle: sets the render length (its commands are not heard).
        (t(14400), vec![msg("/n_free", vec![i(1001), i(1002)])]),
    ])
    .expect("valid scene")
}

/// Mono PlayBuf player: `rate` control, looping, scaled by control bus 0.
const PLAYER_DEF: &str = r#"{
  "name": "player",
  "controls": [{"name": "rate", "default": 1.0}],
  "ugens": [
    {"kind": "PlayBuf", "inputs": [{"const": 0.0}, {"const": 0.0}, {"control": 0}, {"const": 1.0}]},
    {"kind": "InCtl",   "inputs": [{"const": 0.0}]},
    {"kind": "Mul",     "inputs": [{"ugen": 0}, {"ugen": 1}]},
    {"kind": "Out",     "inputs": [{"const": 0.0}, {"ugen": 2}]}
  ]
}"#;

/// Writes the deterministic WAV the `playbuf` scene reads: 0.25 s of a
/// 220 Hz sine at amp 0.5, 44100 Hz mono float32.
pub fn write_playbuf_source(path: &Path) {
    let frames = (SOURCE_RATE * 0.25) as usize;
    let data: Vec<f32> = (0..frames)
        .map(|n| ((2.0 * std::f64::consts::PI * 220.0 * n as f64 / SOURCE_RATE).sin() * 0.5) as f32)
        .collect();
    let buffer = Arc::new(Buffer::new(data, 1, frames, SOURCE_RATE));
    run_job(NrtJob::Write {
        path: path.to_str().expect("utf-8 path").into(),
        sample_format: "float".into(),
        buf_start: 0,
        num_frames: -1,
        buffer,
    })
    .expect("write the playbuf source file");
}

/// `/d_recv` + `/b_allocRead` + PlayBuf at the file/server rate ratio, a
/// sample-accurate `/c_set` amplitude drop and a `/b_zero` mid-playback
/// (buffers are immutable: the swap must land on its exact sample and the
/// tail must be silent). 0.25 s mono. `source` is the file written by
/// [`write_playbuf_source`].
pub fn playbuf(source: &Path) -> Score {
    let path = source.to_str().expect("utf-8 path");
    Score::new([
        (
            t(0),
            vec![
                msg("/d_recv", vec![OscType::Blob(PLAYER_DEF.as_bytes().to_vec())]),
                msg("/b_allocRead", vec![i(0), s(path)]),
                msg("/c_set", vec![i(0), f(0.4)]),
            ],
        ),
        (
            t(2401),
            vec![msg(
                "/s_new",
                vec![
                    s("player"),
                    i(1),
                    i(0),
                    i(0),
                    s("rate"),
                    f((SOURCE_RATE / SAMPLE_RATE) as f32),
                ],
            )],
        ),
        (t(6001), vec![msg("/c_set", vec![i(0), f(0.15)])]),
        (t(9000), vec![msg("/b_zero", vec![i(0)])]),
        (t(12000), vec![msg("/n_free", vec![i(1)])]),
    ])
    .expect("valid scene")
}
