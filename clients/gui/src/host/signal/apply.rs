//! **What a `/gui_set` means to a signal element.**
//!
//! The keys are grouped the way the model is — the source, the value axis, the
//! spectral parameters, the chrome — so a key lands wherever it means
//! something, whatever the element's wire name was: a `window_size` on a
//! spectrogram and an `fft_size` on a spectrum are one field, and a key a
//! presentation does not read is simply not one of its props.
//!
//! The two **mutation points** are here for the same reason: a set that
//! touches what the cached analysis reads re-runs it, and one that touches
//! what a claimed slot is built from marks the slot dirty. Both are properties
//! of *which prop moved*, which is knowable nowhere else.

use serde_json::Value;

use crate::spectrogram::FreqScale;

use super::super::widget::parse::{
    freq_scale_from_str, set_f, set_label, set_opt_f, set_rate, truthy,
};
use super::{Presentation, SignalElement, Source};

impl SignalElement {
    /// Applies one `/gui_set` key/value to this element.
    pub(crate) fn apply(&mut self, key: &str, v: &Value) -> bool {
        let handled = match key {
            // The source.
            "bus" => match self.source.bus_mut() {
                Some(b) => v.as_i64().map(|n| b.bus = n as i32).is_some(),
                None => false,
            },
            "rate" => match self.source.bus_mut() {
                Some(b) => set_rate(&mut b.rate, v),
                None => false,
            },
            "window_ms" => match self.source.bus_mut() {
                Some(b) => set_f(&mut b.window_ms, v),
                None => false,
            },
            "trigger" => match self.source.bus_mut() {
                Some(b) => set_f(&mut b.trigger, v),
                None => false,
            },
            "hold" => match self.source.bus_mut() {
                Some(b) => truthy(v).map(|x| b.hold = x).is_some(),
                None => false,
            },
            // The axis's declared span. Clamped at zero rather than refused: a
            // negative retention is "no history", which is the default anyway.
            "retention" => match self.source.bus_mut() {
                Some(b) => {
                    set_f(&mut b.retention, v) && {
                        b.retention = b.retention.max(0.0);
                        true
                    }
                }
                None => false,
            },
            "channels" => match v.as_i64() {
                Some(n) => {
                    let n = (n as usize).max(1);
                    match &mut self.source {
                        Source::Bus(b) => b.channels = n,
                        Source::Data(d) => d.channels = n,
                    }
                    true
                }
                None => false,
            },
            // The presentation, where the element's name reads one.
            "view" => v
                .as_str()
                .and_then(super::super::plot::PlotView::parse)
                .map(|view| {
                    self.presentation = match view {
                        super::super::plot::PlotView::Signal => Presentation::Signal,
                        super::super::plot::PlotView::Spectrum => Presentation::Spectrum,
                    };
                })
                .is_some(),
            // The value axis. Either side also accepts the string `"auto"`, giving
            // it back to the data fit.
            "min" => set_opt_f(&mut self.value.min, v),
            "max" => set_opt_f(&mut self.value.max, v),
            // The spectral parameters. The analysis size answers to both names —
            // the spectral views say `fft_size`, the time-frequency one
            // `window_size` — since one field is behind them.
            "fft_size" | "window_size" => v
                .as_u64()
                .filter(|n| clausters_core::fft::supports(*n as usize))
                .map(|n| self.spectral.fft_size = n as usize)
                .is_some(),
            "db_floor" => set_f(&mut self.spectral.db_floor, v),
            "db_ceil" => set_f(&mut self.spectral.db_ceil, v),
            "freq_scale" => v
                .as_str()
                .and_then(freq_scale_from_str)
                .map(|s| self.spectral.freq_scale = s)
                .is_some(),
            // Legacy boolean alias: 1 -> log, 0 -> linear.
            "log_freq" => truthy(v)
                .map(|b| {
                    self.spectral.freq_scale = if b { FreqScale::Log } else { FreqScale::Linear }
                })
                .is_some(),
            "averaging" => v
                .as_f64()
                .map(|x| self.spectral.averaging = (x as f32).clamp(0.0, 0.99))
                .is_some(),
            "peak_hold" => truthy(v).map(|b| self.spectral.peak_hold = b).is_some(),
            "colormap" => v
                .as_i64()
                .map(|n| self.spectral.colormap = n as i32)
                .is_some(),
            // The chrome.
            "overlay" => truthy(v).map(|b| self.display.overlay = b).is_some(),
            "label" => set_label(&mut self.display.label, v),
            _ => self.editor.apply(key, v),
        };
        // The cached analysis reads the presentation, the size and the rate.
        if handled && matches!(key, "view" | "fft_size" | "window_size" | "sample_rate") {
            self.refresh_analysis();
        }
        // ...and so does what a claimed slot is built from, plus the channel count
        // that splits the samples into lanes. A `/gui_set` of one of these is a
        // mutation point of the fill, which is the only way an already-uploaded
        // picture is rebuilt.
        if handled
            && matches!(
                key,
                "view" | "fft_size" | "window_size" | "sample_rate" | "channels"
            )
        {
            self.slot_dirty = true;
        }
        handled
    }
}
