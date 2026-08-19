//! NRT (non-real-time) thread: disk I/O and buffer building.
//!
//! Every `/buffer_*` command that touches sample memory runs here, off both the
//! audio and the network threads: allocation, file reading (WAV via hound,
//! other formats via symphonia), WAV writing (hound) and zeroing. The
//! network thread submits [`NrtRequest`]s and drains
//! [`NrtResult`]s on its own schedule, installs the produced buffer in the
//! engine via `Cmd::SetBuffer`, and sends the async `/done`/`/fail` reply —
//! same pattern as the Faust compiler thread.
//!
//! One queue means buffer commands complete **in submission order** (a
//! `/buffer_free` right after a `/buffer_alloc` cannot overtake it), which is why even
//! `/buffer_free` — no I/O at all — travels through here.
//!
//! Every job here **replaces** a buffer rather than writing into one: `/buffer_read`
//! into an existing buffer and `/buffer_zero` build a replacement from the current
//! contents, and the engine swaps it in. A buffer's contents *are* writable
//! (see [`crate::dsp::buffer`]) — this is a property of these commands, which
//! change a buffer wholesale and off the audio thread, and not of the buffer.

use crate::osc::ClientId;
use crate::osc::wake::Waker;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::dsp::buffer::Buffer;

pub enum NrtJob {
    /// `/buffer_alloc` (and `/buffer_zero`, which knows the shape from the mirror):
    /// a fresh zeroed buffer.
    Alloc {
        frames: usize,
        channels: usize,
        sample_rate: f64,
    },
    /// `/buffer_allocRead`: shape, sample rate and contents come from the file.
    /// `num_frames <= 0` reads to the end.
    AllocRead {
        path: String,
        file_start: usize,
        num_frames: i64,
        /// `/buffer_allocReadChannel`'s channel selection, in the order given;
        /// empty is every channel, which is what `/buffer_allocRead` sends.
        channels: Vec<usize>,
    },
    /// `/buffer_read`: overlay file data onto a copy of the current contents,
    /// starting at `buf_start`, keeping the buffer's shape.
    Read {
        path: String,
        file_start: usize,
        num_frames: i64,
        buf_start: usize,
        current: Arc<Buffer>,
        /// `/buffer_readChannel`'s selection; empty is every channel.
        channels: Vec<usize>,
    },
    /// `/buffer_write`: WAV out; `sample_format` is `int16`, `int24` or `float`.
    /// `num_frames <= 0` writes from `buf_start` to the end.
    Write {
        path: String,
        sample_format: String,
        buf_start: usize,
        num_frames: i64,
        buffer: Arc<Buffer>,
    },
    /// `/buffer_gen`: fill/generate into a same-shape replacement of the current
    /// buffer (wavetable generators, waveshaping tables, buffer copies). Pure
    /// computation — no I/O — but ordered through this queue like the rest so a
    /// `/buffer_gen` cannot overtake a pending `/buffer_alloc` on the same buffer.
    Gen {
        current: Arc<Buffer>,
        cmd: crate::dsp::wavetable::GenCommand,
    },
    /// `/buffer_set`, `/buffer_setRange` and their `*Channel` forms: write
    /// client-supplied samples into a copy of the current contents, keeping the
    /// buffer's shape. Each write is a [`SampleWrite`], so the single-sample
    /// form is just a run of one and a channel-addressed write is the same run
    /// with the channel count as its stride.
    ///
    /// `base` is what the *parse* saw, and it is only a fallback: a batch of
    /// writes to one buffer is submitted before any of them completes, so every
    /// one of them would otherwise copy the same pre-batch contents and the last
    /// installed would silently erase the rest. The runner therefore chains
    /// them — see [`NrtChain`].
    Set {
        base: Arc<Buffer>,
        writes: Vec<SampleWrite>,
    },
    /// `/buffer_gain` and `/buffer_reverse`: a destructive edit over a span of
    /// the current contents, laid into a copy that replaces the buffer, like
    /// every other write here. The arithmetic itself is
    /// [`clausters_core::edit`], shared with every other process that edits
    /// samples rather than reimplemented per caller.
    ///
    /// `base` is the parse's snapshot and is only a fallback, exactly as in
    /// [`NrtJob::Set`]: a batch of edits on one buffer is submitted before any
    /// of them completes, so the runner chains them (see [`NrtChain`]).
    Edit { base: Arc<Buffer>, op: EditOp },
    /// `/buffer_fill`: runs of one repeated value, addressed flat like
    /// [`NrtJob::Set`] — `(start, count, value)` each. Kept apart from `Set`
    /// rather than expanded into one: a fill says how *many* samples it writes
    /// and materializing them would allocate the run it exists to avoid.
    ///
    /// Chained on `base` for the same reason `Set` is.
    Fill {
        base: Arc<Buffer>,
        fills: Vec<(usize, usize, f32)>,
    },
    /// `/buffer_free`: ordered behind the other jobs (see module docs).
    Free,
}

