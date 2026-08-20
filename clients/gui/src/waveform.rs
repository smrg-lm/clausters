//! Waveform: the audio-specific **data holder** and the navigation state of a
//! view over it, built on the reusable `viewport::View` and `peaks::Pyramid`.
//!
//! What is *not* here is the drawing. A signal against time is drawn in exactly
//! one place — `host::graphics::signal::trace::draw_channel`, into the window's
//! triangle mesh — and this module is what that renderer reads: per channel, the
//! raw samples (shared, for the zoomed-in regime) plus a peak pyramid (for the
//! zoomed-out one), all sharing the time axis, so an editor-grade view draws
//! stacked lanes or overlaid traces from one [`WaveformData`].
//!
//! [`WaveformData::column`] is the one place the regimes below the screen's
//! resolution are decided, and what it measures is **the column and nothing
//! else** at every zoom: the samples while they are finer than the pyramid's
//! base bucket; the pyramid's whole buckets *folded* for everything inside a
//! wider column, which costs the logarithm of the span rather than its
//! buckets; and the samples again for the two partial edges while they are
//! worth reading. There is no level to switch and so nothing to cross-fade —
//! what the fade used to hide was a level read whole-bucket-wise, which drew a
//! transient that was outside the column.
//!
//! [`WaveformView`] adds what a *navigable* view keeps between frames and the
//! data does not: the vertical (amplitude) window, the value domain, and the
//! drag anchor. Its horizontal window lives in the widget's timeline group.

use std::sync::Arc;

use crate::host::graphics::signal::trace::{self, Trace, TraceStyle};
use crate::host::layout::Rect;
use crate::host::metrics::Metrics;
use crate::host::paint::Mesh;
use crate::host::theme::Theme;
use crate::peaks::{self, MultiPyramid, Pyramid};
use crate::view::TimelineView;
use crate::viewport::{Axis, Unit, View};

/// Where a channel's raw samples are — **owned here, or read where they live**.
///
/// The distinction is the whole of H7: a take the host has mapped is the
/// server's own memory, and a picture of it has no business holding a second
/// copy. A page has no mapping and keeps the owned form, which is also what a
/// fetched buffer, an inline blob and a test all produce.
#[derive(Clone)]
pub enum Samples {
    /// A buffer this view owns. Empty for a cache-only view, which has an
    /// overview and no samples.
    Owned(Arc<[f32]>),
    /// The samples where it lies: one channel of a mapped region, read
    /// through [`peaks::Source`] so this module never learns what a mapping
    /// is. Reading it may cross a writer, which is the buffer model's own
    /// promise and exactly what a picture can live with.
    Shared(Arc<dyn peaks::Source + Send + Sync>),
    /// **A run of the channel, around what is being looked at.** `total` is
    /// the samples' whole length; `data` covers `start .. start + data.len()`
    /// of it and nothing else.
    ///
    /// It is what a picture that cannot map the samples holds when a zoom
    /// goes past its summary: the span on screen is fetched and answers
    /// exactly, everything outside it is the pyramid's as before. So the same
    /// view is sample-exact where the eye is and an overview everywhere else,
    /// which is what a mapping gives for free — the difference is the route
    /// and not the picture.
    Window {
        start: usize,
        data: Arc<[f32]>,
        total: usize,
    },
}

impl Samples {
    /// How many samples the channel holds — the **channel's** length, which
    /// a window states rather than measures (it holds a run of it).
    pub fn len(&self) -> usize {
        match self {
            Self::Owned(s) => s.len(),
            Self::Shared(s) => s.len(),
            Self::Window { total, .. } => *total,
        }
    }

    /// **Whether these samples can answer for `[a, b)`** — the question every
    /// regime decision comes down to, and the one a window makes interesting:
    /// an owned or shared channel answers wherever the samples reaches, a
    /// window only inside the run it holds, and an empty channel nowhere.
    pub fn covers(&self, a: usize, b: usize) -> bool {
        if b <= a {
            return false;
        }
        match self {
            Self::Owned(s) => b <= s.len(),
            Self::Shared(s) => b <= s.len(),
            Self::Window { start, data, .. } => a >= *start && b <= start + data.len(),
        }
    }

    /// Whether there are no samples — a cache-only view, which renders every
    /// regime from its pyramid.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// One sample, clamped to bounds; silence when there is none to give.
    fn at(&self, i: usize) -> f32 {
        match self {
            Self::Owned(s) => s.get(i).copied().unwrap_or(0.0),
            Self::Shared(s) => {
                if i >= s.len() {
                    return 0.0;
                }
                let mut one = [0.0f32];
                s.read_into(i, &mut one);
                one[0]
            }
            Self::Window { start, data, .. } => data
                .get(i.wrapping_sub(*start))
                .copied()
                .filter(|_| i >= *start)
                .unwrap_or(0.0),
        }
    }

    /// Min, max and mean square over `[a, b)` — the three statistics a column
    /// of the fine regime needs, measured in one pass so a shared source is
    /// read once rather than three times.
    ///
    /// A shared source is read through a **fixed stack window**: the fine
    /// regime's columns are smaller than a bucket by definition, and folding
    /// the statistics as they arrive means a column costs no allocation at any
    /// span.
    fn stats(&self, a: usize, b: usize) -> Option<(f32, f32, f32)> {
        if b <= a {
            return None;
        }
        match self {
            Self::Owned(s) => {
                let a = a.min(s.len());
                let b = b.clamp(a, s.len());
                let span = &s[a..b];
                let (lo, hi) = peaks::min_max(span)?;
                Some((lo, hi, peaks::mean_square(span).unwrap_or(0.0)))
            }
            Self::Window { start, data, .. } => {
                // Only where it covers: a caller that did not ask
                // [`Samples::covers`] first gets nothing rather than a
                // measurement of the zeros outside the run.
                let (lo, hi) = (a.checked_sub(*start)?, b.checked_sub(*start)?);
                let span = data.get(lo..hi)?;
                let (min, max) = peaks::min_max(span)?;
                Some((min, max, peaks::mean_square(span).unwrap_or(0.0)))
            }
            Self::Shared(src) => {
                let a = a.min(src.len());
                let b = b.clamp(a, src.len());
                if b <= a {
                    return None;
                }
                let mut window = [0.0f32; STAT_WINDOW];
                let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
                let mut sum = 0.0f64;
                let mut i = a;
                while i < b {
                    let n = STAT_WINDOW.min(b - i);
                    let chunk = &mut window[..n];
                    src.read_into(i, chunk);
                    for &v in chunk.iter() {
                        lo = lo.min(v);
                        hi = hi.max(v);
                        sum += (v as f64) * (v as f64);
                    }
                    i += n;
                }
                Some((lo, hi, (sum / (b - a) as f64) as f32))
            }
        }
    }
}

/// How many samples a shared source is read in at a time when a column is
/// measured. One cache line's worth of columns: big enough that the per-call
/// cost disappears, small enough to sit on the stack in a draw path.
const STAT_WINDOW: usize = 256;

impl From<Arc<[f32]>> for Samples {
    fn from(samples: Arc<[f32]>) -> Self {
        Self::Owned(samples)
    }
}

/// One channel's data: its raw samples (possibly empty, for a cache-only view)
/// plus its peak pyramid.
#[derive(Clone)]
struct Channel {
    samples: Samples,
    pyramid: Pyramid,
}

/// A waveform's data: per channel, the raw samples (shared, for the zoomed-in
/// regimes) plus a peak pyramid (for the zoomed-out regime). The pyramids are
/// the cache that can be persisted via `peaks::MultiPyramid::write_cache`.
#[derive(Clone)]
pub struct WaveformData {
    channels: Vec<Channel>,
}

/// A summary, not a dump: the data behind a view is megabytes of samples, and it
/// lives inside the widget tree (a `clip` body), which is `Debug`-printed in
/// logs and tests.
impl std::fmt::Debug for WaveformData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WaveformData")
            .field("channels", &self.num_channels())
            .field("samples", &self.total_samples())
            .field("raw", &self.has_raw())
            .finish()
    }
}

