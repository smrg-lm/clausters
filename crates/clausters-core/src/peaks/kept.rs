//! **A summary kept beside samples it measures as it is asked**: the peak
//! pyramid of a buffer whose owner answers overviews of it, built a span at a
//! time and never twice.
//!
//! A [`MultiPyramid`] is a summary somebody built whole: one pass over every
//! sample, then the levels. That is right where the pass is already being paid
//! (a take copied into a region a peer maps), and wrong everywhere else: an
//! engine inside a page answers between two audio blocks, and an hour of
//! stereo summarized in one of those turns is a glitch nobody asked for. So
//! the summary here starts **unmeasured** and each question measures only the
//! buckets it needs and does not have; what was measured is kept, so the next
//! question over the same span reads levels and no samples at all.
//!
//! **An answer is exact for any span**, not only for one on the grid
//! ([`Kept::span`]): the whole buckets inside it are folded through the
//! levels, a segment tree's walk, and the two partial edges are read from the
//! samples. So a coarse bucket on the grid reads no samples, a span cut
//! anywhere reads at most two base buckets of them, and the numbers are the
//! ones a direct summary of the samples would give: min and max exactly, the
//! mean square up to the rounding of the levels it was folded from.
//!
//! What it cannot follow is a write it is not told about. A writer that says
//! where it wrote calls [`Kept::rewrite`], which re-measures what was kept of
//! that span; one that cannot say (the audio thread recording into the
//! buffer) leaves the owner to call [`Kept::forget`], and the summary is
//! measured again as it is asked.

use super::{MultiPyramid, Source, mean_square, min_max};

/// **What a span holds, folded**: its extremes and its energy as a sum with
/// its count, so spans combine exactly and the mean is taken once, by whoever
/// reports it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpanStats {
    pub min: f32,
    pub max: f32,
    pub sum_sq: f64,
    pub count: usize,
}

impl SpanStats {
    /// Nothing folded yet: what [`Self::merge`] starts from.
    pub const EMPTY: Self = Self {
        min: f32::INFINITY,
        max: f32::NEG_INFINITY,
        sum_sq: 0.0,
        count: 0,
    };

    /// `count` frames of silence -- what a channel mapped to nothing holds.
    pub fn silence(count: usize) -> Self {
        if count == 0 {
            return Self::EMPTY;
        }
        Self {
            min: 0.0,
            max: 0.0,
            sum_sq: 0.0,
            count,
        }
    }

    /// The statistics of `samples`, read directly: [`min_max`] and
    /// [`mean_square`], the builder's own two passes. Two and not one fused
    /// loop because the extremes vectorize on their own and the energy's
    /// `f64` sum does not, so fused they run at the sum's pace: three times
    /// slower, measured, over the first pass a summary of a take costs.
    pub fn of(samples: &[f32]) -> Self {
        let Some((min, max)) = min_max(samples) else {
            return Self::EMPTY;
        };
        let ms = mean_square(samples).unwrap_or(0.0);
        Self {
            min,
            max,
            sum_sq: ms as f64 * samples.len() as f64,
            count: samples.len(),
        }
    }

    /// Folds `other` in.
    pub fn merge(&mut self, other: Self) {
        if other.count == 0 {
            return;
        }
        self.min = self.min.min(other.min);
        self.max = self.max.max(other.max);
        self.sum_sq += other.sum_sq;
        self.count += other.count;
    }

    /// Min, max and mean square, the three numbers a bucket carries on the
    /// wire; an empty span reads as silence.
    pub fn bucket(&self) -> [f32; 3] {
        if self.count == 0 {
            return [0.0; 3];
        }
        [self.min, self.max, (self.sum_sq / self.count as f64) as f32]
    }
}

/// How many base buckets a question measures at least, aligned: what amortizes
/// the levels above over a view that asks the base grid a bucket at a time.
/// 8192 frames, so a question reads at most that much it did not ask for.
const GROUP: usize = 32;

/// A [`MultiPyramid`] and which of its buckets have been measured.
///
/// `known[level][i]` says the bucket holds what its samples hold: at level 0
/// because it was measured, above because every bucket under it was. The
/// pyramid's own values are combined regardless; the flags are what may be
/// read.
#[derive(Clone)]
pub struct Kept {
    pyramid: MultiPyramid,
    known: Vec<Vec<bool>>,
    /// One base bucket of samples, reused by every measurement.
    scratch: Vec<f32>,
}

impl Kept {
    /// A summary of `frames` frames of `channels` channels with nothing
    /// measured yet: the levels sized, no sample read.
    pub fn unmeasured(frames: usize, channels: usize, base_bucket: usize) -> Self {
        let pyramid = MultiPyramid::empty(frames, channels, base_bucket);
        let known = level_lens(&pyramid)
            .into_iter()
            .map(|n| vec![false; n])
            .collect();
        Self {
            pyramid,
            known,
            scratch: Vec::new(),
        }
    }

    /// A summary built whole elsewhere, every bucket of it measured.
    pub fn measured(pyramid: MultiPyramid) -> Self {
        let known = level_lens(&pyramid)
            .into_iter()
            .map(|n| vec![true; n])
            .collect();
        Self {
            pyramid,
            known,
            scratch: Vec::new(),
        }
    }

