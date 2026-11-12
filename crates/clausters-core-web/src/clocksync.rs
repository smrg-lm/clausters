//! The sample-clock model.
//!
//! How a client paced by its own monotonic clock still schedules on a remote
//! server's sample axis: `/clock_query` replies are fed in as anchors, and the model
//! regresses local time against the sample counter. The in-page carrier needs
//! none of this (the engine shares the page's audio clock); this is the door
//! the WebSocket carrier's tracker drives.

#[cfg(target_arch = "wasm32")]
use clausters_core::clocksync::SampleClockModel;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// The local-time <-> sample regression, the JS face of
/// [`clausters_core::clocksync::SampleClockModel`].
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = SampleClockModel)]
pub struct JsSampleClockModel(SampleClockModel);

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = SampleClockModel)]
impl JsSampleClockModel {
    /// A model seeded with the `nominal_rate`, keeping the last `window`
    /// anchors.
    #[wasm_bindgen(constructor)]
    pub fn new(nominal_rate: f64, window: usize) -> JsSampleClockModel {
        JsSampleClockModel(SampleClockModel::new(nominal_rate, window))
    }

    /// Records one `/clock_query` observation: the local time it was taken at, the
    /// server's counter, and the rate the server reported (0 keeps the
    /// current one).
    #[wasm_bindgen(js_name = addAnchor)]
    pub fn add_anchor(&mut self, t_local: f64, sample: f64, rate: f64) {
        self.0.add_anchor(t_local, sample as i64, rate);
    }

    /// The server's sample at a local time.
    #[wasm_bindgen(js_name = sampleAt)]
    pub fn sample_at(&self, t_local: f64) -> f64 {
        self.0.sample_at(t_local) as f64
    }

    /// The local time at which a server sample falls.
    #[wasm_bindgen(js_name = localTimeOf)]
    pub fn local_time_of(&self, sample: f64) -> f64 {
        self.0.local_time_of(sample as i64)
    }

    /// The measured drift between the two clocks, in parts per million.
    #[wasm_bindgen(getter, js_name = driftPpm)]
    pub fn drift_ppm(&self) -> f64 {
        self.0.drift_ppm()
    }

    /// The measured sample rate (samples per local second).
    #[wasm_bindgen(getter)]
    pub fn rate(&self) -> f64 {
        self.0.rate()
    }

    /// The local-time span the held anchors cover.
    #[wasm_bindgen(getter)]
    pub fn span(&self) -> f64 {
        self.0.span()
    }

    /// How many anchors the model currently holds.
    #[wasm_bindgen(getter)]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[wasm_bindgen(getter, js_name = isEmpty)]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