impl WaveformData {
    /// **Nothing to draw** — no channels, no summary, no allocation.
    ///
    /// It exists so a view can *give the samples back* without becoming an
    /// `Option` everywhere it is read: a slot holds this between the frame it
    /// drew and the frame it is refilled for, which is what leaves the element
    /// the sole owner of the pyramid it is writing into.
    pub fn nothing() -> Self {
        Self {
            channels: Vec::new(),
        }
    }

    /// A mono waveform from `samples`, building its pyramid at `base_bucket`.
    pub fn new(samples: Arc<[f32]>, base_bucket: usize) -> Self {
        let pyramid = Pyramid::build(&samples, base_bucket);
        Self {
            channels: vec![Channel {
                samples: samples.into(),
                pyramid,
            }],
        }
    }

    /// A multichannel view over samples the host **maps**: one
    /// [`peaks::Source`] per channel, each summarized where it lies.
    ///
    /// This is the editor's own case since H7 — the take is the server's
    /// memory, the picture reads it, and the only thing allocated here is the
    /// summary. Building it streams through a bounded window, so opening a
    /// ten-minute take costs its pyramid and a kilobyte.
    pub fn from_sources(
        sources: Vec<Arc<dyn peaks::Source + Send + Sync>>,
        base_bucket: usize,
    ) -> Self {
        assert!(!sources.is_empty());
        let channels = sources
            .into_iter()
            .map(|source| {
                let pyramid = Pyramid::build_from(&*source, base_bucket);
                Channel {
                    samples: Samples::Shared(source),
                    pyramid,
                }
            })
            .collect();
        Self { channels }
    }

    /// The same, with the summary **already built** — read out of the
    /// overview file the server keeps beside a take's region.
    ///
    /// It is the pass [`Self::from_sources`] pays that this saves, and it is
    /// the only difference between them: the samples are still mapped, so a
    /// zoom past the summary still reads the cells themselves. `None` when the
    /// summary does not describe these sources — a different channel count or
    /// length, which is a file from another take or another generation, and
    /// drawing one over the other would be a picture of the wrong audio.
    pub fn from_sources_summarized(
        sources: Vec<Arc<dyn peaks::Source + Send + Sync>>,
        summary: MultiPyramid,
    ) -> Option<Self> {
        if sources.is_empty()
            || summary.num_channels() != sources.len()
            || sources.first().map(|s| s.len()) != Some(summary.frames())
        {
            return None;
        }
        Some(Self {
            channels: sources
                .into_iter()
                .zip(summary.into_channels())
                .map(|(source, pyramid)| Channel {
                    samples: Samples::Shared(source),
                    pyramid,
                })
                .collect(),
        })
    }

    /// A multichannel waveform from `samples` holding `channels` interleaved
    /// channels (a trailing partial frame is ignored), one pyramid per channel.
    pub fn from_interleaved(samples: &[f32], channels: usize, base_bucket: usize) -> Self {
        let channels = channels.max(1);
        let frames = samples.len() / channels;
        let built = (0..channels)
            .map(|ch| {
                let one: Vec<f32> = (0..frames).map(|f| samples[f * channels + ch]).collect();
                let samples: Arc<[f32]> = one.into();
                let pyramid = Pyramid::build(&samples, base_bucket);
                Channel {
                    samples: samples.into(),
                    pyramid,
                }
            })
            .collect();
        Self { channels: built }
    }

    /// Build from samples and an already-computed pyramid (e.g. read back from a
    /// cache file with `Pyramid::read_cache`). The samples may be **empty** — a
    /// cache-only view (the bulk path where the host maps just the compact
    /// pyramid, never the raw buffer): it renders the resolution-matched overview
    /// from the pyramid, and the zoomed-in raw-sample regimes simply have nothing
    /// finer to show.
    pub fn with_pyramid(samples: Arc<[f32]>, pyramid: Pyramid) -> Self {
        Self {
            channels: vec![Channel {
                samples: samples.into(),
                pyramid,
            }],
        }
    }

    /// A multichannel view from already-split raw channels paired with their
    /// pyramids (e.g. a mapped file whose sibling cache was still valid, so
    /// the pyramids were read back instead of rebuilt). Pairs must agree in
    /// length and bucket; the bulk loader validates before calling.
    pub fn from_parts(parts: Vec<(Arc<[f32]>, Pyramid)>) -> Self {
        assert!(!parts.is_empty());
        let channels = parts
            .into_iter()
            .map(|(samples, pyramid)| Channel {
                samples: samples.into(),
                pyramid,
            })
            .collect();
        Self { channels }
    }

    /// A cache-only multichannel view from a mapped [`MultiPyramid`] (no raw
    /// samples; every regime renders from the per-channel pyramids).
    pub fn with_multi_pyramid(multi: MultiPyramid) -> Self {
        let channels = multi
            .into_channels()
            .into_iter()
            .map(|pyramid| Channel {
                samples: Samples::Owned(Arc::from([] as [f32; 0])),
                pyramid,
            })
            .collect();
        Self { channels }
    }

    /// The buffer length the view spans, in per-channel samples. Taken from the
    /// pyramid (which is built over the whole buffer), so a cache-only view with
    /// no raw `samples` still reports the right length. Zero for
    /// [`nothing`](Self::nothing), which spans nothing.
    pub fn total_samples(&self) -> usize {
        self.channels
            .first()
            .map_or(0, |c| c.pyramid.total_samples())
    }

    /// How many channels this waveform holds.
    pub fn num_channels(&self) -> usize {
        self.channels.len()
    }

    /// Channel 0's pyramid (the persistable cache of a mono view).
    ///
    /// # Panics
    /// On [`nothing`](Self::nothing), which has no channel to have one. Every
    /// caller holds samples it has just been given; the released state is the
    /// slot's own and is never asked.
    pub fn pyramid(&self) -> &Pyramid {
        &self.channels[0].pyramid
    }

    /// **Writes a run of samples into one channel and refreshes only the peaks
    /// that cover it**, returning whether it landed.
    ///
    /// This is the drawn copy of a destructive edit: the real samples are the
    /// server buffer's, and what happens here is the picture agreeing with it
    /// without being fetched again. Refreshing the whole pyramid would be a
    /// pause proportional to the *file* on every stroke over a few hundred
    /// samples, which is what [`peaks::Pyramid::update_range`] exists to avoid.
    ///
    /// A write past the end is refused rather than clamped, like every other
    /// write in this system: a stroke that ran off the end is a mistake
    /// about where the samples end, and silently shortening it would draw
    /// something nobody asked for.
    pub fn write_range(&mut self, ch: usize, start: usize, values: &[f32]) -> bool {
        let Some(channel) = self.channels.get_mut(ch) else {
            return false;
        };
        if values.is_empty() || start + values.len() > channel.samples.len() {
            return false;
        }
        match &mut channel.samples {
            // **Shared samples are already written**: the store went into the
            // cells before anything asked the picture to follow, so there is
            // no copy to make and nothing to write here — only the summary of
            // the span, read back out of the samples themselves.
            Samples::Shared(_) => self.resummarize(ch, start, values.len()),
            // An owned buffer is this widget's own, so the write lands here
            // first. It still costs the channel: a page has no other place to
            // put the samples, and that is the trade the browser leg takes.
            Samples::Owned(owned) => {
                let mut samples = owned.to_vec();
                samples[start..start + values.len()].copy_from_slice(values);
                let ok = channel.pyramid.update_range(&samples, start, values.len());
                *owned = samples.into();
                ok
            }
            // **A window holds a run and not the samples**, so a stroke over
            // it is refused rather than half-applied: what a window is for is
            // reading the span the eye is on, and an edit belongs to whoever
            // holds the samples (the mapped path, or the page's own copy).
            // Refusing keeps the picture honest instead of writing into a run
            // the next zoom will drop.
            Samples::Window { .. } => false,
        }
    }