/// One run of client-supplied samples, in the storage's own terms: where it
/// starts (a **flat, interleaved** index) and how far apart consecutive values
/// land.
///
/// The stride is what makes one job serve both addressings. `/buffer_set` and
/// `/buffer_setRange` write *adjacent* samples — stride 1 — which is the right
/// shape for filling a buffer and the wrong one for editing a **channel** of
/// one: in interleaved storage a channel's frames are `channels` apart, so a
/// stereo left channel is a strided run and no contiguous command can express
/// it. Rather than a second job, a second copy-and-swap and a second set of
/// bounds checks, the write carries its own step and the flat forms pass 1.
#[derive(Debug, Clone, PartialEq)]
pub struct SampleWrite {
    /// The first sample's flat index.
    pub at: usize,
    /// The distance between consecutive values: 1 for a flat run, the buffer's
    /// channel count for one channel of it.
    pub stride: usize,
    pub values: Vec<f32>,
}

impl SampleWrite {
    /// A flat (interleaved) run, which is what the non-`Channel` commands send.
    pub fn flat(at: usize, values: Vec<f32>) -> Self {
        SampleWrite {
            at,
            stride: 1,
            values,
        }
    }

    /// One past the last sample this write touches — what a bounds check
    /// compares against, and the one place the stride is arithmetic rather than
    /// a step.
    pub fn end(&self) -> usize {
        match self.values.len() {
            0 => self.at,
            n => self.at + (n - 1) * self.stride + 1,
        }
    }
}

/// One destructive edit, parsed. The span is in **frames** — a selection is a
/// stretch of time across every channel, which is a different unit from the
/// flat interleaved index `/buffer_set*` speaks.
#[derive(Debug, Clone, Copy)]
pub enum EditOp {
    /// Scale by a factor sweeping `from` to `to` along an envelope shape:
    /// constant gain, a fade either way, or silence.
    Gain {
        start: usize,
        frames: usize,
        from: f32,
        to: f32,
        shape: i32,
        curve: f32,
    },
    /// Turn the span's frames around, channels untouched inside each frame.
    Reverse { start: usize, frames: usize },
}

impl EditOp {
    /// The frames this edit touches, as `(start, frames)` — what an in-place
    /// write reads out and puts back, so the cost is the edit's rather than the
    /// samples's.
    pub fn span(&self) -> (usize, usize) {
        match self {
            EditOp::Gain { start, frames, .. } | EditOp::Reverse { start, frames } => {
                (*start, *frames)
            }
        }
    }
}

pub struct NrtRequest {
    /// Command name for the `/done`/`/fail` reply (e.g. `"/buffer_alloc"`).
    pub cmd: &'static str,
    pub index: i32,
    /// Who asked: the async reply goes back to this client.
    pub client: ClientId,
    /// Whether this job must build on what the **queue** last produced for
    /// `index` rather than on the snapshot its parse took. True exactly when
    /// the submitter still has work in flight for that buffer, which is when
    /// the network-side mirror is behind (see [`NrtChain`]).
    pub chained: bool,
    pub job: NrtJob,
}

/// What the network thread must do with the pool on success.
#[derive(Debug)]
pub enum NrtAction {
    Install(Arc<Buffer>),
    Clear,
    /// Nothing to install (`/buffer_write`): just reply `/done`.
    None,
}

pub struct NrtResult {
    pub cmd: &'static str,
    pub index: i32,
    pub client: ClientId,
    pub outcome: Result<NrtAction, String>,
}

/// What the queue has most recently produced per buffer index.
///
/// The queue is the serialization point for buffer mutation, so "the current
/// contents" of a buffer means *current in the queue*, not current in the
/// network-side mirror — the mirror only catches up when results are drained,
/// which happens after a whole batch has been submitted. A job that builds a
/// replacement from the existing contents ([`NrtJob::Set`]) therefore takes its
/// base from here when the queue has already produced one.
///
/// The chain is consulted **only while the queue still owes work on that
/// index**, which the submitter says with [`NrtRequest::chained`]. With nothing
/// in flight the network-side mirror has caught up and its snapshot is the
/// authority — which is also what keeps a buffer installed outside the queue
/// (the embed door's `install_buffer`) from being undone by a stale entry.
#[derive(Default)]
pub struct NrtChain(std::collections::HashMap<i32, Arc<Buffer>>);

