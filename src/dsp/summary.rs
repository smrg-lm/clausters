//! **A buffer's summary, kept with its samples**: what `/buffer_peaks` answers
//! from, for every buffer this server owns.
//!
//! A plain buffer keeps a [`Kept`] pyramid, measured the first time a span of
//! it is asked for and kept after that, so a take is summarized once however
//! many times and at whatever resolution its overview is asked. It lives on
//! the buffer rather than in a table beside the pool because a buffer is not
//! always reached through its number: a join holds its sources by `Arc`, and a
//! source freed under a join it was cut into goes on being read -- and
//! summarized -- through the join.
//!
//! **A join keeps nothing of its own.** Its samples are spans of its sources,
//! so its summary is composed from theirs as it is asked: the span of each part
//! is answered by the source's summary, a fade is read from the join's samples
//! (it is the one place where they are not the source's), and a channel the
//! part maps to nothing is silence. An edit builds a new join, and what that
//! costs the summary is nothing: the sources were summarized already, and the
//! join's first answer reads their levels.
//!
//! Only the network side touches any of this. The audio thread never reads
//! the summary and never takes its lock; what it does to a summary is write the
//! samples under it, which the buffer counts ([`Buffer::writes`]) so the next
//! question finds the summary forgotten rather than stale.

use std::sync::{Mutex, PoisonError};

use clausters_core::peaks::{Kept, MultiPyramid, SpanStats};

use super::buffer::Buffer;
use super::stitch::{Part, Stitch};

/// The base bucket every summary is kept at -- the grid a host's pyramid
/// draws on (`DEFAULT_BASE_BUCKET` in the GUI host, 256 in both clients).
pub const SUMMARY_BUCKET: usize = 256;

/// The summary and the write count it was last true for.
pub struct Summary {
    kept: Kept,
    seen: u64,
}

/// What a buffer holds its summary in: nothing until someone asks.
pub type SummarySlot = Mutex<Option<Summary>>;

impl Buffer {
    /// **The statistics of `channel` over frames `a..b`, out of the summary**:
    /// exact, and reading from the samples only what has not been measured
    /// before and the edges of a span off the grid. `read` grows by the samples
    /// read, which is what a caller bounds a reply by.
    pub fn summary_span(&self, channel: usize, a: usize, b: usize, read: &mut usize) -> SpanStats {
        let b = b.min(self.frames());
        if a >= b {
            return SpanStats::EMPTY;
        }
        if channel >= self.channels() {
            return SpanStats::silence(b - a);
        }
        if let Some(stitch) = self.stitch() {
            return stitch_span(stitch, channel, a, b, read);
        }
        let channels: Vec<_> = (0..self.channels()).map(|ch| self.channel(ch)).collect();
        let sources: Vec<_> = channels.iter().collect();
        self.with_summary(|kept| kept.span(&sources, channel, a, b, read))
    }

    /// **Re-measures what the summary kept of a span a writer said it wrote**
    /// -- a `/buffer_setRange`, a peer's `/buffer_touch`. What was never asked
    /// for stays unmeasured.
    pub fn rewrite_summary(&self, start: usize, frames: usize) {
        if self.is_stitched() {
            return;
        }
        let channels: Vec<_> = (0..self.channels()).map(|ch| self.channel(ch)).collect();
        let sources: Vec<_> = channels.iter().collect();
        self.with_summary(|kept| {
            kept.rewrite(&sources, start, frames);
        });
    }

    /// Keeps a summary built whole somewhere the pass was being paid anyway --
    /// a take copied into a region a peer maps.
    pub fn keep_summary(&self, pyramid: MultiPyramid) {
        let mut slot = self
            .summary_slot()
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *slot = Some(Summary {
            kept: Kept::measured(pyramid),
            seen: self.writes(),
        });
    }

    /// Whether the summary describes every sample as it now stands: every
    /// bucket measured and nothing written under it since. A check per run-loop
    /// turn, so it copies nothing.
    pub fn summary_is_whole(&self) -> bool {
        let slot = self
            .summary_slot()
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        slot.as_ref()
            .is_some_and(|s| s.seen == self.writes() && s.kept.is_whole())
    }

    /// The summary as a pyramid a reader that knows nothing of measuring can
    /// take, when every bucket of it is measured and nothing wrote under it
    /// since; `None` otherwise.
    pub fn whole_summary(&self) -> Option<MultiPyramid> {
        let slot = self
            .summary_slot()
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let summary = slot.as_ref()?;
        (summary.seen == self.writes() && summary.kept.is_whole())
            .then(|| summary.kept.pyramid().clone())
    }