    /// **Re-reads the summary of a span out of shared samples** — what a
    /// picture does when the samples changed underneath it and nobody handed
    /// it any: this host's own store, or a span another writer announced.
    ///
    /// Refuses for an owned buffer, which has no samples to re-read: its
    /// samples are the picture's own, so whoever changed them writes them
    /// here ([`Self::write_range`]).
    pub fn resummarize(&mut self, ch: usize, start: usize, len: usize) -> bool {
        let Some(channel) = self.channels.get_mut(ch) else {
            return false;
        };
        let Samples::Shared(source) = &channel.samples else {
            return false;
        };
        channel.pyramid.update_range_from(&**source, start, len)
    }

    /// **Folds a run of buckets somebody else measured** into the summary —
    /// what a view does when it is *told* about samples it cannot read.
    ///
    /// The mirror of [`Self::resummarize`], and it exists for the picture that
    /// has no samples to re-read: a page holds its own copy of the samples
    /// and maps nothing, so a recording growing in the server's memory reaches
    /// it as the overview of what was written (`/buffer_stream.reply`) rather
    /// than as samples. `stats` is that reply's payload — **bucket-major,
    /// channel-minor**: for each bucket of `bucket` frames in order, for each
    /// channel, `min`, `max` and mean square.
    ///
    /// Only the summary moves. The samples this view owns are whatever it was
    /// built with, so a zoom past the base bucket still shows them (silence,
    /// for a take allocated empty) — the overview is the resolution the wire
    /// carries, and pretending otherwise would mean inventing samples from
    /// their statistics.
    ///
    /// Returns whether it landed: the report has to be on this view's own grid
    /// (`bucket` its base bucket, `start_frame` on a bucket boundary, the run
    /// inside the samples), and a report that is not is refused whole rather
    /// than partly applied.
    pub fn write_buckets(&mut self, start_frame: usize, bucket: usize, stats: &[f32]) -> bool {
        let channels = self.channels.len();
        if channels == 0 || bucket == 0 || !start_frame.is_multiple_of(bucket) {
            return false;
        }
        if self
            .channels
            .iter()
            .any(|c| c.pyramid.base_bucket() != bucket)
        {
            return false;
        }
        let stride = channels * 3;
        if !stats.len().is_multiple_of(stride) {
            return false;
        }
        let first = start_frame / bucket;
        let n = stats.len() / stride;
        // Checked before anything is written: every channel of one view shares
        // the samples' length, so a run that fits one fits all — and a
        // refusal halfway would leave the channels of one picture describing
        // different samples.
        let buckets = self.total_samples().div_ceil(bucket);
        if first + n > buckets {
            return false;
        }
        let mut run: Vec<peaks::Bucket> = Vec::with_capacity(n);
        for (ch, channel) in self.channels.iter_mut().enumerate() {
            run.clear();
            run.extend((0..n).map(|b| {
                let at = b * stride + ch * 3;
                peaks::Bucket {
                    min: stats[at],
                    max: stats[at + 1],
                    ms: stats[at + 2],
                }
            }));
            if !channel.pyramid.write_buckets(first, &run) {
                return false;
            }
        }
        true
    }

    /// **Puts a fetched run of the samples under the summary**: `start` is a
    /// frame index and `samples` is interleaved, every channel of that run.
    ///
    /// It is what a picture that cannot map the samples does when a zoom goes
    /// past its overview — the span on screen is read back and answers
    /// exactly, everything outside it stays the pyramid's. One window at a
    /// time per view: it is replaced when the eye moves, because what it is
    /// for is *where the eye is* and a cache with a policy is a different
    /// design.
    ///
    /// Refuses, changing nothing, when the samples already answers for
    /// itself (mapped or wholly owned), when the run does not fit the
    /// samples, or when the shape does not match — the summary is what says
    /// how long the buffer runs, and a window that disagreed would draw two
    /// different things in one picture.
    pub fn set_window(&mut self, start: usize, channels: usize, samples: &[f32]) -> bool {
        let channels = channels.max(1);
        if self.channels.len() != channels || !samples.len().is_multiple_of(channels) {
            return false;
        }
        let frames = samples.len() / channels;
        let total = self.total_samples();
        if frames == 0 || start + frames > total {
            return false;
        }
        if self
            .channels
            .iter()
            .any(|c| !matches!(c.samples, Samples::Window { .. }) && !c.samples.is_empty())
        {
            return false; // the samples are already here, in a form that answers
        }
        for (ch, channel) in self.channels.iter_mut().enumerate() {
            let data: Arc<[f32]> = (0..frames)
                .map(|f| samples[f * channels + ch])
                .collect::<Vec<f32>>()
                .into();
            channel.samples = Samples::Window { start, data, total };
        }
        true
    }

    /// The summary's finest bucket — the zoom below which only samples can
    /// answer.
    pub fn base_bucket(&self) -> usize {
        self.channels
            .first()
            .map_or(1, |c| c.pyramid.base_bucket().max(1))
    }

    /// **Whether this view can answer for `[a, b)` out of samples** — false
    /// where it would have to draw the span out of its summary instead.
    ///
    /// The question the front asks after laying a column row out: a zoom finer
    /// than the base bucket over a span nothing covers is a picture that has
    /// stopped resolving, and is exactly when the span is worth fetching.
    pub fn covers(&self, a: usize, b: usize) -> bool {
        self.channels
            .first()
            .is_some_and(|c| c.samples.covers(a, b))
    }

    /// Whether raw samples are present. A cache-only view (`with_pyramid` with an
    /// empty buffer) has only the peak pyramid, so every regime — including the
    /// zoomed-in ones — must render from it; reading the empty raw buffer would
    /// instead collapse the wave to a flat line (it "disappears" on zoom-in).
    pub fn has_raw(&self) -> bool {
        self.channels.first().is_some_and(|c| !c.samples.is_empty())
    }

    /// Whether this view reads its samples where it lives rather than owning
    /// a copy of it — what a mapped take gives and a page never can.
    pub fn is_shared(&self) -> bool {
        matches!(
            self.channels.first().map(|c| &c.samples),
            Some(Samples::Shared(_))
        )
    }

    /// Min/max of channel `ch` for a pixel column spanning `[s0, s1)`,
    /// measured over **exactly** that span whatever the zoom.
    ///
    /// Three sources, in the order that costs least for the same answer: the
    /// samples alone while a column is finer than a bucket; the pyramid's
    /// whole buckets folded ([`Pyramid::aligned_stats`]) for everything
    /// inside; and the samples again for the two partial edges, while they are
    /// worth reading. Nothing is cross-faded, because nothing is approximated
    /// — the picture is the same function of the span at every zoom, so there
    /// is no level to switch and no step to hide.
    pub fn column(&self, ch: usize, samples_per_px: f64, s0: f64, s1: f64) -> (f32, f32) {
        self.measure(ch, samples_per_px, s0, s1)
            .map_or((0.0, 0.0), |(lo, hi, _)| (lo, hi))
    }

    /// The **mean square** of channel `ch` over a pixel column spanning
    /// `[s0, s1)`, from the same three sources [`Self::column`] takes its
    /// min/max from and in one pass with them.
    ///
    /// `None` when the source cannot answer — a cache written before the
    /// pyramid carried the statistic. That absence is the whole reason this
    /// returns an option: zeros would be a measurement (silence), and a body
    /// drawn from them would be a flat line across samples that is not flat.
    pub fn column_ms(&self, ch: usize, samples_per_px: f64, s0: f64, s1: f64) -> Option<f32> {
        self.measure(ch, samples_per_px, s0, s1)
            .and_then(|(_, _, ms)| ms)
    }

