//! Streaming disk I/O UGens: `DiskIn` (file -> signal) and `DiskOut`
//! (signal -> WAV file).
//!
//! Unlike `PlayBuf`/`BufRd`, which read a buffer already fully in memory,
//! these stream to/from disk in real time so arbitrarily long files never
//! touch the buffer pool. The design is **self-contained**: each UGen owns one
//! background I/O thread and a single-producer/single-consumer lock-free ring
//! ([`rtrb`]) shared with the audio thread.
//!
//! - **Build** (`/synth_new`, on the network thread): open the file and spawn the
//!   I/O thread. Allocating here is fine.
//! - **`process`** (audio thread): only pop/push the ring — no allocation,
//!   no locking, no I/O. A ring underrun (disk too slow) plays silence;
//!   an overrun (DiskOut) drops samples. Both are rare with the ring sized for
//!   ~1 s of audio.
//! - **Drop** (the freed synth `Box` is dropped on the network thread via the
//!   garbage FIFO): signal the I/O thread to stop and join it. Never on the
//!   audio thread.
//!
//! Both are **mono per UGen**, like our other buffer UGens: `DiskIn` extracts
//! one channel of the file (`chan` input); a stereo file needs two `DiskIn`s.
//! `DiskOut` writes a mono WAV; record stereo with two `DiskOut`s to two paths.
//! `DiskIn` streams one file frame per server sample (no resampling — pitch
//! follows the sample-rate ratio, as in scsynth's `DiskIn`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use rtrb::{Consumer, Producer, RingBuffer};

use crate::dsp::registry::UGenConfig;
use crate::dsp::{ProcessCtx, UGen, at};
use crate::server::nrt::wav_format;

/// Ring capacity in samples (~1.4 s of mono audio at 48 kHz; less per channel
/// for multichannel reads). Absorbs disk/scheduler jitter.
const RING_SAMPLES: usize = 1 << 16;

/// How long the I/O thread parks when its ring is full/empty before retrying.
const PARK: Duration = Duration::from_millis(2);

// ---- symphonia open helper (streaming) ----

struct OpenFile {
    format: Box<dyn symphonia::core::formats::FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::audio::AudioDecoder>,
    track_id: u32,
    channels: usize,
}

/// Opens `path` for streaming and returns the reader/decoder plus its channel
/// count. Called on the network thread (build) and again by the reader thread
/// on each loop restart.
fn open_file(path: &str) -> Result<OpenFile, String> {
    use symphonia::core::codecs::CodecParameters;
    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::{FormatOptions, TrackType};
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;

    let err = |e: String| format!("{path}: {e}");
    let file = std::fs::File::open(path).map_err(|e| err(e.to_string()))?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
    {
        hint.with_extension(ext);
    }
    let format = symphonia::default::get_probe()
        .probe(
            &hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| err(e.to_string()))?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| err("no audio track".into()))?;
    let params = match track.codec_params.as_ref() {
        Some(CodecParameters::Audio(params)) => params,
        _ => return Err(err("default track is not audio".into())),
    };
    let channels = params.channels.as_ref().map_or(0, |c| c.count());
    if channels == 0 {
        return Err(err("could not determine channel count".into()));
    }
    let track_id = track.id;
    let decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|e| err(e.to_string()))?;
    Ok(OpenFile {
        format,
        decoder,
        track_id,
        channels,
    })
}

// ---- DiskIn ----

/// Streams a file from disk. Input 0: channel selector (constant per block,
/// like `PlayBuf`'s `chan`). Output: that channel, one file frame per sample.
/// A file the server could not open is inert (silent).
pub struct DiskIn {
    active: Option<DiskInActive>,
}

