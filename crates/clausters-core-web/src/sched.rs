//! The beat queue behind a client's clock (`clausters_core::tempoclock`).

#[cfg(target_arch = "wasm32")]
use clausters_core::tempoclock::Scheduler;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// The beat-ordered scheduling queue, the JS face of
/// [`clausters_core::tempoclock::Scheduler`]. It holds `(time, id)` pairs and
/// nothing else: the language side maps each id back to the routine it queued,
/// which is what keeps the coroutine driver in the language.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = Scheduler)]
pub struct JsScheduler(Scheduler);

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = Scheduler)]
impl JsScheduler {
    #[wasm_bindgen(constructor)]
    pub fn new() -> JsScheduler {
        JsScheduler(Scheduler::new())
    }

    /// Queues `id` at `time` (beats). Equal times keep insertion order.
    pub fn push(&mut self, time: f64, id: f64) {
        self.0.push(time, id as u64);
    }

    /// The earliest queued time, or `undefined` when the queue is empty.
    #[wasm_bindgen(js_name = peekTime)]
    pub fn peek_time(&self) -> Option<f64> {
        self.0.peek_time()
    }

    /// Pops the earliest entry when it is due at `now`, as `[time, id]`.
    #[wasm_bindgen(js_name = popDue)]
    pub fn pop_due(&mut self, now: f64) -> Option<Vec<f64>> {
        self.0.pop_due(now).map(|(t, id)| vec![t, id as f64])
    }

    /// Drops every entry queued under `id`; returns how many went.
    pub fn remove(&mut self, id: f64) -> usize {
        self.0.remove(id as u64)
    }

    #[wasm_bindgen(getter)]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[wasm_bindgen(getter, js_name = isEmpty)]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }
}

#[cfg(target_arch = "wasm32")]
impl Default for JsScheduler {
    fn default() -> Self {
        Self::new()
    }
}
