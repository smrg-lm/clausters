//! **The signal element behind the trait**: the constructor the schema looks
//! up, and the one `impl Element` that gathers every door the rest of this
//! module answers.
//!
//! Nothing new is decided here. Each method is one line of routing to the file
//! that owns the answer — [`bulk`](super::bulk) for what to load and where it
//! lands, [`slot`](super::slot) for what an uploaded picture is fed,
//! [`live`](super::live) for what a tick accumulates and how wide a read has to
//! be, [`chrome`](super::chrome) for the band left of a shared axis,
//! [`freq`](super::freq) for the one axis this element measures itself,
//! [`body`](super::body) for what it is inside a clip — which is what makes
//! this file the *shape* of the seam rather than a place logic hides in.
//!
//! **What is drawn here and what is not.** The four presentations that go into
//! the window's one mesh draw in [`Element::draw`]; the two that cannot claim a
//! slot and describe their frame in [`Element::slot`], because a trace
//! decimated per pixel and a texture sampled per texel are pipelines the window
//! owns. The chrome *around* a heavy view — the rulers, the playhead, the
//! selection, the readout — stays the frame's: it is shared with the lane, the
//! roll and the free-standing ruler, and belongs to the **axis** rather than to
//! whatever is drawn on it.

use serde_json::{Map, Value};

use super::{Presentation, SignalElement, Source};
use crate::host::graphics::meters;
use crate::host::graphics::signal::{phasescope, plot, spectrum};
use crate::host::layout::Rect;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::widget::element::BodyRole;
use crate::host::widget::element::{
    Ctx, Element, FreqAxis, Input, Live, Loaded, Needs, SampleBlock, SlotFill, SlotFrame,
    TextureLook, ValueAxis,
};
use crate::host::widget::{EditorProps, GestureMap};

/// Builds the element from a `signal` node's props — every view of a signal,
/// in one constructor, because there is one element: the props say which point
/// of the product ([`super::point`]) and the rest are read over that base.
pub(crate) fn build(
    props: &Map<String, Value>,
    blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(crate::host::widget::signal_element(props, blobs)?))
}