struct DiskInActive {
    consumer: Consumer<f32>,
    channels: usize,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl DiskIn {
    pub fn open(config: &UGenConfig) -> Self {
        let Some(path) = config.path.clone() else {
            tracing::warn!("DiskIn has no path; it will be silent");
            return Self { active: None };
        };
        let opened = match open_file(&path) {
            Ok(o) => o,
            Err(e) => {
                tracing::warn!("DiskIn: {e}; it will be silent");
                return Self { active: None };
            }
        };
        let channels = opened.channels;
        // The ring holds whole interleaved frames; round capacity down.
        let cap = (RING_SAMPLES / channels).max(1) * channels;
        let (producer, consumer) = RingBuffer::new(cap);
        let stop = Arc::new(AtomicBool::new(false));
        let looping = config.looping;
        let handle = {
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("diskin".into())
                .spawn(move || reader_thread(path, opened, looping, producer, stop))
                .expect("failed to spawn the DiskIn thread")
        };
        Self {
            active: Some(DiskInActive {
                consumer,
                channels,
                stop,
                handle: Some(handle),
            }),
        }
    }
}

impl UGen for DiskIn {
    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let Some(a) = self.active.as_mut() else {
            output.fill(0.0);
            return;
        };
        let channel = (inputs[0][0].max(0.0) as usize).min(a.channels - 1);
        let chans = a.channels;
        for s in output.iter_mut() {
            // Only consume a full frame; otherwise the ring stays frame-aligned
            // and we emit silence for this sample (underrun).
            if a.consumer.slots() >= chans {
                let mut sample = 0.0;
                for c in 0..chans {
                    let v = a.consumer.pop().unwrap_or(0.0);
                    if c == channel {
                        sample = v;
                    }
                }
                *s = sample;
            } else {
                *s = 0.0;
            }
        }
    }
}

impl Drop for DiskIn {
    fn drop(&mut self) {
        if let Some(a) = self.active.as_mut() {
            a.stop.store(true, Ordering::Release);
            if let Some(h) = a.handle.take() {
                let _ = h.join();
            }
        }
    }
}

/// Decode loop: push interleaved f32 frames into `producer`, restarting from
/// the top of the file when `looping`. Exits on `stop` or end of stream.
fn reader_thread(
    path: String,
    mut opened: OpenFile,
    looping: bool,
    mut producer: Producer<f32>,
    stop: Arc<AtomicBool>,
) {
    use symphonia::core::errors::Error as SymError;

    let mut pending: Vec<f32> = Vec::new();
    let mut idx = 0usize;
    loop {
        if stop.load(Ordering::Acquire) {
            return;
        }
        // Drain whatever is left from the last decoded packet before reading
        // the next one, parking when the ring is full.
        while idx < pending.len() {
            if stop.load(Ordering::Acquire) {
                return;
            }
            match producer.push(pending[idx]) {
                Ok(()) => idx += 1,
                Err(_) => std::thread::sleep(PARK),
            }
        }
        pending.clear();
        idx = 0;

        match opened.format.next_packet() {
            Ok(Some(packet)) => {
                if packet.track_id != opened.track_id {
                    continue;
                }
                match opened.decoder.decode(&packet) {
                    Ok(decoded) => decoded.copy_to_vec_interleaved(&mut pending),
                    Err(SymError::DecodeError(_)) => continue,
                    Err(_) => return,
                }
            }
            Ok(None) => {
                // End of stream: restart from the top (exact, no seek) when
                // looping, otherwise we are done.
                if looping {
                    match open_file(&path) {
                        Ok(o) => opened = o,
                        Err(_) => return,
                    }
                } else {
                    return;
                }
            }
            Err(_) => return,
        }
    }
}

// ---- DiskOut ----

/// Streams its input signal to a mono WAV file. Input 0: the signal (passed
/// through to the output, so a `DiskOut` can sit mid-chain). The file's sample
/// rate is the server's; `format` is `int16` (default), `int24` or `float`.
pub struct DiskOut {
    active: Option<DiskOutActive>,
}

struct DiskOutActive {
    producer: Producer<f32>,
    sample_rate: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    /// Set once we have published the server sample rate to the writer thread.
    rate_published: bool,
}

