//! **What a live view keeps between ticks**, and the one tick that advances it.
//!
//! A signal element over a **bus** is fed forward-only: the source has a
//! present and no past, so whatever a view shows over time is something *it*
//! accumulated — a rolling trace's history, an oscilloscope's triggered window,
//! a spectrum's smoothed curves, a waterfall's rolling transform. All four used
//! to live in the front, in four maps keyed by widget id, filled by four walks
//! of the tree that each matched on the kind. They are one struct here, and one
//! [`SignalElement::tick`].
//!
//! **The rate is the reason this is not the draw.** A tick runs at the front's
//! steady animation rate, so a scope scrolls at a speed the reader can read
//! whatever the window's repaint rate happens to be — a window that repaints
//! twice must not scroll twice, and one that repaints never must not lose its
//! history. The draw that follows only ever draws what the tick left.
//!
//! The one thing that stays outside is the **retained history of a bus**
//! ([`super::super::live::BusHistory`]), because it is the bus's and not the drawing's: one per bus
//! however many views watch it, filled once per tick and read here through
//! [`Live::history`].

use std::collections::VecDeque;
use std::fmt;

use clausters_core::oscil;

use super::super::live::TapWindow;
use super::super::spectrum::SpectrumState;
use super::super::waterfall::Waterfall;
use super::super::widget::element::Live;
use super::{Presentation, SignalElement};

/// Most recent control-bus samples a rolling trace keeps and plots.
pub(crate) const SCOPE_HISTORY: usize = 512;

/// What a live presentation accumulates from a forward-only source. Empty for a
/// view over stored samples, which has its past already.
#[derive(Clone, Default)]
pub struct LiveState {
    /// A control-rate trace's rolling history, oldest first.
    pub history: VecDeque<f32>,
    /// The triggered multichannel window an audio-rate trace draws, or a
    /// phasescope's interleaved `[l, r, l, r, …]` pairs.
    pub window: TapWindow,
    /// The persistent analysis of a live spectrum, one state per channel.
    pub spectra: Vec<SpectrumState>,
    /// The rolling time-frequency transform of a retained waterfall.
    pub roll: Option<Waterfall>,
}

impl fmt::Debug for LiveState {
    /// The accumulated windows are hundreds of samples apiece and say nothing a
    /// reader of a `{:?}` tree wants: the sizes are the state.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LiveState")
            .field("history", &self.history.len())
            .field("window", &self.window.frames())
            .field("spectra", &self.spectra.len())
            .field("roll", &self.roll.is_some())
            .finish()
    }
}

impl SignalElement {
    /// One tick of whatever this element accumulates. A view over stored
    /// samples does nothing here — its past is the data it was given.
    pub fn tick(&mut self, live: &Live) {
        let Some(bus) = self.source.bus().cloned() else {
            return;
        };
        match self.presentation {
            // A control-rate trace takes one sample per tick; an audio-rate one
            // re-triggers a whole window out of the tap.
            Presentation::Signal if !bus.rate.is_audio() => {
                push_sample(&mut self.live.history, live.control(bus.bus));
            }
            Presentation::Signal => self.tick_window(live, &bus),
            Presentation::Phase => self.tick_phase(live, &bus),
            Presentation::Spectrum => self.tick_spectrum(live, &bus),
            Presentation::TimeFrequency => self.tick_roll(live, &bus),
        }
    }

    /// The seconds of its bus this element wants kept addressable, `0.0` for
    /// one that reads only the present.
    pub fn retention(&self) -> f32 {
        self.source.bus().map_or(0.0, |bus| bus.retention.max(0.0))
    }

    /// The window one tick reads out of each of this element's taps, in frames
    /// — the sizing half of what [`tick`](Self::tick) then reads, stated where
    /// the read itself is written so the two cannot drift apart.
    ///
    /// A retained view answers `0`: it reads its bus's *history*, which the
    /// retention span sizes ([`Needs::retention`]), not a window of its own.
    ///
    /// [`Needs::retention`]: super::super::widget::Needs::retention
    pub fn tap_frames(&self, sample_rate: f64) -> usize {
        let Some(bus) = self.source.bus() else {
            return 0;
        };
        match self.presentation {
            // A control-rate trace is read as a bus value, one number per tick.
            Presentation::Signal if !bus.rate.is_audio() => 0,
            // The trigger searches past the display window, so the raw read is
            // wider than what is drawn.
            Presentation::Signal => {
                oscil::raw_frames(oscil::display_frames(bus.window_ms, sample_rate))
            }
            Presentation::Phase => oscil::display_frames(bus.window_ms, sample_rate),
            Presentation::Spectrum => self.spectral.fft_size,
            Presentation::TimeFrequency => 0,
        }
    }

