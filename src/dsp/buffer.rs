//! Sample buffers.
//!
//! A [`Buffer`]'s **shape is fixed and its contents are not**: the frame count,
//! the channel count and the sample rate are decided when it is allocated and
//! never change, while every sample is an atomic cell any thread may read or
//! write at any time. That is scsynth's model — buffer contents are mutable,
//! and a `RecordBuf` writing while a `PlayBuf` reads is the ordinary case, not
//! a hazard to design around.
//!
//! **Why the cells are atomic, since the answer is not the obvious one.** A
//! `u32` holds the sample's `f32` bits exactly (`from_bits`/`to_bits` compile to
//! nothing — there is no `AtomicF32` in the standard library, and that is the
//! only reason for the type). What atomics buy is not indivisibility: a
//! naturally aligned 32-bit store is already indivisible on every target we
//! run on. They buy the right to **write at all**. Two threads touching one
//! non-atomic location with a writer among them is a data race, which is
//! undefined behaviour — so the compiler may hoist a load out of a loop and
//! reuse a value forever, and a plain `&[f32]` written behind its back reads
//! stale samples with no symptom to debug.
//!
//! **What it costs, measured** (2026-08-16, relaxed loads against plain `f32`
//! indexing, per 64-frame block): an interpolated random-access read
//! (`PlayBuf`, `BufRd`) **+5%**, which is 12 ns a block a reader, or a thousandth
//! of a percent of the block budget; a wavetable read hot in cache (`Osc`,
//! `VOsc`, `Shaper`) and a sequential scan (`Conv`'s kernel) **free**, both
//! within noise. The cost is confined to the one shape the optimizer was
//! vectorizing, and it is the price of the capability rather than of the
//! atomics.
//!
//! **What is shared and what is guaranteed.** Per-cell atomicity, and no
//! ordering between cells: a reader crossing a writer sees some old samples and
//! some new, never half of one. That is scsynth's semantics for exactly this
//! case, and it is what a looper crossing its own write head has always sounded
//! like. The **shape** needs no synchronisation at all, being immutable.
//!
//! Freed buffers still leave the audio thread through the garbage FIFO, so the
//! final `Arc` drop (the deallocation) never happens there.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Default buffer-pool size, like scsynth's default `-b`. The live server sizes
/// its pool at boot from `--max-buffers` (see [`empty_pool_with`]); this stays
/// the fallback used by the NRT renderer and tests.
pub const NUM_BUFFERS: usize = 4096;

/// The engine-side pool: index → installed buffer.
pub type BufferPool = Vec<Option<Arc<Buffer>>>;

/// A pool of the default capacity ([`NUM_BUFFERS`]).
pub fn empty_pool() -> BufferPool {
    empty_pool_with(NUM_BUFFERS)
}

/// A pool of exactly `count` empty slots (the boot-time `--max-buffers`). The
/// pool's `len()` is the authoritative buffer-index bound everywhere.
pub fn empty_pool_with(count: usize) -> BufferPool {
    (0..count).map(|_| None).collect()
}

/// Interleaved sample data plus its shape. See the module docs for what is
/// fixed (the shape) and what is not (every sample).
pub struct Buffer {
    /// Interleaved samples as `f32` bit patterns, one atomic cell each — owned
    /// here, or in a region a second process can map (see [`Storage`]).
    data: Storage,
    channels: usize,
    frames: usize,
    sample_rate: f64,
    /// **How far this buffer has been written**, in frames — the buffer's own
    /// counter, always here.
    ///
    /// It used to live only in the shared segment's directory row, which made
    /// it a fact about *sharing* rather than about the samples: a server with
    /// no segment recorded exactly as it does now and could not say how far it
    /// had got, so `/buffer_stream` — the command for clients that cannot map
    /// anything, which is most of them — had nothing to report. The frontier
    /// is the samples's, so it is kept with the samples and published from
    /// here to whoever else wants it.
    written: AtomicU64,
    /// Where else to publish it: the directory row a mapping peer reads
    /// directly. `None` is a buffer no other process can see.
    frontier: Option<std::sync::Arc<dyn Frontier>>,
}

