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
//! - **`process`** (audio thread): only pop/push the ring -- no allocation,
//!   no locking, no I/O. A ring underrun (disk too slow) plays silence;
//!   an overrun (DiskOut) drops samples. Both are rare with the ring sized for
//!   ~1 s of audio.
//! - **Drop** (the freed synth `Box` is dropped on the network thread via the
//!   garbage FIFO): signal the I/O thread to stop and join it. Never on the
//!   audio thread.
//!
//! **The I/O threads are in the load table.** There is one per node rather
//! than a fixed count, so they have no slot each: every reader adds to the
//! `disk-in` role and every writer to `disk-out` (`server::meters`), the
//! stretches it worked and never the parks between them. The table reaches a
//! thread through the build context (`BuildCtx::meters`), which is the
//! server's own when the server builds the synth and a table nobody reads
//! otherwise.
//!
//! Both are **mono per UGen**, like our other buffer UGens: `DiskIn` extracts
//! one channel of the file (`chan` input); a stereo file needs two `DiskIn`s.
//! `DiskOut` writes a mono WAV; record stereo with two `DiskOut`s to two paths.
//! `DiskIn` streams one file frame per server sample (no resampling -- pitch
//! follows the sample-rate ratio, as in scsynth's `DiskIn`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::JoinHandle;

use rtrb::{Consumer, Producer, RingBuffer};

use crate::dsp::decode::Decoded;
use crate::dsp::io_thread::{Turn, wait_for};
use crate::dsp::registry::{BuildCtx, UGenConfig};
use crate::dsp::{ProcessCtx, UGen, at};
use crate::server::meters::Role;

/// Ring capacity in samples (~1.4 s of mono audio at 48 kHz; less per channel
/// for multichannel reads). Absorbs disk/scheduler jitter.
const RING_SAMPLES: usize = 1 << 16;

/// Sets its flag when dropped: how a reader thread says it has returned,
/// whichever way it left.
struct Ended(Arc<AtomicBool>);

impl Drop for Ended {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
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
    /// The reader thread has returned: nothing more will arrive.
    ended: Arc<AtomicBool>,
    /// In an offline render: wait for each frame rather than play silence.
    offline: bool,
}

impl DiskIn {
    /// Opens the file and starts its reader, which accounts its work to the
    /// build's load table.
    pub fn open(config: &UGenConfig, ctx: &BuildCtx) -> Self {
        let meters = &ctx.meters;
        let Some(path) = config.path.clone() else {
            tracing::warn!("DiskIn has no path; it will be silent");
            return Self { active: None };
        };
        // **A W64 or an RF64 is read by arithmetic** (`dsp::pcm`): symphonia
        // reads neither, and a take past WAV's 4 GB is one of the two.
        let long = crate::dsp::pcm::Layout::open(&path)
            .ok()
            .filter(|l| l.container != crate::dsp::pcm::Container::Wav);
        let opened = match long {
            Some(layout) => Opened::Pcm(layout),
            None => match Decoded::open(&path) {
                Ok(o) => Opened::Decoded(o),
                Err(e) => {
                    tracing::warn!("DiskIn: {e}; it will be silent");
                    return Self { active: None };
                }
            },
        };
        let channels = match &opened {
            Opened::Pcm(layout) => layout.channels,
            Opened::Decoded(d) => d.channels,
        };
        // The ring holds whole interleaved frames; round capacity down.
        let cap = (RING_SAMPLES / channels).max(1) * channels;
        let (producer, consumer) = RingBuffer::new(cap);
        let stop = Arc::new(AtomicBool::new(false));
        let ended = Arc::new(AtomicBool::new(false));
        let looping = config.looping;
        let start = config.start;
        let handle = {
            let stop = Arc::clone(&stop);
            let done = Ended(Arc::clone(&ended));
            let turn = Turn::new(Arc::clone(meters), Role::DiskIn);
            std::thread::Builder::new()
                .name("diskin".into())
                .spawn(move || {
                    let _done = done;
                    match opened {
                        Opened::Pcm(layout) => {
                            pcm_reader_thread(layout, looping, start, producer, stop, turn)
                        }
                        Opened::Decoded(d) => {
                            reader_thread(d, looping, start, producer, stop, turn)
                        }
                    }
                })
                .expect("failed to spawn the DiskIn thread")
        };
        Self {
            active: Some(DiskInActive {
                consumer,
                channels,
                stop,
                handle: Some(handle),
                ended,
                offline: ctx.offline,
            }),
        }
    }
}

