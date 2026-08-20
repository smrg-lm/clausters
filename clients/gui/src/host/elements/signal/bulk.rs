//! **What a signal element asks the outside to load, and what it does with the
//! answer** — the bulk half of the element's declaration.
//!
//! Bulk is the data too big for the wire: a minutes-long take, a prebuilt peaks
//! cache, a server buffer. It moves through local shared resources — a mapped
//! file natively, a `fetch` in a page — and never as JSON over OSC, which is
//! the rule the whole crate is built on.
//!
//! Both fronts used to *derive* this from the outside, each in its own walk:
//! the native one matched on the presentation in `windows::collect_slots` and
//! again in `load_element_bulk`, the browser one in `bulk::collect_bulk`, and
//! the buffer-fetch reply forked a third time in each `serverleg`. Every one of
//! those was asking the same two questions — **which resource, in which form**
//! — of an element that knows both. [`SignalElement::want`] answers them once.
//!
//! **Where the answer goes is not the loader's decision either.** An element
//! that claimed a GPU slot is fed through that slot; every other one takes the
//! data home through [`SignalElement::take`]. So a loader resolves a resource
//! and hands it over, and nothing about a presentation is written down in it.
//!
//! A slot is where a picture goes, though, and not where the *samples* lives:
//! a pyramid is both, so a slot-backed element takes it home as well (one
//! shared `Arc`, `frame::keep_data`). That is what lets a copy read the
//! take back out of the element that named it, and lets a window opened later
//! refill its slot from what the element already holds.

use std::sync::Arc;

use super::{Presentation, SignalElement, Source};
use crate::host::widget::element::{Bulk, Loaded};

impl SignalElement {
    /// The bulk resource this element wants, in the form it draws from, or
    /// `None` when it needs nothing loaded — it has its samples inline, it
    /// reads a bus, or it named no resource at all.
    ///
    /// The precedence is the one the source has always had — **cache, then
    /// path, then buffer** — a prebuilt summary being cheaper than the raw
    /// samples, and a server buffer being the one thing the host has to ask
    /// another process for.
    /// Forgets what this element resolved, so the next pass asks for it again
    /// — the mapped half of "the samples are now these".
    ///
    /// A source with nothing behind it (inline samples and no path, cache or
    /// buffer) is left alone: it has nothing to re-read, and clearing it would
    /// erase the samples instead of refreshing it.
    pub fn reread(&mut self) {
        let Source::Data(data) = &mut self.source else {
            return;
        };
        if data.path.is_none() && data.cache.is_none() && data.buffer.is_none() {
            return;
        }
        data.body = None;
        // A sequence keeps its samples inline, so *those* are what it must
        // forget; a take keeps a pyramid, and dropping the body is enough.
        if !data.bulk {
            data.samples = Vec::new().into();
        }
        self.slot_dirty = true;
    }

