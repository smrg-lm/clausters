//! **The overview beside the region**: a take's summary as a file a peer maps,
//! kept current with the samples it describes.
//!
//! A buffer this server shares is a region file
//! ([`crate::dsp::region`]) -- its samples, mapped, named from the segment, the
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
//!   the buckets it touched and their parents, not the take: the pyramid is
//!   the buffer's own summary ([`crate::dsp::summary`]), the one `/buffer_peaks`
//!   answers from, so the file and the answers cannot disagree.
//! - **Written at a bounded rate.** The pyramid is patched in memory as each
//!   write lands and the file is rewritten from the run loop at most every
//!   [`WRITE_PERIOD`], so a hand drawing a stroke does not rewrite a
//!   multi-megabyte file per announcement.
//! - **A file that is behind is not there.** Between a write and the next
//!   [`Self::flush`] the pyramid in memory is ahead of the file, and a peer
//!   that opened the file then would draw the samples as they were, with the
//!   announcement of the write already gone by: nothing would ever correct
//!   it. So the first write that leaves the pyramid dirty unlinks the file,
//!   and the flush writes it back whole (into a file of its own, then renamed
//!   over the name). A peer that opens the take meanwhile finds no overview
//!   and summarizes the samples, which is correct and only slower.
//! - **It can be stale, and only in one direction.** A writer the owner never
//!   hears about -- a peer storing into the mapping without announcing it -- is
//!   the one case the summary cannot follow, exactly as the *pictures* of that
//!   take cannot. Announcing is the contract, and this is one more reader of
//!   it.
//!
//! The generation in the name is what makes a stale file harmless: a freed
//! buffer's overview and its replacement's can never share a name, so a peer
//! that kept a mapping is reading a file nothing describes rather than the
//! wrong take's summary.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clausters_core::peaks::{MultiPyramid, Pyramid};

use crate::dsp::buffer::Buffer;
use crate::dsp::summary::SUMMARY_BUCKET;

/// How often a dirty overview is written out, in seconds. A stroke announces
/// its spans far faster than this; what a reader needs is a file that
/// converges, not one rewritten per millisecond of audio edited.
///
/// The clock is [`crate::osc::server::OscServer::mono_secs`] -- wall time
/// natively and the sample axis in a headless server -- so this paces the same
/// way every other periodic thing here does, and a render stays deterministic.
const WRITE_PERIOD: f64 = 0.25;

/// One buffer's overview file, and whether it is behind the buffer's summary
/// (in which case it has been unlinked until [`Overviews::flush`] writes it).
struct Overview {
    path: PathBuf,
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
    ///
    /// Only a Unix build has regions to put one beside, which is why this and
    /// [`Self::retire`] read as dead code anywhere else -- the same reason the
    /// server's region list does.
    ///
    /// `samples` is what the pyramid is built from -- the region's own cells,
    /// whatever `buffer` is -- and `buffer` is what the server keeps under the
    /// number. A plain one keeps the pyramid as its summary, so `/buffer_peaks`
    /// answers it without measuring again; a join composes its own from its
    /// sources and is never written, so its file is only ever this one.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(in crate::osc::server) fn publish(
        &mut self,
        index: usize,
        region: &Path,
        samples: &Buffer,
        buffer: &Buffer,
    ) {
        if self.slots.len() <= index {
            self.slots.resize_with(index + 1, || None);
        }
        let pyramid = MultiPyramid::from_channels(
            (0..samples.channels().max(1))
                .map(|ch| Pyramid::build_from(&samples.channel(ch), SUMMARY_BUCKET))
                .collect(),
        );
        let path = peaks_path(region);
        if let Err(e) = pyramid.write_cache(&path) {
            // A summary is an optimization: without the file a peer summarizes
            // the samples itself, which is what it did before this existed. So
            // this is a warning and not a failure of the allocation.
            tracing::warn!("buffer {index}: cannot write its overview: {e}");
            self.slots[index] = None;
            return;
        }
        if !buffer.is_stitched() {
            buffer.keep_summary(pyramid);
        }
        self.slots[index] = Some(Overview { path, dirty: false });
    }

    /// Drops a freed buffer's overview and unlinks its file. Every mapping of
    /// it stays valid until its holder drops it, exactly as the region's does.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(in crate::osc::server) fn retire(&mut self, index: usize) {
        if let Some(overview) = self.slots.get_mut(index).and_then(Option::take) {
            let _ = std::fs::remove_file(&overview.path);
        }
    }

    /// **Refreshes the buckets a write touched**, from the samples as they now
    /// stand: the buffer's summary over the span, and the file beside the
    /// region, if there is one, unlinked until [`Self::flush`] writes it back.
    /// `start` and `frames` are frames.
    pub(in crate::osc::server) fn wrote(
        &mut self,
        index: usize,
        buffer: &Buffer,
        start: usize,
        frames: usize,
    ) {
        buffer.rewrite_summary(start, frames);
        let Some(overview) = self.slots.get_mut(index).and_then(Option::as_mut) else {
            return;
        };
        if frames > 0 && !overview.dirty {
            // The file now describes samples that are not there: it goes
            // until the flush writes the pyramid that does.
            overview.dirty = true;
            let _ = std::fs::remove_file(&overview.path);
        }
    }

    /// Writes out whatever has drifted, at most every [`WRITE_PERIOD`]. Called
    /// from the run loop beside the other periodic work, with the pool the
    /// overviews describe.
    ///
    /// A file is also behind when the audio thread recorded under it, which
    /// nobody announces: the buffer's summary forgets then, and so the file is
    /// unlinked. It comes back once the summary is whole again -- a view asked
    /// for all of it -- since a file is read by peers that cannot tell a
    /// measured bucket from an unmeasured one.
    pub(in crate::osc::server) fn flush(&mut self, now: f64, buffers: &[Option<Arc<Buffer>>]) {
        for (index, slot) in self.slots.iter_mut().enumerate() {
            let Some(overview) = slot.as_mut() else {
                continue;
            };
            if !overview.dirty
                && let Some(buffer) = buffers.get(index).and_then(Option::as_ref)
                && !buffer.is_stitched()
                && !buffer.summary_is_whole()
            {
                overview.dirty = true;
                let _ = std::fs::remove_file(&overview.path);
            }
        }
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
        for (index, slot) in self.slots.iter_mut().enumerate() {
            let Some(overview) = slot.as_mut().filter(|o| o.dirty) else {
                continue;
            };
            let Some(pyramid) = buffers
                .get(index)
                .and_then(Option::as_ref)
                .and_then(|b| b.whole_summary())
            else {
                continue;
            };
            overview.dirty = false;
            if let Err(e) = write_whole(&overview.path, &pyramid) {
                tracing::warn!("cannot refresh {}: {e}", overview.path.display());
            }
        }
    }
}

/// Writes `pyramid` to `path` **whole or not at all**: into a file of its own
/// first, then renamed over the name, so a peer opening it reads either no
/// overview or a complete one, never one half written that happens to parse.
fn write_whole(path: &Path, pyramid: &MultiPyramid) -> std::io::Result<()> {
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    let written = pyramid
        .write_cache(&part)
        .and_then(|()| std::fs::rename(&part, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    written
}

/// The overview's name: the region's own, plus `.peaks`. Sibling rather than
/// derived from the segment again, so the two files cannot disagree about which
/// generation they describe.
#[cfg_attr(not(unix), allow(dead_code))]
pub(in crate::osc::server) fn peaks_path(region: &Path) -> PathBuf {
    let mut name = region.as_os_str().to_os_string();
    name.push(".peaks");
    PathBuf::from(name)
}