impl NrtChain {
    /// Swaps in the queue's own view of the buffer this job builds on, and
    /// records what the job produced for the next one.
    fn run(&mut self, index: i32, chained: bool, job: NrtJob) -> Result<NrtAction, String> {
        let job = match job {
            NrtJob::Set { base, writes } if chained => NrtJob::Set {
                base: self.0.get(&index).cloned().unwrap_or(base),
                writes,
            },
            NrtJob::Edit { base, op } if chained => NrtJob::Edit {
                base: self.0.get(&index).cloned().unwrap_or(base),
                op,
            },
            NrtJob::Fill { base, fills } if chained => NrtJob::Fill {
                base: self.0.get(&index).cloned().unwrap_or(base),
                fills,
            },
            other => other,
        };
        let outcome = run_job(job);
        match &outcome {
            Ok(NrtAction::Install(buffer)) => {
                self.0.insert(index, Arc::clone(buffer));
            }
            Ok(NrtAction::Clear) => {
                self.0.remove(&index);
            }
            _ => {}
        }
        outcome
    }
}

pub struct NrtThread {
    /// `Option` so `Drop` can close the channel before joining.
    requests: Option<mpsc::Sender<NrtRequest>>,
    results: mpsc::Receiver<NrtResult>,
    handle: Option<JoinHandle<()>>,
}

impl NrtThread {
    /// Spawns the worker. `waker`, when the server has a socket front, is
    /// poked after each finished job so the command loop reports it at once
    /// instead of at its next idle tick — a job that took 2 ms was answered
    /// 100 ms later without it.
    pub fn spawn(waker: Option<Waker>) -> Self {
        let (req_tx, req_rx) = mpsc::channel::<NrtRequest>();
        let (res_tx, res_rx) = mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("nrt".into())
            .spawn(move || {
                let mut chain = NrtChain::default();
                while let Ok(req) = req_rx.recv() {
                    let result = NrtResult {
                        cmd: req.cmd,
                        index: req.index,
                        client: req.client,
                        outcome: chain.run(req.index, req.chained, req.job),
                    };
                    if res_tx.send(result).is_err() {
                        break; // receiver gone: we are shutting down
                    }
                    // After the send, so the woken loop finds the result.
                    if let Some(waker) = &waker {
                        waker.wake();
                    }
                }
            })
            .expect("failed to spawn the NRT thread");
        Self {
            requests: Some(req_tx),
            results: res_rx,
            handle: Some(handle),
        }
    }

    /// Queues a job. Fails only if the NRT thread died.
    // The error carries the unprocessed request back (same contract as
    // `CompilerThread::submit`); it is bigger than clippy's liking but this
    // is a cold path.
    #[allow(clippy::result_large_err)]
    pub fn submit(&self, request: NrtRequest) -> Result<(), NrtRequest> {
        match &self.requests {
            Some(tx) => tx.send(request).map_err(|e| e.0),
            None => Err(request),
        }
    }

    /// Non-blocking: one finished job, if any.
    pub fn try_result(&self) -> Option<NrtResult> {
        self.results.try_recv().ok()
    }

    /// Blocking with deadline; for tests, which must wait explicitly instead
    /// of sleeping.
    pub fn recv_result_timeout(&self, timeout: Duration) -> Option<NrtResult> {
        self.results.recv_timeout(timeout).ok()
    }
}

