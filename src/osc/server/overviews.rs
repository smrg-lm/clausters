//! **The overview beside the region**: a take's summary as a file a peer maps,
//! kept current with the samples it describes.
//!
//! A buffer this server shares is a region file
//! ([`crate::dsp::region`]) — its samples, mapped, named from the segment, the
//! buffer number and the generation. This is the file beside it,
//! `<segment>.buf<N>.<gen>.peaks`, holding the peak pyramid over those samples
//! in the same CLPK format every client already reads
//! ([`clausters_core::peaks::MultiPyramid`]).
//!
//! **What it saves is the opening pass.** A view of a take has to summarize it
//! before it can draw it, and that pass is over every sample: a ten-minute
//! stereo take is 57.6 million of them, read once, by each process that opens
//! it. With the file there, a peer maps a few megabytes instead, and a client
//! that cannot map at all is answered from it (`/buffer_peaks`) without the
//! server reading the samples either.
//!
//! **What it costs is a second writer of derived state**, which is the whole
//! design question and is answered here rather than left implicit:
//!
//! - **One writer.** The server that owns the samples writes it and nobody
//!   else. A peer maps it read-only; a peer that edits announces the span
//!   (`/buffer_touch`) and the owner refreshes it.
//! - **Refreshed by the span, never rebuilt.** A write of a millisecond costs
//!   the buckets it touched and their parents, not the take —
//!   [`clausters_core::peaks::overview`] over exactly those buckets, the same
//!   function `/buffer_stream` and `/buffer_peaks` answer with, so there is one
//!   arithmetic and not three.
//! - **Written at a bounded rate.** The pyramid is patched in memory as each
//!   write lands and the file is rewritten from the run loop at most every
//!   [`WRITE_PERIOD`], so a hand drawing a stroke does not rewrite a
//!   multi-megabyte file per announcement.
//! - **It can be stale, and only in one direction.** A writer the owner never
//!   hears about — a peer storing into the mapping without announcing it — is
//!   the one case the summary cannot follow, exactly as the *pictures* of that
//!   take cannot. Announcing is the contract, and this is one more reader of
//!   it.
//!
//! The generation in the name is what makes a stale file harmless: a freed
//! buffer's overview and its replacement's can never share a name, so a peer
//! that kept a mapping is reading a file nothing describes rather than the
//! wrong take's summary.

use std::path::{Path, PathBuf};

use clausters_core::peaks::MultiPyramid;

use crate::dsp::buffer::Buffer;

/// How often a dirty overview is written out, in seconds. A stroke announces
/// its spans far faster than this; what a reader needs is a file that
/// converges, not one rewritten per millisecond of audio edited.
///
/// The clock is [`crate::osc::server::OscServer::mono_secs`] — wall time
/// natively and the sample axis in a headless server — so this paces the same
/// way every other periodic thing here does, and a render stays deterministic.
const WRITE_PERIOD: f64 = 0.25;

/// The bucket every overview is built at — the default a picture's pyramid uses
/// (`DEFAULT_BASE_BUCKET` in the GUI host, 256 in both clients), so a client
/// that maps this file has the grid it already draws on.
pub(in crate::osc::server) const BASE_BUCKET: usize = 256;

/// One buffer's overview: the file, the pyramid it holds, and whether the two
/// have drifted apart since the last write.
struct Overview {
    path: PathBuf,
    pyramid: MultiPyramid,
    dirty: bool,
}

/// Every shared buffer's overview, by buffer number.
#[derive(Default)]
pub(in crate::osc::server) struct Overviews {
    /// Sized with the pool, as the region list is.
    slots: Vec<Option<Overview>>,
    /// When the dirty ones were last written out, on the server's own clock.
    last_write: Option<f64>,
}

impl Overviews {
    /// **Builds and writes the overview of a buffer just published to a
    /// region.** The one full pass over the samples, paid where the region's
    /// own copy is already being paid.
    ///
    /// `region` is the region file's path; the overview is its sibling, so the
    /// generation that names one names the other.
    pub(in crate::osc::server) fn publish(&mut self, index: usize, region: &Path, buffer: &Buffer) {
        if self.slots.len() <= index {
            self.slots.resize_with(index + 1, || None);
        }
        let samples = buffer.to_vec();
        let pyramid =
            MultiPyramid::build_interleaved(&samples, buffer.channels().max(1), BASE_BUCKET);
        let path = peaks_path(region);
        if let Err(e) = pyramid.write_cache(&path) {
            // A summary is an optimization: without the file a peer summarizes
            // the samples itself, which is what it did before this existed. So
            // this is a warning and not a failure of the allocation.
            tracing::warn!("buffer {index}: cannot write its overview: {e}");
            self.slots[index] = None;
            return;
        }
        self.slots[index] = Some(Overview {
            path,
            pyramid,
            dirty: false,
        });
    }

