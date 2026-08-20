//! Peak pyramid: resolution-matched min/max **and mean square** for navigable
//! views.
//!
//! A waveform view is never drawn sample-by-sample and never processes millions
//! of samples per frame. Instead a *peak* pyramid is computed once: level 0
//! summarizes every `base_bucket` samples into a `(min, max)` pair **and a mean
//! square**, and each higher level halves the resolution. At draw time the level
//! whose bucket size matches the current `samples_per_px` is selected, so each
//! rendered pixel column reads only ~one bucket — work proportional to the
//! window width, not to the buffer length.
//!
//! The third statistic is stored as **energy (mean square), never as RMS**, and
//! that is what makes a level exact rather than approximate: mean squares
//! combine (a weighted mean over the samples each bucket actually holds) while
//! roots do not, so a bucket at any level equals the direct mean square of its
//! samples, and a renderer folding several levels into one span sums energies
//! without bias.
//! The square root belongs to whoever displays it.
//!
//! This is **general Clausters client functionality**, not real-time audio
//! processing: any client with a waveform view wants it, so it lives once in the
//! shared core (and is reachable from non-Rust clients through the FFI) rather
//! than re-implemented per client. The server itself never needs it.
//!
//! Computing peaks for a long buffer is the expensive part, so the result is a
//! cache: it lives in memory and can be serialized to a file (the way audio
//! editors keep an overview/peak file beside the audio) and read back —
//! `to_bytes`/`from_bytes` and `write_cache`/`read_cache`. The layout (see
//! [`crate::bytes`]) is a flat sequence of `f32` arrays, so a build can
//! memory-map it instead of reading it into RAM — the local shared-resource
//! path the GUI host uses to render a multi-megabyte buffer with no per-frame or
//! over-the-wire re-send. The format is machine-local (native float byte order).

use std::fs;
use std::io;
use std::path::Path;

use crate::bytes;

const MAGIC: &[u8; 4] = b"CLPK";
/// Version 1 is the original mono layout; version 2 prefixed a channel count
/// and carried one level sequence per channel; **version 3 is the current
/// one**, the v2 shape with a mean-square array beside each level's min/max.
/// Both writers emit v3 (a mono cache is one channel), and readers accept all
/// three — a v1 or v2 cache carries no mean square, which
/// [`Pyramid::has_mean_square`] reports rather than faking with zeros.
const VERSION: u32 = 1;
const VERSION_MULTI: u32 = 2;
const VERSION_MEASURE: u32 = 3;

/// A read-only sequence of samples a pyramid can summarize **without owning
/// it** — the door that lets a summary be taken over samples that lives
/// somewhere else.
///
/// The pyramid used to be a function of a `&[f32]`, which quietly required
/// every caller to have the whole take in memory at once: a host drawing a
/// mapped region read it out first, a client rewriting one span of a cache
/// handed over the entire buffer to do it, and a renderer could not summarize
/// what it was streaming because the samples were gone by the end. A source
/// asks for two things instead — how long it is, and a span copied into a
/// caller-sized window — and a bounded scratch buffer is all a build needs.
///
/// It is **mono**: one source is one channel, because that is what a pyramid
/// summarizes. Interleaved samples are read through [`Interleaved`], which is
/// the adapter and not a second shape of the trait.
pub trait Source {
    /// How many samples this source holds.
    fn len(&self) -> usize;

    /// Copies `out.len()` samples starting at `start` into `out`. A read past
    /// the end fills the remainder with zeros rather than panicking — the
    /// callers here never ask for one, and a summary of silence is a better
    /// failure than a crash in a draw path.
    fn read_into(&self, start: usize, out: &mut [f32]);

    /// Whether the source holds no samples.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Source for [f32] {
    fn len(&self) -> usize {
        <[f32]>::len(self)
    }

    fn read_into(&self, start: usize, out: &mut [f32]) {
        let end = start.saturating_add(out.len()).min(<[f32]>::len(self));
        let n = end.saturating_sub(start);
        out[..n].copy_from_slice(&self[start..end]);
        out[n..].fill(0.0);
    }
}

impl<S: Source + ?Sized> Source for &S {
    fn len(&self) -> usize {
        (**self).len()
    }

    fn read_into(&self, start: usize, out: &mut [f32]) {
        (**self).read_into(start, out);
    }
}

/// One channel of an interleaved buffer, read with a stride rather than
/// de-interleaved into a buffer of its own.
///
/// That is its whole reason: copying a channel out to summarize one span of it
/// costs the take, which is exactly what summarizing a span exists to avoid.
pub struct Interleaved<'a> {
    data: &'a [f32],
    channels: usize,
    channel: usize,
}

impl<'a> Interleaved<'a> {
    /// Channel `channel` of `data`, which holds `channels` interleaved
    /// channels. A trailing partial frame is ignored, as everywhere else here.
    pub fn new(data: &'a [f32], channels: usize, channel: usize) -> Self {
        Self {
            data,
            channels: channels.max(1),
            channel,
        }
    }
}

impl Source for Interleaved<'_> {
    fn len(&self) -> usize {
        if self.channel >= self.channels {
            return 0;
        }
        self.data.len() / self.channels
    }

    fn read_into(&self, start: usize, out: &mut [f32]) {
        let frames = Source::len(self);
        for (i, slot) in out.iter_mut().enumerate() {
            let f = start + i;
            *slot = if f < frames {
                self.data[f * self.channels + self.channel]
            } else {
                0.0
            };
        }
    }
}

/// Min/max over a slice, or `None` if empty.
pub fn min_max(samples: &[f32]) -> Option<(f32, f32)> {
    let (&first, rest) = samples.split_first()?;
    let mut lo = first;
    let mut hi = first;
    for &s in rest {
        lo = lo.min(s);
        hi = hi.max(s);
    }
    Some((lo, hi))
}

/// Mean square (energy) over a slice, or `None` if empty — the pyramid's third
/// statistic computed directly, and what a renderer zoomed in past `base_bucket`
/// reads instead of a bucket. Accumulated in `f64`, since a long window of small
/// squares loses the tail of the sum in `f32`.
pub fn mean_square(samples: &[f32]) -> Option<f32> {
    if samples.is_empty() {
        return None;
    }
    let sum: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
    Some((sum / samples.len() as f64) as f32)
}

/// One resolution level: `min[i]`/`max[i]`/`ms[i]` summarize `bucket` source
/// samples. `ms` is `None` for a pyramid parsed from a v1/v2 cache, which
/// predates the statistic — an absent measure, not a zero one.
///
/// `Clone` because an edit copies the pyramid it is about to patch: the picture
/// on screen and the copy being written have to be two objects for the frame in
/// between, and copying a pyramid is a fraction of copying the samples under it.
#[derive(Clone)]
struct Level {
    bucket: usize,
    min: Vec<f32>,
    max: Vec<f32>,
    ms: Option<Vec<f32>>,
}

/// How many source samples the entry at `index` of a `bucket`-sized level holds,
/// given the buffer's length: `bucket`, or the ragged remainder at the tail.
/// **Derived rather than stored**, which is what lets a level combine exactly
/// without the cache growing a count array.
fn bucket_count(total_samples: usize, bucket: usize, index: usize) -> usize {
    total_samples
        .saturating_sub(index.saturating_mul(bucket))
        .min(bucket)
}

/// A min/max pyramid over a mono buffer. Total storage is ~2x the level-0 size,
/// i.e. a small constant fraction of the source (e.g. ~0.8% at `base_bucket`
/// 256), so it is cheap to keep resident or cache to disk.
/// What [`Pyramid::aligned_stats`] folded: the statistics over the whole
/// buckets inside a span, and the sample bounds they cover.
///
/// The energy is a **sum of squares with its count** rather than a mean, so a
/// caller can fold the partial edges in and divide once. Combining means would
/// need the counts anyway, and doing it here would round twice.
#[derive(Clone, Copy, Debug)]
pub struct BucketStats {
    pub min: f32,
    pub max: f32,
    pub sum_sq: f64,
    pub count: usize,
    /// First sample the buckets cover.
    pub start: usize,
    /// One past the last sample they cover.
    pub end: usize,
    /// Whether every bucket carried the mean square — false for a cache
    /// written before the statistic existed, which is an absent measure and
    /// not a zero one.
    pub measured: bool,
}

/// One level-0 bucket **somebody else measured** — min, max and mean square
/// over `base_bucket` samples, the pyramid's own three statistics in its own
/// energy form.
///
/// It exists because a summary does not always come from samples the holder
/// has. A page cannot map the memory a recording is filling, so the server
/// sends it the overview instead (`/buffer_stream.reply`, whose payload is
/// exactly a run of these): the measuring already happened, at the writer's
/// end, and what is left for the receiver is to put the buckets where they
/// belong and recombine the levels above them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bucket {
    pub min: f32,
    pub max: f32,
    /// Mean square (energy) over the bucket's samples, never RMS — see the
    /// module note: means combine and roots do not.
    pub ms: f32,
}