impl Element for SignalElement {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        self.apply(key, v)
    }

    /// The four presentations the shared mesh can carry. The two navigable
    /// heavy ones draw nothing here — their picture is the slot's — and a
    /// stored view of a live source has nothing to draw until it has data.
    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        let rect = ctx.rect;
        // **Placed on somebody else's axis** (a clip's take): the picture is
        // the trace against *that* axis and its span, with no chrome of its
        // own — never the plot's, which spans its own samples and rules its
        // own axes. The container is what says where in time this sits, so
        // this fork comes before the presentations.
        if let Some(time) = ctx.time
            && matches!(self.source, Source::Data(_))
            && !self.caps.navigable
        {
            SignalElement::draw_body(self, d, rect, &time.view, time.span);
            return;
        }
        match (self.presentation, &self.source) {
            // The navigable heavy views: the slot draws them.
            (Presentation::Signal | Presentation::TimeFrequency, _) if self.caps.navigable => {}
            // A forward-only trace: the audio-rate window the tick aligned, or
            // the control bus's rolling history.
            (Presentation::Signal, Source::Bus(bus)) => {
                if bus.rate.is_audio() {
                    meters::draw_wave(
                        d,
                        rect,
                        &meters::WaveParams {
                            window: &self.live.window,
                            min: self.domain().0,
                            max: self.domain().1,
                            window_ms: bus.window_ms,
                            trigger: bus.trigger,
                            overlay: self.display.overlay,
                            ruler: self.editor.ruler != crate::host::widget::Ruler::Off,
                            ruler_y: self.editor.ruler_y != crate::host::widget::RulerY::Off,
                            label: self.display.label.as_deref(),
                            measures: self.measures,
                        },
                    );
                } else {
                    let samples: Vec<f32> = self.live.history.iter().copied().collect();
                    meters::draw_scope(
                        d,
                        rect,
                        &samples,
                        self.domain().0,
                        self.domain().1,
                        self.display.label.as_deref(),
                        self.measures,
                    );
                }
            }
            (Presentation::Phase, _) => phasescope::draw_phasescope(
                d,
                rect,
                self.live.window.samples.as_slice(),
                self.display.label.as_deref(),
            ),
            (Presentation::Spectrum, Source::Bus(_)) => spectrum::draw_spectrum(
                d,
                rect,
                &self.live.spectra,
                &spectrum::SpectrumParams {
                    sample_rate: ctx.world.sample_rate,
                    fft_size: self.spectral.fft_size,
                    db_floor: self.spectral.db_floor,
                    db_ceil: self.spectral.db_ceil,
                    freq_scale: self.spectral.freq_scale,
                    peak_hold: self.spectral.peak_hold,
                    ruler: self.editor.ruler != crate::host::widget::Ruler::Off,
                    ruler_y: self.editor.ruler_y != crate::host::widget::RulerY::Off,
                    // The window the axis can show, not the one that was asked
                    // for: the floor of the analysis is part of the drawing.
                    x_view: self.freq_window(ctx.world.sample_rate),
                    label: self.display.label.as_deref(),
                },
            ),
            // A stored signal nobody navigates: the mesh plot, whichever of the
            // two presentations it shows.
            (_, Source::Data(_)) => plot::draw(d, rect, &self.plot_params(ctx.world.sample_rate)),
            // A live source with no live renderer for its presentation (a
            // stored presentation over a bus): nothing to draw until it has one.
            (_, Source::Bus(_)) => {}
        }
    }

    /// The hover readout of a **stored** view: a hairline and the value under
    /// the pointer. It rides the overlay mesh for the same reason a timeline
    /// view's chrome does — it reads over the picture, not into it.
    fn overlay(&self, d: &mut Draw, ctx: &Ctx) {
        if self.caps.navigable || !matches!(self.source, Source::Data(_)) {
            return;
        }
        if let Some(cursor) = ctx.world.cursor {
            plot::draw_readout(
                d,
                ctx.rect,
                &self.plot_params(ctx.world.sample_rate),
                cursor,
            );
        }
    }

    fn needs(&self) -> Needs {
        let mut needs = Needs {
            retention: self.retention(),
            bulk: self.want(),
            slot: self.slot_kind(),
            ..Default::default()
        };
        // A control-rate trace reads a bus **value**; every other live view
        // reads recorded samples out of a tap.
        if let Source::Bus(bus) = &self.source {
            match self.presentation {
                // The phase view is a stereo pair by construction: a bus and
                // the one beside it, whatever `channels` says.
                Presentation::Phase => needs.taps.extend([bus.bus, bus.bus + 1]),
                Presentation::Signal if !bus.rate.is_audio() => needs.buses.push(bus.bus),
                _ => needs
                    .taps
                    .extend((0..bus.channels as i32).map(|k| bus.bus + k)),
            }
        }
        needs
    }

    fn tap_frames(&self, sample_rate: f64) -> usize {
        SignalElement::tap_frames(self, sample_rate)
    }

    fn tick(&mut self, live: &Live) {
        SignalElement::tick(self, live);
    }

    fn bulk(&mut self, data: Loaded) -> bool {
        self.take(data)
    }

    fn material_shape(&self) -> Option<(usize, u64)> {
        SignalElement::material_shape(self)
    }

    fn material_buffer(&self) -> Option<i32> {
        SignalElement::material_buffer(self)
    }

    fn write_samples(&mut self, ch: usize, start: u64, values: &[f32]) -> bool {
        SignalElement::write_samples(self, ch, start, values)
    }

    fn fill(&mut self) -> Option<SlotFill> {
        SignalElement::fill(self)
    }

    fn slot_dropped(&mut self) {
        self.slot_dirty = true;
    }

    /// What a claimed slot draws this frame. The horizontal window is not here:
    /// it is the **navigation group's**, which is addressed by an id this
    /// element does not carry, and the lane count is the slot's own.
    fn slot(&self, ctx: &Ctx) -> Option<SlotFrame> {
        if !self.caps.navigable {
            return None;
        }
        let body =
            crate::host::frame::timeline_body(ctx.rect, &self.editor, ctx.indent, ctx.metrics);
        match self.presentation {
            Presentation::Signal => Some(SlotFrame::Waveform {
                body,
                domain: self.domain(),
                amp: self.editor.y_view(),
                overlay: self.display.overlay,
                measures: self.measures,
            }),
            Presentation::TimeFrequency => Some(SlotFrame::Spectrogram {
                body,
                freq: self.editor.y_view(),
                look: self.look(),
            }),
            _ => None,
        }
    }

    fn body_role(&self) -> Option<BodyRole> {
        Some(BodyRole::Take)
    }

    /// **A take's material is the whole of the box it fills** — samples run
    /// edge to edge, so there is no empty space inside one — and it is claimed
    /// only when the take can actually be edited (`editable`).
    ///
    /// That gate is what keeps a clip draggable: a clip's take is built with
    /// every capability off, so it never takes the press and the background
    /// stays the clip's. A take a script *did* make editable is a different
    /// statement — the material is what the hand is there for, the way it is in
    /// an editor's own view — and the clip is then moved from its grips or by
    /// naming its layer.
    fn layer_hit(&self, at: (f64, f64), input: &Input) -> bool {
        self.caps.editable && input.rect.contains(at.0, at.1)
    }

    fn draw_body(&self, d: &mut Draw, rect: Rect, local: &crate::viewport::View, dur: f64) {
        SignalElement::draw_body(self, d, rect, local, dur);
    }

    fn texture_body(&self) -> Option<TextureLook> {
        SignalElement::texture_body(self)
    }

    fn editor(&self) -> Option<&EditorProps> {
        Some(&self.editor)
    }

    fn editor_mut(&mut self) -> Option<&mut EditorProps> {
        Some(&mut self.editor)
    }

    fn navigates_time(&self) -> bool {
        SignalElement::navigates_time(self)
    }

    fn hover_readout(&self) -> bool {
        !self.is_live()
    }

    fn gutter(&self, m: &Metrics) -> f32 {
        SignalElement::gutter(self, m)
    }

    fn measured_gutter(&self, rect: Rect, m: &Metrics) -> Option<f32> {
        SignalElement::measured_gutter(self, rect, m)
    }

    fn lanes(&self, uploaded: usize) -> usize {
        SignalElement::lanes(self, uploaded)
    }

    fn centres_y_zoom(&self) -> bool {
        SignalElement::centres_y_zoom(self)
    }

    fn navigates_freq(&self) -> bool {
        SignalElement::navigates_freq(self)
    }

    fn freq_axis(&self, rect: Rect, m: &Metrics, sample_rate: f64) -> Option<FreqAxis> {
        SignalElement::freq_axis(self, rect, m, sample_rate)
    }

    fn sample_block(&self, start: u64, frames: u64, server_rate: f64) -> Option<SampleBlock> {
        SignalElement::sample_block(self, start, frames, server_rate)
    }

    fn pending_edit(&self) -> Option<&crate::host::widget::element::PendingEdit> {
        self.pending.as_ref()
    }

    fn set_pending_edit(
        &mut self,
        pending: Option<crate::host::widget::element::PendingEdit>,
    ) -> bool {
        // Only a navigable trace can hold one: it is the presentation where a
        // sample is a thing on screen, and the only one whose material the
        // owner can be asked to change.
        if !self.caps.navigable || self.presentation != Presentation::Signal {
            return false;
        }
        self.pending = pending;
        true
    }

    fn sample_value(&self, channel: usize, frame: usize) -> Option<f32> {
        let data = self.source.data()?;
        let trace = data.trace();
        trace.has_raw().then(|| trace.at(channel, frame as f64))
    }

    fn value_axis(&self, rect: Rect, indent: f32, m: &Metrics, lanes: usize) -> Option<ValueAxis> {
        SignalElement::value_axis(self, rect, indent, m, lanes)
    }

    fn freq_window_of(&self, sample_rate: f64, want: Option<(f64, f64)>) -> Option<(f64, f64)> {
        self.freq_window_shown(sample_rate, want)
    }

    fn freq_min_span(&self, sample_rate: f64, start: f64) -> Option<f64> {
        SignalElement::freq_min_span(self, sample_rate, start)
    }

    fn gesture_map(&self) -> Option<GestureMap> {
        SignalElement::gesture_map(self)
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }
}