    /// The pyramid as it stands. Only a [`Self::is_whole`] one describes every
    /// sample; an unmeasured bucket reads as silence.
    pub fn pyramid(&self) -> &MultiPyramid {
        &self.pyramid
    }

    /// Whether every bucket has been measured, so the pyramid can be handed to
    /// a reader that does not know about the flags (a file a peer maps).
    pub fn is_whole(&self) -> bool {
        self.known
            .last()
            .is_none_or(|top| top.iter().all(|&known| known))
    }

    pub fn frames(&self) -> usize {
        self.pyramid.frames()
    }

    pub fn num_channels(&self) -> usize {
        self.pyramid.num_channels()
    }

    pub fn base_bucket(&self) -> usize {
        self.pyramid.base_bucket()
    }

    /// Forgets every measurement: the samples changed somewhere nobody said.
    pub fn forget(&mut self) {
        for level in &mut self.known {
            level.fill(false);
        }
    }

    /// **Re-measures what was kept of a span a writer touched**, `start` and
    /// `frames` in frames, from `channels` as they now stand. A bucket never
    /// measured stays unmeasured -- the write did not make anybody ask for it
    /// -- so this costs at most the span the write already paid for.
    ///
    /// Returns the samples read.
    pub fn rewrite<S: Source + ?Sized>(
        &mut self,
        channels: &[&S],
        start: usize,
        frames: usize,
    ) -> usize {
        let base = self.base_bucket();
        let end = start.saturating_add(frames).min(self.frames());
        if start >= end {
            return 0;
        }
        self.measure(channels, start / base, (end - 1) / base + 1, true)
    }

    /// **The statistics of channel `channel` over frames `a..b`, exact**: the
    /// whole base buckets inside the span measured if they were not, then
    /// folded through the levels; the partial edges read from the samples.
    ///
    /// `channels` is every channel's samples, because a bucket is measured for
    /// all of them at once -- the next channel's question over the same span
    /// then reads no samples. `read` grows by the samples read,
    /// which is the whole cost of the answer and what a caller bounds a reply
    /// by.
    pub fn span<S: Source + ?Sized>(
        &mut self,
        channels: &[&S],
        channel: usize,
        a: usize,
        b: usize,
        read: &mut usize,
    ) -> SpanStats {
        let b = b.min(self.frames());
        if a >= b || channel >= channels.len() {
            return SpanStats::EMPTY;
        }
        let base = self.base_bucket();
        let (first, last) = (a.div_ceil(base), b / base);
        if first < last && self.known[0][first..last].contains(&false) {
            // The aligned group around what is missing, not only what is
            // missing: a view on the base grid asks a bucket at a time, and
            // the levels above are recombined once per measurement, so a run
            // of one would pay for every level per bucket.
            let lo = first / GROUP * GROUP;
            let hi = (last.div_ceil(GROUP) * GROUP).min(self.known[0].len());
            *read += self.measure(channels, lo, hi, false);
        }
        let source = channels[channel];
        let mut stats = SpanStats::EMPTY;
        let whole = self
            .pyramid
            .channel(channel)
            .and_then(|p| p.aligned_stats(a, b));
        let (inner_a, inner_b) = match whole {
            Some(w) => {
                stats.merge(SpanStats {
                    min: w.min,
                    max: w.max,
                    sum_sq: w.sum_sq,
                    count: w.count,
                });
                (w.start, w.end)
            }
            None => (b, b),
        };
        let mut scratch = std::mem::take(&mut self.scratch);
        for (from, to) in [(a, inner_a), (inner_b, b)] {
            if from < to {
                scratch.resize(to - from, 0.0);
                source.read_into(from, &mut scratch);
                stats.merge(SpanStats::of(&scratch));
                *read += to - from;
            }
        }
        self.scratch = scratch;
        stats
    }

    /// Measures the base buckets in `lo..hi` whose flag is `known` -- the
    /// missing ones for a question, the kept ones for a rewrite -- across
    /// every channel, then recombines the levels above `lo..hi` once and marks
    /// every parent the run completes. Returns the samples read, every
    /// channel's.
    fn measure<S: Source + ?Sized>(
        &mut self,
        channels: &[&S],
        lo: usize,
        hi: usize,
        known: bool,
    ) -> usize {
        let hi = hi.min(self.known[0].len());
        if lo >= hi {
            return 0;
        }
        let base = self.base_bucket();
        let frames = self.frames();
        // Bucket by bucket rather than through `overview`, which reads a
        // bucket past the end as silence: the ragged tail is summarized over
        // the frames it holds, as the builder does.
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.resize(base, 0.0);
        let mut read = 0;
        for b in lo..hi {
            if self.known[0][b] != known {
                continue;
            }
            self.known[0][b] = true;
            let at = b * base;
            let held = &mut scratch[..base.min(frames - at)];
            for (pyramid, source) in self.pyramid.channels.iter_mut().zip(channels) {
                source.read_into(at, held);
                let [min, max, ms] = SpanStats::of(held).bucket();
                let level = &mut pyramid.levels[0];
                level.min[b] = min;
                level.max[b] = max;
                if let Some(level_ms) = level.ms.as_mut() {
                    level_ms[b] = ms;
                }
            }
            read += held.len() * channels.len();
        }
        self.scratch = scratch;
        if read == 0 {
            return 0;
        }
        for pyramid in &mut self.pyramid.channels {
            pyramid.recombine_above(lo, hi - 1);
        }
        let (mut lo, mut hi) = (lo, hi - 1);
        for l in 1..self.known.len() {
            lo /= 2;
            hi /= 2;
            let (below, here) = self.known.split_at_mut(l);
            let below = &below[l - 1];
            for i in lo..=hi.min(here[0].len().saturating_sub(1)) {
                let a = below[2 * i];
                here[0][i] = a && below.get(2 * i + 1).is_none_or(|&b| b);
            }
        }
        read
    }
}