    /// Runs `f` over the summary, creating it unmeasured on the first call and
    /// forgetting it when the audio thread wrote since it was last read.
    fn with_summary<R>(&self, f: impl FnOnce(&mut Kept) -> R) -> R {
        let mut slot = self
            .summary_slot()
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let writes = self.writes();
        let summary = slot.get_or_insert_with(|| Summary {
            kept: Kept::unmeasured(self.frames(), self.channels().max(1), SUMMARY_BUCKET),
            seen: writes,
        });
        if summary.seen != writes {
            summary.kept.forget();
            summary.seen = writes;
        }
        f(&mut summary.kept)
    }
}

/// [`Buffer::summary_span`] over a join: each part's share of `a..b`, composed.
fn stitch_span(stitch: &Stitch, channel: usize, a: usize, b: usize, read: &mut usize) -> SpanStats {
    let parts = stitch.parts();
    let mut stats = SpanStats::EMPTY;
    let from = parts.partition_point(|p| p.end() <= a);
    for part in parts[from..].iter().take_while(|p| p.start() < b) {
        let (ia, ib) = (
            a.max(part.start()) - part.start(),
            b.min(part.end()) - part.start(),
        );
        stats.merge(part_span(part, channel, ia, ib, read));
    }
    stats
}

/// One part's frames `ia..ib` (counted from the part's start) on the join's
/// `channel`.
fn part_span(part: &Part, channel: usize, ia: usize, ib: usize, read: &mut usize) -> SpanStats {
    let src_ch = match part.map.get(channel) {
        Some(&ch) if ch >= 0 => ch as usize,
        _ => return SpanStats::silence(ib - ia),
    };
    // The fades are the join's own samples -- the source's times a gain -- so
    // they are read; between them the part is its source, summarized.
    let body = (part.fade_in, part.frames - part.fade_out);
    let mut stats = SpanStats::EMPTY;
    for (lo, hi, faded) in [
        (0, body.0, true),
        (body.0, body.1, false),
        (body.1, part.frames, true),
    ] {
        let (lo, hi) = (lo.max(ia), hi.min(ib));
        if lo >= hi {
            continue;
        }
        if faded {
            let samples: Vec<f32> = (lo..hi).map(|i| part.sample(i, channel)).collect();
            *read += hi - lo;
            stats.merge(SpanStats::of(&samples));
        } else {
            stats.merge(source_span(part, src_ch, lo, hi, read));
        }
    }
    stats
}