    pub fn want(&self) -> Option<Bulk> {
        let Source::Data(data) = &self.source else {
            return None; // a bus is fed forward-only; there is nothing to load
        };
        // A spectral view resolves to analyses, a take to peaks, a plotted
        // sequence to the samples themselves. The presentation decides the
        // *form*; `bulk` decides whether a trace is summarized at all.
        let spectral = self.presentation == Presentation::TimeFrequency;
        if spectral {
            if self.is_live() {
                return None; // a waterfall analyzes what the tick retains
            }
            if let Some(cache) = &data.cache {
                return Some(Bulk::StftCache(cache.clone()));
            }
            if let Some(path) = &data.path {
                return Some(Bulk::Stft {
                    path: path.clone(),
                    channels: data.channels,
                    window_size: self.spectral.fft_size,
                    hop: self.spectral.hop,
                    sample_rate: self.editor.sample_rate,
                });
            }
            return data.buffer.filter(|_| data.is_empty()).map(Bulk::Buffer);
        }
        if !data.bulk {
            // A sequence: whole samples, and only when it has none yet.
            if !data.samples.is_empty() {
                return None;
            }
            return match (&data.path, data.buffer) {
                (Some(path), _) => Some(Bulk::Samples {
                    path: path.clone(),
                    channels: data.channels,
                }),
                (None, Some(bufnum)) => Some(Bulk::Buffer(bufnum)),
                _ => None,
            };
        }
        if data.body.is_some() {
            return None; // the pyramid is already here
        }
        // **A take being recorded into is asked for its shape, not its
        // samples.** It holds silence until something records into it, and
        // what fills the picture is the overview the server streams — so
        // pulling the samples would be a download of zeros, at the take's
        // full length, to draw over them.
        //
        // That is all `fills` says here, and all it decides: *these samples
        // are being written*. Which route a take that stands still arrives by
        // is a question about cost and is answered where the size is known
        // (`fetch::BufferFetches::whole`), not by whether something happens to
        // be recording into it.
        if self.fills
            && data.cache.is_none()
            && data.path.is_none()
            && let Some(buffer) = data.buffer
        {
            return Some(Bulk::Recording {
                buffer,
                base_bucket: data.base_bucket,
            });
        }
        if let Some(cache) = &data.cache {
            return Some(Bulk::PeakCache(cache.clone()));
        }
        if let Some(path) = &data.path {
            return Some(Bulk::Peaks {
                path: path.clone(),
                channels: data.channels,
                base_bucket: data.base_bucket,
            });
        }
        data.buffer.filter(|_| data.is_empty()).map(Bulk::Buffer)
    }

    /// The GPU slot this element claims, when its picture cannot go into the
    /// window's one mesh: a **vertex buffer** for a navigable trace (columns
    /// decimated per frame from a peak pyramid) or a **texture** for a
    /// time-frequency view (one texel per pixel, constant cost at any zoom).
    ///
    /// The parameters ride with the slot because whoever fills it has to make
    /// the data fit it — a bucket to summarize at, an analysis to run — and the
    /// element is the only one that knows them. Everything else draws itself.
    pub fn slot_kind(&self) -> Option<crate::host::widget::element::SlotKind> {
        use crate::host::widget::element::SlotKind;
        if !self.needs_gpu_slot() {
            return None;
        }
        Some(if self.is_texture_view() {
            SlotKind::Texture {
                window_size: self.spectral.fft_size,
                hop: self.spectral.hop,
                sample_rate: self.editor.sample_rate,
            }
        } else {
            SlotKind::Geometry {
                base_bucket: self
                    .source
                    .data()
                    .map_or(crate::host::elements::signal::DEFAULT_BASE_BUCKET, |d| {
                        d.base_bucket
                    }),
            }
        })
    }

    /// **The shape of the samples this element holds** — `(channels, frames)`
    /// per channel — whichever form it arrived in: a resolved pyramid (a take)
    /// or the inline samples (a plotted sequence). `None` when it holds
    /// neither, which is a source nobody has resolved yet.
    ///
    /// It is the shape and not the samples because the one caller is a
    /// **write**, which has to know what it may address before it addresses it:
    /// handing out the data to measure it would be a copy of a take per stroke.
    /// **How much of this element's samples exists**, in frames, or `None` when
    /// all of it does — the drawing's half of the `fills` prop, asked wherever
    /// a picture of the samples is built.
    pub fn written_frames(&self) -> Option<u64> {
        self.fills.then_some(self.written)
    }

    pub fn sample_shape(&self) -> Option<(usize, u64)> {
        let data = self.source.data()?;
        if let Some(body) = &data.body {
            return Some((body.num_channels(), body.total_samples() as u64));
        }
        if data.samples.is_empty() {
            return None;
        }
        let channels = data.channels.max(1);
        Some((channels, (data.samples.len() / channels) as u64))
    }

