//! `PeaksOut`: the **peaks file** of the signals it is given, written as they
//! play -- the overview a waveform is drawn from, beside the samples a
//! `DiskOut` writes.
//!
//! A view of a take of hours is drawn from a summary and never from the
//! samples: the base level holds the minimum, the maximum and the mean square
//! of every 256 frames, and each level above halves it (`clausters_core::peaks`,
//! the CLPK file a host maps). The summary of a file is cheapest where its
//! samples are made, so a render or a recording that wants its file drawn
//! puts this UGen on the same signal as its `DiskOut`. **The two are separate
//! UGens**: each writes its own data to its own file, and a file imported
//! without peaks is measured by a render of `DiskIn` into this one alone.
//!
//! The design is `DiskOut`'s. The audio thread folds each channel into its
//! bucket -- a comparison, a comparison and a multiply-add per sample -- and
//! pushes the three numbers per channel into a lock-free ring at each
//! bucket's end; a thread of its own keeps them, and when the synth is freed
//! it builds the pyramid and writes the file. What it keeps is the base
//! level, 12 bytes per channel per 256 frames (2.3 % of the samples as
//! floats), until the file is written.
//!
//! **A summary that lies is worse than none.** A ring the writer did not
//! drain in time would drop buckets, and a pyramid built around the hole
//! would draw a take that is not there; so a lost bucket is counted and no
//! file is written, with a warning that says why. The ring holds about a
//! minute of stereo buckets, which an offline render outruns only if the
//! writer is not scheduled for that long.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::thread::JoinHandle;

use rtrb::{Consumer, Producer, RingBuffer};

use crate::dsp::registry::{BuildCtx, UGenConfig};
use crate::dsp::summary::SUMMARY_BUCKET;
use crate::dsp::{ProcessCtx, UGen, at};
use crate::server::meters::{Meters, Role};

/// The most channels one `PeaksOut` summarizes. Its accumulators are
/// allocated when it is built, so the bound is fixed then.
pub const MAX_CHANNELS: usize = 64;

/// The ring, in floats: three per channel per bucket.
const RING: usize = 1 << 16;

/// What the audio thread and the writer share besides the ring.
struct Shared {
    stop: AtomicBool,
    /// Buckets the ring could not take.
    lost: AtomicBool,
    /// The channels, once the first block has said how many inputs there are.
    channels: AtomicUsize,
    /// Frames summarized, set when the synth is freed.
    frames: AtomicU64,
}

/// Writes the peaks file of its inputs, one channel per input, to the path
/// in its config. Output: its first input, passed through.
pub struct PeaksOut {
    active: Option<Active>,
}

struct Active {
    producer: Producer<f32>,
    shared: Arc<Shared>,
    handle: Option<JoinHandle<()>>,
    /// Per channel: the minimum, the maximum and the sum of squares of the
    /// bucket being filled.
    acc: Box<[(f32, f32, f64)]>,
    channels: usize,
    /// Frames in the bucket being filled.
    filled: usize,
    frames: u64,
    /// In an offline render: wait for room rather than lose a bucket.
    offline: bool,
}

impl PeaksOut {
    /// Starts the writer, which accounts its work to the build's load table
    /// as a `disk-out` thread.
    pub fn open(config: &UGenConfig, ctx: &BuildCtx) -> Self {
        let meters: &Arc<Meters> = &ctx.meters;
        let Some(path) = config.path.clone() else {
            tracing::warn!("PeaksOut has no path; it will write nothing");
            return Self { active: None };
        };
        let (producer, consumer) = RingBuffer::new(RING);
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            lost: AtomicBool::new(false),
            channels: AtomicUsize::new(0),
            frames: AtomicU64::new(0),
        });
        let handle = {
            let shared = Arc::clone(&shared);
            let turn = crate::dsp::disk::Turn::new(Arc::clone(meters), Role::DiskOut);
            std::thread::Builder::new()
                .name("peaksout".into())
                .spawn(move || writer(path, consumer, shared, turn))
                .expect("failed to spawn the PeaksOut thread")
        };
        Self {
            active: Some(Active {
                producer,
                shared,
                handle: Some(handle),
                acc: vec![EMPTY; MAX_CHANNELS].into_boxed_slice(),
                channels: 0,
                filled: 0,
                frames: 0,
                offline: ctx.offline,
            }),
        }
    }
}

