//! **What a `/gui_set` means to a signal element.**
//!
//! The keys are grouped the way the model is -- the source, the value axis, the
//! spectral parameters, the chrome -- so a key lands wherever it means
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

use super::{Presentation, SignalElement, Source};
use crate::host::widget::parse::{
    freq_scale_from_str, set_f, set_label, set_opt_f, set_rate, truthy,
};

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
                .and_then(crate::host::graphics::signal::plot::PlotView::parse)
                .map(|view| {
                    self.presentation = match view {
                        crate::host::graphics::signal::plot::PlotView::Signal => {
                            Presentation::Signal
                        }
                        crate::host::graphics::signal::plot::PlotView::Spectrum => {
                            Presentation::Spectrum
                        }
                    };
                })
                .is_some(),
            // The value axis. Either side also accepts the string `"auto"`, giving
            // it back to the data fit.
            "min" => set_opt_f(&mut self.value.min, v),
            "max" => set_opt_f(&mut self.value.max, v),
            // The spectral parameters. The analysis size answers to both names --
            // the spectral views say `fft_size`, the time-frequency one
            // `window_size` -- since one field is behind them.
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
            // The loudness layer's own scale and guides -- the parameters of a
            // measure live with the measure, on the element.
            "loudness_target" => v.as_f64().map(|x| self.loudness.target = x).is_some(),
            // Tech 3341 specifies two scales and names them by their top; a
            // number between them is taken at its word, so a script can draw a
            // strip of its own.
            "loudness_scale" => v
                .as_f64()
                .filter(|x| *x > 0.0)
                .map(|x| self.loudness.scale = x)
                .is_some(),
            "loudness_ruler" => truthy(v).map(|b| self.loudness.ruler = b).is_some(),
            "loudness_stats" => truthy(v).map(|b| self.loudness.stats = b).is_some(),
            // The chrome.
            "overlay" => truthy(v).map(|b| self.display.overlay = b).is_some(),
            // **Live, and both ways.** A client arms it when it starts
            // recording into the buffer and clears it when the take is
            // finished -- and clearing it is what turns the picture back into
            // the whole of the samples, which is right: what was written is
            // now all there is, and a frontier stopped at the buffer's end
            // would have said the same thing only by accident.
            "fills" => truthy(v)
                .map(|b| {
                    self.fills = b;
                    if !b {
                        self.written = 0;
                    }
                })
                .is_some(),
            // Live, because a picture is read by turning its measures on and
            // off: the same element shows peaks, level, or both between two
            // frames, with nothing rebuilt.
            // Live, because a picture is read by turning its layers on and
            // off: the same element shows the envelope, the level, a texture
            // or all three between two frames, with nothing rebuilt.
            //
            // **A stack whose axis is claimed twice is refused**, the way the
            // def that carried one is: the set leaves the picture as it was
            // and says why, rather than drawing one of the two layers on a
            // scale that is not its own.
            "layers" | "measure" => match super::Stack::parse(v) {
                Some(stack) => match stack.axis_domain() {
                    Ok(_) => {
                        self.layers = stack;
                        true
                    }
                    Err(why) => {
                        crate::host::diag::warn!("gui_set {key}: {why}");
                        false
                    }
                },
                None => false,
            },
            // **The samples, live.** An owner that applied an edit pushes the
            // samples that now hold, and the picture becomes the document's
            // again -- which is what "the acknowledgement corrects the picture"
            // means for an inline source, and what lets a pending drawing be
            // dropped without the edit disappearing with it. Only inline
            // samples: a mapped file or cache is re-read by remapping it, which
            // is a different door and is not this one.
            // The def's word for *the samples you already have*. It says
            // nothing new to a live widget -- the samples it names are the ones
            // this element is holding -- so it is accepted and does nothing.
            // Accepted rather than refused because it is the same props map a
            // def carries, and a client that reuses one for a set should not
            // have to strip a word that means "no change" out of it.
            "data" if v.as_str() == Some(crate::host::widget::parse::KEEP) => true,
            "data" => match (v.as_array(), &mut self.source) {
                // Inline only: a source that names a file, a cache or a server
                // buffer is re-read by resolving that resource again, and
                // pushing samples at one would leave the picture half from each.
                (Some(items), Source::Data(data))
                    if data.path.is_none() && data.cache.is_none() && data.buffer.is_none() =>
                {
                    match items
                        .iter()
                        .map(|x| x.as_f64().map(|f| f as f32))
                        .collect::<Option<Vec<f32>>>()
                    {
                        Some(samples) => {
                            data.samples = samples.into();
                            data.body = None; // the inline samples *are* the body
                            true
                        }
                        None => false,
                    }
                }
                _ => false,
            },
            // **The samples changed where they live; read them again.** Bulk
            // resolution is idempotent by design -- a resolved source stops
            // asking -- so re-reading is the element *forgetting* what it
            // resolved, and the loader picking it up on the next pass. One door
            // for every form: a mapped file, a peaks cache, a server buffer.
            //
            // It is the mapped sibling of a `/gui_set data`: an owner that
            // applied an edit says either *the samples are now these* (inline)
            // or *it is where it always was, and it moved* (mapped). A source
            // with nothing behind it ignores this rather than erasing itself.
            "reload" => truthy(v)
                .map(|yes| {
                    if yes {
                        self.reread();
                    }
                })
                .is_some(),
            "label" => set_label(&mut self.display.label, v),
            _ => self.editor.apply(key, v),
        };
        // The cached analysis reads the presentation, the size and the rate.
        if handled && matches!(key, "view" | "fft_size" | "window_size" | "sample_rate") {
            self.refresh_analysis();
        }
        // **The loudness layer's own mutation points.** Measuring is a pass
        // over the samples, so it happens where what it reads moved: the
        // measures themselves (a layer turned on), the rate a window is
        // counted in, and the samples. The *span* is cheaper and moves far more
        // often -- a hand sweeping a selection -- so it is its own point.
        if handled
            && matches!(
                key,
                "layers" | "measure" | "sample_rate" | "channels" | "data" | "reload"
            )
        {
            self.refresh_loudness();
        } else if handled && matches!(key, "sel_start" | "sel_len") {
            self.refresh_loudness_summary();
        }
        // ...and so does what a claimed slot is built from, plus the channel count
        // that splits the samples into lanes. A `/gui_set` of one of these is a
        // mutation point of the fill, which is the only way an already-uploaded
        // picture is rebuilt.
        if handled
            && matches!(
                key,
                "view"
                    | "fft_size"
                    | "window_size"
                    | "sample_rate"
                    | "channels"
                    | "data"
                    | "reload"
            )
            // ...except a `data` that changed nothing: an already-uploaded
            // picture of the same samples is the picture that holds.
            && !(key == "data" && v.as_str() == Some(crate::host::widget::parse::KEEP))
        {
            self.slot_dirty = true;
        }
        handled
    }
}

