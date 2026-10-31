//! **The engraver of a page's applications**: the pinned verovio in a worker of
//! its own, which an application's thread asks synchronously over the shared
//! memory, as it asks the file worker for the page's storage (the applications'
//! build only).
//!
//! A call is a [`Request`] in the shared memory: its address goes to the worker
//! (`clients/web/src/gui/apps-engraver.ts`) and the thread waits on its state.
//! The worker loads verovio the first time it is asked, so a page that never
//! engraves never fetches it. The port is the one every engraver is
//! ([`clausters_core::notation::Engraver`]), so the score in a page's application
//! takes the same calls to the same engraver as anywhere else.

use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};

use clausters_core::notation::{AnyEngraver, Engraver, Score, engrave_options};
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    /// The page's door to its engraver, on a thread: hands the request at
    /// `request` to the worker and waits until it is answered.
    #[wasm_bindgen(js_namespace = __claustersApps, js_name = engrave)]
    fn page_engrave(request: u32);
}

/// **One call to the engraver, as it sits in the shared memory**, read by
/// `apps-engraver.ts` at these offsets.
#[repr(C)]
struct Request {
    /// 0 until the worker answers, then 1.
    state: AtomicI32,
    /// The call, as UTF-8 JSON.
    call: u32,
    call_len: u32,
    /// Where the answer goes, and how much fits.
    out: u32,
    out_cap: u32,
    /// How long the answer is, written or not.
    out_len: u32,
}

/// What numbers the toolkits a page's applications hold.
static NEXT: AtomicU32 = AtomicU32::new(1);

/// One call, and the JSON it answers. An answer too long for the first buffer
/// is asked for again, into one that fits: the worker keeps its last answer.
fn ask(call: &Value) -> Value {
    let call = call.to_string();
    let mut out = vec![0u8; 64 * 1024];
    let mut text = call.as_str();
    loop {
        let request = Request {
            state: AtomicI32::new(0),
            call: text.as_ptr() as u32,
            call_len: text.len() as u32,
            out: out.as_mut_ptr() as u32,
            out_cap: out.len() as u32,
            out_len: 0,
        };
        page_engrave(&request as *const Request as u32);
        while request.state.load(Ordering::Acquire) == 0 {
            std::hint::spin_loop();
        }
        // SAFETY: the worker has answered, and writes this field no more.
        let len = unsafe { std::ptr::read_volatile(&request.out_len) } as usize;
        if len > out.len() {
            out = vec![0u8; len];
            text = r#"{"m":"again"}"#;
            continue;
        }
        return serde_json::from_slice(&out[..len]).unwrap_or(Value::Null);
    }
}

/// **A toolkit in the page's engraver worker**, freed with this.
struct WorkerEngraver(u32);

impl WorkerEngraver {
    fn call(&self, method: &str, arg: Value) -> Value {
        ask(&json!({"id": self.0, "m": method, "a": arg}))
    }

    fn text(&self, method: &str, arg: Value) -> String {
        self.call(method, arg)
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    fn yes(&self, method: &str, arg: Value) -> bool {
        self.call(method, arg).as_bool().unwrap_or(false)
    }
}

impl Drop for WorkerEngraver {
    fn drop(&mut self) {
        self.call("free", Value::Null);
    }
}

impl Engraver for WorkerEngraver {
    /// The worker answers one call at a time, and each toolkit is its own.
    type Guard = ();

    fn lock(&self) -> Self::Guard {}
    fn load_data(&self, data: &str) -> bool {
        self.yes("loadData", json!(data))
    }
    fn render_svg(&self, page: i32) -> String {
        self.text("renderSvg", json!(page))
    }
    fn mei(&self) -> String {
        self.text("mei", Value::Null)
    }
    fn edit(&self, action: &str) -> bool {
        self.yes("edit", json!(action))
    }
    fn timemap(&self, options: &str) -> String {
        self.text("timemap", json!(options))
    }
    fn midi_values(&self, xml_id: &str) -> Option<String> {
        let text = self.text("midiValues", json!(xml_id));
        (!text.is_empty()).then_some(text)
    }
    fn set_options(&self, options: &str) -> bool {
        self.yes("setOptions", json!(options))
    }
    fn page_count(&self) -> i32 {
        self.call("pageCount", Value::Null).as_i64().unwrap_or(1) as i32
    }
}

/// **A symbolic score read by the page's engraver**, from an application's
/// thread, or why not: the worker could not load verovio, or the document does
/// not read. Configured as every client configures its engraver.
pub(crate) fn open(data: &str) -> Result<Score<AnyEngraver>, String> {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let opened = ask(&json!({"id": id, "m": "open", "a": engrave_options(40, 2100, None)}));
    if let Some(why) = opened.get("error").and_then(Value::as_str) {
        return Err(format!("the page's engraver: {why}"));
    }
    Score::open(AnyEngraver::new(WorkerEngraver(id)), data)
        .ok_or_else(|| "the engraver could not read the score".to_string())
}