    /// The **server buffer** this element's samples came from, if it named one.
    pub fn source_buffer(&self) -> Option<i32> {
        self.source.data()?.buffer
    }

    /// **Re-reads the summary of a span** of shared samples, returning
    /// whether this element draws any. The other half of
    /// [`Self::write_samples`]: there the host wrote the samples, here
    /// somebody else did and only said where.
    pub fn resummarize(&mut self, ch: Option<usize>, start: u64, frames: usize) -> bool {
        let Some(data) = self.source.data_mut() else {
            return false;
        };
        let Some(body) = &data.body else {
            return false;
        };
        if !body.is_shared() {
            return false;
        }
        // **Patched where nobody else is looking, copied where they are.** The
        // work of a refresh is proportional to the span — `resummarize` touches
        // the buckets over it and their parents — while a *copy* is
        // proportional to the whole take, so a picture that follows a recording
        // closely pays for the take once per step instead of for the step. The
        // front is single-threaded (this runs between frames and the draw runs
        // in one), so what the sharing has to defend against is a borrow and
        // not a race: where the element is the only holder there is nothing to
        // defend and the pyramid is written in place.
        //
        // `Arc::make_mut` is that rule and nothing else: `&mut` when this is
        // the sole owner, a copy first when a slot is still holding the
        // samples it draws. **One copy, however many channels** either way —
        // taking one per channel is what made following a multichannel
        // recording scale with the square of nothing useful.
        let body = Arc::make_mut(data.body.as_mut().expect("just matched"));
        let start = start as usize;
        let done = match ch {
            Some(ch) => body.resummarize(ch, start, frames),
            None => (0..body.num_channels())
                .map(|ch| body.resummarize(ch, start, frames))
                .fold(false, |acc, ok| acc | ok),
        };
        if !done {
            return false;
        }
        self.slot_dirty = true;
        true
    }

    /// **Folds a report of buckets into the summary**, returning whether this
    /// element draws samples it applies to.
    ///
    /// The third way a picture changes, beside [`Self::write_samples`] (the
    /// host wrote the samples) and [`Self::resummarize`] (somebody else
    /// wrote them where this element can read them): here nobody can read them
    /// at all — the samples are in the server's memory and this element holds
    /// its own copy — so what arrives is the *overview* of what was written.
    ///
    /// It applies only to an **owned** body, which is exactly the case that
    /// cannot re-read: a shared one is a mapping and follows the frontier for
    /// free (and would have this report's own information twice over).
    pub fn write_buckets(&mut self, start_frame: u64, bucket: usize, stats: &[f32]) -> bool {
        let Some(data) = self.source.data_mut() else {
            return false;
        };
        let Some(body) = &data.body else {
            return false;
        };
        if body.is_shared() {
            return false;
        }
        // Sole owner where nobody is holding it, a copy where a slot still is
        // — the same rule a refresh follows, for the same reason.
        let body = Arc::make_mut(data.body.as_mut().expect("just matched"));
        if !body.write_buckets(start_frame as usize, bucket, stats) {
            return false;
        }
        self.slot_dirty = true;
        true
    }

    /// **Puts a fetched run of the samples under the summary**, returning
    /// whether this element took it.
    ///
    /// The other half of a zoom that went past the overview: the picture said
    /// which span it could not answer, the leg fetched it, and this is where
    /// it lands. Only a body that holds no samples of its own takes one — a
    /// mapped body reads the samples where it lies, and a wholly owned one
    /// already has it.
    pub fn set_window(&mut self, start: u64, channels: usize, samples: &[f32]) -> bool {
        let Some(data) = self.source.data_mut() else {
            return false;
        };
        if data.body.is_none() {
            return false;
        }
        let body = Arc::make_mut(data.body.as_mut().expect("just checked"));
        if !body.set_window(start as usize, channels, samples) {
            return false;
        }
        self.slot_dirty = true;
        true
    }