impl Drop for NrtThread {
    fn drop(&mut self) {
        // Closing the request channel ends the thread's recv loop.
        self.requests.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// How the server runs its NRT jobs: on the background [`NrtThread`] (the
/// native run loop), or **inline** on the calling thread (the headless pulled
/// server) — same submission order, same results, no thread. Inline is
/// the wasm mode (no threads there) and the accepted relaxation that buffer
/// work happens on whichever thread drives the server.
pub enum NrtRunner {
    Thread(NrtThread),
    /// Results of inline-executed jobs, drained like the thread's queue.
    Inline(std::collections::VecDeque<NrtResult>, NrtChain),
}

impl NrtRunner {
    pub fn spawn(waker: Option<Waker>) -> Self {
        NrtRunner::Thread(NrtThread::spawn(waker))
    }

    pub fn inline() -> Self {
        NrtRunner::Inline(std::collections::VecDeque::new(), NrtChain::default())
    }

    /// Queues a job (thread mode) or runs it right now (inline mode). Fails
    /// only if the NRT thread died.
    #[allow(clippy::result_large_err)]
    pub fn submit(&mut self, request: NrtRequest) -> Result<(), NrtRequest> {
        match self {
            NrtRunner::Thread(t) => t.submit(request),
            NrtRunner::Inline(results, chain) => {
                results.push_back(NrtResult {
                    cmd: request.cmd,
                    index: request.index,
                    client: request.client,
                    outcome: chain.run(request.index, request.chained, request.job),
                });
                Ok(())
            }
        }
    }

    /// Non-blocking: one finished job, if any.
    pub fn try_result(&mut self) -> Option<NrtResult> {
        match self {
            NrtRunner::Thread(t) => t.try_result(),
            NrtRunner::Inline(results, _) => results.pop_front(),
        }
    }
}

/// Performs one job. The NRT thread calls this per request; the offline
/// renderer (`server::render`) calls it directly, synchronously.
pub fn run_job(job: NrtJob) -> Result<NrtAction, String> {
    match job {
        NrtJob::Alloc {
            frames,
            channels,
            sample_rate,
        } => Ok(NrtAction::Install(Arc::new(Buffer::zeroed(
            frames,
            channels,
            sample_rate,
        )))),
        NrtJob::AllocRead {
            path,
            file_start,
            num_frames,
            channels,
        } => {
            let buffer = read_audio(&path, file_start, num_frames)?;
            let buffer = select_channels(&buffer, &channels)?;
            Ok(NrtAction::Install(Arc::new(buffer)))
        }
        NrtJob::Read {
            path,
            file_start,
            num_frames,
            buf_start,
            current,
            channels,
        } => {
            let file = read_audio(&path, file_start, num_frames)?;
            let file = select_channels(&file, &channels)?;
            if file.channels() != current.channels() {
                return Err(format!(
                    "channel count mismatch: buffer has {}, {path} has {}",
                    current.channels(),
                    file.channels()
                ));
            }
            let channels = current.channels();
            let mut data = current.to_vec();
            let take = file
                .frames()
                .min(current.frames().saturating_sub(buf_start));
            let to = buf_start * channels;
            data[to..to + take * channels].copy_from_slice(&file.to_vec()[..take * channels]);
            Ok(NrtAction::Install(Arc::new(Buffer::new(
                data,
                channels,
                current.frames(),
                file.sample_rate(),
            ))))
        }
        NrtJob::Write {
            path,
            sample_format,
            buf_start,
            num_frames,
            buffer,
        } => {
            write_wav(&path, &sample_format, buf_start, num_frames, &buffer)?;
            Ok(NrtAction::None)
        }
        NrtJob::Gen { current, cmd } => Ok(NrtAction::Install(Arc::new(cmd.apply(&current)))),
        NrtJob::Set {
            base: current,
            writes,
        } => {
            // **In place**: the samples go into the cells the engine is reading,
            // and nothing is installed. The parse already bounded every run
            // against the buffer's length, so a write cannot fail halfway and
            // there is nothing to roll back.
            for write in writes {
                for (i, v) in write.values.iter().enumerate() {
                    current.set_at(write.at + i * write.stride, *v);
                }
            }
            Ok(NrtAction::None)
        }
        NrtJob::Edit { base, op } => {
            // The edit itself is the core's, so a fade sounds the same wherever
            // it is applied -- and the core is pure over `&mut [f32]`, which is
            // what decides the shape here: read the span out, apply, write the
            // span back. What is copied is the **span**, not the buffer, so the
            // cost is the edit's and a five-minute take is no dearer than a
            // ten-second one.
            let channels = base.channels();
            let (span_start, span_frames) = op.span();
            let start = span_start * channels;
            let count = span_frames * channels;
            let mut data: Vec<f32> = (start..(start + count).min(base.len()))
                .map(|i| base.at(i))
                .collect();
            let out = match op {
                EditOp::Gain {
                    frames,
                    from,
                    to,
                    shape,
                    curve,
                    ..
                } => clausters_core::edit::gain(
                    &mut data,
                    channels,
                    0,
                    frames.min(span_frames),
                    clausters_core::edit::Fade::from_to(from, to, shape, curve),
                ),
                EditOp::Reverse { frames, .. } => {
                    clausters_core::edit::reverse(&mut data, channels, 0, frames.min(span_frames))
                }
            };
            out.map_err(|e| e.to_string())?;
            for (i, v) in data.iter().enumerate() {
                base.set_at(start + i, *v);
            }
            Ok(NrtAction::None)
        }
        NrtJob::Fill { base, fills } => {
            // In place, like every other write of a span. A fill says how many
            // samples it writes rather than carrying them, which is why it is
            // its own job: materializing the run would allocate what it exists
            // to avoid.
            for (at, count, value) in fills {
                for i in at..at + count {
                    base.set_at(i, value);
                }
            }
            Ok(NrtAction::None)
        }
        NrtJob::Free => Ok(NrtAction::Clear),
    }
}

/// Keeps only `channels` of `buffer`, in the order given — the de-interleave
/// behind `/buffer_readChannel` and `/buffer_allocReadChannel`.
///
/// An empty selection is every channel, unchanged, which is what the two
/// commands without `Channel` in their name send. A channel the file does not
/// have fails rather than reading silence: asking for the right channel of a
/// mono file is a mistake worth hearing about, not a silent track.
///
/// The order is honoured and repeats are allowed, so `[1, 0]` swaps a stereo
/// pair and `[0, 0]` makes a mono file into a two-channel one — both are what
/// naming channels explicitly is *for*, and neither costs anything to permit.
fn select_channels(buffer: &Buffer, channels: &[usize]) -> Result<Buffer, String> {
    if channels.is_empty() {
        // Nothing to do, but the caller owns the value: rebuild rather than
        // clone the Arc, since this arm is the one that already read a file.
        return Ok(Buffer::new(
            buffer.to_vec(),
            buffer.channels(),
            buffer.frames(),
            buffer.sample_rate(),
        ));
    }
    let have = buffer.channels();
    if let Some(bad) = channels.iter().find(|c| **c >= have) {
        return Err(format!(
            "channel {bad} was asked for, but the file has {have}"
        ));
    }
    let frames = buffer.frames();
    let src = buffer.to_vec();
    let mut out = Vec::with_capacity(frames * channels.len());
    for f in 0..frames {
        for c in channels {
            out.push(src[f * have + c]);
        }
    }
    Ok(Buffer::new(
        out,
        channels.len(),
        frames,
        buffer.sample_rate(),
    ))
}

/// Reads an audio-file slice into an interleaved buffer. WAV goes through
/// hound (exact, int24-aware, cheap frame seek); every other extension decodes
/// through symphonia (FLAC, OGG/Vorbis, MP3, MP4/AAC, ALAC, AIFF, ...). Both
/// keep the file's own sample rate — the engine never resamples; clients
/// compensate via `PlayBuf`'s rate.
///
/// Public because it is the server's whole answer to "read a soundfile", and
/// a client that needs the same answer should get *this* one rather than a
/// second decoder of its own: `/buffer_allocRead` and the embed FFI's
/// `clausters_read_soundfile` are two doors onto this function.
pub fn read_audio(path: &str, file_start: usize, num_frames: i64) -> Result<Buffer, String> {
    let is_wav = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("wav") || e.eq_ignore_ascii_case("wave"));
    if is_wav {
        read_wav(path, file_start, num_frames)
    } else {
        read_symphonia(path, file_start, num_frames)
    }
}

/// Decodes a compressed/other-format file fully into an interleaved f32 buffer,
/// then slices `[file_start, file_start + frames)`. Compressed formats have no
/// cheap exact frame seek, so we decode the whole file and slice afterwards;
/// this runs on the NRT thread, where allocation is fine.
fn read_symphonia(path: &str, file_start: usize, num_frames: i64) -> Result<Buffer, String> {
    use symphonia::core::codecs::CodecParameters;
    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::errors::Error as SymError;
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
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| err(e.to_string()))?;

