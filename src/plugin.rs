//! Isolated native plugin defs and block-staged synth nodes.
//!
//! References persist identities, port routing and optional opaque state, never
//! plugin binaries. Helpers are constructed and retired on non-audio threads.
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::dsp::{BusUsage, ProcessCtx, ReplyKind, ReplyMsg};
use crate::node::{ControlMap, SynthNode};
use clausters_core::plugin_shm::{
    Config, DEFAULT_DEPTH, Event, EventKind, EventList, FRAMES, Failure, Job, SPANS, Span,
};
use clausters_plugin_host::format::PluginInfo;
use clausters_plugin_host::runtime::{
    AudioInstance, DEFAULT_TIMEOUT, Instance, helper_path, probe,
};
use serde::{Deserialize, Serialize};

static GENERATION: AtomicU64 = AtomicU64::new(1_000_000);

/// One native port routed to consecutive server audio buses.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortReference {
    pub id: u32,
    pub channels: u32,
    #[serde(default)]
    pub bus: u32,
}
/// Persistent recipe; state bytes are an opaque initial-state snapshot.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginReference {
    pub format: String,
    pub path: String,
    pub id: String,
    pub inputs: Vec<PortReference>,
    pub outputs: Vec<PortReference>,
    #[serde(default)]
    pub state: Vec<u8>,
    #[serde(default = "default_depth")]
    pub depth: u32,
}
fn default_depth() -> u32 {
    DEFAULT_DEPTH
}
/// On-disk record preserves the original name across filename sanitizing.
#[derive(Serialize, Deserialize)]
pub struct PluginRecord {
    pub name: String,
    pub reference: PluginReference,
}
/// Validated reference and the control surface shared by all its live nodes.
pub struct PluginDef {
    pub reference: PluginReference,
    pub info: PluginInfo,
    pub names: Vec<String>,
    pub defaults: Vec<f32>,
}
impl PluginDef {
    /// Probe in disposable helpers, then activate and validate the initial state.
    pub fn probe(reference: PluginReference, sample_rate: f32) -> Result<Self, String> {
        if reference.format != "clap" {
            return Err("unsupported plugin format".into());
        }
        let helper = helper_path()?;
        let info = probe(
            &helper,
            Path::new(&reference.path),
            &reference.id,
            DEFAULT_TIMEOUT,
        )?;
        for (ports, routes) in [
            (&info.inputs, &reference.inputs),
            (&info.outputs, &reference.outputs),
        ] {
            if ports.len() != routes.len() {
                return Err("plugin port configuration mismatch".into());
            }
            for port in ports {
                if routes
                    .iter()
                    .filter(|r| r.id == port.id && r.channels == port.channels)
                    .count()
                    != 1
                {
                    return Err("plugin port configuration mismatch".into());
                }
            }
        }
        let mut def = Self {
            reference,
            info,
            names: Vec::new(),
            defaults: Vec::new(),
        };
        def.info.parameters.sort_by_key(|p| p.id);
        for p in &def.info.parameters {
            def.names.push(format!("param:{}", p.id));
            def.defaults.push(p.default as f32);
        }
        for (prefix, ports, routes) in [
            ("in", &def.info.inputs, &def.reference.inputs),
            ("out", &def.info.outputs, &def.reference.outputs),
        ] {
            for p in ports {
                def.names.push(format!("{prefix}:{}", p.id));
                def.defaults
                    .push(routes.iter().find(|r| r.id == p.id).unwrap().bus as f32);
            }
        }
        let mut instance = def.instance(sample_rate)?;
        // State can change parameter defaults, but cannot change the loaded surface.
        def.info = instance.metadata().clone();
        def.info.parameters.sort_by_key(|p| p.id);
        for (i, p) in def.info.parameters.iter().enumerate() {
            def.defaults[i] = instance.parameter(p.id)? as f32;
        }
        drop(instance);
        Ok(def)
    }
    fn config(&self, sample_rate: f32) -> Config {
        Config {
            generation: GENERATION.fetch_add(1, Ordering::Relaxed),
            sample_rate: sample_rate as f64,
            sample_origin: 0,
            inputs: self.info.inputs.iter().map(|p| p.channels).sum(),
            outputs: self.info.outputs.iter().map(|p| p.channels).sum(),
            depth: self.reference.depth,
            reserved: 0,
        }
    }
    fn instance(&self, sample_rate: f32) -> Result<Instance, String> {
        let mut instance = Instance::load(
            &helper_path()?,
            Path::new(&self.reference.path),
            &self.reference.id,
            self.config(sample_rate),
            DEFAULT_TIMEOUT,
        )?;
        if !self.reference.state.is_empty() {
            instance.load_state(&self.reference.state)?;
        }
        let info = instance.metadata();
        if info.inputs != self.info.inputs
            || info.outputs != self.info.outputs
            || info
                .parameters
                .iter()
                .map(|p| p.id)
                .collect::<std::collections::HashSet<_>>()
                != self.info.parameters.iter().map(|p| p.id).collect()
        {
            return Err("plugin instance surface changed".into());
        }
        Ok(instance)
    }
    pub fn control_index(&self, name: &str) -> Option<u32> {
        self.names.iter().position(|n| n == name).map(|n| n as u32)
    }
    /// Exactly the bus sets that processing reads and sums into.
    pub fn usage(&self, values: &[f32]) -> (BusUsage, Vec<u32>) {
        let mut usage = BusUsage::default();
        let mut indices = Vec::new();
        let mut index = self.info.parameters.len();
        for (read, ports) in [(true, &self.info.inputs), (false, &self.info.outputs)] {
            for port in ports {
                let first = values.get(index).copied().unwrap_or(0.0).max(0.0) as usize;
                for channel in 0..port.channels as usize {
                    usage.mark_bus(first.saturating_add(channel), read, !read);
                }
                indices.push(index as u32);
                index += 1;
            }
        }
        (usage, indices)
    }
}
/// One persistent helper, one bounded producer and preallocated block staging.
/// Dropped through the engine's garbage FIFO, never inside the audio callback.
pub struct PluginSynth {
    def: Arc<PluginDef>,
    audio: AudioInstance,
    values: Vec<f32>,
    sent: Vec<f32>,
    maps: Vec<ControlMap>,
    input: Vec<f32>,
    output: Vec<f32>,
    job: Job,
    open: bool,
    fault: u32,
    reported: bool,
}
impl PluginSynth {
    pub fn new(def: Arc<PluginDef>, sample_rate: f32) -> Result<Self, String> {
        let audio = AudioInstance::new(def.instance(sample_rate)?)?;
        let inputs = audio
            .metadata()
            .inputs
            .iter()
            .map(|p| p.channels as usize)
            .sum::<usize>();
        let outputs = audio
            .metadata()
            .outputs
            .iter()
            .map(|p| p.channels as usize)
            .sum::<usize>();
        Ok(Self {
            values: def.defaults.clone(),
            sent: def.defaults[..def.info.parameters.len()].to_vec(),
            maps: vec![ControlMap::UNMAPPED; def.names.len()],
            def,
            audio,
            input: vec![0.0; inputs * FRAMES],
            output: vec![0.0; outputs * FRAMES],
            job: Job::default(),
            open: false,
            fault: 0,
            reported: false,
        })
    }
    fn fail(&mut self, failure: Failure) {
        self.audio.fail(failure);
    }
    fn parameter(&mut self, index: usize, offset: usize) {
        if self.sent[index] == self.values[index] {
            return;
        }
        let p = &self.def.info.parameters[index];
        if p.flags & 8 != 0 {
            return;
        } // CLAP_PARAM_IS_READONLY
        let value = self.values[index] as f64;
        if !value.is_finite() || !((p.min as f32)..=(p.max as f32)).contains(&(value as f32)) {
            self.fail(Failure::Protocol);
            return;
        }
        if !self.job.events.push(Event {
            offset: offset as u32,
            kind: EventKind::Parameter as u32,
            id: p.id,
            value,
            note_id: -1,
            port: -1,
            channel: -1,
            key: -1,
            ..Event::default()
        }) {
            self.fail(Failure::Capacity);
        }
        self.sent[index] = self.values[index];
    }
}
impl SynthNode for PluginSynth {
    fn prepare_slice(&mut self, ctx: &ProcessCtx, active: bool) {
        if !self.open {
            self.open = true;
            self.job = Job::default();
            self.input.fill(0.0);
            let mut events = EventList::default();
            self.audio.receive(&mut self.output, &mut events);
            // A node introduced mid-block has no input before its first slice.
            if ctx.offset != 0 {
                self.job.spans[0] = Span {
                    frames: ctx.offset as u32,
                    tempo: 120.0,
                    ..Span::default()
                };
                self.job.span_count = 1;
            }
        }
        let n = self.job.span_count as usize;
        if n == SPANS {
            self.fail(Failure::Capacity);
            return;
        }
        self.job.spans[n] = Span {
            offset: ctx.offset as u32,
            frames: ctx.frames as u32,
            active: u32::from(active),
            playing: u32::from(ctx.transport.rolling),
            position: ctx.transport.position.min(i64::MAX as u64) as i64,
            tempo: 120.0,
        };
        self.job.span_count += 1;
    }
    fn finish_block(&mut self) {
        if self.open {
            self.audio.publish(&self.job, &self.input);
            self.open = false;
        }
        self.fault = self.audio.failure();
    }
    fn process(&mut self, ctx: &mut ProcessCtx) {
        for i in 0..self.values.len() {
            let map = self.maps[i];
            if map.bus >= 0 {
                self.values[i] = if map.audio {
                    ctx.buses
                        .audio((map.bus as usize).min(ctx.buses.audio_count() - 1))[ctx.offset]
                } else {
                    ctx.buses.control.get(map.bus as usize)
                };
            }
        }
        for i in 0..self.sent.len() {
            self.parameter(i, ctx.offset);
        }
        let range = ctx.offset..ctx.offset + ctx.frames;
        let mut control = self.def.info.parameters.len();
        let mut channel = 0;
        for port in &self.def.info.inputs {
            let first = self.values[control].max(0.0) as usize;
            for ch in 0..port.channels as usize {
                if first.saturating_add(ch) < ctx.buses.audio_count() {
                    self.input[channel * FRAMES + range.start..channel * FRAMES + range.end]
                        .copy_from_slice(&ctx.buses.audio(first.saturating_add(ch))[range.clone()]);
                }
                channel += 1;
            }
            control += 1;
        }
        channel = 0;
        for port in &self.def.info.outputs {
            let first = self.values[control].max(0.0) as usize;
            for ch in 0..port.channels as usize {
                if first.saturating_add(ch) < ctx.buses.audio_count() {
                    // SAFETY: the scheduler owns these buses through this node's StageMask.
                    for (dst, src) in unsafe { ctx.buses.audio_mut(first.saturating_add(ch)) }
                        [range.clone()]
                    .iter_mut()
                    .zip(&self.output[channel * FRAMES + range.start..channel * FRAMES + range.end])
                    {
                        *dst += src;
                    }
                }
                channel += 1;
            }
            control += 1;
        }
    }
    fn set_control(&mut self, index: u32, value: f32) {
        if self
            .def
            .info
            .parameters
            .get(index as usize)
            .is_some_and(|p| p.flags & 8 != 0)
        {
            self.fail(Failure::Protocol);
            return;
        }
        if let Some(v) = self.values.get_mut(index as usize) {
            *v = value;
            self.maps[index as usize] = ControlMap::UNMAPPED;
        }
    }
    fn map_control(&mut self, index: u32, bus: i32, audio: bool) {
        if self
            .def
            .info
            .parameters
            .get(index as usize)
            .is_some_and(|p| p.flags & 8 != 0)
        {
            self.fail(Failure::Protocol);
            return;
        }
        if let Some(m) = self.maps.get_mut(index as usize) {
            *m = ControlMap { bus, audio };
        }
    }
    fn incarnation(&self) -> u64 {
        self.audio.generation()
    }
    fn control_state(&self, index: usize) -> Option<(f32, ControlMap)> {
        self.values.get(index).map(|v| (*v, self.maps[index]))
    }
    fn restore_controls_from(&mut self, prior: &dyn SynthNode) {
        for i in 0..self.values.len() {
            if let Some((value, map)) = prior.control_state(i) {
                self.values[i] = value;
                self.maps[i] = map;
            }
        }
    }
    fn ugen_count(&self) -> usize {
        1
    }
    fn latency(&self) -> usize {
        self.def.reference.depth as usize * FRAMES + self.audio.metadata().latency as usize
    }
    fn has_replies(&self) -> bool {
        true
    }
    fn drain_replies(&mut self, node_id: i32, sink: &mut dyn FnMut(ReplyMsg)) {
        if self.fault != 0 && !self.reported {
            let mut msg = ReplyMsg::new(ReplyKind::Fault, self.fault as i32, "Plugin");
            msg.node_id = node_id;
            sink(msg);
            self.reported = true;
        }
    }
}