#[derive(Clone)]
pub struct Pyramid {
    base_bucket: usize,
    total_samples: usize,
    levels: Vec<Level>,
}

impl Pyramid {
    /// Build the pyramid from mono `samples`. `base_bucket` is the level-0
    /// bucket size (e.g. 256). Smaller buckets give finer detail before the
    /// renderer must fall back to raw samples, at the cost of more storage.
    pub fn build(samples: &[f32], base_bucket: usize) -> Self {
        Self::build_from(samples, base_bucket)
    }

    /// [`Self::build`] over any [`Source`] — the general form, which the slice
    /// one calls.
    ///
    /// Level 0 is filled a bucket at a time through **one scratch window**, so
    /// building a summary of a ten-minute take over a mapped region allocates
    /// the summary and a kilobyte, not the take.
    pub fn build_from<S: Source + ?Sized>(source: &S, base_bucket: usize) -> Self {
        assert!(base_bucket >= 1);
        let total_samples = source.len();

        let n0 = total_samples.div_ceil(base_bucket);
        let mut min0 = vec![0.0f32; n0];
        let mut max0 = vec![0.0f32; n0];
        let mut ms0 = vec![0.0f32; n0];
        let mut window = vec![0.0f32; base_bucket];
        for b in 0..n0 {
            let from = b * base_bucket;
            let n = bucket_count(total_samples, base_bucket, b);
            let chunk = &mut window[..n];
            source.read_into(from, chunk);
            let (lo, hi) = min_max(chunk).unwrap_or((0.0, 0.0));
            min0[b] = lo;
            max0[b] = hi;
            ms0[b] = mean_square(chunk).unwrap_or(0.0);
        }
        let mut levels = vec![Level {
            bucket: base_bucket,
            min: min0,
            max: max0,
            ms: Some(ms0),
        }];

        while levels.last().unwrap().min.len() > 1 {
            let prev = levels.last().unwrap();
            let n = prev.min.len().div_ceil(2);
            let mut min = vec![0.0f32; n];
            let mut max = vec![0.0f32; n];
            let mut ms = vec![0.0f32; n];
            let prev_ms = prev.ms.as_ref().expect("a built level carries its measure");
            for i in 0..n {
                let a = 2 * i;
                // The odd tail has no sibling. Min/max may read `a` twice (both
                // are idempotent), but an energy averaged with itself would
                // weigh the tail bucket as if it held twice its samples, so the
                // sibling is an `Option` here rather than a clamped index.
                let b = (2 * i + 1 < prev.min.len()).then_some(2 * i + 1);
                min[i] = prev.min[a].min(prev.min[b.unwrap_or(a)]);
                max[i] = prev.max[a].max(prev.max[b.unwrap_or(a)]);
                // The exact combination: each child weighted by the samples it
                // actually holds, which is the buffer's length and not the
                // bucket size at the ragged end.
                let na = bucket_count(total_samples, prev.bucket, a);
                let nb = b.map_or(0, |b| bucket_count(total_samples, prev.bucket, b));
                let total = na + nb;
                ms[i] = if total == 0 {
                    0.0
                } else {
                    let sum = prev_ms[a] as f64 * na as f64
                        + b.map_or(0.0, |b| prev_ms[b] as f64 * nb as f64);
                    (sum / total as f64) as f32
                };
            }
            levels.push(Level {
                bucket: prev.bucket * 2,
                min,
                max,
                ms: Some(ms),
            });
        }

        Self {
            base_bucket,
            total_samples,
            levels,
        }
    }

    /// **An empty summary of a given length** — every bucket a measured zero,
    /// the levels sized as a build over `total_samples` would size them, and
    /// no samples read or held anywhere.
    ///
    /// It is what a take **allocated to be recorded into** is: the picture is
    /// the whole of the box it will fill, so the axis does not move while it
    /// fills, and the only thing missing is the samples — which is exactly
    /// what has not happened yet. A client that cannot map the memory being
    /// written builds one of these and fills it from
    /// [`Self::write_buckets`] as the reports arrive; building it out of a
    /// buffer of silence instead would allocate the take (230 MB for ten
    /// minutes of stereo) to summarize samples nobody wrote.
    ///
    /// The zeros are honest here, unlike a cache with no measure at all: an
    /// unwritten frame *is* silence in the buffer. Whether a view draws that
    /// stretch or leaves it empty is the view's own question — the host's
    /// `fills` prop — and not the summary's.
    pub fn empty(total_samples: usize, base_bucket: usize) -> Self {
        assert!(base_bucket >= 1);
        let mut levels = Vec::new();
        let mut n = total_samples.div_ceil(base_bucket);
        let mut bucket = base_bucket;
        loop {
            levels.push(Level {
                bucket,
                min: vec![0.0; n],
                max: vec![0.0; n],
                ms: Some(vec![0.0; n]),
            });
            if n <= 1 {
                break;
            }
            n = n.div_ceil(2);
            bucket *= 2;
        }
        Self {
            base_bucket,
            total_samples,
            levels,
        }
    }

    /// Rebuilds only the part of the pyramid a sample span touches — the
    /// summary's answer to an edit, so a redraw costs the span rather than the
    /// take.
    ///
    /// `samples` is the **whole** buffer as it now stands, not the span: a
    /// bucket at either edge of the span holds untouched samples too, and
    /// summarizing it needs them. `start` and `len` are sample positions, and
    /// what is rebuilt is every level-0 bucket the span overlaps and every
    /// bucket above them — `span/base_bucket + levels` work instead of the
    /// buffer's.
    ///
    /// The result is **identical to a full rebuild**, which the tests assert
    /// rather than trust: the combination upward is the same sample-weighted
    /// mean the builder uses, so an updated pyramid and a fresh one cannot
    /// drift apart over a session of edits.
    ///
    /// Returns `false`, changing nothing, when `samples` is not the buffer this
    /// pyramid describes — an edit that changed the *length* is a rebuild and
    /// not an update, and quietly summarizing the wrong samples would be worse
    /// than refusing. A cache written before the mean square joined (v1/v2)
    /// keeps its min/max updated and stays without a measure rather than
    /// gaining an invented one.
    pub fn update_range(&mut self, samples: &[f32], start: usize, len: usize) -> bool {
        self.update_range_from(&samples, start, len)
    }

    /// [`Self::update_range`] over any [`Source`] — the general form, and the
    /// one an editor wants: the samples are already where they belong (a
    /// server buffer, a mapped region), and what is left is the summary of the
    /// span that moved.
    ///
    /// Refuses, changing nothing, when the source is not the length this
    /// pyramid describes, for the reason the slice form gives: a length change
    /// is a rebuild, not an update.
    pub fn update_range_from<S: Source + ?Sized>(
        &mut self,
        source: &S,
        start: usize,
        len: usize,
    ) -> bool {
        if source.len() != self.total_samples {
            return false;
        }
        let end = start.saturating_add(len).min(self.total_samples);
        if start >= end || self.levels.is_empty() {
            // An empty span is nothing to do rather than a failure: a gesture
            // that wrote no samples has nothing to re-summarize.
            return true;
        }

        // Level 0, from the samples themselves.
        let base = self.base_bucket;
        let (lo, hi) = (start / base, (end - 1) / base);
        {
            let total = self.total_samples;
            let level = &mut self.levels[0];
            let last = level.min.len().saturating_sub(1);
            let mut window = vec![0.0f32; base];
            for b in lo..=hi.min(last) {
                let n = bucket_count(total, base, b);
                let chunk = &mut window[..n];
                source.read_into(b * base, chunk);
                let (mn, mx) = min_max(chunk).unwrap_or((0.0, 0.0));
                level.min[b] = mn;
                level.max[b] = mx;
                if let Some(ms) = level.ms.as_mut() {
                    ms[b] = mean_square(chunk).unwrap_or(0.0);
                }
            }
        }
        self.recombine_above(lo, hi);
        true
    }