    /// **Puts a finer summary over the span this element is showing**, beside
    /// the one it holds — the summary counterpart of [`Self::set_window`], and
    /// what a zoom past the base bucket asks for where the samples cannot be
    /// mapped. `stats` is the wire's own overview blob, unconverted.
    pub fn set_detail(&mut self, start: u64, bucket: usize, stats: &[f32]) -> bool {
        let channels = self.sample_shape().map_or(1, |(ch, _)| ch.max(1));
        let Some(data) = self.source.data_mut() else {
            return false;
        };
        if data.body.is_none() {
            return false;
        }
        let body = Arc::make_mut(data.body.as_mut().expect("just checked"));
        if !body.set_detail(start as usize, bucket, channels, stats) {
            return false;
        }
        self.slot_dirty = true;
        true
    }

    /// **The summary's finest bucket**, or `None` when this element holds no
    /// summary — what a span read back has to be aligned to before it can
    /// replace what the summary says over it.
    pub fn summary_bucket(&self) -> Option<usize> {
        Some(self.source.data()?.body.as_ref()?.base_bucket())
    }

    /// **Takes a span another peer wrote**, returning whether this element
    /// drew any of it. `samples` is interleaved, every channel of the frames
    /// `[start, start + n)`, read back out of the buffer itself.
    ///
    /// The third way somebody else's write reaches a picture, and the one for
    /// a view that can neither re-read the samples nor be streamed an
    /// overview: [`Self::resummarize`] is for samples this element can read
    /// where they lie, [`Self::write_buckets`] for a recording the server
    /// reports on, and this for an **edit** — announced once, over a span,
    /// into samples this element holds its own copy of.
    ///
    /// Which of the two halves lands depends on what the copy is, and they are
    /// exclusive by construction: samples that are here are written and
    /// re-summarized over the span ([`Self::write_samples`], which is what a
    /// stroke does and behaves the same whether the copy is owned or mapped),
    /// while a summary with **no** samples under it (a take being recorded
    /// into, whose picture is buckets and a window) takes the span as the
    /// buckets it covers and as the window itself.
    pub fn patch_span(&mut self, start: u64, channels: usize, samples: &[f32]) -> bool {
        let channels = channels.max(1);
        if samples.is_empty() || !samples.len().is_multiple_of(channels) {
            return false;
        }
        let frames = samples.len() / channels;
        let mut wrote = false;
        for ch in 0..channels {
            let run: Vec<f32> = (0..frames).map(|f| samples[f * channels + ch]).collect();
            wrote |= self.write_samples(ch, start, &run);
        }
        if wrote {
            return true;
        }
        // No samples to write into: the picture is a summary. The buckets the
        // span **wholly** covers are recomputed from it — a partial one at
        // either edge is left alone rather than written from the fraction of
        // it that arrived, which would report a peak that is not there.
        let Some(bucket) = self.summary_bucket().filter(|b| *b > 0) else {
            return false;
        };
        let first = (start as usize).div_ceil(bucket);
        let last = (start as usize + frames) / bucket;
        let mut folded = false;
        if last > first {
            let mut stats = Vec::with_capacity((last - first) * channels * 3);
            for b in first..last {
                let at = b * bucket - start as usize;
                for ch in 0..channels {
                    let run: Vec<f32> = (0..bucket)
                        .map(|f| samples[(at + f) * channels + ch])
                        .collect();
                    let (lo, hi) = clausters_core::peaks::min_max(&run).unwrap_or((0.0, 0.0));
                    let ms = clausters_core::peaks::mean_square(&run).unwrap_or(0.0);
                    stats.extend_from_slice(&[lo, hi, ms]);
                }
            }
            folded = self.write_buckets((first * bucket) as u64, bucket, &stats);
        }
        // The samples themselves answer where the eye is, exactly as a zoom's
        // window does: an edit under a zoomed-in view is drawn from what came
        // back, not from the buckets it was folded into.
        folded | self.set_window(start, channels, samples)
    }