    /// Min, max and mean square of one column, measured once — the one place
    /// the regimes are decided, so the envelope and the body it holds can
    /// never disagree about which samples they are about.
    fn measure(
        &self,
        ch: usize,
        samples_per_px: f64,
        s0: f64,
        s1: f64,
    ) -> Option<(f32, f32, Option<f32>)> {
        let channel = self.channels.get(ch)?;
        let total = channel.pyramid.total_samples();
        let a = (s0.floor().max(0.0) as usize).min(total);
        let b = (s1.ceil() as usize).clamp(a, total);
        if b <= a {
            return None;
        }
        let base = channel.pyramid.base_bucket();
        // **Not "are there samples" but "do they answer here"**: a window
        // covers the run it holds and no more, and a view that owns the whole
        // samples covers all of it — one question, both forms.
        let raw = channel.samples.covers(a, b);
        // Finer than a bucket: the samples are the only thing that can answer,
        // and they answer exactly.
        if samples_per_px < base as f64 && raw {
            let (lo, hi, ms) = channel.samples.stats(a, b)?;
            return Some((lo, hi, Some(ms)));
        }
        // **Which buckets belong to this column**, and it is a *tiling*: the
        // span is rounded down at both ends, so consecutive columns cover
        // every bucket exactly once. A bucket straddling a boundary lands in
        // one of them rather than in both (which duplicates and widens — the
        // defect this replaced) or in neither (which loses a transient
        // outright). What is left is a **position error of under a bucket**,
        // one pixel at the zoom where a column *is* a bucket and shrinking
        // from there, which is the resolution the summary has.
        //
        // Reading the two partial edges out of the samples instead would make
        // it exact, and that is what this did first — measured at 700 µs per
        // frame of 900 columns against 10 µs for the fold, because a column of
        // two buckets is 500 samples of edge. Exactness that costs a read of
        // the samples at every zoom is not exactness a picture can afford; the
        // regime that *does* read every sample is the one below, where a
        // column is finer than a bucket and there is nothing else to read.
        let (qa, qb) = (a - a % base, b - b % base);
        let Some(whole) = channel.pyramid.aligned_stats(qa, qb) else {
            // No whole bucket fits: the column straddles one boundary and is
            // narrower than two buckets, so the samples are the only answer
            // and they are a short read.
            if raw && let Some((lo, hi, ms)) = channel.samples.stats(a, b) {
                return Some((lo, hi, Some(ms)));
            }
            // A cache-only view zoomed past its own resolution shows the
            // bucket it has rather than nothing, which is the finest overview
            // it can honestly draw.
            let (lo, hi) = channel.pyramid.column(0, a as f64, b as f64)?;
            let ms = channel.pyramid.column_ms(0, a as f64, b as f64);
            return Some((lo, hi, ms));
        };
        let (lo, hi) = (whole.min, whole.max);
        let (sum_sq, count) = (whole.sum_sq, whole.count);
        let ms = (whole.measured && count > 0).then(|| (sum_sq / count as f64) as f32);
        Some((lo, hi, ms))
    }

    /// Single-sample access for the line regime, clamped to bounds. A
    /// cache-only view has no sample to give and answers silence, which is why
    /// the renderer asks [`Self::has_raw`] before entering that regime.
    pub fn samples_at(&self, ch: usize, i: usize) -> f32 {
        self.channels.get(ch).map_or(0.0, |c| c.samples.at(i))
    }

    /// `frames` frames from `start`, **interleaved** — the shape a block of
    /// audio travels in everywhere else in this project, and what a copy puts
    /// on the clipboard.
    ///
    /// `None` for a **cache-only** view: a mapped pyramid has an overview and
    /// no samples, so there is nothing here that could honestly be copied, and
    /// a block of silence is the one answer worse than declining. Clamped at the
    /// end rather than refused, because a selection reaching past the last
    /// sample is an ordinary thing a sweep does.
    pub fn block(&self, start: usize, frames: usize) -> Option<Vec<f32>> {
        if !self.has_raw() {
            return None;
        }
        let channels = self.num_channels().max(1);
        let end = start.saturating_add(frames).min(self.total_samples());
        let start = start.min(end);
        let mut out = Vec::with_capacity((end - start) * channels);
        for f in start..end {
            for ch in 0..channels {
                out.push(self.samples_at(ch, f));
            }
        }
        Some(out)
    }
}

/// The vertical margin the trace leaves inside its lane: the value domain's
/// full span maps to this fraction of the lane's height. Shared with the
/// amplitude ruler and the cursor readout so a tick labeled 1.0 sits exactly on
/// the trace's full-scale line.
pub(crate) const AMP_MARGIN: f32 = 0.92;

/// The **default value domain** of a trace: full-scale amplitude. An element
/// that names no `min`/`max` is audio, and audio is bipolar about zero.
pub const DEFAULT_DOMAIN: (f32, f32) = (-1.0, 1.0);

/// The **zero line** of a value domain, or `None` when the domain does not
/// straddle it and there is no silence to draw.
///
/// It is a line and nothing more. A column is **never** extended to reach it:
/// the GPU pipeline used to clamp every column to zero and the mesh renderers
/// did not, and closing that divergence the other way — by clamping everywhere
/// — was the wrong half to keep. Filling to the baseline **inks a band the
/// signal was never in**: a column covering three samples that all sit at +0.6
/// is drawn from 0 to 0.6, which is a lie at any zoom where cycles are legible,
/// and it needs a threshold nobody can name to decide where that zoom begins.
///
/// The solid body of an overview needs no rule, because at that zoom the data
/// already fills it: a column summarizing hundreds of samples of audio crosses
/// zero by itself. So the envelope is drawn as it is measured, everywhere, and
/// what changes with the zoom is the signal — not the drawing's mind about it.
///
/// **And the zoom could not have been the criterion anyway.** A subsonic
/// signal — a 1 Hz LFO, a control curve, a long envelope — has far more samples
/// than the screen has pixels at any zoom where a whole cycle is visible, so
/// every "fill once the samples no longer fit" rule fills it; and a cycle a
/// second is a *curve*, which is exactly what a filled body destroys. What
/// separates a body from a curve is whether the signal crosses the span inside
/// one column, and the min/max already answers that — measured, per column, at
/// no cost.
pub fn baseline_of(min: f32, max: f32) -> Option<f32> {
    (min < 0.0 && max > 0.0).then_some(0.0)
}

/// Display coordinate of a value in the domain `[min, max]`: 0 at the lane
/// bottom, 1 at its top, with [`AMP_MARGIN`] of headroom left about the
/// domain's centre. The default domain reduces it to `amp * AMP_MARGIN`
/// mapped about the half-lane, which is what every view drew before a domain
/// could be named.
pub fn value_to_display(v: f32, min: f32, max: f32) -> f64 {
    let (centre, half) = domain_centre_half(min, max);
    ((v - centre) as f64 / half as f64 * AMP_MARGIN as f64) * 0.5 + 0.5
}

/// The inverse of [`value_to_display`] — what the cursor's height names.
pub fn display_to_value(d: f64, min: f32, max: f32) -> f32 {
    let (centre, half) = domain_centre_half(min, max);
    centre + ((d - 0.5) * 2.0 / AMP_MARGIN as f64) as f32 * half
}

/// How much of one lane a unit of value covers, before the vertical window is
/// applied — the resolution the cursor readout rounds to.
pub fn value_per_display(min: f32, max: f32) -> f64 {
    let (_, half) = domain_centre_half(min, max);
    2.0 * half as f64 / AMP_MARGIN as f64
}

/// A domain as its centre and half-span, with a degenerate one (`min == max`,
/// or reversed) widened so nothing divides by zero and the value simply sits
/// in the middle of its lane.
fn domain_centre_half(min: f32, max: f32) -> (f32, f32) {
    let (lo, hi) = (min.min(max), min.max(max));
    let half = ((hi - lo) * 0.5).max(f32::MIN_POSITIVE);
    ((lo + hi) * 0.5, half)
}

/// A `WaveformData` paired with **what a navigable view keeps between frames**:
/// the vertical (amplitude) display window, the value domain the trace is
/// mapped through, and the drag anchor. Nothing here is GPU state — the picture
/// is drawn into the window's mesh by
/// `host::graphics::signal::trace::draw_channel`, like every other signal.
///
/// The horizontal window is deliberately absent: it lives in the widget's
/// timeline group, because a group may span windows while a slot is per window.
pub struct WaveformView {
    /// The data, **shared** with the element that named the resource: the
    /// pyramid a loader resolved is that element's samples as much as it is
    /// this view's picture, and a read of it (a copy) is answered from there.
    data: Arc<WaveformData>,
    /// The vertical display axis: the visible slice of the value domain,
    /// normalized (`0, 1` = no zoom).
    amp: Axis,
    /// The **value domain** the trace is mapped through — the element's
    /// `min`/`max`, [`DEFAULT_DOMAIN`] when it names none.
    domain: (f32, f32),
    /// The amplitude window's start, snapshotted for absolute drag panning.
    drag_amp_start: f64,
}