/// The source's summary under a part's unfaded frames `lo..hi`.
///
/// A part at its join's own rate reads its source frame for frame, so this is
/// the source's span, exact. One at another rate reads between two source
/// frames, and a line between two samples never leaves them: the extremes are
/// the source's over the frames it reads between, which bound the join's, and
/// the energy is the source's over that span, taken as the join's mean.
fn source_span(part: &Part, src_ch: usize, lo: usize, hi: usize, read: &mut usize) -> SpanStats {
    let src = &part.src;
    if !part.resampled() {
        return src.summary_span(src_ch, part.src_start + lo, part.src_start + hi, read);
    }
    let first = (part.src_start as f64 + lo as f64 * part.rate).floor() as usize;
    let last = (part.src_start as f64 + (hi - 1) as f64 * part.rate).ceil() as usize;
    let span = src.summary_span(src_ch, first, (last + 1).min(src.frames()), read);
    if span.count == 0 {
        return SpanStats::silence(hi - lo);
    }
    let ms = span.sum_sq / span.count as f64;
    SpanStats {
        sum_sq: ms * (hi - lo) as f64,
        count: hi - lo,
        ..span
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::dsp::stitch::{FadeShape, PartSpec};

    fn noise(n: usize, seed: u32) -> Vec<f32> {
        let mut s = seed.max(1);
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                (s as f32 / u32::MAX as f32) * 2.0 - 1.0
            })
            .collect()
    }

    fn take(frames: usize, channels: usize, seed: u32) -> Arc<Buffer> {
        Arc::new(Buffer::new(
            noise(frames * channels, seed),
            channels,
            frames,
            48_000.0,
        ))
    }

    fn spec(src: &Arc<Buffer>, src_start: usize, frames: usize, map: Vec<i32>) -> PartSpec {
        PartSpec {
            src: Arc::clone(src),
            src_index: 0,
            src_start,
            frames,
            fade_in: 0,
            fade_out: 0,
            fade_in_shape: FadeShape::LINEAR,
            fade_out_shape: FadeShape::LINEAR,
            map,
        }
    }

    /// What a summary of the samples says, read directly.
    fn direct(buffer: &Buffer, channel: usize, a: usize, b: usize) -> SpanStats {
        let samples: Vec<f32> = (a..b).map(|f| buffer.sample(f, channel)).collect();
        SpanStats::of(&samples)
    }

    fn assert_close(got: SpanStats, want: SpanStats, at: &str) {
        assert_eq!(
            (got.min, got.max, got.count),
            (want.min, want.max, want.count),
            "{at}"
        );
        let (g, w) = (got.bucket()[2], want.bucket()[2]);
        assert!(
            (g - w).abs() <= 1e-5 * w.abs().max(1e-3),
            "{at}: ms {g} vs {w}"
        );
    }

    /// **A join answers what its samples hold, bucket for bucket**, across
    /// cuts off the grid, fades with shapes, a channel map that swaps and
    /// silences, and a source that is itself a join.
    #[test]
    fn a_join_composes_its_summary_from_its_parts() {
        let a = take(20_000, 2, 5);
        let b = take(9_000, 1, 9);
        let inner = Arc::new(Buffer::stitched(
            Stitch::new(
                vec![
                    spec(&a, 777, 3_001, vec![1, 0]),
                    spec(&b, 13, 2_500, vec![0, 0]),
                ],
                2,
                48_000.0,
            )
            .unwrap(),
            2,
            48_000.0,
        ));
        let mut faded = spec(&a, 4_321, 6_000, vec![0, -1]);
        faded.fade_in = 300;
        faded.fade_out = 517;
        faded.fade_out_shape = FadeShape {
            shape: clausters_core::envshape::SHAPE_CURVE,
            curve: -3.0,
        };
        let join = Buffer::stitched(
            Stitch::new(
                vec![
                    spec(&a, 100, 1_234, vec![0, 1]),
                    faded,
                    spec(&inner, 50, 5_000, vec![1, 0]),
                    spec(&b, 0, 3_000, vec![0, -1]),
                ],
                2,
                48_000.0,
            )
            .unwrap(),
            2,
            48_000.0,
        );
        for bucket in [256, 1_024, 4_096] {
            for i in 0..join.frames() / bucket {
                for ch in 0..2 {
                    let (s, e) = (i * bucket, (i + 1) * bucket);
                    let mut read = 0;
                    let got = join.summary_span(ch, s, e, &mut read);
                    assert_close(got, direct(&join, ch, s, e), &format!("{bucket} {i} {ch}"));
                }
            }
        }
    }

    /// **What an edit costs the summary is its parts, not its length**: once
    /// the source is summarized, a coarse overview of a new join over it reads
    /// only the cut edges and the fades.
    #[test]
    fn a_new_join_over_summarized_sources_reads_little() {
        let frames = 2_000_000;
        let src = take(frames, 2, 3);
        let mut read = 0;
        for ch in 0..2 {
            src.summary_span(ch, 0, frames, &mut read);
        }
        assert!(read >= 2 * frames, "the first question measures the take");

        // A paste: the head, a pasted span with fades, the tail shifted by it.
        let mut pasted = spec(&src, 1_000_003, 48_001, vec![0, 1]);
        pasted.fade_in = 64;
        pasted.fade_out = 64;
        let join = Buffer::stitched(
            Stitch::new(
                vec![
                    spec(&src, 0, 500_017, vec![0, 1]),
                    pasted,
                    spec(&src, 500_017, frames - 500_017, vec![0, 1]),
                ],
                2,
                48_000.0,
            )
            .unwrap(),
            2,
            48_000.0,
        );
        let bucket = 65_536;
        let mut read = 0;
        for i in 0..join.frames() / bucket {
            for ch in 0..2 {
                join.summary_span(ch, i * bucket, (i + 1) * bucket, &mut read);
            }
        }
        // Each bucket off the source's grid reads at most two base buckets of
        // edges per channel; the fades read themselves.
        let buckets = join.frames() / bucket;
        assert!(
            read <= (buckets + 4) * 2 * 2 * SUMMARY_BUCKET + 4 * 64,
            "read {read} frames for {buckets} buckets"
        );
    }

    /// A write the audio thread made is not described to the summary, so the
    /// summary forgets and measures again; one a writer described is followed.
    #[test]
    fn a_write_reaches_the_summary() {
        let src = take(4_096, 1, 1);
        let mut read = 0;
        src.summary_span(0, 0, 4_096, &mut read);
        src.set_sample(10, 0, 5.0);
        src.rewrite_summary(10, 1);
        let mut read = 0;
        assert_eq!(src.summary_span(0, 0, 4_096, &mut read).max, 5.0);
        assert_eq!(read, 0, "the rewrite re-measured it");

        src.set_sample(3_000, 0, 7.0);
        src.raise_frontier(3_001);
        let mut read = 0;
        assert_eq!(src.summary_span(0, 0, 4_096, &mut read).max, 7.0);
        assert_eq!(read, 4_096, "a recording made it measure again (mono)");
        assert!(src.whole_summary().is_some());
    }
}
