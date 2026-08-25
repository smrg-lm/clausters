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

use clausters::server::nrt::{encode_wav_frames, read_audio_bytes, select_channels, wav_header};

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
/// `num_frames <= 0` meaning "to the end", and `channels` selects and reorders
/// them exactly as `/buffer_allocReadChannel` does — empty being every channel.
///
/// The selection goes through the server's own `select_channels` rather than a
/// de-interleave written here: one rule, one implementation, or the two clients
/// come to disagree about what `[1, 0]` means.
///
/// Fails with the decoder's own message, which is the one a native server would
/// have replied with.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = decodeAudio))]
pub fn decode_audio(
    bytes: Vec<u8>,
    ext: &str,
    label: &str,
    file_start: usize,
    // A double, not the `i64` the decoder takes: wasm-bindgen maps `i64` to
    // `BigInt`, and every caller here has a plain JS number (the wire carries
    // this as an OSC int). The value is a frame count, so a double is exact
    // far past any file.
    num_frames: f64,
    channels: Vec<u32>,
) -> Result<Decoded, String> {
    let buffer = read_audio_bytes(bytes, ext, label, file_start, num_frames as i64)?;
    let wanted: Vec<usize> = channels.iter().map(|c| *c as usize).collect();
    let buffer = if wanted.is_empty() {
        buffer
    } else {
        select_channels(&buffer, &wanted)?
    };
    Ok(Decoded {
        samples: buffer.to_vec(),
        channels: buffer.channels(),
        frames: buffer.frames(),
        sample_rate: buffer.sample_rate(),
    })
}

/// A canonical 44-byte WAV header for `dataBytes` of sample data — the first
/// half of a file a page writes in pieces.
///
/// The recording door is two calls rather than one because the header carries a
/// length nobody knows until the take ends: a writer lays down a placeholder,
/// appends bytes as they come, and rewrites these 44 at the close.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = wavHeader))]
pub fn wav_header_bytes(
    channels: u32,
    sample_rate: u32,
    sample_format: &str,
    data_bytes: u32,
) -> Result<Vec<u8>, String> {
    wav_header(channels as u16, sample_rate, sample_format, data_bytes)
}

/// Encodes interleaved samples into WAV sample bytes, at the same scale and
/// with the same clamp a native `DiskOut` writes — which is the whole reason it
/// is here rather than in the page: a second conversion is a second answer.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = encodeWavFrames))]
pub fn encode_wav_frames_bytes(samples: &[f32], sample_format: &str) -> Result<Vec<u8>, String> {
    encode_wav_frames(samples, sample_format)
}