impl WaveformView {
    pub fn new(data: impl Into<Arc<WaveformData>>) -> Self {
        Self {
            data: data.into(),
            amp: Axis::normalized(Unit::Norm),
            domain: DEFAULT_DOMAIN,
            drag_amp_start: 0.0,
        }
    }

    /// The samples and pyramids behind this view — what the renderer reads.
    pub fn data(&self) -> &WaveformData {
        &self.data
    }

    /// Puts a **rewritten** copy of the same samples behind the view, keeping
    /// every navigation state it holds.
    ///
    /// An edit replaces the pyramid rather than mutating it (the element and
    /// the view share one `Arc`, so nothing may be patched under the other's
    /// feet), and the view is not the picture's owner — it is where the eye
    /// currently is. Rebuilding it whole would snap the amplitude window back
    /// to full scale mid-stroke, which reads as the view jumping every time a
    /// sample is drawn.
    pub fn set_data(&mut self, data: impl Into<Arc<WaveformData>>) {
        self.data = data.into();
    }

    /// **Gives the samples back**, keeping every navigation state.
    ///
    /// A slot draws the element's pyramid through a shared `Arc`, and while it
    /// holds one the element cannot write into the pyramid without copying it
    /// first — a copy proportional to the whole take, paid once per step by
    /// anything following a recording. So the holder lets go before the write
    /// and is refilled before the next draw (`Element::fill`, which the repaint
    /// runs first), and in between the element is the only owner and writes in
    /// place.
    ///
    /// A view that draws before it is refilled draws nothing, which is why the
    /// two are ordered rather than merely near each other.
    pub fn release_data(&mut self) {
        self.data = Arc::new(WaveformData::nothing());
    }

    /// Sets the **value domain** the trace maps through — the element's
    /// `min`/`max`. Left alone it is [`DEFAULT_DOMAIN`], full-scale amplitude,
    /// which is what every view that names no bounds draws at.
    pub fn set_domain(&mut self, min: f32, max: f32) {
        self.domain = (min, max);
    }

    /// The domain in force, which the vertical ruler and the cursor readout
    /// must name the same values through.
    pub fn domain(&self) -> (f32, f32) {
        self.domain
    }

    /// How many channels the underlying data holds (the lane count).
    pub fn num_channels(&self) -> usize {
        self.data.num_channels()
    }

    /// The buffer length the view spans, in per-channel samples.
    pub fn total_samples(&self) -> usize {
        self.data.total_samples()
    }

    /// Sets the visible vertical display window (normalized; clamped) — the
    /// live `y_start`/`y_len` props of the editor-grade widget.
    pub fn set_amp_window(&mut self, start: f64, len: f64) {
        self.amp.set_span(start, len);
    }

    /// The visible vertical display window, as `(start, len)`.
    pub fn amp_window(&self) -> (f64, f64) {
        self.amp.span()
    }
}

/// The y **pixel** a value lands on inside `lane`, through the value `domain`
/// and the visible vertical window `amp` (`(0.0, 1.0)` = the whole axis).
///
/// Display coordinate 0 is the lane *bottom* — the convention the vertical
/// ruler reads too, so a vertical zoom moves the trace and the ticks by exactly
/// the same amount. A value outside the window lands outside the lane, and the
/// mesh's clip rectangle cuts it there.
pub fn value_to_y(v: f32, domain: (f32, f32), amp: (f64, f64), lane: Rect) -> f32 {
    let (y0, y_len) = (amp.0, amp.1.max(crate::viewport::MIN_SPAN));
    let d = value_to_display(v, domain.0, domain.1);
    lane.y + lane.h * (1.0 - ((d - y0) / y_len) as f32)
}

/// **Draws one lane of a navigable waveform** — the whole of what a `waveform`
/// element's picture is, and the same call the demo harness makes.
///
/// It is three coordinate maps handed to the one signal renderer
/// ([`trace::draw_channel`]): `view` places the horizontal window, `domain` and
/// the vertical window `amp` place the values. Nothing else distinguishes a
/// navigable view from a clip's take or a plot's series.
// The lane, the source, the channel and the two axes it is placed on: distinct
// inputs to one drawing pass, clearer flat than bundled — as in `draw_channel`,
// which this hands them to.
#[allow(clippy::too_many_arguments)]
pub fn draw_lane(
    mesh: &mut Mesh,
    lane: Rect,
    trace: &Trace,
    ch: usize,
    view: &View,
    domain: (f32, f32),
    amp: (f64, f64),
    style: TraceStyle,
) {
    let w = lane.w.max(1.0) as f64;
    trace::draw_channel(
        mesh,
        lane,
        trace,
        ch,
        |x| view.start + (x - lane.x) as f64 / w * view.len,
        |s| lane.x + ((s - view.start) / view.len * w) as f32,
        |v| value_to_y(v, domain, amp, lane),
        style,
    );
}

/// The lane one channel of `lanes` occupies inside `body`, stacked top to
/// bottom. Overlaid traces are `lanes == 1`: every channel takes the whole body.
pub fn lane_rect(body: Rect, lanes: usize, ch: usize) -> Rect {
    let lanes = lanes.max(1) as f32;
    let h = body.h / lanes;
    Rect::new(body.x, body.y + ch as f32 * h, body.w, h)
}

impl TimelineView for WaveformView {
    fn total_samples(&self) -> usize {
        self.data.total_samples()
    }

    fn mesh(&self, mesh: &mut Mesh, rect: Rect, view: &View, m: &Metrics, theme: &Theme) {
        let lanes = self.num_channels();
        let trace = Trace::Data(&self.data);
        for ch in 0..lanes {
            draw_lane(
                mesh,
                lane_rect(rect, lanes, ch),
                &trace,
                ch,
                view,
                self.domain,
                self.amp.span(),
                TraceStyle::new(theme.series(ch), m.trace_w).with_dots(m.point_radius),
            );
        }
    }

    fn on_vertical_zoom(&mut self, factor: f64, anchor: f64) -> bool {
        self.amp.zoom(factor, anchor);
        true
    }

    fn on_vertical_drag_begin(&mut self) {
        self.drag_amp_start = self.amp.start();
    }

    fn on_vertical_drag(&mut self, total: f64) -> bool {
        // Dragging down (total > 0) moves the window down with the cursor.
        // Absolute from the snapshot.
        self.amp
            .set_start(self.drag_amp_start + total * self.amp.len());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An alternating +/-0.5 signal: every base bucket has min -0.5, max +0.5.
    fn envelope_signal(n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| if i % 2 == 0 { 0.5 } else { -0.5 })
            .collect()
    }

    #[test]
    fn cache_only_view_resolves_zoom_in_from_the_pyramid() {
        // Cache-only: no raw samples, only the pyramid (the bulk `cache=` path).
        let pyramid = Pyramid::build(&envelope_signal(4096), 256);
        let data = WaveformData::with_pyramid(Arc::from([] as [f32; 0]), pyramid);
        assert!(!data.has_raw());
        // Zoomed in past the base bucket (spp < 256): the raw regime would read
        // the empty buffer and collapse to (0, 0) — the disappearing wave. The
        // fallback reads the pyramid's finest level, so the envelope survives.
        let (lo, hi) = data.column(0, 8.0, 0.0, 8.0);
        assert!(
            lo <= -0.4 && hi >= 0.4,
            "cache-only zoom-in should show the pyramid envelope, got ({lo}, {hi})"
        );
    }

    #[test]
    fn raw_view_still_uses_raw_samples_when_zoomed_in() {
        let data = WaveformData::new(Arc::from(envelope_signal(4096)), 256);
        assert!(data.has_raw());
        let (lo, hi) = data.column(0, 8.0, 0.0, 8.0);
        assert!(
            lo <= -0.4 && hi >= 0.4,
            "raw zoom-in lost the signal: ({lo}, {hi})"
        );
    }

