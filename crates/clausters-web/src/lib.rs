//! The engine's JS door (the B track): a thin wasm-bindgen shell over the
//! `clausters` server crate, the browser sibling of `clausters-ffi` (the C
//! door) and of the embed C ABI in `src/embed.rs`.
//!
//! The shell owns no logic: everything it exposes is a one-call wrapper over
//! the crate's own entry points (`Score::from_bytes`, `render_to_vec`), so
//! native cargo tests exercise the identical code path the browser runs. The
//! wasm-bindgen attributes exist only on the wasm target; natively this is a
//! plain rlib.
//!
//! Workers are always 0 on wasm: the browser has no worker-pool threads, and
//! `workers = 0` is the sequential in-thread schedule, bit-identical to any
//! worker count by design.
//!
//! Parity caveat (recorded in `docs/decisions.md`): wasm has no flush-to-zero
//! mode, so native↔wasm bit-identity holds where the render stays out of the
//! denormal range — the parity harness asserts on denormal-free scores.

use clausters::server::render::{RenderConfig, Score, render_to_vec};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// The embed / IPC ABI version this build speaks (`clausters_abi_version`).
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn abi_version() -> u32 {
    clausters::embed::clausters_abi_version()
}

/// Renders a binary score (the `--nrt` format: length-prefixed OSC packets,
/// timetags in seconds from the start) synchronously into interleaved `f32`
/// samples (`frames * channels`). The JS side receives a `Float32Array`.
fn render_score(score: &[u8], sample_rate: f64, channels: u32) -> Result<Vec<f32>, String> {
    let score = Score::from_bytes(score)?;
    let cfg = RenderConfig {
        sample_rate,
        channels: channels as usize,
        workers: 0,
    };
    render_to_vec(&score, &cfg).map(|(samples, _stats)| samples)
}

/// Native face of [`render`], for the in-crate tests.
#[cfg(not(target_arch = "wasm32"))]
pub fn render(score: &[u8], sample_rate: f64, channels: u32) -> Result<Vec<f32>, String> {
    render_score(score, sample_rate, channels)
}

/// JS face: `render(scoreBytes, sampleRate, channels) -> Float32Array`,
/// throwing a `JsError` with the render's message on failure.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn render(score: &[u8], sample_rate: f64, channels: u32) -> Result<Vec<f32>, JsError> {
    render_score(score, sample_rate, channels).map_err(|e| JsError::new(&e))
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use clausters::rosc::{OscBundle, OscMessage, OscPacket, OscTime, OscType, encoder};

    /// One length-prefixed `/s_new` + end bundle in the binary score format.
    fn tiny_score() -> Vec<u8> {
        let packet = |secs: u32, msgs: Vec<OscMessage>| {
            let bundle = OscPacket::Bundle(OscBundle {
                timetag: OscTime {
                    seconds: secs,
                    fractional: 0,
                },
                content: msgs.into_iter().map(OscPacket::Message).collect(),
            });
            encoder::encode(&bundle).unwrap()
        };
        let s_new = OscMessage {
            addr: "/s_new".into(),
            args: vec![
                OscType::String("default".into()),
                OscType::Int(1000),
                OscType::Int(0),
                OscType::Int(0),
                OscType::String("freq".into()),
                OscType::Float(440.0),
            ],
        };
        let n_free = OscMessage {
            addr: "/n_free".into(),
            args: vec![OscType::Int(1000)],
        };
        let mut score = Vec::new();
        for bytes in [packet(0, vec![s_new]), packet(1, vec![n_free])] {
            score.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
            score.extend_from_slice(&bytes);
        }
        score
    }

    /// The shell renders through the same path the browser calls: a one-second
    /// default-def note comes out with signal in it.
    #[test]
    fn shell_renders_a_score() {
        let out = super::render(&tiny_score(), 48000.0, 2).unwrap();
        assert_eq!(out.len(), 2 * 48000);
        let rms = (out.iter().map(|x| x * x).sum::<f32>() / out.len() as f32).sqrt();
        assert!(rms > 0.05, "audible signal expected, rms = {rms}");
        assert!(out.iter().all(|x| x.is_finite()));
    }

    /// A malformed score reports, not panics.
    #[test]
    fn shell_reports_bad_scores() {
        assert!(super::render(&[1, 2, 3], 48000.0, 2).is_err());
    }
}