/// Where a buffer says **how far it has been written**.
///
/// A trait rather than a segment handle because `dsp` knows nothing about the
/// IPC layer and must not learn: the server implements this over the buffer
/// directory's row, an offline render implements nothing, and the audio thread
/// calls one method that stores a number.
pub trait Frontier: Send + Sync {
    /// Raises the published frontier to `frame` — the highest wins, so two
    /// writers on one buffer cannot pull it backwards.
    fn raise(&self, frame: u64);
}

/// Where a buffer's cells live.
///
/// **Matched on every read, and that is measured rather than assumed.** An
/// interpolated random read over a million cells costs 169-278 ns per 64-frame
/// block on the owned form, 172-281 ns matching this enum once per block and
/// 171-184 ns matching it per sample: the four measurements are one
/// measurement, because the load from memory is what costs and a buffer's
/// storage never changes, so the branch is perfectly predicted. A raw pointer
/// resolved once measured the same, which is why it is not what this is.
#[derive(Debug)]
pub enum Storage {
    /// The server's own memory: what a buffer is with no segment attached.
    Owned(Vec<AtomicU32>),
    /// A region a peer can map by name, for a server that has an IPC segment —
    /// the samples an editor draws and writes without a message
    /// (`dsp::region`).
    #[cfg(unix)]
    Shared(std::sync::Arc<crate::dsp::region::Region>),
}

impl Storage {
    /// The cells, whichever side they live on.
    #[inline]
    pub fn cells(&self) -> &[AtomicU32] {
        match self {
            Storage::Owned(v) => v,
            #[cfg(unix)]
            Storage::Shared(r) => r.cells(),
        }
    }
}

impl std::fmt::Debug for Buffer {
    /// Shape only — buffers hold millions of samples.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Buffer")
            .field("frames", &self.frames)
            .field("channels", &self.channels)
            .field("sample_rate", &self.sample_rate)
            .finish_non_exhaustive()
    }
}

impl Buffer {
    /// `data` is interleaved; its length must be `frames * channels`.
    pub fn new(data: Vec<f32>, channels: usize, frames: usize, sample_rate: f64) -> Self {
        assert_eq!(data.len(), frames * channels);
        Self {
            data: Storage::Owned(
                data.into_iter()
                    .map(|s| AtomicU32::new(s.to_bits()))
                    .collect(),
            ),
            channels,
            frames,
            sample_rate,
            written: AtomicU64::new(0),
            frontier: None,
        }
    }

    /// The same shape over storage a peer can map. The region's cells are the
    /// buffer's cells: nothing is copied here or ever after.
    #[cfg(unix)]
    pub fn shared(
        region: std::sync::Arc<crate::dsp::region::Region>,
        channels: usize,
        frames: usize,
        sample_rate: f64,
    ) -> Self {
        assert!(region.cells().len() >= frames * channels);
        Self {
            data: Storage::Shared(region),
            channels,
            frames,
            sample_rate,
            written: AtomicU64::new(0),
            frontier: None,
        }
    }

    /// The same buffer, publishing **how far it has been written** to whoever
    /// gave it a sink — the directory row a peer reads to draw a recording as
    /// it fills.
    ///
    /// A buffer with no sink (every buffer with no segment behind it) records
    /// exactly as it always did and tells nobody, which is the same split
    /// every other shared-samples path has.
    pub fn with_frontier(mut self, frontier: std::sync::Arc<dyn Frontier>) -> Self {
        self.frontier = Some(frontier);
        self
    }