    #[test]
    fn interleaved_channels_split_and_share_the_time_axis() {
        // Stereo: channel 0 the envelope, channel 1 silence.
        let inter: Vec<f32> = envelope_signal(2048)
            .into_iter()
            .flat_map(|s| [s, 0.0])
            .collect();
        let data = WaveformData::from_interleaved(&inter, 2, 64);
        assert_eq!(data.num_channels(), 2);
        assert_eq!(data.total_samples(), 2048, "frames, not flat samples");
        let (lo0, hi0) = data.column(0, 128.0, 0.0, 128.0);
        assert!(lo0 <= -0.4 && hi0 >= 0.4, "channel 0 keeps the envelope");
        let (lo1, hi1) = data.column(1, 128.0, 0.0, 128.0);
        assert_eq!((lo1, hi1), (0.0, 0.0), "channel 1 is silent");
        // An out-of-range channel reads zero instead of panicking.
        assert_eq!(data.column(5, 128.0, 0.0, 128.0), (0.0, 0.0));
    }

    #[test]
    fn cache_only_multichannel_view_reads_every_lane() {
        let inter: Vec<f32> = envelope_signal(2048)
            .into_iter()
            .flat_map(|s| [s, s * 0.5])
            .collect();
        let multi = MultiPyramid::build_interleaved(&inter, 2, 64);
        let data = WaveformData::with_multi_pyramid(multi);
        assert_eq!(data.num_channels(), 2);
        assert!(!data.has_raw());
        let (_, hi0) = data.column(0, 8.0, 0.0, 64.0);
        let (_, hi1) = data.column(1, 8.0, 0.0, 64.0);
        assert!(hi0 >= 0.4 && (0.2..0.4).contains(&hi1));
    }

    /// The lane the vertical-mapping tests measure against: 100 px tall, so a
    /// display coordinate reads straight off the y in percent from the bottom.
    const LANE: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 200.0,
        h: 100.0,
    };

    #[test]
    fn amp_window_maps_the_trace_through_the_visible_slice() {
        // Full axis: the classic margin map — full scale stops AMP_MARGIN of
        // the way to the top, and silence sits on the middle line.
        let top = value_to_y(1.0, DEFAULT_DOMAIN, (0.0, 1.0), LANE);
        assert!(
            (top - 100.0 * (1.0 - (1.0 + AMP_MARGIN) / 2.0)).abs() < 1e-4,
            "{top}"
        );
        assert!((value_to_y(0.0, DEFAULT_DOMAIN, (0.0, 1.0), LANE) - 50.0).abs() < 1e-4);
        // Zoomed into the top half: the zero line sits on the lane's bottom
        // edge and full scale inside the lane, above the middle.
        assert!((value_to_y(0.0, DEFAULT_DOMAIN, (0.5, 0.5), LANE) - 100.0).abs() < 1e-4);
        let full = value_to_y(1.0, DEFAULT_DOMAIN, (0.5, 0.5), LANE);
        assert!((0.0..50.0).contains(&full), "{full}");
        // A value below the window leaves the lane (the clip rect cuts it).
        assert!(value_to_y(-1.0, DEFAULT_DOMAIN, (0.5, 0.5), LANE) > 100.0);
    }

    /// A named domain is the *same* map over another range: its ends land where
    /// full scale lands on the amplitude axis, so the margin is a property of
    /// the lane and not of what the signal happens to measure.
    #[test]
    fn a_named_domain_maps_its_ends_where_full_scale_maps() {
        for (min, max) in [(0.0f32, 1.0f32), (-0.25, 0.75), (20.0, 20_000.0)] {
            for (v, amp) in [(min, -1.0f32), (max, 1.0)] {
                let named = value_to_display(v, min, max);
                let default = value_to_display(amp, DEFAULT_DOMAIN.0, DEFAULT_DOMAIN.1);
                assert!(
                    (named - default).abs() < 1e-9,
                    "[{min}, {max}] end {v} at {named}, full scale at {default}"
                );
            }
            // ...and the inverse names it back, which is what the readout does.
            let mid = (min + max) * 0.5;
            let back = display_to_value(value_to_display(mid, min, max), min, max);
            assert!(
                (back - mid).abs() <= (max - min).abs() * 1e-6,
                "{back} {mid}"
            );
        }
    }

    /// A degenerate domain divides by nothing and parks the value mid-lane,
    /// rather than producing a NaN the vertex buffer would carry to the GPU.
    #[test]
    fn a_degenerate_domain_is_finite() {
        let d = value_to_display(3.0, 3.0, 3.0);
        assert!(d.is_finite(), "{d}");
        assert!(value_to_y(3.0, (3.0, 3.0), (0.0, 1.0), LANE).is_finite());
    }

    /// The fill rule, which the three renderers now share: a domain straddling
    /// zero has a baseline (audio is a deviation from silence), one that does
    /// not is drawn as its own envelope (an envelope, an automation, a
    /// unipolar take).
    #[test]
    fn only_a_domain_that_straddles_zero_has_a_baseline() {
        assert_eq!(baseline_of(-1.0, 1.0), Some(0.0));
        assert_eq!(baseline_of(-0.25, 0.75), Some(0.0));
        assert_eq!(
            baseline_of(0.0, 1.0),
            None,
            "unipolar: no baseline to fill to"
        );
        assert_eq!(baseline_of(20.0, 20_000.0), None, "an offset quantity");
        assert_eq!(baseline_of(-1.0, 0.0), None, "wholly negative");
    }

    /// **A transient lands in exactly one column, within a bucket of where it
    /// is.** What this replaced drew it in *two* — a column read every bucket
    /// overlapping it, so a spike a hundred samples outside was drawn inside,
    /// and the picture stepped as the zoom crossed a bucket. The buckets are
    /// tiled now: no duplication, no loss, and a position good to the
    /// resolution the summary has.
    #[test]
    fn a_transient_lands_in_one_column_within_a_bucket_of_itself() {
        let n = 1 << 16;
        let mut samples = vec![0.0f32; n];
        let spike = 900usize;
        samples[spike] = 1.0;
        let data = WaveformData::new(Arc::from(samples.as_slice()), 256);
        for spp in [256.1f64, 300.0, 512.0, 1_000.0, 4_000.0] {
            let hits: Vec<usize> = (0..40)
                .filter(|c| {
                    let s0 = *c as f64 * spp;
                    data.column(0, spp, s0, s0 + spp).1 > 0.5
                })
                .collect();
            assert_eq!(hits.len(), 1, "spp {spp}: drawn in {hits:?}");
            let s0 = hits[0] as f64 * spp;
            let off = if (spike as f64) < s0 {
                s0 - spike as f64
            } else {
                (spike as f64 - (s0 + spp)).max(0.0)
            };
            assert!(
                off <= 256.0,
                "spp {spp}: {off} samples away from its column"
            );
        }
    }

    /// Below a bucket there is nothing to summarize with, so the column is the
    /// samples in it and the answer is exact — envelope and body alike, since
    /// two pictures of one column must be about the same samples.
    #[test]
    fn a_column_finer_than_a_bucket_is_exact() {
        let samples: Vec<f32> = (0..20_000).map(|i| (i as f32 * 0.01).sin() * 0.5).collect();
        let data = WaveformData::new(Arc::from(samples.as_slice()), 256);
        for spp in [4.0, 60.0, 255.0] {
            let (s0, s1) = (3_333.0, 3_333.0 + spp);
            let (lo, hi) = data.column(0, spp, s0, s1);
            let ms = data
                .column_ms(0, spp, s0, s1)
                .expect("a built pyramid measures");
            let brute = &samples[s0 as usize..(s1.ceil() as usize).min(20_000)];
            let (blo, bhi) = peaks::min_max(brute).unwrap();
            let bms = peaks::mean_square(brute).unwrap();
            assert!(
                (lo - blo).abs() < 1e-6 && (hi - bhi).abs() < 1e-6,
                "spp {spp}"
            );
            assert!((ms - bms).abs() < 1e-6, "spp {spp}: {ms} vs {bms}");
        }
    }

    /// And above it the two pictures still agree with **each other**: the
    /// envelope and the measured body read one set of buckets, so a body can
    /// never sit outside the trace around it.
    #[test]
    fn the_body_and_the_envelope_read_one_set_of_buckets() {
        let samples: Vec<f32> = (0..200_000)
            .map(|i| (i as f32 * 0.003).sin() * (0.2 + 0.8 * (i as f32 * 0.00002).sin().abs()))
            .collect();
        let data = WaveformData::new(Arc::from(samples.as_slice()), 256);
        for spp in [300.0, 533.0, 2_000.0, 20_000.0] {
            let cols = ((200_000.0 / spp) as usize).min(20);
            for c in 0..cols {
                let s0 = c as f64 * spp;
                let (lo, hi) = data.column(0, spp, s0, s0 + spp);
                let ms = data.column_ms(0, spp, s0, s0 + spp).expect("measured");
                let rms = ms.sqrt();
                assert!(
                    rms <= hi.max(-lo) + 1e-6,
                    "spp {spp}, column {c}: body {rms} outside envelope ({lo}, {hi})"
                );
            }
        }
    }

    /// The envelope moves continuously through the zoom the pyramid's regimes    /// The envelope moves continuously through the zoom the pyramid's regimes
    /// used to switch at: there is no switch left, so the only test worth
    /// keeping is that the answer is the same one either side of it.
    #[test]
    fn the_envelope_is_continuous_where_the_regimes_meet() {
        let samples: Vec<f32> = (0..1 << 16)
            .map(|i| {
                let t = i as f32 / 1_000.0;
                (t * 6.0).sin() * (1.0 - i as f32 / (1 << 16) as f32)
            })
            .collect();
        let data = WaveformData::new(Arc::from(samples.as_slice()), 64);
        let (s0, s1) = (40_000.0, 40_256.0);
        for switch in [64.0, 128.0, 256.0] {
            let (lo_a, hi_a) = data.column(0, switch - 1e-3, s0, s1);
            let (lo_b, hi_b) = data.column(0, switch + 1e-3, s0, s1);
            assert!(
                (lo_a - lo_b).abs() < 1e-3 && (hi_a - hi_b).abs() < 1e-3,
                "at {switch}: ({lo_a},{hi_a}) vs ({lo_b},{hi_b})"
            );
        }
    }

    /// Consecutive columns cover every bucket **exactly once**, so a bucket
    /// that straddles a column boundary is displaced and never lost. It is the
    /// property the whole tiling rests on, and the one a naive "buckets
    /// strictly inside" would break in the direction that costs a transient.
    #[test]
    fn no_column_drops_a_bucket_at_a_coarse_zoom() {
        let n = 1 << 18;
        let mut samples = vec![0.0f32; n];
        let spp = 256.0 * 16.0;
        let boundary = 10.0 * spp;
        let straddling = boundary as usize + 4; // inside the bucket that edge cuts
        samples[straddling] = 1.0;
        let data = WaveformData::new(Arc::from(samples.as_slice()), 256);
        let hit = (0..40)
            .map(|c| {
                let s0 = c as f64 * spp;
                data.column(0, spp, s0, s0 + spp).1
            })
            .fold(f32::NEG_INFINITY, f32::max);
        assert_eq!(hit, 1.0, "the spike is in exactly one column, not in none");
    }
}

