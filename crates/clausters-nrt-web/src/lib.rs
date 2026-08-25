//! The NRT worker's JS door (the B track): a thin wasm-bindgen shell over the
//! server crate's soundfile decoder, sibling of `clausters-web` (the engine's
//! door) and of `clausters-core-web` (the shared core's).
//!
//! **Why a second wasm module and not the engine's.** The work this carries is
//! the work that natively belongs to a thread that is neither audio nor UI —
//! reading and decoding a soundfile — and in a browser that thread is a
//! dedicated Worker. The Worker cannot use the engine's module: that one lives
//! in the AudioWorklet, holds the node tree, and is exactly the thread this
//! work has to leave. So the decoder is bound again, in a shell that carries
//! nothing else — no engine, no def families, no ring.
//!
//! **Why not the browser's own decoder.** `decodeAudioData` is right there and
//! is the wrong answer: it is a different decoder from the one a native server
//! runs, so the same file would become different samples in a tab and in a
//! window. That is a divergence in *values*, which is worse than one in surface
//! because nothing names it and nobody reports it. The shell owns no logic —
//! `read_audio_bytes` is the server's own answer to "read a soundfile", reached
//! here as it is reached over the C ABI.
//!
//! The Worker has no filesystem either: OPFS is a JS API, so bytes come in
//! from the page and a path never crosses. What the page reads, this decodes.

use clausters::server::nrt::read_audio_bytes;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// One decoded soundfile: interleaved `f32` samples plus the shape they are in.
///
/// The samples come out as their own `Float32Array`, which is what lets the
/// Worker **transfer** them to the worklet rather than copy them across.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub struct Decoded {
    samples: Vec<f32>,
    channels: usize,
    frames: usize,
    sample_rate: f64,
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
impl Decoded {
    /// Interleaved samples, `frames * channels` of them. Moves the vector out,
    /// so a second call returns nothing — the buffer is meant to be handed on.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(getter))]
    pub fn samples(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.samples)
    }

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(getter))]
    pub fn channels(&self) -> usize {
        self.channels
    }

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(getter))]
    pub fn frames(&self) -> usize {
        self.frames
    }

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(getter, js_name = sampleRate))]
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }
}

/// Decodes a soundfile already in memory — the Worker's whole job.
///
/// `ext` is the format hint (`"wav"`, `"flac"`, …, no dot; an empty hint still
/// probes by content). `label` names the source in an error. `file_start` and
/// `num_frames` slice it exactly as `/buffer_allocRead` does, with
/// `num_frames <= 0` meaning "to the end".
///
/// Fails with the decoder's own message, which is the one a native server would
/// have replied with.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = decodeAudio))]
pub fn decode_audio(
    bytes: Vec<u8>,
    ext: &str,
    label: &str,
    file_start: usize,
    num_frames: i64,
) -> Result<Decoded, String> {
    let buffer = read_audio_bytes(bytes, ext, label, file_start, num_frames)?;
    Ok(Decoded {
        samples: buffer.to_vec(),
        channels: buffer.channels(),
        frames: buffer.frames(),
        sample_rate: buffer.sample_rate(),
    })
}