    /// Writes level-0 buckets **already summarized elsewhere** at `first` and
    /// recombines the levels above them — the pyramid's door for a summary
    /// that arrives instead of being taken.
    ///
    /// This is [`Self::update_range_from`] with the measuring skipped, and the
    /// reason it is a separate door rather than a `Source` of buckets is that
    /// there are no samples anywhere in it: the caller holds a picture and
    /// receives an overview of samples it will never see (`/buffer_stream`,
    /// which sends 2 kB/s where the audio is 190). Level 0 takes the buckets
    /// as given and every level above is recombined the way the builder
    /// combines them, so a pyramid filled this way answers exactly as one
    /// built from the samples would.
    ///
    /// `first` is a **bucket index**, not a sample position, because that is
    /// the only unit in which this is well defined — a bucket that started
    /// somewhere else is a different bucket.
    ///
    /// Returns `false`, changing nothing, when the run does not fit: level 0
    /// has as many buckets as the buffer this pyramid describes, and writing
    /// past them would either grow the summary past its samples or wrap it.
    /// A pyramid parsed from a v1/v2 cache keeps its min/max updated and stays
    /// without a measure, like every other write here.
    pub fn write_buckets(&mut self, first: usize, buckets: &[Bucket]) -> bool {
        let Some(level0) = self.levels.first_mut() else {
            return false;
        };
        let last = first.saturating_add(buckets.len());
        if last > level0.min.len() {
            return false;
        }
        if buckets.is_empty() {
            // Nothing to do rather than a failure, as an empty span is: a
            // report that carried no whole bucket is a report about nothing.
            return true;
        }
        for (i, b) in buckets.iter().enumerate() {
            level0.min[first + i] = b.min;
            level0.max[first + i] = b.max;
            if let Some(ms) = level0.ms.as_mut() {
                ms[first + i] = b.ms;
            }
        }
        self.recombine_above(first, last - 1);
        true
    }

    /// Rebuilds every level above 0 whose children moved, from the one below —
    /// the builder's own combination, applied to the buckets `lo..=hi` of level
    /// 0 and to their parents upward.
    fn recombine_above(&mut self, mut lo: usize, mut hi: usize) {
        for l in 1..self.levels.len() {
            lo /= 2;
            hi /= 2;
            let (below, here) = self.levels.split_at_mut(l);
            let prev = &below[l - 1];
            let level = &mut here[0];
            let prev_len = prev.min.len();
            let last = level.min.len().saturating_sub(1);
            for i in lo..=hi.min(last) {
                let a = 2 * i;
                let b = (2 * i + 1 < prev_len).then_some(2 * i + 1);
                level.min[i] = prev.min[a].min(prev.min[b.unwrap_or(a)]);
                level.max[i] = prev.max[a].max(prev.max[b.unwrap_or(a)]);
                if let (Some(ms), Some(prev_ms)) = (level.ms.as_mut(), prev.ms.as_ref()) {
                    let na = bucket_count(self.total_samples, prev.bucket, a);
                    let nb = b.map_or(0, |b| bucket_count(self.total_samples, prev.bucket, b));
                    let total = na + nb;
                    ms[i] = if total == 0 {
                        0.0
                    } else {
                        let sum = prev_ms[a] as f64 * na as f64
                            + b.map_or(0.0, |b| prev_ms[b] as f64 * nb as f64);
                        (sum / total as f64) as f32
                    };
                }
            }
        }
    }

    /// [`Self::update_range`] over one channel of an interleaved buffer, read
    /// with a stride instead of being de-interleaved first.
    ///
    /// That is the whole reason it exists: a take is interleaved, and copying
    /// every channel out of it to update one span would cost the buffer —
    /// exactly what updating a range is for avoiding. `start` and `len` are in
    /// **frames**, since that is what the caller's span is.
    pub(crate) fn update_interleaved(
        &mut self,
        data: &[f32],
        channels: usize,
        channel: usize,
        start: usize,
        len: usize,
    ) -> bool {
        self.update_range_from(&Interleaved::new(data, channels, channel), start, len)
    }

    /// Min, max, energy and sample count over the level-0 buckets **fully
    /// contained** in `[a, b)`, folded through the pyramid rather than read at
    /// one level — and the sample bounds of what it covers, so a caller with
    /// the samples can close the two partial edges itself.
    ///
    /// **Why this exists rather than [`Self::column`].** Reading a column at
    /// one level takes every bucket *overlapping* it, so an unaligned column
    /// one bucket wide reads two: a transient a hundred samples outside the
    /// column is drawn inside it, and it appears the moment a zoom crosses
    /// into the pyramid's regime. Folding instead of reading makes the answer
    /// independent of the level it came from, which is what removes the step —
    /// the walk takes the largest aligned block at each position, a segment
    /// tree's own walk, so it costs the logarithm of the span rather than its
    /// buckets.
    ///
    /// `None` when no whole bucket fits: the span is all edge, and the edge is
    /// the samples' business.
    pub fn aligned_stats(&self, a: usize, b: usize) -> Option<BucketStats> {
        let base = self.base_bucket;
        let level0 = self.levels.first()?;
        let buckets = level0.min.len();
        let first = a.div_ceil(base);
        let last = (b / base).min(buckets);
        if last <= first {
            return None;
        }
        let mut stats = BucketStats {
            min: f32::INFINITY,
            max: f32::NEG_INFINITY,
            sum_sq: 0.0,
            count: 0,
            start: first * base,
            end: (last * base).min(self.total_samples),
            measured: true,
        };
        let mut i = first;
        while i < last {
            // The largest level whose bucket starts here and still fits. A
            // block of 2^level level-0 buckets is aligned exactly when the
            // index has that many trailing zeros.
            let mut level = 0;
            while level + 1 < self.levels.len()
                && (i >> level).is_multiple_of(2)
                && i + (2usize << level) <= last
            {
                level += 1;
            }
            let lvl = &self.levels[level];
            let index = i >> level;
            if index >= lvl.min.len() {
                break;
            }
            stats.min = stats.min.min(lvl.min[index]);
            stats.max = stats.max.max(lvl.max[index]);
            let n = bucket_count(self.total_samples, lvl.bucket, index);
            stats.count += n;
            match lvl.ms.as_ref() {
                Some(ms) => stats.sum_sq += ms[index] as f64 * n as f64,
                None => stats.measured = false,
            }
            i += 1usize << level;
        }
        (stats.count > 0).then_some(stats)
    }

    pub fn base_bucket(&self) -> usize {
        self.base_bucket
    }

    pub fn total_samples(&self) -> usize {
        self.total_samples
    }

    pub fn num_levels(&self) -> usize {
        self.levels.len()
    }

    /// The bucket size (source samples per entry) of `level`, if it exists.
    /// A renderer folding levels into one span uses it to weight the
    /// blend by where `samples_per_px` sits between the two buckets.
    pub fn level_bucket(&self, level: usize) -> Option<usize> {
        self.levels.get(level).map(|l| l.bucket)
    }

    /// Pick the finest level whose bucket does not exceed `samples_per_px`, so
    /// each pixel column aggregates ~one bucket (no gaps, minimal work). When
    /// zoomed in finer than level 0, level 0 is returned and the caller should
    /// read raw samples instead (see the waveform view's `column`).
    pub fn level_for(&self, samples_per_px: f64) -> usize {
        let mut chosen = 0;
        for (i, lvl) in self.levels.iter().enumerate() {
            if (lvl.bucket as f64) <= samples_per_px {
                chosen = i;
            } else {
                break;
            }
        }
        chosen
    }

    /// Min/max of the buckets overlapping `[s0, s1)` at `level`, or `None` if
    /// the level is empty.
    pub fn column(&self, level: usize, s0: f64, s1: f64) -> Option<(f32, f32)> {
        let lvl = self.levels.get(level)?;
        if lvl.min.is_empty() {
            return None;
        }
        let last = lvl.min.len() - 1;
        let b0 = ((s0 / lvl.bucket as f64).floor().max(0.0) as usize).min(last);
        let b1 = ((s1 / lvl.bucket as f64).ceil() as usize)
            .saturating_sub(1)
            .clamp(b0, last);
        let lo = lvl.min[b0..=b1]
            .iter()
            .copied()
            .fold(f32::INFINITY, f32::min);
        let hi = lvl.max[b0..=b1]
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        Some((lo, hi))
    }

    /// Whether this pyramid carries the mean-square statistic — true of anything
    /// [`Pyramid::build`] produced, false of a cache parsed from the v1/v2
    /// layouts, which predate it. A view asks before drawing a measured layer:
    /// the honest answer to an old cache is to draw no layer, never a layer of
    /// zeros (which is silence, a measurement it never made).
    pub fn has_mean_square(&self) -> bool {
        self.levels.first().is_none_or(|l| l.ms.is_some())
    }

    /// The mean square of the buckets overlapping `[s0, s1)` at `level`, each
    /// weighted by the samples it holds — the energy sibling of [`column`], and
    /// `None` when the level is empty or the cache carries no measure. The span
    /// is taken bucket-wise exactly as `column` takes it, so the two answer over
    /// the same samples; below `base_bucket` a caller reads the samples through
    /// [`mean_square`] instead.
    ///
    /// [`column`]: Pyramid::column
    pub fn column_ms(&self, level: usize, s0: f64, s1: f64) -> Option<f32> {
        let lvl = self.levels.get(level)?;
        let ms = lvl.ms.as_ref()?;
        if ms.is_empty() {
            return None;
        }
        let last = ms.len() - 1;
        let b0 = ((s0 / lvl.bucket as f64).floor().max(0.0) as usize).min(last);
        let b1 = ((s1 / lvl.bucket as f64).ceil() as usize)
            .saturating_sub(1)
            .clamp(b0, last);
        let mut sum = 0.0f64;
        let mut count = 0usize;
        for (i, &e) in ms[b0..=b1].iter().enumerate() {
            let n = bucket_count(self.total_samples, lvl.bucket, b0 + i);
            sum += e as f64 * n as f64;
            count += n;
        }
        (count > 0).then(|| (sum / count as f64) as f32)
    }

