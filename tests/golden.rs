//! NRT golden tests (M7): scenes render offline and must match the
//! reference WAVs in `tests/golden/`, plus independent signal asserts so a
//! stale golden cannot silently bless a broken render.
//!
//! Regenerate the references with `cargo run --example render_golden` after
//! an intended change, and **listen to them** before committing.

#[path = "common/scenes.rs"]
mod scenes;

use std::path::{Path, PathBuf};

use clausters::rosc::{OscBundle, OscMessage, OscPacket, OscTime, OscType, encoder};
use clausters::server::nrt::{NrtAction, NrtJob, run_job};
use clausters::server::render::{RenderConfig, Score, render, render_to_vec};

/// Cross-platform tolerance: same-platform renders are bit-identical, but
/// `sin` may differ by a few ULP across libm implementations.
const TOLERANCE: f32 = 1e-4;

fn golden_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name)
}

fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("clausters_golden_{}_{name}", std::process::id()))
}

fn load_golden(name: &str) -> Vec<f32> {
    let path = golden_path(name);
    let outcome = run_job(NrtJob::AllocRead {
        path: path.to_str().unwrap().into(),
        file_start: 0,
        num_frames: 0,
    });
    match outcome {
        Ok(NrtAction::Install(buf)) => buf.data().to_vec(),
        other => panic!(
            "cannot load golden {name} ({other:?}); regenerate with `cargo run --example render_golden`"
        ),
    }
}

fn assert_matches_golden(name: &str, rendered: &[f32]) {
    let golden = load_golden(name);
    assert_eq!(
        rendered.len(),
        golden.len(),
        "length mismatch vs {name}: if the scene changed on purpose, regenerate \
         (cargo run --example render_golden) and listen to the new file"
    );
    for (i, (a, b)) in rendered.iter().zip(&golden).enumerate() {
        assert!(a.is_finite(), "sample {i} is not finite");
        let d = (a - b).abs();
        assert!(
            d <= TOLERANCE,
            "sample {i} of {name}: rendered {a} vs golden {b} (|delta| = {d})"
        );
    }
}

fn rms(buf: &[f32]) -> f32 {
    (buf.iter().map(|x| x * x).sum::<f32>() / buf.len() as f32).sqrt()
}

/// Frequency estimate via positive-going zero crossings.
fn freq(buf: &[f32], sr: f64) -> f64 {
    let crossings = buf.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
    crossings as f64 * sr / buf.len() as f64
}

#[test]
fn arpeggio_matches_golden() {
    let (out, stats) = render_to_vec(&scenes::arpeggio(), &scenes::config()).unwrap();
    assert_eq!(stats.frames, 14400, "0.3 s at 48 kHz");
    assert_eq!(out.len(), 14400, "mono interleaved = frames");

    // Independent signal asserts: first segment is a lone 330 Hz sine at
    // amp 0.3 (RMS = 0.3/sqrt(2)); the overlap segment is louder than one
    // voice alone.
    let lone = &out[0..4800];
    let estimated = freq(lone, scenes::SAMPLE_RATE);
    assert!(
        (estimated - 330.0).abs() < 330.0 * 0.02,
        "first segment should be 330 Hz, estimated {estimated}"
    );
    let lone_rms = rms(lone);
    assert!(
        (lone_rms - 0.3 / std::f32::consts::SQRT_2).abs() < 0.01,
        "lone-voice RMS {lone_rms}"
    );
    assert!(rms(&out[5100..9500]) > lone_rms, "two voices mix louder");

    assert_matches_golden("arpeggio.wav", &out);
}

#[test]
fn playbuf_matches_golden() {
    let source = temp_path("source.wav");
    scenes::write_playbuf_source(&source);
    let (out, stats) = render_to_vec(&scenes::playbuf(&source), &scenes::config()).unwrap();
    assert_eq!(stats.frames, 12000, "0.25 s at 48 kHz");

    // Silent before the synth starts and after /b_zero swaps in a zeroed
    // buffer; in between, a 220 Hz sine at 0.5·0.4 then 0.5·0.15.
    assert!(rms(&out[0..2401]) < 1e-6, "silence before /s_new");
    let estimated = freq(&out[2500..5900], scenes::SAMPLE_RATE);
    assert!(
        (estimated - 220.0).abs() < 220.0 * 0.03,
        "playback should be 220 Hz (rate-compensated), estimated {estimated}"
    );
    let loud = rms(&out[2500..5900]);
    let soft = rms(&out[6100..8900]);
    assert!(loud > 0.10 && soft > 0.03, "audible segments: {loud}, {soft}");
    assert!(
        loud / soft > 2.0,
        "the scheduled /c_set must drop the level: {loud} vs {soft}"
    );
    assert!(rms(&out[9001..12000]) < 1e-6, "silence after /b_zero");

    assert_matches_golden("playbuf.wav", &out);
    let _ = std::fs::remove_file(&source);
}

const DC_DEF: &str = r#"{"name": "dc", "ugens": [
  {"kind": "Add", "inputs": [{"const": 0.5}, {"const": 0.5}]},
  {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
]}"#;

fn msg(addr: &str, args: Vec<OscType>) -> OscMessage {
    OscMessage {
        addr: addr.into(),
        args,
    }
}