/// The samples a view shares rather than owns: the picture reads it where it
/// lies, so a write by anyone is a write to what is drawn.
#[cfg(test)]
mod shared_tests {
    use super::*;
    use std::sync::Mutex;

    /// Stands in for a mapped region: samples somebody else may write while
    /// the picture is reading them.
    struct Cells(Mutex<Vec<f32>>);

    impl peaks::Source for Cells {
        fn len(&self) -> usize {
            self.0.lock().unwrap().len()
        }

        fn read_into(&self, start: usize, out: &mut [f32]) {
            let cells = self.0.lock().unwrap();
            for (i, slot) in out.iter_mut().enumerate() {
                *slot = cells.get(start + i).copied().unwrap_or(0.0);
            }
        }
    }

    fn samples(n: usize) -> Vec<f32> {
        (0..n).map(|i| (i as f32 * 0.01).sin() * 0.7).collect()
    }

    fn shared(cells: &Arc<Cells>) -> WaveformData {
        WaveformData::from_sources(
            vec![Arc::clone(cells) as Arc<dyn peaks::Source + Send + Sync>],
            256,
        )
    }

    /// A view over a source draws exactly what a view over the same samples
    /// draws — at every regime, so the mapped path is not a second picture.
    #[test]
    fn a_shared_view_draws_what_an_owned_one_draws() {
        let samples = samples(20_000);
        let cells = Arc::new(Cells(Mutex::new(samples.clone())));
        let mapped = shared(&cells);
        let owned = WaveformData::new(samples.into(), 256);
        assert!(mapped.is_shared() && !owned.is_shared());
        for spp in [0.5, 8.0, 100.0, 255.0, 256.0, 1_000.0, 5_000.0] {
            for c in 0..30 {
                let s0 = (c * 613) as f64;
                let s1 = s0 + spp;
                assert_eq!(
                    mapped.column(0, spp, s0, s1),
                    owned.column(0, spp, s0, s1),
                    "spp {spp}, column {c}"
                );
                assert_eq!(
                    mapped.column_ms(0, spp, s0, s1),
                    owned.column_ms(0, spp, s0, s1),
                    "spp {spp}, column {c} (measure)"
                );
            }
        }
        assert_eq!(mapped.samples_at(0, 777), owned.samples_at(0, 777));
        assert_eq!(mapped.block(100, 50), owned.block(100, 50));
    }

    /// **The point of the whole thing**: somebody else writes the samples and
    /// the zoomed-in picture is already the new one, with nothing told to it.
    #[test]
    fn a_write_by_somebody_else_is_already_in_the_picture() {
        let cells = Arc::new(Cells(Mutex::new(samples(20_000))));
        let view = shared(&cells);
        let fine = 4.0; // finer than a bucket: the raw regime, read from the source
        let before = view.column(0, fine, 5_000.0, 5_004.0);
        cells.0.lock().unwrap()[5_000..5_004].fill(-0.9);
        let after = view.column(0, fine, 5_000.0, 5_004.0);
        assert_ne!(before, after);
        assert_eq!(after.0, -0.9);
    }

    /// A stroke over shared samples re-summarizes its span and copies
    /// nothing: the samples were already written where they live.
    #[test]
    fn a_stroke_moves_the_summary_and_not_the_samples() {
        let cells = Arc::new(Cells(Mutex::new(samples(20_000))));
        let mut view = shared(&cells);
        let coarse = 2_000.0; // the pyramid's regime, which a stroke must refresh
        let before = view.column(0, coarse, 4_096.0, 6_096.0);
        // The writer's half, exactly as the host does it: the samples first.
        cells.0.lock().unwrap()[4_500..4_700].fill(0.99);
        assert!(view.write_range(0, 4_500, &[0.99; 200]));
        let after = view.column(0, coarse, 4_096.0, 6_096.0);
        assert_ne!(before, after);
        assert!(
            (after.1 - 0.99).abs() < 1e-6,
            "the summary followed: {after:?}"
        );
        // And it equals a pyramid built from scratch over the same samples.
        let rebuilt = shared(&cells);
        assert_eq!(after, rebuilt.column(0, coarse, 4_096.0, 6_096.0));
    }
}

#[cfg(test)]
mod stream_tests {
    use super::*;

    fn samples(frames: usize, channels: usize) -> Vec<f32> {
        (0..frames * channels)
            .map(|i| {
                let f = (i / channels) as f32;
                let c = (i % channels) as f32;
                ((f * 0.019 + c * 1.3).sin() * 0.8) - c * 0.05
            })
            .collect()
    }