    /// The audio-rate trace's triggered window: the trigger is searched in the
    /// **first** channel and the found alignment applied to every channel, so
    /// the channels keep their true relative phase. A `hold` trace keeps its
    /// last window; one whose first channel has no data yet is left alone
    /// (later channels with no data draw silence, so a short run does not blank
    /// the whole view).
    fn tick_window(&mut self, live: &Live, bus: &super::Bus) {
        if bus.hold {
            return;
        }
        let display = oscil::display_frames(bus.window_ms, live.rate());
        let mut raw = vec![0.0f32; oscil::raw_frames(display)];
        if !live.read(bus.bus, &mut raw) {
            return;
        }
        let (start, locked) = oscil::align(&raw, display, bus.trigger);
        let end = (start + display).min(raw.len());
        let mut chans: Vec<Vec<f32>> = vec![raw[start..end].to_vec()];
        for k in 1..bus.channels {
            raw.fill(0.0);
            let _ = live.read(bus.bus + k as i32, &mut raw);
            chans.push(raw[start..end].to_vec());
        }
        let frames = end - start;
        let mut samples = Vec::with_capacity(frames * bus.channels);
        for f in 0..frames {
            for ch in &chans {
                samples.push(ch[f]);
            }
        }
        self.live.window = TapWindow {
            samples,
            channels: bus.channels,
            locked,
        };
    }

    /// The goniometer's window: a bus and the one beside it, interleaved. No
    /// trigger — the phase view shows the freshest pairs directly.
    fn tick_phase(&mut self, live: &Live, bus: &super::Bus) {
        if bus.hold {
            return;
        }
        let n = oscil::display_frames(bus.window_ms, live.rate());
        let (mut l, mut r) = (vec![0.0f32; n], vec![0.0f32; n]);
        if !live.read(bus.bus, &mut l) || !live.read(bus.bus + 1, &mut r) {
            return;
        }
        let mut samples = Vec::with_capacity(n * 2);
        for i in 0..n {
            samples.push(l[i]);
            samples.push(r[i]);
        }
        self.live.window = TapWindow {
            samples,
            channels: 2,
            locked: false,
        };
    }

    /// One FFT window per channel, folded into the persistent analysis (the
    /// smoothed and peak-hold curves). A channel with no data yet keeps the
    /// curve it had, so a gap does not flash the display to the floor.
    fn tick_spectrum(&mut self, live: &Live, bus: &super::Bus) {
        let (size, averaging, peak_hold) = (
            self.spectral.fft_size,
            self.spectral.averaging,
            self.spectral.peak_hold,
        );
        self.live
            .spectra
            .resize_with(bus.channels, || SpectrumState::new(size));
        let mut raw = Vec::new();
        for (k, state) in self.live.spectra.iter_mut().enumerate() {
            state.ensure_size(size);
            raw.resize(state.window_len(), 0.0);
            if live.read(bus.bus + k as i32, &mut raw) {
                state.update(&raw, averaging, peak_hold);
            }
        }
    }

    /// The retained waterfall: the columns the bus's history has grown since
    /// the last tick. The **history is the bus's and the transform is the
    /// view's** — two views of one bus may analyze it at different sizes — and
    /// this is where the two meet.
    fn tick_roll(&mut self, live: &Live, bus: &super::Bus) {
        if bus.retention <= 0.0 {
            self.live.roll = None;
            return;
        }
        let rate = live.rate() as f32;
        let (window_size, hop) = (self.spectral.fft_size, self.spectral.hop.max(1));
        // The span in columns: the seconds the axis declared, at this
        // transform's own hop. The texture's cap applies on top.
        let columns = ((bus.retention as f64 * rate as f64) / hop as f64).ceil() as usize;
        let roll = match self.live.roll.take() {
            // A `/gui_set` of the analysis restarts the roll: the columns of one
            // transform are not the columns of another.
            Some(roll) if roll.matches(window_size, hop, rate) => roll,
            _ => Waterfall::new(window_size, hop, rate, columns),
        };
        let roll = self.live.roll.insert(roll);
        roll.set_capacity(columns);
        if let Some(history) = live.history(bus.bus)
            && let Some(end) = history.end()
        {
            roll.advance(history.samples(), end);
        }
    }
}