impl SignalElement {
    /// The look a texture slot is sampled with — shared by the view and by the
    /// clip body, which differ only in the axis they are drawn against.
    pub(crate) fn look(&self) -> TextureLook {
        TextureLook {
            db_floor: self.spectral.db_floor,
            db_ceil: self.spectral.db_ceil,
            freq_scale: self.spectral.freq_scale,
            colormap: self.spectral.colormap,
        }
    }

    /// The mesh plot's parameters, borrowed out of the element — the drawing
    /// and its hover readout read the same ones, so a hairline lands on the
    /// curve under it. A plot over a bus has no samples: it is the case that
    /// draws nothing at all.
    fn plot_params(&self, sample_rate: f64) -> plot::PlotParams<'_> {
        let (samples, channels): (&[f32], usize) = match &self.source {
            Source::Data(data) => (&data.samples, data.channels),
            Source::Bus(_) => (&[], 1),
        };
        plot::PlotParams {
            samples,
            channels,
            view: if self.presentation == Presentation::Spectrum {
                plot::PlotView::Spectrum
            } else {
                plot::PlotView::Signal
            },
            overlay: self.display.overlay,
            sample_rate: self.editor.sample_rate,
            min: self.value.min,
            max: self.value.max,
            ruler: self.editor.ruler,
            ruler_y: self.editor.ruler_y != crate::host::widget::RulerY::Off,
            spectrum: self.analysis.as_deref(),
            db_floor: self.spectral.db_floor,
            db_ceil: self.spectral.db_ceil,
            freq_scale: self.spectral.freq_scale,
            // What the axis can show, not what was asked of it.
            x_view: self.freq_window(sample_rate),
            label: self.display.label.as_deref(),
            measures: self.measures,
        }
    }
}