    /// Serialize to a flat byte buffer (the on-disk/cache layout) — the v3
    /// layout, as one channel, so a mono and a multichannel cache differ only in
    /// their channel count and one reader serves both.
    pub fn to_bytes(&self) -> Vec<u8> {
        MultiPyramid::write(std::slice::from_ref(self))
    }

    /// Parse a buffer produced by `to_bytes`, or `None` if malformed — the v3
    /// and v2 layouts when they hold exactly one channel (a multichannel cache
    /// is [`MultiPyramid::from_bytes`]'s to read, never silently narrowed to its
    /// first channel here), and the v1 mono layout that predates both.
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        let mut r = bytes::Reader::new(data);
        r.tag(MAGIC)?;
        if r.u32()? != VERSION {
            let mut channels = MultiPyramid::from_bytes(data)?.into_channels();
            return (channels.len() == 1).then(|| channels.remove(0));
        }
        let base_bucket = r.usize()?;
        let total_samples = r.usize()?;
        let n_levels = r.usize()?;
        let mut levels = Vec::with_capacity(n_levels);
        for _ in 0..n_levels {
            let bucket = r.usize()?;
            let len = r.usize()?;
            let min = r.f32_vec(len)?;
            let max = r.f32_vec(len)?;
            levels.push(Level {
                bucket,
                min,
                max,
                ms: None,
            });
        }
        Some(Self {
            base_bucket,
            total_samples,
            levels,
        })
    }

    /// Write the cache to `path` (e.g. a file beside the audio).
    pub fn write_cache(&self, path: impl AsRef<Path>) -> io::Result<()> {
        fs::write(path, self.to_bytes())
    }

    /// Read a cache from `path`. Returns `Ok(None)` if the file is malformed
    /// (e.g. an older format), so the caller can recompute.
    pub fn read_cache(path: impl AsRef<Path>) -> io::Result<Option<Self>> {
        Ok(Self::from_bytes(&fs::read(path)?))
    }
}

/// The exact [`Pyramid::to_bytes`] length for a pyramid of `total_samples`
/// samples at `base_bucket`, computed **without** building it. A client (e.g.
/// over the FFI) sizes its output buffer with this before calling `build`. It
/// mirrors the level structure `build` produces (level 0 of `ceil(n /
/// base_bucket)` buckets, each higher level halving until length 1), so the
/// `cache_size_matches_to_bytes_len` test pins the two together. Since v3 a mono
/// cache *is* a one-channel one, so this is [`multi_cache_size`] at one channel
/// rather than a second count of the same bytes.
pub fn cache_size(total_samples: usize, base_bucket: usize) -> usize {
    multi_cache_size(total_samples, 1, base_bucket)
}

/// A peak pyramid per channel of a multichannel buffer, sharing one
/// `base_bucket` and one per-channel length. This is **one cache resource**
/// (a single file/byte buffer, `to_bytes` version 2) rather than per-channel
/// sibling files, so a multichannel waveform view names exactly one `cache`
/// prop and the channels can never drift apart. `from_bytes` also accepts the
/// version-1 mono layout (as one channel), so existing caches keep working.
pub struct MultiPyramid {
    channels: Vec<Pyramid>,
}

impl MultiPyramid {
    /// Builds one pyramid per channel from `samples` holding `channels`
    /// interleaved channels (`channels >= 1`; a trailing partial frame is
    /// ignored). The de-interleave lives here — core-side — so every client
    /// builds the identical multichannel cache from the same flat buffer, and
    /// it is a **stride** rather than a copy ([`Interleaved`]): a channel is
    /// read where it lies.
    pub fn build_interleaved(samples: &[f32], channels: usize, base_bucket: usize) -> Self {
        let channels = channels.max(1);
        let pyramids = (0..channels)
            .map(|ch| Pyramid::build_from(&Interleaved::new(samples, channels, ch), base_bucket))
            .collect();
        Self { channels: pyramids }
    }

    /// **An empty multichannel summary**: [`Pyramid::empty`] per channel — the
    /// picture of a take that has been allocated and not yet recorded into.
    pub fn empty(frames: usize, channels: usize, base_bucket: usize) -> Self {
        let channels = channels.max(1);
        Self {
            channels: (0..channels)
                .map(|_| Pyramid::empty(frames, base_bucket))
                .collect(),
        }
    }

    /// Wraps already-built per-channel pyramids (they must share `base_bucket`
    /// and length; `build_interleaved` guarantees it).
    /// [`Pyramid::update_range`] across every channel, over the interleaved
    /// buffer as it now stands. `start` and `len` are **frames**.
    ///
    /// Each channel is read with a stride rather than de-interleaved, so the
    /// cost is the span and not the take — which is the point of the whole
    /// function, and would be lost by copying the channels out first.
    ///
    /// Returns `false`, changing nothing, when the buffer is not the one this
    /// cache describes.
    pub fn update_range(&mut self, interleaved: &[f32], start: usize, len: usize) -> bool {
        let channels = self.channels.len();
        if channels == 0 || interleaved.len() != self.frames() * channels {
            return false;
        }
        for (ch, pyr) in self.channels.iter_mut().enumerate() {
            if !pyr.update_interleaved(interleaved, channels, ch, start, len) {
                return false;
            }
        }
        true
    }

    /// [`Pyramid::write_buckets`] across every channel, from one run of
    /// buckets in the layout the wire uses: **bucket-major, channel-minor** —
    /// for each bucket in order, for each channel, `min`, `max` and `ms`.
    ///
    /// That is `/buffer_stream.reply`'s payload read as `f32`s, so a client
    /// folding a recording into the picture it holds converts nothing: it
    /// hands over the numbers as they arrived. `start_frame` is where the
    /// report begins on the buffer's own sample axis, which is what the reply
    /// carries.
    ///
    /// Returns `false`, changing nothing, when the report and this cache do
    /// not describe the same grid:
    ///
    /// - `bucket` differs from this cache's `base_bucket`. A coarser report
    ///   would have to be spread over buckets it never measured separately,
    ///   and a finer one folded in groups that straddle report boundaries —
    ///   both are answers this cannot give honestly, and the caller chooses
    ///   the bucket when it subscribes, so agreeing is free.
    /// - `start_frame` is not on a bucket boundary, for the same reason.
    /// - the run does not fit the buffer, or its length is not a whole number
    ///   of buckets across every channel.
    pub fn write_buckets(&mut self, start_frame: usize, bucket: usize, stats: &[f32]) -> bool {
        let channels = self.channels.len();
        if channels == 0 || bucket == 0 || bucket != self.base_bucket() {
            return false;
        }
        if !start_frame.is_multiple_of(bucket) {
            return false;
        }
        let stride = channels * 3;
        if !stats.len().is_multiple_of(stride) {
            return false;
        }
        let first = start_frame / bucket;
        let n = stats.len() / stride;
        // Checked once, before anything is written: every channel shares this
        // cache's length and grid, so a run that fits one fits all — and a
        // refusal halfway would leave the channels describing different
        // samples, which is the one state this format promises cannot happen.
        if first + n > self.frames().div_ceil(bucket) {
            return false;
        }
        let mut run: Vec<Bucket> = Vec::with_capacity(n);
        for (ch, pyr) in self.channels.iter_mut().enumerate() {
            run.clear();
            run.extend((0..n).map(|b| {
                let at = b * stride + ch * 3;
                Bucket {
                    min: stats[at],
                    max: stats[at + 1],
                    ms: stats[at + 2],
                }
            }));
            if !pyr.write_buckets(first, &run) {
                return false;
            }
        }
        true
    }

    pub fn from_channels(channels: Vec<Pyramid>) -> Self {
        assert!(!channels.is_empty());
        Self { channels }
    }

    pub fn num_channels(&self) -> usize {
        self.channels.len()
    }

    /// Channel `ch`'s pyramid.
    pub fn channel(&self, ch: usize) -> Option<&Pyramid> {
        self.channels.get(ch)
    }

    /// Consumes the cache into its per-channel pyramids.
    pub fn into_channels(self) -> Vec<Pyramid> {
        self.channels
    }

    /// Samples per channel (the length a view of this cache spans).
    pub fn frames(&self) -> usize {
        self.channels[0].total_samples()
    }

    pub fn base_bucket(&self) -> usize {
        self.channels[0].base_bucket()
    }

