//! NRT (non-real-time) thread: disk I/O and buffer building (M5).
//!
//! Every `/b_*` command that touches sample memory runs here, off both the
//! audio and the network threads: allocation, WAV reading/writing (hound)
//! and zeroing. The network thread submits [`NrtRequest`]s and drains
//! [`NrtResult`]s on its own schedule, installs the produced buffer in the
//! engine via `Cmd::SetBuffer`, and sends the async `/done`/`/fail` reply —
//! same pattern as the Faust compiler thread.
//!
//! One queue means buffer commands complete **in submission order** (a
//! `/b_free` right after a `/b_alloc` cannot overtake it), which is why even
//! `/b_free` — no I/O at all — travels through here.
//!
//! Buffers are immutable (see [`crate::dsp::buffer`]), so `/b_read` into an
//! existing buffer and `/b_zero` build a *replacement* from the current
//! contents instead of mutating shared memory.

use crate::osc::ClientId;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::dsp::buffer::Buffer;

pub enum NrtJob {
    /// `/b_alloc` (and `/b_zero`, which knows the shape from the mirror):
    /// a fresh zeroed buffer.
    Alloc {
        frames: usize,
        channels: usize,
        sample_rate: f64,
    },
    /// `/b_allocRead`: shape, sample rate and contents come from the file.
    /// `num_frames <= 0` reads to the end.
    AllocRead {
        path: String,
        file_start: usize,
        num_frames: i64,
    },
    /// `/b_read`: overlay file data onto a copy of the current contents,
    /// starting at `buf_start`, keeping the buffer's shape.
    Read {
        path: String,
        file_start: usize,
        num_frames: i64,
        buf_start: usize,
        current: Arc<Buffer>,
    },
    /// `/b_write`: WAV out; `sample_format` is `int16`, `int24` or `float`.
    /// `num_frames <= 0` writes from `buf_start` to the end.
    Write {
        path: String,
        sample_format: String,
        buf_start: usize,
        num_frames: i64,
        buffer: Arc<Buffer>,
    },
    /// `/b_free`: ordered behind the other jobs (see module docs).
    Free,
}

pub struct NrtRequest {
    /// Command name for the `/done`/`/fail` reply (e.g. `"/b_alloc"`).
    pub cmd: &'static str,
    pub index: i32,
    /// Who asked: the async reply goes back to this client.
    pub client: ClientId,
    pub job: NrtJob,
}

/// What the network thread must do with the pool on success.
#[derive(Debug)]
pub enum NrtAction {
    Install(Arc<Buffer>),
    Clear,
    /// Nothing to install (`/b_write`): just reply `/done`.
    None,
}

pub struct NrtResult {
    pub cmd: &'static str,
    pub index: i32,
    pub client: ClientId,
    pub outcome: Result<NrtAction, String>,
}

pub struct NrtThread {
    /// `Option` so `Drop` can close the channel before joining.
    requests: Option<mpsc::Sender<NrtRequest>>,
    results: mpsc::Receiver<NrtResult>,
    handle: Option<JoinHandle<()>>,
}

impl NrtThread {
    pub fn spawn() -> Self {
        let (req_tx, req_rx) = mpsc::channel::<NrtRequest>();
        let (res_tx, res_rx) = mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("nrt".into())
            .spawn(move || {
                while let Ok(req) = req_rx.recv() {
                    let result = NrtResult {
                        cmd: req.cmd,
                        index: req.index,
                        client: req.client,
                        outcome: run_job(req.job),
                    };
                    if res_tx.send(result).is_err() {
                        break; // receiver gone: we are shutting down
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
        } => {
            let buffer = read_wav(&path, file_start, num_frames)?;
            Ok(NrtAction::Install(Arc::new(buffer)))
        }
        NrtJob::Read {
            path,
            file_start,
            num_frames,
            buf_start,
            current,
        } => {
            let file = read_wav(&path, file_start, num_frames)?;
            if file.channels() != current.channels() {
                return Err(format!(
                    "channel count mismatch: buffer has {}, {path} has {}",
                    current.channels(),
                    file.channels()
                ));
            }
            let channels = current.channels();
            let mut data = current.data().to_vec();
            let take = file.frames().min(current.frames().saturating_sub(buf_start));
            let to = buf_start * channels;
            data[to..to + take * channels].copy_from_slice(&file.data()[..take * channels]);
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
        NrtJob::Free => Ok(NrtAction::Clear),
    }
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
    let samples =
        &buffer.data()[start * buffer.channels()..(start + frames) * buffer.channels()];

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