impl UGen for DiskIn {
    fn reset(&mut self) {
        // A stream is its I/O thread's and cannot be restarted from here: a
        // `Voices` node refuses a def that holds one.
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let Some(a) = self.active.as_mut() else {
            output.fill(0.0);
            return;
        };
        let channel = (inputs[0][0].max(0.0) as usize).min(a.channels - 1);
        let chans = a.channels;
        for s in output.iter_mut() {
            if a.offline && a.consumer.slots() < chans {
                // Offline the frame is waited for, as scsynth's NRT reads it:
                // until it is there or the reader has nothing more.
                let (consumer, ended) = (&a.consumer, &a.ended);
                wait_for(|| consumer.slots() >= chans || ended.load(Ordering::Acquire));
            }
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

/// What a `DiskIn` reads: a PCM file by arithmetic, or anything through the
/// decoder.
enum Opened {
    Pcm(crate::dsp::pcm::Layout),
    Decoded(Decoded),
}

/// [`reader_thread`] for a PCM file read by arithmetic: the start is a seek
/// rather than a decode up to it, and a loop goes back to frame 0.
fn pcm_reader_thread(
    layout: crate::dsp::pcm::Layout,
    looping: bool,
    start: u64,
    mut producer: Producer<f32>,
    stop: Arc<AtomicBool>,
    mut turn: Turn,
) {
    let mut reader = match crate::dsp::pcm::Reader::open(&layout) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("DiskIn: {e}; it will be silent");
            return;
        }
    };
    let channels = layout.channels;
    let mut frame = start;
    let mut span = Vec::new();
    loop {
        if stop.load(Ordering::Acquire) {
            return;
        }
        let room = producer.slots() / channels;
        if room < 1024 {
            turn.park();
            continue;
        }
        turn.worked();
        match reader.read(frame, room.min(4096), &mut span) {
            Ok(0) if looping && layout.frames > 0 => frame = 0,
            Ok(0) | Err(_) => return,
            Ok(n) => {
                if let Ok(chunk) = producer.write_chunk_uninit(span.len()) {
                    chunk.fill_from_iter(span.iter().copied());
                }
                frame += n as u64;
            }
        }
    }
}

/// Decode loop: push interleaved f32 frames into `producer`, from frame `start`
/// on and restarting from the top of the file when `looping`. Exits on `stop`
/// or end of stream.
///
/// **The start is reached by decoding up to it**, on this thread, and not by a
/// seek: a compressed format has no exact frame seek (the reason
/// `/buffer_read` decodes whole), and the frames before it are thrown away
/// before the ring sees them, so the first one the audio thread pops is
/// `start` whatever the format.
fn reader_thread(
    mut opened: Decoded,
    looping: bool,
    start: u64,
    mut producer: Producer<f32>,
    stop: Arc<AtomicBool>,
    mut turn: Turn,
) {
    let mut pending: Vec<f32> = Vec::new();
    let mut idx = 0usize;
    // samples still to throw away before the start
    let mut skip = usize::try_from(start)
        .unwrap_or(usize::MAX)
        .saturating_mul(opened.channels);
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
                Ok(()) => {
                    idx += 1;
                    turn.worked();
                }
                Err(_) => turn.park(),
            }
        }
        idx = 0;

        // Reading and decoding a packet is the work, whatever it yields.
        turn.worked();
        match opened.next(&mut pending) {
            Ok(true) => {
                let dropped = skip.min(pending.len());
                idx = dropped;
                skip -= dropped;
            }
            // End of stream: restart from the top (exact, no seek) when
            // looping, otherwise we are done.
            Ok(false) if looping => {
                if opened.again().is_err() {
                    return;
                }
            }
            Ok(false) | Err(_) => return,
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
    /// In an offline render: wait for room rather than drop a sample.
    offline: bool,
}

impl DiskOut {
    /// Starts the writer, which accounts its work to the build's load table.
    pub fn open(config: &UGenConfig, ctx: &BuildCtx) -> Self {
        let meters = &ctx.meters;
        let Some(path) = config.path.clone() else {
            tracing::warn!("DiskOut has no path; it will discard its input");
            return Self { active: None };
        };
        let format = config.format.clone().unwrap_or_else(|| "int16".into());
        if let Err(e) = crate::dsp::pcm::Encoding::named(&format) {
            tracing::warn!("DiskOut: {e}; it will discard its input");
            return Self { active: None };
        }
        let (producer, consumer) = RingBuffer::new(RING_SAMPLES);
        let stop = Arc::new(AtomicBool::new(false));
        let sample_rate = Arc::new(AtomicU32::new(0));
        let handle = {
            let stop = Arc::clone(&stop);
            let sr = Arc::clone(&sample_rate);
            let turn = Turn::new(Arc::clone(meters), Role::DiskOut);
            std::thread::Builder::new()
                .name("diskout".into())
                .spawn(move || writer_thread(path, format, consumer, stop, sr, turn))
                .expect("failed to spawn the DiskOut thread")
        };
        Self {
            active: Some(DiskOutActive {
                producer,
                sample_rate,
                stop,
                handle: Some(handle),
                rate_published: false,
                offline: ctx.offline,
            }),
        }
    }
}

impl UGen for DiskOut {
    fn reset(&mut self) {
        // A stream is its I/O thread's and cannot be restarted from here: a
        // `Voices` node refuses a def that holds one.
    }

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
            if a.offline && a.producer.is_full() {
                // Offline the writer is waited for; nothing is dropped.
                let producer = &a.producer;
                wait_for(|| !producer.is_full());
            }
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
    mut turn: Turn,
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
        turn.park();
    };

    // Creating the file, writing into it and finalizing it are the work. The
    // container is the path's: a WAV, closed as an RF64 if the take outgrew
    // 4 GB, or a W64 or an RF64 (`dsp::pcm`).
    turn.worked();
    let mut writer = match crate::dsp::pcm::Writer::create(&path, 1, f64::from(rate), &format) {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!("DiskOut: {e}");
            return;
        }
    };
    let mut run: Vec<f32> = Vec::with_capacity(4096);
    let mut write_one = |writer: &mut crate::dsp::pcm::Writer, s: f32| {
        run.push(s);
        if run.len() == run.capacity() {
            let ok = writer.write(&run).is_ok();
            run.clear();
            ok
        } else {
            true
        }
    };

    loop {
        match consumer.pop() {
            Ok(s) => {
                turn.worked();
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
                turn.park();
            }
        }
    }
    turn.worked();
    let tail = std::mem::take(&mut run);
    if let Err(e) = writer.write(&tail).and_then(|()| writer.finalize()) {
        tracing::warn!("DiskOut: {e}");
    }
}