impl DiskOut {
    pub fn open(config: &UGenConfig) -> Self {
        let Some(path) = config.path.clone() else {
            tracing::warn!("DiskOut has no path; it will discard its input");
            return Self { active: None };
        };
        let format = config.format.clone().unwrap_or_else(|| "int16".into());
        if let Err(e) = wav_format(&format) {
            tracing::warn!("DiskOut: {e}; it will discard its input");
            return Self { active: None };
        }
        let (producer, consumer) = RingBuffer::new(RING_SAMPLES);
        let stop = Arc::new(AtomicBool::new(false));
        let sample_rate = Arc::new(AtomicU32::new(0));
        let handle = {
            let stop = Arc::clone(&stop);
            let sr = Arc::clone(&sample_rate);
            std::thread::Builder::new()
                .name("diskout".into())
                .spawn(move || writer_thread(path, format, consumer, stop, sr))
                .expect("failed to spawn the DiskOut thread")
        };
        Self {
            active: Some(DiskOutActive {
                producer,
                sample_rate,
                stop,
                handle: Some(handle),
                rate_published: false,
            }),
        }
    }
}

impl UGen for DiskOut {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let signal = inputs[0];
        // Pass the signal through regardless, so this UGen can be heard.
        for (i, s) in output.iter_mut().enumerate() {
            *s = at(signal, i);
        }
        let Some(a) = self.active.as_mut() else {
            return;
        };
        // The writer needs the sample rate for the WAV header; publish it once
        // from the audio thread (an atomic store, RT-safe).
        if !a.rate_published {
            a.sample_rate
                .store(ctx.sample_rate as u32, Ordering::Release);
            a.rate_published = true;
        }
        for s in output.iter() {
            // Drop on overrun (writer behind): a fixed ring never blocks here.
            let _ = a.producer.push(*s);
        }
    }
}

impl Drop for DiskOut {
    fn drop(&mut self) {
        if let Some(a) = self.active.as_mut() {
            a.stop.store(true, Ordering::Release);
            if let Some(h) = a.handle.take() {
                let _ = h.join();
            }
        }
    }
}

/// Consume samples from `consumer` and write them to a mono WAV. Waits for the
/// audio thread to publish the sample rate, then drains until `stop` and
/// finalizes the header.
fn writer_thread(
    path: String,
    format: String,
    mut consumer: Consumer<f32>,
    stop: Arc<AtomicBool>,
    sample_rate: Arc<AtomicU32>,
) {
    // Wait for the first process() to publish the rate (or an early stop).
    let rate = loop {
        let sr = sample_rate.load(Ordering::Acquire);
        if sr != 0 {
            break sr;
        }
        if stop.load(Ordering::Acquire) {
            return; // freed before it ever ran: nothing to write
        }
        std::thread::sleep(PARK);
    };

    let (bits, sample_format) = match wav_format(&format) {
        Ok(v) => v,
        Err(_) => return,
    };
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: bits,
        sample_format,
    };
    let mut writer = match hound::WavWriter::create(&path, spec) {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!("DiskOut: {path}: {e}");
            return;
        }
    };
    let scale = ((1u64 << (bits - 1)) - 1) as f32;
    let write_one = |writer: &mut hound::WavWriter<std::io::BufWriter<std::fs::File>>, s: f32| {
        let r = match sample_format {
            hound::SampleFormat::Float => writer.write_sample(s),
            hound::SampleFormat::Int => {
                writer.write_sample((s.clamp(-1.0, 1.0) * scale).round() as i32)
            }
        };
        r.is_ok()
    };

    loop {
        match consumer.pop() {
            Ok(s) => {
                if !write_one(&mut writer, s) {
                    break;
                }
            }
            Err(_) => {
                // Ring empty: if asked to stop, drain anything that arrived in
                // the meantime and finish; otherwise park and retry.
                if stop.load(Ordering::Acquire) {
                    while let Ok(s) = consumer.pop() {
                        if !write_one(&mut writer, s) {
                            break;
                        }
                    }
                    break;
                }
                std::thread::sleep(PARK);
            }
        }
    }
    if let Err(e) = writer.finalize() {
        tracing::warn!("DiskOut: {path}: {e}");
    }
}