#[cfg(test)]
mod data_tests {
    use crate::host::widget::signal_element;

    fn el(json: &str) -> crate::host::elements::signal::SignalElement {
        signal_element(
            &serde_json::from_str::<serde_json::Value>(json)
                .unwrap()
                .as_object()
                .unwrap()
                .clone(),
            &[],
        )
        .unwrap()
    }

    /// The owner's half of an edit: the samples that now hold are pushed, and
    /// the picture is the document's again. Without this the pending drawing
    /// could never be dropped -- letting go of it would take the edit with it.
    #[test]
    fn inline_samples_can_be_replaced_live() {
        use crate::host::widget::element::Element;
        let mut e = el(r#"{"id":1,"type":"signal","view":"trace","data":[0.0,1.0,-1.0,0.5]}"#);
        assert!(e.set("data", &serde_json::json!([0.0, 0.25, -0.25, 0.5])));
        let crate::host::elements::signal::Source::Data(d) = &e.source else {
            panic!("still an inline source")
        };
        assert_eq!(&d.samples[..], &[0.0, 0.25, -0.25, 0.5]);
        assert!(e.slot_dirty, "the picture is rebuilt from it");
    }

    /// The mapped half: an owner says *it moved where it lives*, and the
    /// element forgets what it resolved so the loader asks again. This is the
    /// door the grabbable sample turned out to need and did not have.
    #[test]
    fn a_mapped_source_can_be_told_to_read_itself_again() {
        use crate::host::widget::element::Element;
        let mut e = el(r#"{"id":1,"type":"signal","view":"trace","path":"take.f32","bulk":true}"#);
        assert!(e.needs().bulk.is_some(), "it asks on the way up");
        // Pretend a loader answered.
        let crate::host::elements::signal::Source::Data(d) = &mut e.source else {
            panic!("a data source")
        };
        d.body = Some(std::sync::Arc::new(
            crate::waveform::WaveformData::from_interleaved(&[0.0, 1.0, -1.0, 0.5], 1, 2),
        ));
        assert!(e.needs().bulk.is_none(), "and stops once it has it");
        assert!(e.set("reload", &serde_json::json!(1)));
        assert!(e.needs().bulk.is_some(), "told to, it asks again");
        assert!(
            e.slot_dirty,
            "and the picture is rebuilt from what comes back"
        );
    }

    /// **And the ask is one-shot**, which is what makes a front able to run the
    /// pass every frame.
    ///
    /// The defect this pins (found 2026-09-07, on `editors/edit_samples`): a
    /// take backed by a server buffer, told to `reload` by an undo, forgot its
    /// body and *nothing ever asked for it again* -- the bulk walk ran once,
    /// when the window opened. The picture stayed on the card (a fill with no
    /// data returns `None` and leaves the slot alone), so the window looked
    /// right over an element holding nothing, and every read that goes through
    /// the element -- `sample_value`, and so the pencil -- was dead from then
    /// on. Deriving the ask from "it has no body" instead would have re-asked
    /// every frame for as long as the fetch took.
    #[test]
    fn a_reload_asks_once_and_the_ask_clears() {
        use crate::host::widget::element::{Element, Samples};
        let mut e =
            el(r#"{"id":1,"type":"signal","view":"trace","buffer":7,"bulk":true,"channels":1}"#);
        assert!(
            e.wants_reload().is_none(),
            "an element nobody told anything asks for nothing"
        );
        assert!(e.set("reload", &serde_json::json!(1)));
        let asked = e.wants_reload().expect("told to, it asks");
        assert!(
            matches!(asked, crate::host::widget::element::Bulk::Buffer(7)),
            "and it asks for the buffer it draws: {asked:?}"
        );
        assert!(
            e.wants_reload().is_none(),
            "once -- a front's per-frame pass must not send a query per frame"
        );
    }

    /// A source with nothing behind it is left alone: reloading it would erase
    /// the samples rather than refresh it.
    #[test]
    fn an_inline_source_ignores_a_reload() {
        use crate::host::widget::element::Element;
        let mut e = el(r#"{"id":1,"type":"signal","view":"trace","data":[0.0,1.0,-1.0,0.5]}"#);
        assert!(e.set("reload", &serde_json::json!(1)));
        let crate::host::elements::signal::Source::Data(d) = &e.source else {
            panic!("a data source")
        };
        assert_eq!(&d.samples[..], &[0.0, 1.0, -1.0, 0.5], "still there");
    }

    /// The def's `keep` is a word about a **redraw**, so on a live widget it
    /// says nothing new. It is accepted rather than refused because it is the
    /// same props map a def carries, and a client should not have to strip a
    /// word meaning "no change" out of one to reuse it.
    #[test]
    fn a_set_of_keep_changes_nothing_and_is_not_refused() {
        use crate::host::widget::element::Element;
        let mut e = el(r#"{"id":1,"type":"signal","view":"trace","data":[0.0,1.0,-1.0,0.5]}"#);
        e.slot_dirty = false;
        assert!(e.set("data", &serde_json::json!("keep")));
        let crate::host::elements::signal::Source::Data(d) = &e.source else {
            panic!("a data source")
        };
        assert_eq!(&d.samples[..], &[0.0, 1.0, -1.0, 0.5]);
        assert!(
            !e.slot_dirty,
            "and the uploaded picture is still the picture"
        );
    }

    /// A mapped resource is re-read by remapping it, which is a different door:
    /// pushing samples at one would half-replace what it draws.
    #[test]
    fn a_mapped_source_does_not_take_samples() {
        use crate::host::widget::element::Element;
        let mut e = el(r#"{"id":1,"type":"signal","view":"trace","path":"take.f32"}"#);
        assert!(!e.set("data", &serde_json::json!([0.0, 1.0])));
    }
}