/// Pushes one sample into a rolling history, capped at [`SCOPE_HISTORY`].
fn push_sample(history: &mut VecDeque<f32>, value: f32) {
    history.push_back(value);
    while history.len() > SCOPE_HISTORY {
        history.pop_front();
    }
}

#[cfg(test)]
mod tests {
    //! Driven the way the front drives it: build a tree, hand it a source, and
    //! tick — because the tick *is* what these accumulate through. The source
    //! is a `BusSource` rather than a closure, so a test reads its data through
    //! the same door the segment and the stream do.

    use std::collections::HashMap;

    use super::*;
    use crate::host::BusSource;
    use crate::host::guidef::GuiNode;
    use crate::host::live::tick_tree;
    use crate::host::widget::Widget;

    /// A source that answers control buses with their index and fills a tap
    /// window from `fill`, so what a view accumulated says where it read.
    struct Source<F: Fn(i32, &mut [f32])> {
        offset: f32,
        fill: F,
    }

    impl<F: Fn(i32, &mut [f32]) + Send + Sync> BusSource for Source<F> {
        fn control(&self, index: usize) -> f32 {
            index as f32 + self.offset
        }

        fn read_bus(&self, bus: i32, out: &mut [f32]) -> bool {
            (self.fill)(bus, out);
            true
        }
    }

    fn tree(json: &str) -> Widget {
        Widget::from_node(1, &GuiNode::parse(json.as_bytes()).unwrap(), &[]).unwrap()
    }

    /// The live state of the one signal element in `tree`.
    fn state(tree: &Widget, id: i32) -> &LiveState {
        &tree.find(id).unwrap().signal().expect("a signal").live
    }

    fn tick(tree: &mut Widget, source: &dyn BusSource) {
        let histories = HashMap::new();
        tick_tree(
            tree,
            &Live {
                bus: Some(source),
                sample_rate: 48_000.0,
                histories: &histories,
            },
        );
    }

    /// A control-rate trace takes **one sample per tick** and keeps the newest
    /// window of them — which is what makes the scroll time-based rather than
    /// repaint-based.
    #[test]
    fn a_control_trace_advances_one_sample_a_tick_and_caps() {
        let mut w = tree(
            r#"{"type":"window","children":[{"id":2,"type":"signal","view":"trace","bus":3,"rate":"control"}]}"#,
        );
        for i in 0..(SCOPE_HISTORY + 10) {
            tick(
                &mut w,
                &Source {
                    offset: i as f32,
                    fill: |_, _: &mut [f32]| {},
                },
            );
        }
        let history = &state(&w, 2).history;
        assert_eq!(history.len(), SCOPE_HISTORY, "history is capped");
        // Oldest samples fell off the front; the newest is the last push.
        assert_eq!(
            *history.back().unwrap(),
            3.0 + (SCOPE_HISTORY + 9) as f32,
            "newest sample read from the trace's own bus"
        );
    }