    /// Serialize to the version-3 flat byte layout (see [`crate::bytes`]) — the
    /// v2 shape with each level's mean square after its min and max.
    pub fn to_bytes(&self) -> Vec<u8> {
        Self::write(&self.channels)
    }

    /// The one writer both caches go through, so a mono cache is exactly a
    /// one-channel one and the two layouts cannot drift.
    fn write(channels: &[Pyramid]) -> Vec<u8> {
        let first = &channels[0];
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        bytes::push_u32(&mut out, VERSION_MEASURE);
        bytes::push_u64(&mut out, first.base_bucket);
        bytes::push_u64(&mut out, first.total_samples);
        bytes::push_u64(&mut out, channels.len());
        bytes::push_u64(&mut out, first.levels.len());
        for ch in channels {
            for lvl in &ch.levels {
                bytes::push_u64(&mut out, lvl.bucket);
                bytes::push_u64(&mut out, lvl.min.len());
                bytes::push_f32s(&mut out, &lvl.min);
                bytes::push_f32s(&mut out, &lvl.max);
                // A level with no measure is one parsed from an older cache;
                // re-serializing it writes zeros in a v3 slot, which is why
                // `has_mean_square` rides with the data rather than being
                // inferred from the version at every read.
                let zeros;
                let ms = match &lvl.ms {
                    Some(ms) => ms,
                    None => {
                        zeros = vec![0.0f32; lvl.min.len()];
                        &zeros
                    }
                };
                bytes::push_f32s(&mut out, ms);
            }
        }
        out
    }

    /// Parse a version-3 buffer, a version-2 one (no mean square), or a
    /// version-1 (mono) one as a single channel. `None` if malformed.
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        let mut r = bytes::Reader::new(data);
        r.tag(MAGIC)?;
        let version = r.u32()?;
        if version == VERSION {
            return Pyramid::from_bytes(data).map(|p| Self { channels: vec![p] });
        }
        if version != VERSION_MULTI && version != VERSION_MEASURE {
            return None;
        }
        let measured = version == VERSION_MEASURE;
        let base_bucket = r.usize()?;
        let total_samples = r.usize()?;
        let n_channels = r.usize()?.max(1);
        let n_levels = r.usize()?;
        let mut channels = Vec::with_capacity(n_channels);
        for _ in 0..n_channels {
            let mut levels = Vec::with_capacity(n_levels);
            for _ in 0..n_levels {
                let bucket = r.usize()?;
                let len = r.usize()?;
                let min = r.f32_vec(len)?;
                let max = r.f32_vec(len)?;
                let ms = if measured {
                    Some(r.f32_vec(len)?)
                } else {
                    None
                };
                levels.push(Level {
                    bucket,
                    min,
                    max,
                    ms,
                });
            }
            channels.push(Pyramid {
                base_bucket,
                total_samples,
                levels,
            });
        }
        Some(Self { channels })
    }

    /// Write the cache to `path` (one file for all channels).
    pub fn write_cache(&self, path: impl AsRef<Path>) -> io::Result<()> {
        fs::write(path, self.to_bytes())
    }

    /// Read a cache from `path`. `Ok(None)` if malformed, so the caller can
    /// recompute.
    pub fn read_cache(path: impl AsRef<Path>) -> io::Result<Option<Self>> {
        Ok(Self::from_bytes(&fs::read(path)?))
    }
}