const EMPTY: (f32, f32, f64) = (f32::INFINITY, f32::NEG_INFINITY, 0.0);

impl Active {
    /// Pushes the bucket being filled, as `filled` frames' worth.
    fn push(&mut self) {
        let n = self.channels * 3;
        let filled = self.filled.max(1) as f64;
        if self.offline && self.producer.slots() < n {
            let producer = &self.producer;
            crate::dsp::disk::wait_for(|| producer.slots() >= n);
        }
        match self.producer.write_chunk_uninit(n) {
            Ok(chunk) => {
                let acc = &self.acc[..self.channels];
                chunk.fill_from_iter(
                    acc.iter()
                        .flat_map(|&(lo, hi, sq)| [lo, hi, (sq / filled) as f32]),
                );
            }
            Err(_) => self.shared.lost.store(true, Ordering::Relaxed),
        }
        self.acc[..self.channels].fill(EMPTY);
        self.filled = 0;
    }
}

impl UGen for PeaksOut {
    fn reset(&mut self) {
        // A file is its writer's and cannot be restarted from here: a
        // `Voices` node refuses a def that holds one.
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let first = inputs.first().copied().unwrap_or(&[]);
        for (i, s) in output.iter_mut().enumerate() {
            *s = at(first, i);
        }
        let Some(a) = self.active.as_mut() else {
            return;
        };
        if a.channels == 0 {
            a.channels = inputs.len().clamp(1, MAX_CHANNELS);
            a.shared.channels.store(a.channels, Ordering::Release);
        }
        for i in 0..output.len() {
            for (c, cell) in a.acc[..a.channels].iter_mut().enumerate() {
                let x = inputs.get(c).map_or(0.0, |input| at(input, i));
                cell.0 = cell.0.min(x);
                cell.1 = cell.1.max(x);
                cell.2 += f64::from(x) * f64::from(x);
            }
            a.filled += 1;
            if a.filled == SUMMARY_BUCKET {
                a.push();
            }
        }
        a.frames += output.len() as u64;
    }
}

impl Drop for PeaksOut {
    fn drop(&mut self) {
        let Some(a) = self.active.as_mut() else {
            return;
        };
        // The last bucket is short, and is the writer's like the others. This
        // is the network thread, where waiting for room is legal.
        if a.filled > 0 && a.channels > 0 {
            let n = a.channels * 3;
            for _ in 0..1000 {
                if a.producer.slots() >= n {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            a.push();
        }
        a.shared.frames.store(a.frames, Ordering::Release);
        a.shared.stop.store(true, Ordering::Release);
        if let Some(h) = a.handle.take() {
            h.thread().unpark();
            let _ = h.join();
        }
    }
}

/// Keeps the buckets as they arrive and writes the pyramid at the end.
fn writer(
    path: String,
    mut consumer: Consumer<f32>,
    shared: Arc<Shared>,
    mut turn: crate::dsp::disk::Turn,
) {
    let mut stats: Vec<f32> = Vec::new();
    loop {
        let n = consumer.slots();
        if n > 0 {
            turn.worked();
            if let Ok(chunk) = consumer.read_chunk(n) {
                let (a, b) = chunk.as_slices();
                stats.extend_from_slice(a);
                stats.extend_from_slice(b);
                chunk.commit_all();
            }
            continue;
        }
        if shared.stop.load(Ordering::Acquire) {
            if consumer.slots() > 0 {
                continue;
            }
            break;
        }
        turn.park();
    }
    turn.worked();
    let channels = shared.channels.load(Ordering::Acquire);
    let frames = shared.frames.load(Ordering::Acquire) as usize;
    if channels == 0 || frames == 0 {
        return;
    }
    if shared.lost.load(Ordering::Relaxed) {
        tracing::warn!(
            "PeaksOut: {path}: buckets were lost while writing (the writer fell behind), \
             so no peaks file is written rather than one that draws what is not there"
        );
        return;
    }
    let mut pyramid = clausters_core::peaks::MultiPyramid::empty(frames, channels, SUMMARY_BUCKET);
    if !pyramid.write_buckets(0, SUMMARY_BUCKET, &stats) {
        tracing::warn!("PeaksOut: {path}: the buckets do not cover the frames summarized");
        return;
    }
    if let Err(e) = pyramid.write_cache(&path) {
        tracing::warn!("PeaksOut: {path}: {e}");
    }
}