    /// **Publishes the write frontier**: the highest frame a writer has
    /// filled, kept here and mirrored into the directory row when there is
    /// one.
    ///
    /// Called from the audio thread once per block by whoever wrote — one or
    /// two relaxed read-modify-writes and nothing else. A picture of a
    /// recording is the only reader, and what it does with a frame that is
    /// being written as it reads is what it does with every other one.
    pub fn raise_frontier(&self, frame: usize) {
        self.written.fetch_max(frame as u64, Ordering::Relaxed);
        if let Some(sink) = &self.frontier {
            sink.raise(frame as u64);
        }
    }

    /// **How far this buffer has been written**, in frames, or `0` for one
    /// nothing recorded into — a buffer that arrived whole is samples
    /// everywhere and has no frontier at all.
    ///
    /// It is a *hint*, like the row a peer reads: several writers may share a
    /// buffer and nothing here says which of them wrote what. Its one reader
    /// is a picture of a recording — `/buffer_stream` summarizes up to it, and
    /// a mapping peer reads the same number out of the segment.
    pub fn frontier(&self) -> u64 {
        self.written.load(Ordering::Relaxed)
    }

    /// Where this buffer's samples live — what a pool consults to hand a peer
    /// the name of a region, and nothing else.
    pub fn storage(&self) -> &Storage {
        &self.data
    }

    pub fn zeroed(frames: usize, channels: usize, sample_rate: f64) -> Self {
        Self::new(vec![0.0; frames * channels], channels, frames, sample_rate)
    }

    pub fn frames(&self) -> usize {
        self.frames
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// The raw cells, `frames * channels` of them, interleaved — for a reader
    /// running its own tight loop over a span (a convolution kernel, a
    /// wavetable). Read one with [`load`](Self::load); nothing else about the
    /// representation is anybody's business.
    #[inline]
    pub fn cells(&self) -> &[AtomicU32] {
        self.data.cells()
    }

    /// One cell's value. The single door every read goes through, so the
    /// ordering is stated once: **relaxed**, because a sample carries no
    /// happens-before relationship to any other — see the module docs.
    #[inline]
    pub fn load(cell: &AtomicU32) -> f32 {
        f32::from_bits(cell.load(Ordering::Relaxed))
    }

    /// One sample by flat interleaved index (`frame * channels + channel`);
    /// out of range reads as 0.
    #[inline]
    pub fn at(&self, index: usize) -> f32 {
        self.cells().get(index).map_or(0.0, Self::load)
    }

    /// One sample; out-of-range frames or channels read as 0.
    #[inline]
    pub fn sample(&self, frame: usize, channel: usize) -> f32 {
        if frame >= self.frames || channel >= self.channels {
            return 0.0;
        }
        Self::load(&self.cells()[frame * self.channels + channel])
    }

    /// Writes one sample by flat interleaved index; out of range writes
    /// nothing. Takes `&self`: a buffer in the pool is reached through an
    /// `Arc`, so there is no `&mut` to be had and the cells carry the
    /// mutability instead.
    #[inline]
    pub fn set_at(&self, index: usize, value: f32) {
        if let Some(cell) = self.cells().get(index) {
            cell.store(value.to_bits(), Ordering::Relaxed);
        }
    }

    /// Writes one sample; out-of-range frames or channels write nothing.
    #[inline]
    pub fn set_sample(&self, frame: usize, channel: usize, value: f32) {
        if frame < self.frames && channel < self.channels {
            self.set_at(frame * self.channels + channel, value);
        }
    }

    /// A snapshot of the whole buffer, interleaved — what a caller that wants a
    /// plain slice takes instead of borrowing one. It is a *reading*, not a
    /// view: samples written after it are not in it, which is the honest shape
    /// for the network and NRT sides that serve, resample or write out a
    /// buffer while the engine may be recording into it.
    pub fn to_vec(&self) -> Vec<f32> {
        self.cells().iter().map(Self::load).collect()
    }

    /// The number of samples this holds, `frames * channels`.
    #[inline]
    pub fn len(&self) -> usize {
        self.cells().len()
    }

    /// Whether it holds no samples at all.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.cells().is_empty()
    }
}
