//! Waveform prototype: a navigable view of a large buffer.
//!
//! Controls: mouse wheel zooms toward the pointer, left-drag pans, `R` resets,
//! `Esc` quits. Needs a display and a Vulkan/Metal/DX12/GL adapter.

use std::sync::Arc;

use clausters_gui::demo;
use clausters_gui::native;
use clausters_gui::waveform::{WaveformData, WaveformView};

const SIGNAL_LEN: usize = 4_000_000; // ~83 s at 48 kHz.
const BASE_BUCKET: usize = 256;

fn main() {
    let samples: Arc<[f32]> = demo::sweep(SIGNAL_LEN).into();
    native::run(
        "Clausters - waveform (wheel: zoom, drag: pan, R: reset)",
        Box::new(move |_device, _queue, _renderers| {
            Box::new(WaveformView::new(WaveformData::new(samples, BASE_BUCKET)))
        }),
    );
}
