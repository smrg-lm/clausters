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
use std::time::Duration;

use rtrb::{Consumer, Producer, RingBuffer};

use crate::dsp::registry::{BuildCtx, UGenConfig};
use crate::dsp::{ProcessCtx, UGen, at};
use crate::server::meters::{Meters, Role, Stamp, stamp};

/// Ring capacity in samples (~1.4 s of mono audio at 48 kHz; less per channel
/// for multichannel reads). Absorbs disk/scheduler jitter.
const RING_SAMPLES: usize = 1 << 16;

/// How long the I/O thread parks when its ring is full/empty before retrying.
const PARK: Duration = Duration::from_millis(2);

// ---- the load table ----

/// An I/O thread's account in the load table: the stretches it worked, never
/// the parks between them.
///
/// A stretch runs from one park to the next and is added only if something was
/// done in it -- a thread that wakes to a ring still full, or still empty, did
/// no work and is not a call. Dropping it closes the last stretch, so every
/// way out of a thread's loop is accounted.
pub(crate) struct Turn {
    meters: Arc<Meters>,
    role: Role,
    since: Stamp,
    worked: bool,
}

impl Turn {
    pub(crate) fn new(meters: Arc<Meters>, role: Role) -> Self {
        Self {
            meters,
            role,
            since: stamp(),
            worked: false,
        }
    }

    /// Something was done in this stretch.
    pub(crate) fn worked(&mut self) {
        self.worked = true;
    }

    /// Closes the stretch, parks, and opens the next one. A park is cut short
    /// by an unpark, which is how a reader that follows the transport is
    /// woken for a refill.
    pub(crate) fn park(&mut self) {
        self.close();
        std::thread::park_timeout(PARK);
        self.since = stamp();
    }

    fn close(&mut self) {
        if self.worked {
            self.meters.add(self.role, 0, self.since.elapsed_nanos());
            self.worked = false;
        }
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        self.close();
    }
}

/// **Waits for `ready`**, for a stream an offline render plays: nothing waits
/// on the clock there, so a `DiskIn` waits for its frames and a `DiskOut` for
/// room rather than losing the race. Gives up after ten seconds of nothing --
/// a thread that died -- and says so by answering `false`.
pub(crate) fn wait_for(mut ready: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut spins = 0u32;
    while !ready() {
        if std::time::Instant::now() > deadline {
            return false;
        }
        spins += 1;
        if spins < 64 {
            std::thread::yield_now();
        } else {
            std::thread::sleep(Duration::from_micros(50));
        }
    }
    true
}

/// Sets its flag when dropped: how a reader thread says it has returned,
/// whichever way it left.
struct Ended(Arc<AtomicBool>);

impl Drop for Ended {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

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
            None => match open_file(&path) {
                Ok(o) => Opened::Decoded(o),
                Err(e) => {
                    tracing::warn!("DiskIn: {e}; it will be silent");
                    return Self { active: None };
                }
            },
        };
        let channels = match &opened {
            Opened::Pcm(layout) => layout.channels,
            Opened::Decoded(o) => o.channels,
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
                        Opened::Decoded(o) => {
                            reader_thread(path, o, looping, start, producer, stop, turn)
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
    Decoded(OpenFile),
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
    path: String,
    mut opened: OpenFile,
    looping: bool,
    start: u64,
    mut producer: Producer<f32>,
    stop: Arc<AtomicBool>,
    mut turn: Turn,
) {
    use symphonia::core::errors::Error as SymError;

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
        pending.clear();
        idx = 0;

        // Reading and decoding a packet is the work, whatever it yields.
        turn.worked();
        match opened.format.next_packet() {
            Ok(Some(packet)) => {
                if packet.track_id != opened.track_id {
                    continue;
                }
                match opened.decoder.decode(&packet) {
                    Ok(decoded) => {
                        decoded.copy_to_vec_interleaved(&mut pending);
                        let dropped = skip.min(pending.len());
                        idx = dropped;
                        skip -= dropped;
                    }
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
    // container is the path's: a WAV, or a W64 or an RF64 for a take past
    // WAV's 4 GB (`dsp::pcm`).
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