    /// The trigger is searched in the first channel and the alignment applied
    /// to every channel, so a multichannel window keeps its true relative
    /// phase.
    #[test]
    fn a_tap_window_interleaves_channels_aligned_on_the_first() {
        let mut w = tree(
            r#"{"type":"window","children":[
                {"id":7,"type":"signal","view":"trace","bus":0,"channels":2,"window_ms":1.0}]}"#,
        );
        // Channel 0 rises through zero at a known index; channel 1 counts, so
        // the alignment applied to it is directly observable.
        tick(
            &mut w,
            &Source {
                offset: 0.0,
                fill: |tap: i32, out: &mut [f32]| {
                    for (i, s) in out.iter_mut().enumerate() {
                        *s = if tap == 0 {
                            if i % 24 < 12 { -1.0 } else { 1.0 }
                        } else {
                            i as f32
                        };
                    }
                },
            },
        );
        let win = &state(&w, 7).window;
        assert_eq!(win.channels, 2);
        assert!(win.locked, "a periodic square locks");
        let frames = win.frames();
        assert!(frames >= 16);
        assert_eq!(win.samples.len(), frames * 2);
        assert_eq!(win.samples[0], 1.0, "starts at the rising edge");
        let ch1_start = win.samples[1];
        assert_eq!(win.samples[3], ch1_start + 1.0, "channel 1 is consecutive");
        assert_eq!(ch1_start % 24.0, 12.0, "aligned to channel 0's crossing");
    }

    /// One analysis state per channel, each folding its **own** tap: the
    /// channels of one spectrum are separate measurements.
    #[test]
    fn a_spectrum_keeps_one_state_per_channel() {
        let mut w = tree(
            r#"{"type":"window","children":[
                {"id":9,"type":"signal","view":"spectrum","bus":2,"channels":2,"fft_size":256}]}"#,
        );
        // Bus 2 carries a tone, bus 3 silence: the two channel states diverge.
        tick(
            &mut w,
            &Source {
                offset: 0.0,
                fill: |bus: i32, out: &mut [f32]| {
                    for (i, s) in out.iter_mut().enumerate() {
                        *s = if bus == 2 {
                            (std::f32::consts::TAU * i as f32 / 8.0).sin()
                        } else {
                            0.0
                        };
                    }
                },
            },
        );
        let chans = &state(&w, 9).spectra;
        assert_eq!(chans.len(), 2);
        let peak = |s: &SpectrumState| s.avg_db.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        assert!(peak(&chans[0]) > -6.0, "the tone channel peaks near 0 dB");
        assert!(peak(&chans[1]) < -60.0, "the silent channel stays down");
    }

    /// **Each tick brings the newest window of both buses**, and the pairs are
    /// interleaved in read order — the property a jittering goniometer would
    /// break, since a figure drawn from a left window of one tick and a right
    /// window of another is not a phase relationship at all.
    #[test]
    fn a_phase_window_pairs_the_two_buses_of_the_same_tick() {
        let mut w = tree(
            r#"{"type":"window","children":[
                {"id":5,"type":"signal","view":"phase","bus":0,"window_ms":1.0}]}"#,
        );
        // Left counts up from the tick number, right down from it: a stale
        // half would show as a pair that does not belong to one tick.
        for tick_no in 0..3 {
            tick(
                &mut w,
                &Source {
                    offset: 0.0,
                    fill: move |bus: i32, out: &mut [f32]| {
                        for (i, s) in out.iter_mut().enumerate() {
                            *s = if bus == 0 {
                                tick_no as f32 + i as f32
                            } else {
                                -(tick_no as f32) - i as f32
                            };
                        }
                    },
                },
            );
            let win = &state(&w, 5).window;
            assert_eq!(win.channels, 2);
            let frames = win.frames();
            assert!(frames > 0);
            for f in 0..frames {
                assert_eq!(win.samples[2 * f], tick_no as f32 + f as f32);
                assert_eq!(win.samples[2 * f + 1], -(tick_no as f32) - f as f32);
            }
        }
    }

    /// A frozen trace keeps the window it had: `hold` is what makes a
    /// measurement readable.
    #[test]
    fn a_held_trace_keeps_its_window() {
        let mut w = tree(
            r#"{"type":"window","children":[
                {"id":3,"type":"signal","view":"trace","bus":0,"window_ms":1.0}]}"#,
        );
        let ones = Source {
            offset: 0.0,
            fill: |_: i32, out: &mut [f32]| out.fill(1.0),
        };
        tick(&mut w, &ones);
        let held = state(&w, 3).window.samples.clone();
        assert!(!held.is_empty());
        // Freeze it, then feed something else entirely.
        let widget = w.find_mut(3).unwrap();
        assert!(crate::host::widget::apply_widget(
            widget,
            "hold",
            &serde_json::Value::from(true)
        ));
        tick(
            &mut w,
            &Source {
                offset: 0.0,
                fill: |_: i32, out: &mut [f32]| out.fill(-1.0),
            },
        );
        assert_eq!(state(&w, 3).window.samples, held, "a held trace is frozen");
    }
}