/// How many buckets each level of `pyramid` holds, finest first.
fn level_lens(pyramid: &MultiPyramid) -> Vec<usize> {
    let levels = pyramid.channel(0).map_or(0, |p| p.num_levels());
    let mut n = pyramid.frames().div_ceil(pyramid.base_bucket());
    let mut lens = Vec::with_capacity(levels);
    for _ in 0..levels {
        lens.push(n);
        n = n.div_ceil(2);
    }
    lens
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peaks::Interleaved;

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

    fn assert_close(got: SpanStats, want: SpanStats) {
        assert_eq!(
            (got.min, got.max, got.count),
            (want.min, want.max, want.count)
        );
        let (g, w) = (got.bucket()[2], want.bucket()[2]);
        assert!((g - w).abs() <= 1e-5 * w.abs().max(1e-3), "ms {g} vs {w}");
    }

    /// Any span, on the grid or not, answers what its samples hold, and asking
    /// again reads no samples for what was measured the first time.
    #[test]
    fn a_span_answers_its_samples_and_measures_once() {
        let (frames, channels) = (10_000, 2);
        let data = noise(frames * channels, 7);
        let views: Vec<_> = (0..channels)
            .map(|ch| Interleaved::new(&data, channels, ch))
            .collect();
        let sources: Vec<_> = views.iter().collect();
        let mut kept = Kept::unmeasured(frames, channels, 256);
        for (a, b) in [
            (0, 10_000),
            (300, 4_097),
            (5, 200),
            (512, 1_024),
            (9_000, 10_000),
        ] {
            for ch in 0..channels {
                let mut read = 0;
                let got = kept.span(&sources, ch, a, b, &mut read);
                let want: Vec<f32> = (a..b).map(|f| data[f * channels + ch]).collect();
                assert_close(got, SpanStats::of(&want));
            }
        }
        assert!(kept.known[0][..frames / 256].iter().all(|&k| k));
        let mut read = 0;
        kept.span(&sources, 0, 512, 8_192, &mut read);
        assert_eq!(
            read, 0,
            "an aligned span over measured buckets reads no samples"
        );
        kept.span(&sources, 1, 300, 8_000, &mut read);
        assert_eq!(read, (512 - 300) + (8_000 - 7_936), "only the two edges");
    }

    /// A summary measured a piece at a time ends as the one built whole, and
    /// says so.
    #[test]
    fn measured_in_pieces_it_equals_the_whole_build() {
        let frames = 30_000;
        let data = noise(frames, 3);
        let mut kept = Kept::unmeasured(frames, 1, 256);
        let sources = [&data[..]];
        let mut read = 0;
        kept.span(&sources, 0, 1_000, 2_000, &mut read);
        assert!(!kept.is_whole());
        kept.span(&sources, 0, 0, frames, &mut read);
        // The ragged tail is never a whole bucket of a span; the group around
        // the last whole one measured it.
        assert!(kept.is_whole());
        let built = MultiPyramid::build_interleaved(&data, 1, 256);
        assert_eq!(kept.pyramid().to_bytes(), built.to_bytes());
    }

    /// A write re-measures what was kept of its span and nothing else; a write
    /// nobody described is forgotten whole.
    #[test]
    fn a_rewrite_follows_the_span_and_forget_starts_over() {
        let frames = 16_384;
        let mut data = noise(frames, 11);
        let mut kept = Kept::unmeasured(frames, 1, 256);
        let mut read = 0;
        kept.span(&[&data[..]], 0, 0, 2_048, &mut read);
        assert_eq!(read, 8_192, "the question measured its aligned group");
        for s in &mut data[7_000..9_000] {
            *s = 0.0;
        }
        let read = kept.rewrite(&[&data[..]], 7_000, 2_000);
        assert_eq!(
            read,
            8_192 - 6_912,
            "buckets 27..32 were kept, 32..36 were not (mono)"
        );
        assert!(!kept.known[0][32]);
        let mut read = 0;
        let got = kept.span(&[&data[..]], 0, 0, 8_192, &mut read);
        assert_eq!(read, 0);
        assert_close(got, SpanStats::of(&data[..8_192]));
        kept.forget();
        assert!(kept.known.iter().all(|l| l.iter().all(|&k| !k)));
    }
}