    // Pick the default audio track and build its decoder. This borrow of
    // `format` ends before the decode loop, which needs `format` mutably.
    let (track_id, mut channels, mut sample_rate, mut decoder) = {
        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| err("no audio track".into()))?;
        let params = match track.codec_params.as_ref() {
            Some(CodecParameters::Audio(params)) => params,
            _ => return Err(err("default track is not audio".into())),
        };
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())
            .map_err(|e| err(e.to_string()))?;
        (
            track.id,
            params.channels.as_ref().map_or(0, |c| c.count()),
            params.sample_rate.unwrap_or(0),
            decoder,
        )
    };

    let mut data: Vec<f32> = Vec::new();
    let mut packet_buf: Vec<f32> = Vec::new();
    loop {
        // `next_packet` returns `Ok(None)` at clean end of stream.
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(e) => return Err(err(e.to_string())),
        };
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = decoded.spec();
                if channels == 0 {
                    channels = spec.channels().count();
                }
                if sample_rate == 0 {
                    sample_rate = spec.rate();
                }
                decoded.copy_to_vec_interleaved(&mut packet_buf);
                data.extend_from_slice(&packet_buf);
            }
            // A recoverable decode error skips one packet; keep going.
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(err(e.to_string())),
        }
    }
    if channels == 0 {
        return Err(err("could not determine channel count".into()));
    }

    let total = data.len() / channels;
    let start = file_start.min(total);
    let frames = if num_frames <= 0 {
        total - start
    } else {
        (num_frames as usize).min(total - start)
    };
    let slice = data[start * channels..(start + frames) * channels].to_vec();
    Ok(Buffer::new(slice, channels, frames, sample_rate as f64))
}

