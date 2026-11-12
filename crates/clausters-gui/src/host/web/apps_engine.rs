//! **The engine an editing application holds, in a page**: its handle on the
//! application's thread, and its processing half in an AudioWorklet of the
//! page, over the one shared memory (the applications' build only).
//!
//! Natively the engine runs on the machine's output device
//! (`AppEngine::open`). A page's output is an AudioWorklet, which runs on a
//! thread of its own and instantiates the same module over the same memory
//! (`clients/web/src/gui/apps-worklet.ts`): the application makes the engine
//! pair, keeps the handle, and hands the processing half to the page, whose
//! worklet calls [`apps_engine_process`] each block. Built commands cross the
//! engine's lock-free FIFOs exactly as they do natively.

use std::sync::atomic::{AtomicU32, Ordering};

use clausters_app_engine::{AppEngine, BLOCK_SIZE, Engine};
use wasm_bindgen::prelude::*;

/// How many channels the application's engine plays.
const CHANNELS: usize = 2;

/// The rate the page's audio runs at, as the page says it
/// ([`apps_engine_rate`]); 48 kHz until it does.
static RATE: AtomicU32 = AtomicU32::new(0x473b_8000);

#[wasm_bindgen]
extern "C" {
    /// The page's door to its output, from the application's thread: makes
    /// the worklet that processes the engine at `engine`, and answers
    /// whether it could.
    #[wasm_bindgen(js_namespace = __claustersApps, js_name = engine)]
    fn page_engine(engine: u32, rate: f32, channels: u32) -> bool;
}

/// **The rate the page's audio runs at**: its AudioContext's, said by the
/// page before an application opens an engine.
#[wasm_bindgen]
pub fn apps_engine_rate(rate: f32) {
    RATE.store(rate.to_bits(), Ordering::Release);
}

/// The engine's processing half, and the block it writes.
struct PageEngine {
    engine: Engine,
    out: Vec<f32>,
}

/// **An engine whose output is the page's**: the pair made, the processing
/// half handed to the page's worklet, the handle answered.
pub(crate) fn open() -> Result<AppEngine, String> {
    let rate = f32::from_bits(RATE.load(Ordering::Acquire));
    let (app, engine) = AppEngine::detached(rate, CHANNELS);
    let page = Box::into_raw(Box::new(PageEngine {
        engine,
        out: vec![0.0; BLOCK_SIZE * CHANNELS],
    }));
    if page_engine(page as u32, rate, CHANNELS as u32) {
        return Ok(app);
    }
    // SAFETY: the page made no worklet, so nothing else holds the box.
    drop(unsafe { Box::from_raw(page) });
    Err("the page could not make the engine's worklet".into())
}

/// **One block of the engine**, processed on the worklet's thread: answers
/// where its interleaved samples are in the memory, `BLOCK_SIZE` frames of
/// `channels`.
#[wasm_bindgen]
pub fn apps_engine_process(engine: u32) -> u32 {
    // SAFETY: `engine` is the box `open` handed the page, which only the
    // worklet processes, one block at a time.
    let page = unsafe { &mut *(engine as *mut PageEngine) };
    page.engine.process_block(&mut page.out);
    page.out.as_ptr() as u32
}

/// The frames a block of the engine is ([`apps_engine_process`]).
#[wasm_bindgen]
pub fn apps_engine_block() -> u32 {
    BLOCK_SIZE as u32
}