/// The exact [`MultiPyramid::to_bytes`] length for `frames` samples per channel
/// across `channels` channels at `base_bucket`, computed without building —
/// the multichannel sibling of [`cache_size`], pinned to `to_bytes` by the
/// `multi_cache_size_matches_to_bytes_len` test.
pub fn multi_cache_size(frames: usize, channels: usize, base_bucket: usize) -> usize {
    assert!(base_bucket >= 1);
    let channels = channels.max(1);
    // Header: MAGIC(4) + VERSION(4) + base_bucket(8) + frames(8) + channels(8)
    // + n_levels(8).
    let mut size = 4 + 4 + 8 + 8 + 8 + 8;
    let mut level_len = frames.div_ceil(base_bucket);
    loop {
        // Per level per channel: bucket(8) + len(8) + min/max/ms (4*len each).
        size += channels * (8 + 8 + 12 * level_len);
        if level_len <= 1 {
            break;
        }
        level_len = level_len.div_ceil(2);
    }
    size
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(n: usize) -> Vec<f32> {
        (0..n).map(|i| (i as f32 * 0.01).sin()).collect()
    }

    #[test]
    fn level0_matches_bruteforce() {
        let s = ramp(1000);
        let p = Pyramid::build(&s, 4);
        for (b, chunk) in s.chunks(4).enumerate() {
            let (lo, hi) = min_max(chunk).unwrap();
            let (clo, chi) = p.column(0, (b * 4) as f64, (b * 4 + 4) as f64).unwrap();
            assert_eq!((lo, hi), (clo, chi));
        }
    }

    #[test]
    fn top_level_is_global_min_max() {
        let s = ramp(1000);
        let p = Pyramid::build(&s, 4);
        let top = p.num_levels() - 1;
        let (lo, hi) = p.column(top, 0.0, s.len() as f64).unwrap();
        let (glo, ghi) = min_max(&s).unwrap();
        assert!((lo - glo).abs() < 1e-6 && (hi - ghi).abs() < 1e-6);
    }

    #[test]
    fn level_for_tracks_resolution() {
        let s = ramp(100_000);
        let p = Pyramid::build(&s, 256);
        // Zoomed in (few samples/px) -> finest level.
        assert_eq!(p.level_for(10.0), 0);
        // Zoomed out -> a coarser level whose bucket fits in samples_per_px.
        let coarse = p.level_for(5000.0);
        assert!(coarse > 0);
    }

    #[test]
    fn cache_round_trip() {
        let s = ramp(5000);
        let p = Pyramid::build(&s, 64);
        let raw = p.to_bytes();
        let q = Pyramid::from_bytes(&raw).expect("parse");
        assert_eq!(p.base_bucket(), q.base_bucket());
        assert_eq!(p.total_samples(), q.total_samples());
        assert_eq!(p.num_levels(), q.num_levels());
        let top = p.num_levels() - 1;
        assert_eq!(
            p.column(top, 0.0, s.len() as f64),
            q.column(top, 0.0, s.len() as f64)
        );
    }

    #[test]
    fn cache_file_round_trip() {
        let s = ramp(3000);
        let p = Pyramid::build(&s, 32);
        let path = std::env::temp_dir().join(format!("clausters_peaks_{}.bin", std::process::id()));
        p.write_cache(&path).expect("write");
        let q = Pyramid::read_cache(&path).expect("read").expect("parse");
        let _ = std::fs::remove_file(&path);
        let top = p.num_levels() - 1;
        assert_eq!(
            p.column(top, 0.0, s.len() as f64),
            q.column(top, 0.0, s.len() as f64)
        );
    }

    #[test]
    fn from_bytes_rejects_garbage() {
        assert!(Pyramid::from_bytes(b"not a pyramid").is_none());
        assert!(Pyramid::from_bytes(&[]).is_none());
    }

    #[test]
    fn multi_build_matches_per_channel_builds() {
        // Interleaved stereo whose channels are a ramp and its negation: each
        // channel's pyramid must equal the one built from the channel alone.
        let frames = 3000;
        let (l, r): (Vec<f32>, Vec<f32>) =
            (ramp(frames), ramp(frames).iter().map(|x| -x).collect());
        let inter: Vec<f32> = l.iter().zip(&r).flat_map(|(&a, &b)| [a, b]).collect();
        let multi = MultiPyramid::build_interleaved(&inter, 2, 64);
        assert_eq!(multi.num_channels(), 2);
        assert_eq!(multi.frames(), frames);
        for (ch, mono) in [(0, &l), (1, &r)] {
            let alone = Pyramid::build(mono, 64);
            let got = multi.channel(ch).unwrap();
            for level in 0..alone.num_levels() {
                assert_eq!(
                    got.column(level, 0.0, frames as f64),
                    alone.column(level, 0.0, frames as f64),
                    "channel {ch} level {level}"
                );
            }
        }
    }

    #[test]
    fn multi_cache_round_trip_and_v1_compatibility() {
        let inter: Vec<f32> = ramp(4000);
        let multi = MultiPyramid::build_interleaved(&inter, 2, 32);
        let back = MultiPyramid::from_bytes(&multi.to_bytes()).expect("parse v2");
        assert_eq!(back.num_channels(), 2);
        assert_eq!(back.frames(), multi.frames());
        assert_eq!(back.base_bucket(), 32);
        let top = back.channel(1).unwrap().num_levels() - 1;
        assert_eq!(
            back.channel(1).unwrap().column(top, 0.0, 2000.0),
            multi.channel(1).unwrap().column(top, 0.0, 2000.0)
        );
        // A v1 (mono) cache parses as one channel.
        let mono = Pyramid::build(&ramp(1000), 16);
        let as_multi = MultiPyramid::from_bytes(&mono.to_bytes()).expect("parse v1");
        assert_eq!(as_multi.num_channels(), 1);
        assert_eq!(as_multi.frames(), 1000);
        // And garbage is still rejected.
        assert!(MultiPyramid::from_bytes(b"junk").is_none());
    }

    #[test]
    fn multi_cache_size_matches_to_bytes_len() {
        for &(frames, channels, base) in &[(0, 1, 256), (1000, 2, 64), (5000, 4, 256), (77, 3, 8)] {
            let inter = ramp(frames * channels);
            let built = MultiPyramid::build_interleaved(&inter, channels, base)
                .to_bytes()
                .len();
            assert_eq!(
                multi_cache_size(frames, channels, base),
                built,
                "frames={frames} channels={channels} base={base}"
            );
        }
    }

    #[test]
    fn mean_square_matches_bruteforce_at_every_level() {
        // A signal whose energy varies along the buffer, and a length that is
        // *not* a multiple of the bucket, so the ragged tail is exercised too.
        let n: usize = 4321;
        let s: Vec<f32> = (0..n)
            .map(|i| (i as f32 * 0.03).sin() * (0.1 + i as f32 / n as f32))
            .collect();
        let base = 16;
        let p = Pyramid::build(&s, base);
        assert!(p.has_mean_square());
        for level in 0..p.num_levels() {
            let bucket = p.level_bucket(level).unwrap();
            for b in 0..n.div_ceil(bucket) {
                let lo = b * bucket;
                let hi = (lo + bucket).min(n);
                let want = mean_square(&s[lo..hi]).unwrap();
                let got = p.column_ms(level, lo as f64, hi as f64).unwrap();
                assert!(
                    (got - want).abs() <= 1e-6 * want.max(1e-6),
                    "level {level} bucket {b}: {got} != {want}"
                );
            }
        }
        // And the whole buffer at the top level equals the direct mean square:
        // combining levels is exact, not an approximation that accumulates.
        let top = p.num_levels() - 1;
        let want = mean_square(&s).unwrap();
        let got = p.column_ms(top, 0.0, n as f64).unwrap();
        assert!((got - want).abs() <= 1e-6 * want, "top: {got} != {want}");
    }

    #[test]
    fn mean_square_survives_the_cache() {
        let s = ramp(2000);
        let p = Pyramid::build(&s, 32);
        let q = Pyramid::from_bytes(&p.to_bytes()).expect("parse v3");
        assert!(q.has_mean_square());
        for level in 0..p.num_levels() {
            assert_eq!(
                p.column_ms(level, 0.0, s.len() as f64),
                q.column_ms(level, 0.0, s.len() as f64)
            );
        }
        // The multichannel cache carries it per channel.
        let inter: Vec<f32> = s.iter().flat_map(|&x| [x, -x * 0.5]).collect();
        let m = MultiPyramid::build_interleaved(&inter, 2, 32);
        let back = MultiPyramid::from_bytes(&m.to_bytes()).expect("parse v3 multi");
        for ch in 0..2 {
            assert_eq!(
                back.channel(ch).unwrap().column_ms(0, 0.0, 2000.0),
                m.channel(ch).unwrap().column_ms(0, 0.0, 2000.0)
            );
        }
    }

    /// The v1 and v2 layouts, written by hand — the caches an older build left
    /// on disk. Rewriting the old writers here (rather than keeping them in the
    /// module) is what keeps "v1/v2 still load" a claim about the *reader*.
    fn legacy_bytes(p: &Pyramid, channels: usize) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        if channels == 1 {
            bytes::push_u32(&mut out, VERSION);
            bytes::push_u64(&mut out, p.base_bucket);
            bytes::push_u64(&mut out, p.total_samples);
        } else {
            bytes::push_u32(&mut out, VERSION_MULTI);
            bytes::push_u64(&mut out, p.base_bucket);
            bytes::push_u64(&mut out, p.total_samples);
            bytes::push_u64(&mut out, channels);
        }
        bytes::push_u64(&mut out, p.levels.len());
        for _ in 0..channels {
            for lvl in &p.levels {
                bytes::push_u64(&mut out, lvl.bucket);
                bytes::push_u64(&mut out, lvl.min.len());
                bytes::push_f32s(&mut out, &lvl.min);
                bytes::push_f32s(&mut out, &lvl.max);
            }
        }
        out
    }

    #[test]
    fn v1_and_v2_caches_still_load_and_say_they_have_no_measure() {
        let s = ramp(1500);
        let p = Pyramid::build(&s, 64);
        let top = p.num_levels() - 1;

        let v1 = Pyramid::from_bytes(&legacy_bytes(&p, 1)).expect("parse v1");
        assert_eq!(v1.total_samples(), 1500);
        assert_eq!(v1.num_levels(), p.num_levels());
        assert_eq!(
            v1.column(top, 0.0, 1500.0),
            p.column(top, 0.0, 1500.0),
            "the peaks are unchanged by the version"
        );
        // The measure is *absent*, not zero: a view asks and draws no layer.
        assert!(!v1.has_mean_square());
        assert_eq!(v1.column_ms(0, 0.0, 1500.0), None);

        let v2 = MultiPyramid::from_bytes(&legacy_bytes(&p, 2)).expect("parse v2");
        assert_eq!(v2.num_channels(), 2);
        assert!(!v2.channel(0).unwrap().has_mean_square());
        assert_eq!(v2.channel(1).unwrap().column_ms(0, 0.0, 1500.0), None);

        // A v1 mono cache read through the multichannel door is one channel.
        let as_multi = MultiPyramid::from_bytes(&legacy_bytes(&p, 1)).expect("parse v1 as multi");
        assert_eq!(as_multi.num_channels(), 1);
        assert!(!as_multi.channel(0).unwrap().has_mean_square());
    }

    #[test]
    fn a_multichannel_cache_is_not_narrowed_to_its_first_channel() {
        // The mono reader takes a one-channel v3 cache and refuses a wider one,
        // rather than silently dropping the channels it cannot return.
        let inter: Vec<f32> = ramp(800);
        let stereo = MultiPyramid::build_interleaved(&inter, 2, 16);
        assert!(Pyramid::from_bytes(&stereo.to_bytes()).is_none());
        let mono = MultiPyramid::build_interleaved(&inter, 1, 16);
        let one = Pyramid::from_bytes(&mono.to_bytes()).expect("one channel parses");
        assert_eq!(one.total_samples(), 800);
    }

    #[test]
    fn cache_size_matches_to_bytes_len() {
        // The size predicted without building must equal the built cache length,
        // across small/large and exact/ragged bucket counts (and the empty case).
        for &(n, base) in &[(0, 256), (1, 256), (1000, 4), (5000, 64), (100_000, 256)] {
            let built = Pyramid::build(&ramp(n), base).to_bytes().len();
            assert_eq!(cache_size(n, base), built, "n={n} base={base}");
        }
    }
}

#[cfg(test)]
mod source_tests {
    use super::*;

    /// A source that owns nothing the caller can see — the shape a mapped
    /// region has, so the test proves the door works for something that is not
    /// a slice in disguise.
    struct Generated {
        len: usize,
    }

    impl Source for Generated {
        fn len(&self) -> usize {
            self.len
        }

        fn read_into(&self, start: usize, out: &mut [f32]) {
            for (i, slot) in out.iter_mut().enumerate() {
                let f = start + i;
                *slot = if f < self.len { value(f) } else { 0.0 };
            }
        }
    }

    fn value(i: usize) -> f32 {
        (i as f32 * 0.013).sin() * 0.8 + (i % 11) as f32 * 0.02
    }

    fn materialized(n: usize) -> Vec<f32> {
        (0..n).map(value).collect()
    }

    #[test]
    fn a_pyramid_over_a_source_equals_one_over_the_samples() {
        let n = 9_000;
        let from_source = Pyramid::build_from(&Generated { len: n }, 256);
        let from_slice = Pyramid::build(&materialized(n), 256);
        assert_eq!(from_source.num_levels(), from_slice.num_levels());
        for level in 0..from_slice.num_levels() {
            for b in 0..40 {
                let s0 = (b * 137) as f64;
                let s1 = s0 + 500.0;
                assert_eq!(
                    from_source.column(level, s0, s1),
                    from_slice.column(level, s0, s1),
                    "level {level}, column {b}"
                );
                assert_eq!(
                    from_source.column_ms(level, s0, s1),
                    from_slice.column_ms(level, s0, s1),
                    "level {level}, column {b} (measure)"
                );
            }
        }
    }

