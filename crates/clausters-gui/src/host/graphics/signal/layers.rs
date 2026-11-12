//! Signal layer data is shared with applications; drawing stays in this host.

pub use clausters_document::view::layers::{Domain, Layer, Paint, Stack, Vertical};

/// The default stack for this presentation.
pub fn of_presentation(p: crate::host::elements::signal::Presentation) -> Stack {
    match p {
        crate::host::elements::signal::Presentation::TimeFrequency => {
            Stack::from_layers(vec![Layer::new(Paint::Spectrogram)])
        }
        _ => Stack::default(),
    }
}
