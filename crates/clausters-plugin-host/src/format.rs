//! Format-independent discovery, metadata, lifecycle and processing interface.

use clausters_core::plugin_shm::{Config, EventList, Span};
use serde::{Deserialize, Serialize};
use std::any::Any;

/// Largest metadata catalogue from one binary.
pub const MAX_PLUGINS: usize = 512;
/// Port rows per direction.
pub const MAX_PORTS: usize = 16;
/// Stable native parameters per instance.
pub const MAX_PARAMETERS: usize = 1024;
/// Opaque state limit on the administrative plane.
pub const MAX_STATE_BYTES: usize = 1024 * 1024;

/// An audio port's native identity and channel configuration.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AudioPort {
    /// Native port identity, independent of enumeration order.
    pub id: u32,
    /// Human-readable label, never used as an identity.
    pub name: String,
    /// Number of channels in this port.
    pub channels: u32,
    /// Native flags (CLAP audio port flags for CLAP).
    pub flags: u32,
}
/// An event port's supported native dialects.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NotePort {
    /// Stable native identity.
    pub id: u32,
    /// Display label.
    pub name: String,
    /// CLAP dialect bits: CLAP=1, MIDI=2, MIDI MPE=4, MIDI2=8.
    pub dialects: u32,
}
/// An adapter-native parameter; CLAP uses plain units, VST3 normalized units.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Parameter {
    /// Stable native identity; display name/order does not identify a parameter.
    pub id: u32,
    /// Display label.
    pub name: String,
    /// Hierarchical display grouping.
    pub module: String,
    /// Common CLAP-compatible flags, including stepped/read-only/automatable.
    pub flags: u32,
    /// Minimum in the adapter-native parameter units.
    pub min: f64,
    /// Maximum in the same units.
    pub max: f64,
    /// Quantization interval in the reported units; zero means continuous.
    #[serde(default)]
    pub step: f64,
    /// Default in the same parameter units.
    pub default: f64,
}
/// Stable reference and processing surface reported by isolated discovery.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PluginInfo {
    /// Adapter name: `clap` or `vst3`.
    pub format: String,
    /// Canonical binary path, on the server's filesystem.
    pub path: String,
    /// Native plugin identity within that binary.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Vendor label.
    pub vendor: String,
    /// Vendor's plugin version string.
    pub version: String,
    /// Audio inputs in processing-port order.
    pub inputs: Vec<AudioPort>,
    /// Audio outputs in processing-port order.
    pub outputs: Vec<AudioPort>,
    /// Note inputs in processing-port order.
    pub note_inputs: Vec<NotePort>,
    /// Note outputs in processing-port order.
    pub note_outputs: Vec<NotePort>,
    /// Parameters with stable native ids and adapter-native ranges.
    pub parameters: Vec<Parameter>,
    /// Declared intrinsic latency after activation.
    pub latency: u32,
    /// Whether the plugin exposes opaque state save/load.
    pub state: bool,
    /// Whether a native X11 editor is supported (opening it is separate work).
    pub editor: bool,
}
impl PluginInfo {
    /// Validate metadata capacities, finite ranges and unique native identities.
    pub fn validate(&self) -> Result<(), String> {
        if self.format.is_empty()
            || self.id.is_empty()
            || self.path.is_empty()
            || self.parameters.len() > MAX_PARAMETERS
        {
            return Err("invalid plugin metadata".into());
        }
        for ports in [&self.inputs, &self.outputs] {
            let mut identities = std::collections::HashSet::new();
            let mut channels = 0u64;
            if ports.len() > MAX_PORTS {
                return Err("plugin audio port capacity exceeded".into());
            }
            for port in ports {
                channels += port.channels as u64;
                if port.channels == 0 || port.id == u32::MAX || !identities.insert(port.id) {
                    return Err("invalid plugin audio port".into());
                }
            }
            if channels > clausters_core::plugin_shm::CHANNELS as u64 {
                return Err("plugin audio channel capacity exceeded".into());
            }
        }
        if self.outputs.is_empty() {
            return Err("plugin has no audio output".into());
        }
        for ports in [&self.note_inputs, &self.note_outputs] {
            let mut identities = std::collections::HashSet::new();
            if ports.len() > MAX_PORTS {
                return Err("plugin note port capacity exceeded".into());
            }
            for port in ports {
                if port.dialects == 0 || port.id == u32::MAX || !identities.insert(port.id) {
                    return Err("invalid plugin note port".into());
                }
            }
        }
        let mut identities = std::collections::HashSet::new();
        for p in &self.parameters {
            if p.id == u32::MAX
                || !identities.insert(p.id)
                || !p.min.is_finite()
                || !p.max.is_finite()
                || !p.default.is_finite()
                || !p.step.is_finite()
                || p.step < 0.0
                || p.min > p.max
                || !(p.min..=p.max).contains(&p.default)
            {
                return Err("invalid plugin parameter".into());
            }
        }
        Ok(())
    }
    /// Check the negotiated shared plane against the plugin's current ports.
    pub fn matches(&self, config: Config) -> bool {
        self.inputs.iter().map(|p| p.channels).sum::<u32>() == config.inputs
            && self.outputs.iter().map(|p| p.channels).sum::<u32>() == config.outputs
    }
}

/// Each scan row is either metadata or a per-binary diagnostic; errors survive caching.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScanEntry {
    /// A compatible plugin in a binary.
    Plugin {
        /// Validated metadata.
        plugin: Box<PluginInfo>,
    },
    /// Failure of one candidate; other binaries continue scanning.
    Error {
        /// Candidate binary/directory path.
        path: String,
        /// Timeout, crash, capacity or compatibility diagnostic.
        error: String,
    },
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
