//! Spectrogram prototype: a navigable STFT of a large buffer, sharing the
//! waveform's zoom/pan navigation.
//!
//! Controls: wheel zooms time toward the pointer, left-drag pans time;
//! Shift+wheel / Shift+drag zoom and pan frequency; `L` toggles linear/log
//! frequency; `[` / `]` lower/raise the dB floor (contrast); `/` cycles the
//! colormap; `R` resets time, `Esc` quits. Needs a display and a
//! Vulkan/Metal/DX12/GL adapter.

use std::sync::Arc;

use clausters_gui::demo;
use clausters_gui::native;
use clausters_gui::spectrogram::{SpectrogramView, Stft};

const SIGNAL_LEN: usize = 4_000_000; // ~83 s at 48 kHz.
const WINDOW: usize = 1024;
const HOP: usize = 512;

fn main() {
    let samples = demo::sweep(SIGNAL_LEN);
    // The STFT is the one-time analysis (the cache); compute it before opening
    // the window so the factory just uploads it as a texture.
    let stft = Arc::new(Stft::compute(
        &samples,
        WINDOW,
        HOP,
        demo::SAMPLE_RATE as f32,
    ));
    native::run(
        "Clausters - spectrogram (wheel: time, Shift+wheel: freq, L: lin/log, [ ]: dB, /: color)",
        Box::new(move |device, queue, renderers| {
            Box::new(SpectrogramView::new(
                device,
                queue,
                &renderers.spectrogram,
                stft,
            ))
        }),
    );
}
