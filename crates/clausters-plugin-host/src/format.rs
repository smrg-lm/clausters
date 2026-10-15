//! Format-independent discovery, metadata, lifecycle and processing interface.

pub use clausters_core::plugin_catalog::{
    AudioPort, MAX_PARAMETERS, MAX_PLUGINS, MAX_PORTS, NOTE_CHANNELS, NotePort, Parameter,
    PluginInfo, ScanEntry,
};
use clausters_core::plugin_shm::{Config, EventList, Span};
use serde::{Deserialize, Serialize};
use std::any::Any;

/// Opaque state limit on the administrative plane.
pub const MAX_STATE_BYTES: usize = 1024 * 1024;

/// Beats per bar the transport reports. The server's grid has a tempo and no
/// meter, and a plugin reads a meter anyway: 4/4 is what hosts report when the
/// project states none, and a zero denominator is a division a plugin makes.
pub const BEATS_PER_BAR: u32 = 4;
/// Where a span starts on the musical axis, for every format: the beat, the
/// beat its bar starts on, and that bar's zero-based number.
pub fn musical_position(span: &Span, sample_rate: f64) -> (f64, f64, i32) {
    let beats = span.position as f64 / sample_rate * span.tempo / 60.0;
    let bar = (beats / BEATS_PER_BAR as f64).floor();
    (beats, bar * BEATS_PER_BAR as f64, bar as i32)
}

/// Check the negotiated shared plane against the plugin's current ports.
pub fn fits(info: &PluginInfo, config: Config) -> bool {
    info.inputs.iter().map(|p| p.channels).sum::<u32>() == config.inputs
        && info.outputs.iter().map(|p| p.channels).sum::<u32>() == config.outputs
}

/// Native editor operation, executed on the helper main thread.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum EditorCommand {
    /// Open or raise the editor.
    Open,
    /// Close and release its resources.
    Close,
    /// Request an embedded editor size, in physical pixels.
    Resize { width: u32, height: u32 },
    /// Return the current window state.
    Query,
}
/// Current native editor state, independent of the requesting client's platform.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EditorState {
    /// Whether a visible editor exists.
    pub open: bool,
    /// Client area width; zero when closed.
    pub width: u32,
    /// Client area height; zero when closed.
    pub height: u32,
}
impl EditorState {
    /// Reject zero or unreasonable window dimensions before entering an adapter.
    pub fn valid_size(width: u32, height: u32) -> bool {
        (1..=16384).contains(&width) && (1..=16384).contains(&height)
    }
}

/// Main-thread format instance, never sent to the server audio thread.
pub trait FormatInstance {
    /// Re-read metadata on the helper main thread.
    fn metadata(&mut self) -> Result<PluginInfo, String>;
    /// Activate on the main thread and transfer only the processor to audio.
    fn activate(&mut self, config: Config) -> Result<Box<dyn FormatProcessor>, String>;
    /// Accept the stopped processor back and deactivate on the main thread.
    fn deactivate(&mut self, processor: Box<dyn Any + Send>) -> Result<(), String>;
    /// Opaque state save, with processing stopped.
    fn save_state(&mut self) -> Result<Vec<u8>, String>;
    /// Opaque state restore, with processing stopped.
    fn load_state(&mut self, bytes: &[u8]) -> Result<(), String>;
    /// Parameter query in adapter-native units on the main thread.
    fn parameter(&mut self, id: u32) -> Result<f64, String>;
    /// Operate the native editor without stopping audio processing.
    fn editor(&mut self, command: EditorCommand) -> Result<EditorState, String>;
    /// Serve requested main-thread callbacks.
    fn main_callback(&mut self);
    /// A restart/rescan request cannot silently change the negotiated layout.
    fn restart_requested(&self) -> bool;
}
/// Audio-thread half, common to CLAP and subsequent native adapters.
pub trait FormatProcessor: Send {
    /// Start on the helper audio thread.
    fn start(&mut self) -> Result<(), String>;
    /// Pause processing on audio, retaining DSP and deferred-event state.
    fn pause(&mut self);
    /// Flush editor feedback without advancing DSP, including frozen spans.
    fn editor_events(&mut self, _result: &mut EventList) -> Result<(), String> {
        Ok(())
    }
    /// Process one active span; input/output are channel-major engine blocks.
    fn process(
        &mut self,
        span: Span,
        steady_time: u64,
        input: &[f32],
        output: &mut [f32],
        events: &EventList,
        result: &mut EventList,
    ) -> Result<(), String>;
    /// Stop on the audio thread; transfer the stopped opaque processor to main.
    fn stop(self: Box<Self>) -> Box<dyn Any + Send>;
}