    #[test]
    fn a_span_updated_through_a_source_equals_a_rebuild() {
        let n = 9_000;
        let mut samples = materialized(n);
        let mut pyramid = Pyramid::build(&samples, 256);
        // The samples moves where it lies, as a store into a mapped region
        // does, and only the summary is told about it.
        for s in samples.iter_mut().skip(3_000).take(1_500) {
            *s = -0.75;
        }
        assert!(pyramid.update_range_from(&samples.as_slice(), 3_000, 1_500));
        let rebuilt = Pyramid::build(&samples, 256);
        for level in 0..rebuilt.num_levels() {
            for b in 0..40 {
                let s0 = (b * 211) as f64;
                let s1 = s0 + 400.0;
                assert_eq!(
                    pyramid.column(level, s0, s1),
                    rebuilt.column(level, s0, s1),
                    "level {level}, column {b}"
                );
            }
        }
    }

    #[test]
    fn a_source_of_another_length_is_refused() {
        let mut pyramid = Pyramid::build(&materialized(4_000), 256);
        assert!(!pyramid.update_range_from(&Generated { len: 4_001 }, 0, 10));
    }

    #[test]
    fn an_interleaved_channel_reads_its_own_samples() {
        let frames = 1_000;
        let interleaved: Vec<f32> = (0..frames).flat_map(|f| [value(f), -value(f)]).collect();
        let right = Interleaved::new(&interleaved, 2, 1);
        assert_eq!(Source::len(&right), frames);
        let mut out = [0.0f32; 4];
        right.read_into(10, &mut out);
        assert_eq!(out, [-value(10), -value(11), -value(12), -value(13)]);
        // Past the end is silence rather than a panic: the pyramid never asks
        // for one, and a draw path is the wrong place to learn that.
        right.read_into(frames - 2, &mut out);
        assert_eq!(out, [-value(frames - 2), -value(frames - 1), 0.0, 0.0]);
    }
}

#[cfg(test)]
mod update_tests {
    use super::*;

    fn ramp(n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| ((i as f32 * 0.017).sin() * 0.9) + (i % 7) as f32 * 0.01)
            .collect()
    }

    fn same(a: &Pyramid, b: &Pyramid) -> Result<(), String> {
        if a.levels.len() != b.levels.len() {
            return Err(format!(
                "{} levels against {}",
                a.levels.len(),
                b.levels.len()
            ));
        }
        for (l, (x, y)) in a.levels.iter().zip(&b.levels).enumerate() {
            if x.min != y.min || x.max != y.max {
                return Err(format!("level {l}: min/max differ"));
            }
            match (&x.ms, &y.ms) {
                (Some(p), Some(q)) if p == q => {}
                (None, None) => {}
                _ => return Err(format!("level {l}: the measure differs")),
            }
        }
        Ok(())
    }

    /// The claim the whole function rests on: updating a span leaves exactly the
    /// pyramid a rebuild would, at **every** level, so a session of edits cannot
    /// drift away from the samples.
    #[test]
    fn an_updated_pyramid_equals_a_rebuilt_one() {
        // Deliberately not a multiple of the bucket, so the ragged tail is in.
        let n = 4000;
        let base = 64;
        for (start, len) in [
            (0, 1),     // the first sample
            (1999, 1),  // one in the middle
            (n - 1, 1), // the last, in the ragged bucket
            (100, 500), // a span inside one bucket's reach and beyond
            (0, n),     // everything
            (3990, 10), // the tail exactly
            (63, 2),    // straddling a bucket boundary
        ] {
            let mut samples = ramp(n);
            let mut pyr = Pyramid::build(&samples, base);
            for s in &mut samples[start..start + len] {
                *s = -*s * 0.5 + 0.3;
            }
            assert!(pyr.update_range(&samples, start, len), "({start}, {len})");
            let fresh = Pyramid::build(&samples, base);
            same(&pyr, &fresh).unwrap_or_else(|e| panic!("({start}, {len}): {e}"));
        }
    }

    /// Several edits in a row, each updated, still equal one rebuild — the
    /// actual editing session, where drift would accumulate if it existed.
    #[test]
    fn edits_compose_without_drifting() {
        let n = 3000;
        let base = 32;
        let mut samples = ramp(n);
        let mut pyr = Pyramid::build(&samples, base);
        for (i, (start, len)) in [(10, 3), (1000, 200), (2999, 1), (500, 700)]
            .into_iter()
            .enumerate()
        {
            for s in &mut samples[start..start + len] {
                *s = (*s + i as f32 * 0.1).clamp(-1.0, 1.0);
            }
            assert!(pyr.update_range(&samples, start, len));
        }
        same(&pyr, &Pyramid::build(&samples, base)).unwrap();
    }

    #[test]
    fn a_buffer_of_another_length_is_refused_and_nothing_moves() {
        let samples = ramp(1000);
        let mut pyr = Pyramid::build(&samples, 64);
        let before = Pyramid::build(&samples, 64);
        assert!(
            !pyr.update_range(&samples[..999], 0, 10),
            "a shorter buffer"
        );
        assert!(!pyr.update_range(&ramp(1001), 0, 10), "a longer one");
        same(&pyr, &before).unwrap();
    }

    #[test]
    fn an_empty_span_is_a_no_op() {
        let samples = ramp(500);
        let mut pyr = Pyramid::build(&samples, 64);
        let before = Pyramid::build(&samples, 64);
        assert!(pyr.update_range(&samples, 100, 0));
        assert!(
            pyr.update_range(&samples, 500, 10),
            "a span at the very end"
        );
        same(&pyr, &before).unwrap();
    }

    /// A cache written before the mean square joined keeps min/max current and
    /// stays without a measure: an invented one would read as silence measured,
    /// which is the distinction A1 drew and this must not undo.
    #[test]
    fn a_pyramid_without_a_measure_does_not_gain_one() {
        let mut samples = ramp(1000);
        let mut pyr = Pyramid::build(&samples, 64);
        for level in &mut pyr.levels {
            level.ms = None;
        }
        assert!(!pyr.has_mean_square());
        samples[500] = 0.99;
        assert!(pyr.update_range(&samples, 500, 1));
        assert!(!pyr.has_mean_square(), "still no measure");
        let fresh = Pyramid::build(&samples, 64);
        for (l, (x, y)) in pyr.levels.iter().zip(&fresh.levels).enumerate() {
            assert_eq!(x.min, y.min, "level {l} min");
            assert_eq!(x.max, y.max, "level {l} max");
        }
    }
}

#[cfg(test)]
mod multi_update_tests {
    use super::*;

    fn interleaved(frames: usize, channels: usize) -> Vec<f32> {
        (0..frames * channels)
            .map(|i| {
                let f = (i / channels) as f32;
                let c = (i % channels) as f32;
                ((f * 0.013 + c).sin() * 0.8) + c * 0.05
            })
            .collect()
    }

    /// The multichannel claim, and the one a take actually leans on: updating a
    /// frame span leaves exactly the cache a rebuild would, per channel.
    #[test]
    fn an_updated_cache_equals_a_rebuilt_one() {
        let frames = 2500; // not a multiple of the bucket
        let base = 64;
        for channels in [1usize, 2, 3] {
            for (start, len) in [(0, 1), (777, 40), (frames - 1, 1), (0, frames)] {
                let mut data = interleaved(frames, channels);
                let mut pyr = MultiPyramid::build_interleaved(&data, channels, base);
                for f in start..start + len {
                    for c in 0..channels {
                        data[f * channels + c] *= -0.25;
                    }
                }
                assert!(
                    pyr.update_range(&data, start, len),
                    "{channels} ch, ({start}, {len})"
                );
                let fresh = MultiPyramid::build_interleaved(&data, channels, base);
                for ch in 0..channels {
                    let a = pyr.channel(ch).unwrap();
                    let b = fresh.channel(ch).unwrap();
                    for (l, (x, y)) in a.levels.iter().zip(&b.levels).enumerate() {
                        assert_eq!(x.min, y.min, "{channels} ch, channel {ch}, level {l} min");
                        assert_eq!(x.max, y.max, "{channels} ch, channel {ch}, level {l} max");
                        assert_eq!(x.ms, y.ms, "{channels} ch, channel {ch}, level {l} measure");
                    }
                }
            }
        }
    }

    /// An edit to one channel leaves the others untouched — which is what
    /// reading with a stride has to get right and de-interleaving would hide.
    #[test]
    fn editing_one_channel_moves_only_that_one() {
        let (frames, channels, base) = (1000, 2, 32);
        let mut data = interleaved(frames, channels);
        let mut pyr = MultiPyramid::build_interleaved(&data, channels, base);
        let untouched: Vec<f32> = pyr.channel(1).unwrap().levels[0].max.clone();
        for f in 100..140 {
            data[f * channels] = 0.99; // the left channel only
        }
        assert!(pyr.update_range(&data, 100, 40));
        assert_eq!(
            pyr.channel(1).unwrap().levels[0].max,
            untouched,
            "the right channel is not the one that changed"
        );
        assert!(
            pyr.channel(0).unwrap().levels[0]
                .max
                .iter()
                .any(|&m| m > 0.98),
            "and the left one is"
        );
    }