/// Reads a WAV slice into an interleaved buffer. Integer samples are scaled
/// to ±1 by their bit depth; the buffer keeps the file's sample rate (the
/// engine does not resample — clients compensate via `PlayBuf`'s rate).
fn read_wav(path: &str, file_start: usize, num_frames: i64) -> Result<Buffer, String> {
    let err = |e: hound::Error| format!("{path}: {e}");
    let mut reader = hound::WavReader::open(path).map_err(err)?;
    let spec = reader.spec();
    let channels = spec.channels as usize;
    let total = reader.duration() as usize;
    let start = file_start.min(total);
    let frames = if num_frames <= 0 {
        total - start
    } else {
        (num_frames as usize).min(total - start)
    };
    reader
        .seek(start as u32)
        .map_err(|e| format!("{path}: {e}"))?;

    let n = frames * channels;
    let data: Result<Vec<f32>, hound::Error> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, 32) => reader.samples::<f32>().take(n).collect(),
        (hound::SampleFormat::Int, bits @ 1..=32) => {
            let scale = 1.0 / (1u64 << (bits - 1)) as f32;
            reader
                .samples::<i32>()
                .take(n)
                .map(|s| s.map(|x| x as f32 * scale))
                .collect()
        }
        (format, bits) => return Err(format!("{path}: unsupported format {format:?}/{bits}-bit")),
    };
    let data = data.map_err(err)?;
    if data.len() != n {
        return Err(format!("{path}: file ended early"));
    }
    Ok(Buffer::new(data, channels, frames, spec.sample_rate as f64))
}

/// Maps a scsynth-style sample-format name to a hound WAV spec fragment.
/// Shared with the offline renderer (`server::render`).
pub fn wav_format(sample_format: &str) -> Result<(u16, hound::SampleFormat), String> {
    match sample_format {
        "int16" => Ok((16, hound::SampleFormat::Int)),
        "int24" => Ok((24, hound::SampleFormat::Int)),
        "float" | "float32" => Ok((32, hound::SampleFormat::Float)),
        other => Err(format!("unsupported sample format {other:?}")),
    }
}

fn write_wav(
    path: &str,
    sample_format: &str,
    buf_start: usize,
    num_frames: i64,
    buffer: &Buffer,
) -> Result<(), String> {
    let err = |e: hound::Error| format!("{path}: {e}");
    let (bits, format) = wav_format(sample_format)?;
    let spec = hound::WavSpec {
        channels: buffer.channels() as u16,
        sample_rate: buffer.sample_rate() as u32,
        bits_per_sample: bits,
        sample_format: format,
    };
    let start = buf_start.min(buffer.frames());
    let frames = if num_frames <= 0 {
        buffer.frames() - start
    } else {
        (num_frames as usize).min(buffer.frames() - start)
    };
    let held = buffer.to_vec();
    let samples = &held[start * buffer.channels()..(start + frames) * buffer.channels()];

    let mut writer = hound::WavWriter::create(path, spec).map_err(err)?;
    match format {
        hound::SampleFormat::Float => {
            for &s in samples {
                writer.write_sample(s).map_err(err)?;
            }
        }
        hound::SampleFormat::Int => {
            let scale = ((1u64 << (bits - 1)) - 1) as f32;
            for &s in samples {
                let q = (s.clamp(-1.0, 1.0) * scale).round() as i32;
                writer.write_sample(q).map_err(err)?;
            }
        }
    }
    writer.finalize().map_err(err)
}