    /// The report the server sends for `count` whole buckets from `start`:
    /// bucket-major, channel-minor, measured from the samples the way the
    /// writer measures it.
    fn report(
        data: &[f32],
        channels: usize,
        bucket: usize,
        start: usize,
        count: usize,
    ) -> Vec<f32> {
        let mut out = Vec::new();
        for b in 0..count {
            for ch in 0..channels {
                let chunk: Vec<f32> = (0..bucket)
                    .map(|i| data[(start + b * bucket + i) * channels + ch])
                    .collect();
                let (lo, hi) = peaks::min_max(&chunk).unwrap();
                out.extend([lo, hi, peaks::mean_square(&chunk).unwrap()]);
            }
        }
        out
    }

    /// **The page's half of a recording**: a view over an empty take, told
    /// nothing but the overview of what was written, draws the overview a view
    /// over the samples draws. Column for column, at the zooms the summary
    /// answers.
    #[test]
    fn a_told_view_draws_what_a_reading_one_draws() {
        let (bucket, channels) = (256, 2);
        let frames = bucket * 24;
        let data = samples(frames, channels);
        let mut told =
            WaveformData::from_interleaved(&vec![0.0; frames * channels], channels, bucket);
        // The reports arrive in runs, as a recording fills.
        let mut at = 0;
        for count in [3usize, 9, 12] {
            assert!(told.write_buckets(
                at * bucket,
                bucket,
                &report(&data, channels, bucket, at * bucket, count)
            ));
            at += count;
        }
        let reading = WaveformData::from_interleaved(&data, channels, bucket);
        for ch in 0..channels {
            for spp in [bucket as f64, 1_000.0, 4_000.0] {
                for x in 0..8 {
                    let (a, b) = (x as f64 * 1_000.0, x as f64 * 1_000.0 + 1_000.0);
                    assert_eq!(
                        told.column(ch, spp, a, b),
                        reading.column(ch, spp, a, b),
                        "channel {ch}, {spp} samples per pixel, column {x}"
                    );
                }
            }
        }
    }

    /// What it does **not** claim: the samples. A page holds its own copy and
    /// the wire carries no audio, so zoomed past the base bucket the picture is
    /// still the silence the take was allocated as — the resolution the report
    /// has, rather than samples invented from their statistics.
    #[test]
    fn the_samples_are_not_invented_from_the_summary() {
        let (bucket, frames) = (256, 4_096);
        let data = samples(frames, 1);
        let mut told = WaveformData::from_interleaved(&vec![0.0; frames], 1, bucket);
        assert!(told.write_buckets(0, bucket, &report(&data, 1, bucket, 0, frames / bucket)));
        assert_ne!(
            told.column(0, bucket as f64, 0.0, 1_024.0),
            (0.0, 0.0),
            "the overview grew"
        );
        assert_eq!(
            told.column(0, 4.0, 100.0, 104.0),
            (0.0, 0.0),
            "and the raw regime is the page's own copy, untouched"
        );
    }

    /// A report on another grid is refused whole, so a picture is never part
    /// one samples and part another.
    #[test]
    fn a_report_on_another_grid_is_refused() {
        let (bucket, frames) = (256, 4_096);
        let data = samples(frames, 1);
        let mut told = WaveformData::from_interleaved(&vec![0.0; frames], 1, bucket);
        let one = report(&data, 1, bucket, 0, 1);
        assert!(!told.write_buckets(0, bucket * 2, &one), "a coarser bucket");
        assert!(!told.write_buckets(bucket / 2, bucket, &one), "unaligned");
        assert!(!told.write_buckets(frames, bucket, &one), "past the end");
        assert!(!told.write_buckets(0, bucket, &one[..2]), "a ragged run");
        assert_eq!(
            told.column(0, bucket as f64, 0.0, 1_024.0),
            (0.0, 0.0),
            "nothing was applied"
        );
        assert!(
            told.write_buckets(0, bucket, &one),
            "and the right one lands"
        );
    }
}

#[cfg(test)]
mod window_tests {
    use super::*;

    fn samples(frames: usize, channels: usize) -> Vec<f32> {
        (0..frames * channels)
            .map(|i| {
                let f = (i / channels) as f32;
                let c = (i % channels) as f32;
                ((f * 0.37 + c).sin() * 0.9).clamp(-1.0, 1.0)
            })
            .collect()
    }

    /// **The claim the whole path exists for**: a view that holds only a
    /// summary, given the run the eye is on, draws that run exactly as a view
    /// that holds the samples does — and goes on drawing the summary
    /// everywhere else, at the same columns.
    #[test]
    fn a_window_draws_the_samples_where_it_covers_and_the_summary_elsewhere() {
        let (bucket, channels, frames) = (256, 2, 256 * 40);
        let data = samples(frames, channels);
        let whole = WaveformData::from_interleaved(&data, channels, bucket);
        // The page's side: the summary of the same samples, no samples.
        let mut told =
            WaveformData::with_multi_pyramid(peaks::MultiPyramid::empty(frames, channels, bucket));
        for (ch, pyr) in told.channels.iter_mut().enumerate() {
            pyr.pyramid = whole.channels[ch].pyramid.clone();
        }

        // Zoomed in past the bucket, it can only draw its bucket.
        let (a, b) = (5_000.0, 5_064.0);
        assert_ne!(
            told.column(0, 4.0, a, b),
            whole.column(0, 4.0, a, b),
            "without the samples the fine regime is the bucket"
        );

        // The run the eye is on arrives...
        let (start, len) = (4_096usize, 2_048usize);
        let run = &data[start * channels..(start + len) * channels];
        assert!(told.set_window(start, channels, run));

        // ...and inside it the picture is the samples', column for column.
        for ch in 0..channels {
            for x in 0..16 {
                let (a, b) = (4_200.0 + x as f64 * 64.0, 4_200.0 + x as f64 * 64.0 + 64.0);
                assert_eq!(
                    told.column(ch, 1.0, a, b),
                    whole.column(ch, 1.0, a, b),
                    "channel {ch}, column {x}"
                );
            }
        }
        // Outside it, the summary answers as it did — and agrees with the
        // the take's own summary, since it is the same summary.
        for x in 0..8 {
            let (a, b) = (x as f64 * 4_000.0, x as f64 * 4_000.0 + 4_000.0);
            assert_eq!(
                told.column(0, 2_000.0, a, b),
                whole.column(0, 2_000.0, a, b)
            );
        }
    }

    /// A window is a run and not the samples: it is refused where it would
    /// not fit, and it never replaces samples that already answers.
    #[test]
    fn a_window_is_refused_where_it_would_not_be_the_samples() {
        let (bucket, channels, frames) = (64, 2, 1_024);
        let data = samples(frames, channels);
        let mut told =
            WaveformData::with_multi_pyramid(peaks::MultiPyramid::empty(frames, channels, bucket));
        assert!(
            !told.set_window(0, 1, &data[..64]),
            "the wrong channel count"
        );
        assert!(!told.set_window(0, channels, &data[..7]), "a ragged run");
        assert!(
            !told.set_window(frames - 4, channels, &data[..64 * channels]),
            "past the end of the samples"
        );
        assert!(
            told.set_window(0, channels, &data[..64 * channels]),
            "and a run that fits"
        );

        // A view that owns its samples takes none: it already answers.
        let mut owned = WaveformData::from_interleaved(&data, channels, bucket);
        assert!(!owned.set_window(0, channels, &data[..64 * channels]));
    }

    /// The window moves with the eye: a second one replaces the first, so the
    /// picture holds one run and not a growing cache.
    #[test]
    fn a_second_window_replaces_the_first() {
        let (bucket, channels, frames) = (64, 1, 4_096);
        let data = samples(frames, channels);
        let mut told =
            WaveformData::with_multi_pyramid(peaks::MultiPyramid::empty(frames, channels, bucket));
        assert!(told.set_window(0, channels, &data[..512]));
        assert!(told.covers(0, 512));
        assert!(told.set_window(2_048, channels, &data[2_048..2_560]));
        assert!(told.covers(2_048, 2_560));
        assert!(!told.covers(0, 512), "the run the eye left is not kept");
    }
}
