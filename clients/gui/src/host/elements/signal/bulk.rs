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
//! A slot is where a picture goes, though, and not where the *material* lives:
//! a pyramid is both, so a slot-backed element takes it home as well (one
//! shared `Arc`, `frame::keep_material`). That is what lets a copy read the
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
            // A pyramid is the material this element holds, whether or not a
            // slot also draws it: the `Arc` is the one a slot was filled with,
            // and a read of the source (a copy) is answered out of it.
            Loaded::Peaks(peaks) => {
                source.body = Some(peaks);
                true
            }
            Loaded::Samples(samples) => {
                source.samples = samples;
                self.refresh_analysis();
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
}