    #[test]
    fn a_buffer_of_another_shape_is_refused() {
        let (frames, channels, base) = (500, 2, 32);
        let data = interleaved(frames, channels);
        let mut pyr = MultiPyramid::build_interleaved(&data, channels, base);
        assert!(!pyr.update_range(&data[..data.len() - 2], 0, 10), "short");
        assert!(
            !pyr.update_range(&interleaved(frames, 3), 0, 10),
            "wrong width"
        );
    }
}

#[cfg(test)]
mod stream_tests {
    use super::*;

    fn interleaved(frames: usize, channels: usize) -> Vec<f32> {
        (0..frames * channels)
            .map(|i| {
                let f = (i / channels) as f32;
                let c = (i % channels) as f32;
                ((f * 0.017 + c * 1.7).sin() * 0.9) - c * 0.03
            })
            .collect()
    }

    /// The report a server sends for `[start, start + n * bucket)`: whole
    /// buckets only, bucket-major and channel-minor, exactly the layout of
    /// `/buffer_stream.reply`'s blob. Measured here from the samples the way
    /// the writer measures them, which is the point of the test below — the
    /// receiver never sees these samples.
    fn report(data: &[f32], channels: usize, bucket: usize, start: usize, n: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(n * channels * 3);
        for b in 0..n {
            for ch in 0..channels {
                let from = start + b * bucket;
                let chunk: Vec<f32> = (from..from + bucket)
                    .map(|f| data[f * channels + ch])
                    .collect();
                let (lo, hi) = min_max(&chunk).unwrap();
                out.push(lo);
                out.push(hi);
                out.push(mean_square(&chunk).unwrap());
            }
        }
        out
    }

    /// The claim the door exists for: a picture filled **only** from reports —
    /// no samples on this side at all — is the picture the samples would have
    /// built. That is what makes a page's recording view agree with the host's
    /// rather than merely look similar.
    #[test]
    fn a_streamed_pyramid_equals_one_built_from_the_samples() {
        let base = 32;
        for channels in [1usize, 2, 3] {
            let frames = base * 40; // whole buckets: what a stream reports
            let data = interleaved(frames, channels);
            // The receiver's side: an allocated buffer, still silent.
            let mut pyr =
                MultiPyramid::build_interleaved(&vec![0.0f32; frames * channels], channels, base);
            // The reports, as they arrive: uneven runs, in order.
            let mut at = 0;
            for n in [1usize, 7, 12, 20] {
                assert!(
                    pyr.write_buckets(
                        at * base,
                        base,
                        &report(&data, channels, base, at * base, n)
                    ),
                    "{channels} ch, {n} buckets at {at}"
                );
                at += n;
            }
            assert_eq!(at, frames / base, "the whole take was reported");

            let fresh = MultiPyramid::build_interleaved(&data, channels, base);
            for ch in 0..channels {
                let a = pyr.channel(ch).unwrap();
                let b = fresh.channel(ch).unwrap();
                for (l, (x, y)) in a.levels.iter().zip(&b.levels).enumerate() {
                    assert_eq!(x.min, y.min, "{channels} ch, channel {ch}, level {l} min");
                    assert_eq!(x.max, y.max, "{channels} ch, channel {ch}, level {l} max");
                    assert_eq!(x.ms, y.ms, "{channels} ch, channel {ch}, level {l} measure");
                }
            }
        }
    }

    /// A recording is drawn while it is short of its buffer, so the levels
    /// above a partial report have to be right at every step — not only once
    /// the take is complete.
    #[test]
    fn every_level_is_true_while_the_take_is_still_filling() {
        let (base, channels) = (16, 2);
        let frames = base * 25;
        let data = interleaved(frames, channels);
        let mut pyr =
            MultiPyramid::build_interleaved(&vec![0.0f32; frames * channels], channels, base);
        for n in 1..=frames / base {
            assert!(pyr.write_buckets(0, base, &report(&data, channels, base, 0, n)));
            // What has arrived is summarized as the samples would be; what has
            // not is still the silence the buffer was allocated as.
            let mut so_far = vec![0.0f32; frames * channels];
            so_far[..n * base * channels].copy_from_slice(&data[..n * base * channels]);
            let fresh = MultiPyramid::build_interleaved(&so_far, channels, base);
            for ch in 0..channels {
                let a = pyr.channel(ch).unwrap();
                let b = fresh.channel(ch).unwrap();
                for (l, (x, y)) in a.levels.iter().zip(&b.levels).enumerate() {
                    assert_eq!(x.min, y.min, "{n} buckets, channel {ch}, level {l} min");
                    assert_eq!(x.max, y.max, "{n} buckets, channel {ch}, level {l} max");
                    assert_eq!(x.ms, y.ms, "{n} buckets, channel {ch}, level {l} measure");
                }
            }
        }
    }

    /// The refusals, and the one thing they all share: nothing is written.
    #[test]
    fn a_report_on_another_grid_is_refused_whole() {
        let (base, channels) = (32, 2);
        let frames = base * 10;
        let data = interleaved(frames, channels);
        let mut pyr =
            MultiPyramid::build_interleaved(&vec![0.0f32; frames * channels], channels, base);
        let before = pyr.channel(0).unwrap().levels[0].max.clone();
        let one = report(&data, channels, base, 0, 1);

        assert!(!pyr.write_buckets(0, base * 2, &one), "a coarser bucket");
        assert!(!pyr.write_buckets(0, base / 2, &one), "a finer one");
        assert!(
            !pyr.write_buckets(base / 2, base, &one),
            "an unaligned start"
        );
        assert!(!pyr.write_buckets(frames, base, &one), "past the end");
        assert!(
            !pyr.write_buckets(0, base, &one[..one.len() - 1]),
            "a ragged run"
        );
        assert_eq!(
            pyr.channel(0).unwrap().levels[0].max,
            before,
            "a refused report changes nothing"
        );

        // And the run that reaches exactly the last bucket is not past the end.
        assert!(pyr.write_buckets(
            frames - base,
            base,
            &report(&data, channels, base, frames - base, 1)
        ));
    }

    /// The tail bucket a stream never reports stays as it was, rather than
    /// being invented from the buckets around it.
    #[test]
    fn a_ragged_tail_is_left_alone() {
        let base = 64;
        let frames = base * 4 + 5; // five frames nobody will ever report
        let data = interleaved(frames, 1);
        let mut pyr = MultiPyramid::build_interleaved(&vec![0.0f32; frames], 1, base);
        assert!(pyr.write_buckets(0, base, &report(&data, 1, base, 0, 4)));
        let level0 = &pyr.channel(0).unwrap().levels[0];
        assert_eq!(level0.min.len(), 5, "four whole buckets and the remainder");
        assert_eq!((level0.min[4], level0.max[4]), (0.0, 0.0), "still silent");
    }
}

#[cfg(test)]
mod empty_tests {
    use super::*;

    /// An empty summary is the summary of silence — the same levels, the same
    /// buckets, the same answers — without the silence being anywhere.
    #[test]
    fn an_empty_pyramid_equals_one_built_over_silence() {
        for (frames, base) in [(4_096usize, 256usize), (1_000, 64), (1, 256), (0, 256)] {
            let built = Pyramid::build(&vec![0.0f32; frames], base);
            let empty = Pyramid::empty(frames, base);
            assert_eq!(empty.num_levels(), built.num_levels(), "{frames} @{base}");
            assert_eq!(empty.total_samples(), built.total_samples());
            for (l, (a, b)) in empty.levels.iter().zip(&built.levels).enumerate() {
                assert_eq!(a.bucket, b.bucket, "{frames} @{base}, level {l}");
                assert_eq!(a.min, b.min);
                assert_eq!(a.max, b.max);
                assert_eq!(a.ms, b.ms);
            }
        }
    }

    /// And it is a picture that can be filled: the reports land in it exactly
    /// as they land in a built one, which is what a take being recorded needs.
    #[test]
    fn an_empty_pyramid_takes_reports() {
        let mut multi = MultiPyramid::empty(2_048, 2, 256);
        assert_eq!(multi.frames(), 2_048);
        assert_eq!(multi.num_channels(), 2);
        let report: Vec<f32> = vec![-0.5, 0.5, 0.25, -0.25, 0.75, 0.3];
        assert!(multi.write_buckets(256, 256, &report));
        let left = multi.channel(0).unwrap();
        assert_eq!(left.column(0, 256.0, 512.0).unwrap(), (-0.5, 0.5));
        assert_eq!(left.column(0, 0.0, 256.0).unwrap(), (0.0, 0.0));
    }
}
