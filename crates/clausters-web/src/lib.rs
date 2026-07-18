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

/// The live engine in pulled mode: a 1:1 JS face over
/// `clausters::embed::ClaustersHeadless` (which owns all the logic and is
/// exercised by the native `tests/headless.rs` suite). The AudioWorklet
/// processor drives it: OSC packets in over `send`, one `process` per render
/// quantum, replies drained with `poll`.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub struct WebServer {
    inner: clausters::embed::ClaustersHeadless,
    reply_buf: Vec<u8>,
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
impl WebServer {
    /// `unix_epoch`: Unix seconds at sample 0 (JS: `Date.now() / 1000`), the
    /// anchor that lets wall-clocked clients' bundle timetags land on this
    /// server's sample axis.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(constructor))]
    pub fn new(sample_rate: f64, channels: u32, unix_epoch: f64) -> Result<WebServer, JsErrorish> {
        let inner =
            clausters::embed::ClaustersHeadless::new(sample_rate, channels as usize, unix_epoch)
                .map_err(err)?;
        Ok(WebServer {
            inner,
            reply_buf: vec![0; 64 * 1024],
        })
    }

    /// Pushes one complete OSC packet into the command ring. `false` =
    /// momentarily full (backpressure): retry next quantum.
    pub fn send(&self, packet: &[u8]) -> bool {
        self.inner.send(packet)
    }

    /// One pending reply as bytes, or `undefined`/`None` when none is.
    pub fn poll(&mut self) -> Option<Vec<u8>> {
        let len = self.inner.poll_into(&mut self.reply_buf)?;
        Some(self.reply_buf[..len].to_vec())
    }

    /// Renders into `out` (interleaved, a multiple of `block_frames() *
    /// channels` samples): a serving turn before each engine block.
    pub fn process(&mut self, out: &mut [f32]) -> Result<(), JsErrorish> {
        self.inner.process_block(out).map_err(err)
    }

    /// The engine block size in frames (the granularity `process` needs).
    pub fn block_frames(&self) -> u32 {
        clausters::server::engine::BLOCK_SIZE as u32
    }

    /// The engine's sample counter (block-accurate; exact in an f64 for the
    /// first 2^53 samples — thousands of years of audio).
    pub fn clock(&self) -> f64 {
        self.inner.clock() as f64
    }

    /// Whether a `/quit` arrived; the page decides what closing means.
    pub fn quit_requested(&self) -> bool {
        self.inner.quit_requested()
    }

    /// Data-plane control-bus write (no command round trip).
    pub fn ctl_set(&self, index: u32, value: f32) {
        self.inner.ctl_set(index as usize, value);
    }

    /// Data-plane control-bus read.
    pub fn ctl_get(&self, index: u32) -> f32 {
        self.inner.ctl_get(index as usize)
    }

    /// Installs host-decoded samples as buffer `index` (the browser's
    /// `/b_allocRead` replacement: fetch + `decodeAudioData`, then this).
    pub fn b_load(
        &mut self,
        index: u32,
        channels: u32,
        sample_rate: f64,
        data: &[f32],
    ) -> Result<(), JsErrorish> {
        self.inner
            .b_load(index as usize, channels as usize, sample_rate, data)
            .map_err(err)
    }
}

/// The error type of the JS-facing results: a real `JsError` on wasm, the
/// plain message natively (so the same methods compile and test on both).
#[cfg(target_arch = "wasm32")]
type JsErrorish = JsError;
#[cfg(not(target_arch = "wasm32"))]
type JsErrorish = String;

#[cfg(target_arch = "wasm32")]
fn err(e: String) -> JsErrorish {
    JsError::new(&e)
}
#[cfg(not(target_arch = "wasm32"))]
fn err(e: String) -> JsErrorish {
    e
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

    /// The live face, exactly as the worklet drives it: send an `/s_new`,
    /// pull a second of quanta, hear the tone, drain the `/done`s.
    #[test]
    fn web_server_pulls_a_tone() {
        let mut server = super::WebServer::new(48000.0, 2, 0.0).unwrap();
        let s_new = OscMessage {
            addr: "/s_new".into(),
            args: vec![
                OscType::String("default".into()),
                OscType::Int(1000),
                OscType::Int(0),
                OscType::Int(0),
            ],
        };
        assert!(server.send(&encoder::encode(&OscPacket::Message(s_new)).unwrap()));
        let block = server.block_frames() as usize * 2;
        let mut quantum = vec![0.0f32; 128 * 2];
        assert_eq!(
            quantum.len() % block,
            0,
            "128-frame quanta hold whole blocks"
        );
        let mut energy = 0.0f32;
        for _ in 0..375 {
            server.process(&mut quantum).unwrap();
            energy += quantum.iter().map(|x| x * x).sum::<f32>();
        }
        assert_eq!(server.clock(), 48000.0, "one second of quanta");
        let rms = (energy / (375.0 * quantum.len() as f32)).sqrt();
        assert!(rms > 0.05, "audible tone expected, rms = {rms}");
        assert!(server.poll().is_none(), "/s_new is fire-and-forget");
        assert!(!server.quit_requested());
    }
}