    /// **The stream this element wants to be told about**, as
    /// `(buffer, bucket)`, or `None` when it wants none.
    ///
    /// It is a want and not a subscription: the host collects them, and one
    /// `/buffer_stream` covers every view of every window. What makes an
    /// element want one is the pair of facts nothing else can supply — the
    /// client said these samples are being written (`fills`), and the body is
    /// this element's **own copy**, so no frontier in memory can tell it what
    /// grew. A mapped body is deliberately absent: it reads the samples where
    /// it lies and would be paying twice for one picture.
    pub fn stream_want(&self) -> Option<(i32, usize)> {
        if !self.fills {
            return None;
        }
        let data = self.source.data()?;
        if data.body.as_ref()?.is_shared() {
            return None;
        }
        Some((data.buffer?, data.base_bucket))
    }

    /// **Writes a run of samples into the samples**, returning whether it
    /// landed. `start` is a frame index in channel `ch`.
    ///
    /// The element does it rather than the host, because the host does not know
    /// which of the two forms this source is in — and both are real: a clip's
    /// take draws from inline samples, while the same buffer opened as a
    /// navigable view draws from a pyramid. A host that patched only the
    /// pyramid left the clip showing the samples as it was before the stroke.
    ///
    /// Both are **replaced rather than mutated**: the pyramid is shared with
    /// whatever slot is drawing it, so patching it in place would rewrite a
    /// picture under a renderer that never asked. Only the columns over the
    /// span are recomputed ([`crate::waveform::WaveformData::write_range`]).
    pub fn write_samples(&mut self, ch: usize, start: u64, values: &[f32]) -> bool {
        let Some(data) = self.source.data_mut() else {
            return false;
        };
        let start = start as usize;
        if values.is_empty() {
            return false;
        }
        let mut wrote = false;
        if let Some(body) = &data.body {
            let mut copy = (**body).clone();
            if !copy.write_range(ch, start, values) {
                return false;
            }
            data.body = Some(std::sync::Arc::new(copy));
            wrote = true;
        }
        if !data.samples.is_empty() {
            let channels = data.channels.max(1);
            let end = start + values.len();
            if ch >= channels || end * channels > data.samples.len() {
                return false;
            }
            // Interleaved, so a channel is a stride — the one place the picture
            // and the server's flat addressing spell the same span differently.
            let mut samples = data.samples.to_vec();
            for (i, v) in values.iter().enumerate() {
                samples[(start + i) * channels + ch] = *v;
            }
            data.samples = samples.into();
            wrote = true;
        }
        if wrote {
            self.slot_dirty = true;
            self.refresh_analysis();
        }
        wrote
    }