#[test]
fn render_is_sample_accurate_mid_block() {
    // A DC=1 source scheduled at sample 100 (block 1, offset 36) and freed
    // at sample 200: the rendered edges must land on those exact samples.
    let score = Score::new([
        (0.0, vec![msg("/d_recv", vec![OscType::String(DC_DEF.into())])]),
        (
            100.0 / 48000.0,
            vec![msg(
                "/s_new",
                vec![
                    OscType::String("dc".into()),
                    OscType::Int(1),
                    OscType::Int(0),
                    OscType::Int(0),
                ],
            )],
        ),
        (200.0 / 48000.0, vec![msg("/n_free", vec![OscType::Int(1)])]),
    ])
    .unwrap();
    let cfg = RenderConfig {
        sample_rate: 48000.0,
        channels: 1,
    };
    let (out, stats) = render_to_vec(&score, &cfg).unwrap();
    assert_eq!(stats.frames, 200);
    for (i, s) in out.iter().enumerate() {
        let expected = if i < 100 { 0.0 } else { 1.0 };
        assert_eq!(*s, expected, "sample {i}");
    }
}

#[test]
fn score_file_round_trips_through_disk() {
    // Same scene as above, but encoded as a binary score file. The def blob
    // is padded to a multiple of 4 bytes to exercise the rosc blob-decoding
    // workaround (see CLAUDE.md).
    let mut def = DC_DEF.as_bytes().to_vec();
    while !def.len().is_multiple_of(4) {
        def.push(b' ');
    }
    let timetag = |t: f64| OscTime {
        seconds: t.trunc() as u32,
        fractional: (t.fract() * 2f64.powi(32)).round() as u32,
    };
    let mut bytes = Vec::new();
    let bundles = [
        (0.0, msg("/d_recv", vec![OscType::Blob(def)])),
        (
            100.0 / 48000.0,
            msg(
                "/s_new",
                vec![
                    OscType::String("dc".into()),
                    OscType::Int(1),
                    OscType::Int(0),
                    OscType::Int(0),
                ],
            ),
        ),
        (200.0 / 48000.0, msg("/n_free", vec![OscType::Int(1)])),
    ];
    for (time, message) in bundles {
        let packet = OscPacket::Bundle(OscBundle {
            timetag: timetag(time),
            content: vec![OscPacket::Message(message)],
        });
        let encoded = encoder::encode(&packet).unwrap();
        bytes.extend_from_slice(&(encoded.len() as i32).to_be_bytes());
        bytes.extend_from_slice(&encoded);
    }
    let path = temp_path("score.osc");
    std::fs::write(&path, &bytes).unwrap();

    let score = Score::load(&path).unwrap();
    assert_eq!(score.events().len(), 3);
    let cfg = RenderConfig {
        sample_rate: 48000.0,
        channels: 1,
    };
    let (out, _) = render_to_vec(&score, &cfg).unwrap();
    assert_eq!(out.len(), 200);
    assert!(out[..100].iter().all(|s| *s == 0.0), "silence before");
    assert!(out[100..].iter().all(|s| *s == 1.0), "DC after sample 100");
    let _ = std::fs::remove_file(&path);
}

/// `/d_faust` in a score compiles synchronously on the render thread.
#[cfg(feature = "faust")]
#[test]
fn faust_def_renders_in_nrt() {
    let score = Score::new([
        (
            0.0,
            vec![msg(
                "/d_faust",
                vec![
                    OscType::String("fdc".into()),
                    OscType::String("process = 0.8;".into()),
                ],
            )],
        ),
        (
            100.0 / 48000.0,
            vec![msg(
                "/s_new",
                vec![
                    OscType::String("fdc".into()),
                    OscType::Int(1),
                    OscType::Int(0),
                    OscType::Int(0),
                ],
            )],
        ),
        (200.0 / 48000.0, vec![msg("/n_free", vec![OscType::Int(1)])]),
    ])
    .unwrap();
    let cfg = RenderConfig {
        sample_rate: 48000.0,
        channels: 1,
    };
    let (out, _) = render_to_vec(&score, &cfg).unwrap();
    for (i, s) in out.iter().enumerate() {
        let expected = if i < 100 { 0.0 } else { 0.8 };
        assert_eq!(*s, expected, "sample {i}");
    }
}

#[test]
fn zero_length_score_is_an_error() {
    let score = Score::new([(0.0, vec![msg("/n_free", vec![OscType::Int(1)])])]).unwrap();
    let err = render_to_vec(&score, &RenderConfig::default()).unwrap_err();
    assert!(err.contains("empty render"), "got: {err}");
}

#[test]
fn unsupported_command_in_score_is_an_error() {
    let score = Score::new([
        (0.0, vec![msg("/status", vec![])]),
        (1.0, vec![msg("/n_free", vec![OscType::Int(1)])]),
    ])
    .unwrap();
    let err = render_to_vec(&score, &RenderConfig::default()).unwrap_err();
    assert!(err.contains("/status"), "got: {err}");
}

#[test]
fn render_reports_failing_buffer_jobs() {
    let score = Score::new([
        (
            0.0,
            vec![msg(
                "/b_allocRead",
                vec![OscType::Int(0), OscType::String("/nonexistent.wav".into())],
            )],
        ),
        (1.0, vec![msg("/n_free", vec![OscType::Int(1)])]),
    ])
    .unwrap();
    let err = render_to_vec(&score, &RenderConfig::default()).unwrap_err();
    assert!(err.contains("/b_allocRead"), "got: {err}");
}

#[test]
fn sink_errors_propagate() {
    let err = render(
        &scenes::arpeggio(),
        &scenes::config(),
        |_| Err("disk full".into()),
    )
    .unwrap_err();
    assert!(err.contains("disk full"));
}