    /// Drops a freed buffer's overview and unlinks its file. Every mapping of
    /// it stays valid until its holder drops it, exactly as the region's does.
    pub(in crate::osc::server) fn retire(&mut self, index: usize) {
        if let Some(overview) = self.slots.get_mut(index).and_then(Option::take) {
            let _ = std::fs::remove_file(&overview.path);
        }
    }

    /// **Refreshes the buckets a write touched**, from the samples as they now
    /// stand. `start` and `frames` are frames; the span is widened to whole
    /// buckets, since a bucket summarized from part of itself would report a
    /// peak the samples do not have.
    ///
    /// The file is not written here — the pyramid is patched and marked, and
    /// [`Self::flush`] writes it at a bounded rate.
    pub(in crate::osc::server) fn wrote(
        &mut self,
        index: usize,
        buffer: &Buffer,
        start: usize,
        frames: usize,
    ) {
        let Some(overview) = self.slots.get_mut(index).and_then(Option::as_mut) else {
            return;
        };
        let first = (start / BASE_BUCKET) * BASE_BUCKET;
        let end = (start + frames).min(overview.pyramid.frames());
        let buckets = end.saturating_sub(first) / BASE_BUCKET;
        if buckets == 0 {
            return;
        }
        let channels: Vec<_> = (0..buffer.channels().max(1))
            .map(|ch| buffer.channel(ch))
            .collect();
        let sources: Vec<_> = channels.iter().collect();
        let stats = clausters_core::peaks::overview(&sources, first, BASE_BUCKET, buckets);
        if overview.pyramid.write_buckets(first, BASE_BUCKET, &stats) {
            overview.dirty = true;
        }
    }

    /// **The overview of a span, out of the summary rather than the samples** —
    /// what lets `/buffer_peaks` answer a long take without reading it.
    ///
    /// `None` when this buffer has no overview or the caller asked at another
    /// bucket than the file is built at: a coarser answer would have to be
    /// spread over buckets nothing measured separately and a finer one folded
    /// from groups that straddle it, so the samples are read instead.
    pub(in crate::osc::server) fn span(
        &self,
        index: usize,
        first_frame: usize,
        bucket: usize,
        buckets: usize,
    ) -> Option<Vec<f32>> {
        let overview = self.slots.get(index)?.as_ref()?;
        if bucket != BASE_BUCKET || !first_frame.is_multiple_of(BASE_BUCKET) {
            return None;
        }
        let channels = overview.pyramid.num_channels();
        let mut out = Vec::with_capacity(buckets * channels * 3);
        for b in 0..buckets {
            let (a, z) = (
                (first_frame + b * bucket) as f64,
                (first_frame + (b + 1) * bucket) as f64,
            );
            for ch in 0..channels {
                let pyramid = overview.pyramid.channel(ch)?;
                let (lo, hi) = pyramid.column(0, a, z)?;
                let ms = pyramid.column_ms(0, a, z).unwrap_or(0.0);
                out.extend_from_slice(&[lo, hi, ms]);
            }
        }
        Some(out)
    }

    /// Writes out whatever has drifted, at most every [`WRITE_PERIOD`]. Called
    /// from the run loop beside the other periodic work.
    pub(in crate::osc::server) fn flush(&mut self, now: f64) {
        // Nothing dirty costs one walk of a short list and, above all, does not
        // start the clock: the period is between *writes*, so an edit is never
        // held back by an idle pass that happened just before it.
        if !self.slots.iter().flatten().any(|o| o.dirty) {
            return;
        }
        if self
            .last_write
            .is_some_and(|last| now - last < WRITE_PERIOD)
        {
            return;
        }
        self.last_write = Some(now);
        for overview in self.slots.iter_mut().flatten() {
            if !overview.dirty {
                continue;
            }
            overview.dirty = false;
            if let Err(e) = overview.pyramid.write_cache(&overview.path) {
                tracing::warn!("cannot refresh {}: {e}", overview.path.display());
            }
        }
    }
}

/// The overview's name: the region's own, plus `.peaks`. Sibling rather than
/// derived from the segment again, so the two files cannot disagree about which
/// generation they describe.
pub(in crate::osc::server) fn peaks_path(region: &Path) -> PathBuf {
    let mut name = region.as_os_str().to_os_string();
    name.push(".peaks");
    PathBuf::from(name)
}