    /// Takes a resolved resource into the element, returning whether it fit
    /// what this element draws from. Landing samples also refreshes the cached
    /// spectral analysis, which is the element's one derived value and belongs
    /// at its mutation points rather than in a frame.
    ///
    /// The **STFT lanes are not taken here**: an element that analyzes into a
    /// texture claimed a GPU slot, and a slot is fed by the frame that owns the
    /// pipeline — the same boundary a `canvas`' shader crosses.
    pub fn take(&mut self, data: Loaded) -> bool {
        let Some(source) = self.source.data_mut() else {
            return false;
        };
        match data {
            // Raw samples: what this element draws from decides what becomes of
            // them — a take is summarized at its own bucket, a sequence is kept
            // whole. That is why a loader can hand these over without knowing
            // anything about the drawing.
            Loaded::Raw { samples, channels } => {
                if source.bulk {
                    source.body = Some(Arc::new(crate::waveform::WaveformData::from_interleaved(
                        &samples,
                        channels,
                        source.base_bucket,
                    )));
                } else {
                    source.channels = channels.max(1);
                    source.samples = samples.into();
                    self.refresh_analysis();
                    self.slot_dirty = true;
                }
                true
            }
            // A pyramid is the samples this element holds, whether or not a
            // slot also draws it: the `Arc` is the one a slot was filled with,
            // and a read of the source (a copy) is answered out of it.
            Loaded::Peaks(peaks) => {
                source.body = Some(peaks);
                // A pyramid also *replaces* one, and a destructive edit is how:
                // the samples are rewritten and a new pyramid arrives here, so
                // the slot has to be filled from it or the window keeps drawing
                // the picture from before the stroke. The refill after a plain
                // load is the same `Arc` going back where it already is, and
                // the fill keeps the view's navigation.
                self.slot_dirty = true;
                true
            }
            Loaded::Samples(samples) => {
                source.samples = samples;
                self.refresh_analysis();
                self.slot_dirty = true;
                true
            }
            // Samples read where they live. A take draws the summary and
            // takes the whole thing as it is; a run of samples has to be read
            // out, because that is what it draws from.
            Loaded::Shared(data) => {
                if source.bulk {
                    source.body = Some(data);
                } else {
                    source.channels = data.num_channels().max(1);
                    source.samples = data
                        .block(0, data.total_samples())
                        .unwrap_or_default()
                        .into();
                    self.refresh_analysis();
                }
                self.slot_dirty = true;
                true
            }
            Loaded::Stfts(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::guidef::GuiNode;
    use crate::host::widget::Widget;

    fn element(json: &str) -> SignalElement {
        let w = Widget::from_node(1, &GuiNode::parse(json.as_bytes()).unwrap(), &[]).unwrap();
        w.signal().expect("a signal element").clone()
    }

    /// The form follows what the element **draws from**, not what the file is:
    /// one raw path is a pyramid to a take and a run of samples to a plot.
    #[test]
    fn one_path_is_two_wants_depending_on_the_drawing() {
        let take = element(
            r#"{"id":1,"type":"signal","view":"trace","path":"take.f32","bulk":true,"channels":2}"#,
        );
        assert_eq!(
            take.want(),
            Some(Bulk::Peaks {
                path: "take.f32".into(),
                channels: 2,
                base_bucket: crate::host::elements::signal::DEFAULT_BASE_BUCKET,
            })
        );
        let plot =
            element(r#"{"id":1,"type":"signal","view":"trace","path":"seq.f32","navigable":0}"#);
        assert_eq!(
            plot.want(),
            Some(Bulk::Samples {
                path: "seq.f32".into(),
                channels: 1,
            })
        );
    }

    /// **A view that cannot read its own samples asks to be told about it.**
    /// The want is the pair of facts nothing else supplies: the client said
    /// this take is being written (`fills`), and the body is this element's
    /// own copy — a mapped one reads the frontier and needs no wire.
    #[test]
    fn an_owned_take_being_recorded_wants_the_stream() {
        let mut take = element(
            r#"{"id":1,"type":"signal","view":"trace","bulk":true,"buffer":7,"fills":true}"#,
        );
        assert_eq!(
            take.stream_want(),
            None,
            "no body yet: nothing to fold into"
        );
        assert!(take.take(Loaded::Peaks(Arc::new(
            crate::waveform::WaveformData::from_interleaved(&[0.0; 2048], 1, 256)
        ))));
        assert_eq!(
            take.stream_want(),
            Some((7, crate::host::elements::signal::DEFAULT_BASE_BUCKET)),
            "an owned body being written wants the reports"
        );

        let loaded = element(r#"{"id":1,"type":"signal","view":"trace","bulk":true,"buffer":7}"#);
        assert_eq!(
            loaded.stream_want(),
            None,
            "a take nobody is recording into is not followed"
        );
    }

    /// **An edit somebody else made, read back.** A page's copy is owned, so
    /// the span lands as samples and the summary over it follows.
    #[test]
    fn a_patch_lands_in_owned_samples_and_in_the_summary() {
        let mut take = element(
            r#"{"id":1,"type":"signal","view":"trace","bulk":true,"buffer":7,"channels":2}"#,
        );
        take.take(Loaded::Raw {
            samples: vec![0.0; 1024 * 2],
            channels: 2,
        });
        // A stereo run of 256 frames at frame 256: full scale left, silent right.
        let mut span = Vec::new();
        for _ in 0..256 {
            span.extend_from_slice(&[1.0, 0.0]);
        }
        assert!(take.patch_span(256, 2, &span));
        let body = take.source.data().unwrap().body.clone().unwrap();
        assert_eq!(body.column(0, 256.0, 256.0, 512.0), (1.0, 1.0));
        assert_eq!(
            body.column(0, 256.0, 0.0, 256.0),
            (0.0, 0.0),
            "and only where it was written"
        );
        assert_eq!(
            body.column(1, 256.0, 256.0, 512.0),
            (0.0, 0.0),
            "the other channel is what came back for it"
        );
    }

    /// A picture that is a **summary with no samples under it** — a take being
    /// recorded into — takes the same span as the buckets it covers, so the
    /// overview shows the edit at every zoom.
    #[test]
    fn a_patch_folds_into_a_summary_with_no_samples() {
        let mut take = element(
            r#"{"id":1,"type":"signal","view":"trace","bulk":true,"buffer":7,"fills":true}"#,
        );
        take.take(Loaded::Peaks(Arc::new(
            crate::waveform::WaveformData::with_multi_pyramid(
                clausters_core::peaks::MultiPyramid::empty(1024, 1, 256),
            ),
        )));
        let span: Vec<f32> = (0..512)
            .map(|i| if i % 2 == 0 { 0.5 } else { -0.5 })
            .collect();
        assert!(take.patch_span(256, 1, &span));
        let body = take.source.data().unwrap().body.clone().unwrap();
        assert_eq!(body.column(0, 256.0, 256.0, 512.0), (-0.5, 0.5));
        assert_eq!(
            body.column(0, 256.0, 0.0, 256.0),
            (0.0, 0.0),
            "and nothing outside the span it was given"
        );
    }

    /// And the report lands in the summary: the element folds buckets it was
    /// handed exactly as it re-summarizes a span it can read.
    #[test]
    fn a_report_folds_into_an_owned_summary() {
        let mut take = element(
            r#"{"id":1,"type":"signal","view":"trace","bulk":true,"buffer":7,"fills":true}"#,
        );
        take.take(Loaded::Peaks(Arc::new(
            crate::waveform::WaveformData::from_interleaved(&[0.0; 1024], 1, 256),
        )));
        // One bucket, one channel: min, max and mean square, as the wire has it.
        assert!(take.write_buckets(256, 256, &[-0.5, 0.5, 0.25]));
        let body = take.source.data().unwrap().body.clone().unwrap();
        assert_eq!(body.column(0, 256.0, 256.0, 512.0), (-0.5, 0.5));
        assert_eq!(
            body.column(0, 256.0, 0.0, 256.0),
            (0.0, 0.0),
            "and only the bucket it named"
        );
        assert!(
            !take.write_buckets(1, 256, &[-0.5, 0.5, 0.25]),
            "a report off the grid is refused"
        );
    }

    /// A prebuilt summary wins over the raw samples, and a server buffer is the
    /// last resort — the precedence the source has always had.
    #[test]
    fn a_cache_wins_over_a_path_and_a_buffer_is_last() {
        let both = element(
            r#"{"id":1,"type":"signal","view":"trace","bulk":true,"cache":"peaks.clpk","path":"take.f32","buffer":3}"#,
        );
        assert_eq!(both.want(), Some(Bulk::PeakCache("peaks.clpk".into())));
        let buffered = element(r#"{"id":1,"type":"signal","view":"trace","bulk":true,"buffer":3}"#);
        assert_eq!(buffered.want(), Some(Bulk::Buffer(3)));
    }

    /// Nothing to want: a bus is fed forward-only, and an element that already
    /// holds its data asks for nothing on the next walk — which is what keeps a
    /// tree change from re-fetching what is already here.
    #[test]
    fn an_element_that_has_its_data_wants_nothing() {
        let live = element(r#"{"id":1,"type":"signal","view":"trace","bus":0}"#);
        assert_eq!(live.want(), None);
        let inline = element(r#"{"id":1,"type":"signal","view":"trace","data":[0.0,1.0]}"#);
        assert_eq!(inline.want(), None);

        let mut take =
            element(r#"{"id":1,"type":"signal","view":"trace","path":"t.f32","bulk":true}"#);
        assert!(take.want().is_some());
        assert!(take.take(Loaded::Peaks(Arc::new(
            crate::waveform::WaveformData::from_interleaved(&[0.0, 1.0, 0.5, -1.0], 1, 2,)
        ))));
        assert_eq!(take.want(), None, "the pyramid is here now");
    }

    /// A spectral view asks for analyses, and a **live** one asks for nothing:
    /// a waterfall analyzes what the tick retains, so there is no resource.
    #[test]
    fn a_spectral_view_wants_analyses_unless_it_is_live() {
        let stored = element(
            r#"{"id":1,"type":"signal","view":"spectrogram","path":"take.f32","channels":2,"window_size":512,"hop":128}"#,
        );
        assert!(matches!(stored.want(), Some(Bulk::Stft { .. })));
        let waterfall = element(
            r#"{"id":1,"type":"signal","view":"spectrogram","bus":0,"retention":4.0,"navigable":1}"#,
        );
        assert_eq!(waterfall.want(), None);
    }

    /// Shared samples: the samples move where they live and only the summary
    /// is told, which is what makes a second peer's edit visible here at all.
    #[test]
    fn a_take_reading_shared_samples_refreshes_a_span() {
        use std::sync::Mutex;

        struct Cells(Mutex<Vec<f32>>);

        impl clausters_core::peaks::Source for Cells {
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

        let cells = Arc::new(Cells(Mutex::new(vec![0.1f32; 4_000])));
        let shared = Arc::new(crate::waveform::WaveformData::from_sources(
            vec![Arc::clone(&cells) as Arc<dyn clausters_core::peaks::Source + Send + Sync>],
            256,
        ));
        let mut take =
            element(r#"{"id":1,"type":"signal","view":"trace","path":"t.f32","bulk":true}"#);
        assert!(take.take(Loaded::Shared(shared)));

        let column = |el: &SignalElement| {
            el.source
                .data()
                .and_then(|d| d.body.as_ref())
                .map(|b| b.column(0, 1_000.0, 0.0, 1_000.0))
                .unwrap()
        };
        assert_eq!(column(&take), (0.1, 0.1));
        // Somebody else's write: the cells first, then the span announced.
        cells.0.lock().unwrap()[100..200].fill(-0.8);
        assert!(take.resummarize(Some(0), 100, 100));
        assert_eq!(column(&take).0, -0.8);

        // Every channel at once is what a recording asks for, and it is one
        // copy of the summary rather than one per channel.
        cells.0.lock().unwrap()[400..500].fill(0.6);
        assert!(take.resummarize(None, 400, 100));
        assert_eq!(column(&take).1, 0.6);

        // An owned body has nothing to re-read: its samples are its own.
        let mut owned =
            element(r#"{"id":1,"type":"signal","view":"trace","path":"t.f32","bulk":true}"#);
        assert!(owned.take(Loaded::Peaks(Arc::new(
            crate::waveform::WaveformData::from_interleaved(&[0.0, 1.0, 0.5, -1.0], 1, 2)
        ))));
        assert!(!owned.resummarize(Some(0), 0, 2));
    }
}
