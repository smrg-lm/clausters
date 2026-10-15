//! Isolated native plugin defs and block-staged synth nodes.
//!
//! References persist identities, port routing and optional opaque state, never
//! plugin binaries. Helpers are constructed and retired on non-audio threads.
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::dsp::{BusUsage, ProcessCtx, ReplyKind, ReplyMsg};
use crate::node::{ControlMap, SynthNode};
use clausters_core::plugin_shm::{
    Config, Event, EventKind, EventList, FRAMES, Failure, Job, MAX_DEPTH, SPANS, Span,
};
use clausters_plugin_host::format::PluginInfo;
use clausters_plugin_host::runtime::{
    AudioInstance, DEFAULT_TIMEOUT, Instance, helper_path, probe,
};
use serde::{Deserialize, Serialize};

static GENERATION: AtomicU64 = AtomicU64::new(1_000_000);

pub use clausters_core::plugin_catalog::{PluginReference, PortReference};
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
    /// What else names a control: its port's or parameter's alias, mapped to
    /// the native-id name (`clausters_core::plugin_catalog`).
    pub aliases: std::collections::HashMap<String, String>,
    initial_state: bool,
}
impl PluginDef {
    /// Probe in disposable helpers, then activate and validate the initial state.
    pub fn probe(reference: PluginReference, sample_rate: f32) -> Result<Self, String> {
        let present = !reference.state.is_empty();
        Self::probe_inner(reference, sample_rate, present)
    }
    /// Probe a captured state, including a valid empty image.
    pub fn probe_state(reference: PluginReference, sample_rate: f32) -> Result<Self, String> {
        Self::probe_inner(reference, sample_rate, true)
    }
    fn probe_inner(
        reference: PluginReference,
        sample_rate: f32,
        initial_state: bool,
    ) -> Result<Self, String> {
        if !matches!(reference.format.as_str(), "clap" | "vst3") {
            return Err("unsupported plugin format".into());
        }
        let helper = helper_path()?;
        let info = probe(
            &helper,
            Path::new(&reference.path),
            &reference.id,
            DEFAULT_TIMEOUT,
        )?;
        reference.check(&info)?;
        let mut def = Self {
            reference,
            info,
            names: Vec::new(),
            defaults: Vec::new(),
            aliases: Default::default(),
            initial_state,
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
        let mut instance = def.instance(sample_rate, def.reference.depth)?;
        // State can change parameter defaults, but cannot change the loaded surface.
        def.info = instance.metadata().clone();
        def.info.parameters.sort_by_key(|p| p.id);
        for (i, p) in def.info.parameters.iter().enumerate() {
            def.defaults[i] = instance.parameter(p.id)? as f32;
        }
        drop(instance);
        def.aliases = def.info.control_aliases().into_iter().collect();
        Ok(def)
    }
    /// The pipeline a live instance gets: the reference's depth, raised to
    /// cover one whole device callback. A callback of `n` blocks runs them
    /// back to back, so a result due inside it can only come from a job
    /// published in an earlier callback.
    pub fn live_depth(&self, device_frames: u32) -> u32 {
        self.reference
            .depth
            .max((device_frames as usize).div_ceil(FRAMES) as u32)
            .min(MAX_DEPTH)
    }
    fn config(&self, sample_rate: f32, depth: u32) -> Config {
        Config {
            generation: GENERATION.fetch_add(1, Ordering::Relaxed),
            sample_rate: sample_rate as f64,
            sample_origin: 0,
            inputs: self.info.inputs.iter().map(|p| p.channels).sum(),
            outputs: self.info.outputs.iter().map(|p| p.channels).sum(),
            depth,
            reserved: 0,
        }
    }
    fn instance(&self, sample_rate: f32, depth: u32) -> Result<Instance, String> {
        let instance = Instance::load_with_state(
            &helper_path()?,
            Path::new(&self.reference.path),
            &self.reference.id,
            self.config(sample_rate, depth),
            DEFAULT_TIMEOUT,
            self.initial_state
                .then_some(self.reference.state.as_slice()),
        )?;
        let info = instance.metadata();
        let ports = |ports: &[clausters_plugin_host::format::AudioPort]| {
            ports.iter().map(|p| (p.id, p.channels)).collect::<Vec<_>>()
        };
        if ports(&info.inputs) != ports(&self.info.inputs)
            || ports(&info.outputs) != ports(&self.info.outputs)
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
    /// The same recipe carrying `state` as its initial state: the image a
    /// node's restart restores after a save, without probing the plugin again
    /// (a save changes no surface).
    pub fn with_state(&self, state: Vec<u8>) -> Self {
        Self {
            reference: PluginReference {
                state,
                ..self.reference.clone()
            },
            info: self.info.clone(),
            names: self.names.clone(),
            defaults: self.defaults.clone(),
            aliases: self.aliases.clone(),
            initial_state: true,
        }
    }
    /// A control by its native-id name or by its alias.
    pub fn control_index(&self, name: &str) -> Option<u32> {
        let name = self.aliases.get(name).map_or(name, String::as_str);
        self.names.iter().position(|n| n == name).map(|n| n as u32)
    }
    /// Exactly the bus sets that processing reads and sums into.
    pub fn usage(&self, values: &[f32]) -> (BusUsage, Vec<u32>) {
        let mut usage = BusUsage::default();
        let mut indices = Vec::new();
        let mut index = self.info.parameters.len();
        for (read, ports) in [(true, &self.info.inputs), (false, &self.info.outputs)] {
            for port in ports {
                // A negative bus leaves the port unconnected.
                let first = values.get(index).copied().unwrap_or(0.0);
                for channel in 0..port.channels as usize {
                    if first >= 0.0 {
                        usage.mark_bus((first as usize).saturating_add(channel), read, !read);
                    }
                }
                indices.push(index as u32);
                index += 1;
            }
        }
        (usage, indices)
    }
}
/// Per-node administrative lease and block-boundary quiescence handshake.
/// Only the non-RT worker locks the socket; the callback touches atomics alone.
pub struct PluginAdmin {
    phase: AtomicU32,
    control: std::sync::Mutex<clausters_plugin_host::runtime::Administration>,
}
impl PluginAdmin {
    /// Reserve this generation; overlapping state/restart requests are rejected.
    pub fn reserve(&self) -> Result<(), String> {
        self.phase
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| "plugin state operation already pending".into())
    }
    pub fn pending(&self) -> bool {
        matches!(self.phase.load(Ordering::Acquire), 1 | 2)
    }
    /// A failed transaction remains silent until explicit recovery.
    pub fn abort(&self) {
        self.phase.store(3, Ordering::Release);
    }
    /// Wait until the callback has stopped submitting jobs at a block boundary,
    /// before a state load replaces this generation.
    pub fn wait_boundary(&self) -> Result<(), String> {
        let start = std::time::Instant::now();
        while self.phase.load(Ordering::Acquire) != 2 {
            if start.elapsed() >= DEFAULT_TIMEOUT {
                return Err("plugin state boundary timeout".into());
            }
            std::thread::sleep(std::time::Duration::from_micros(50));
        }
        Ok(())
    }
    /// Operate the editor off RT without reserving or silencing the audio plane.
    pub fn editor(
        &self,
        command: clausters_plugin_host::format::EditorCommand,
    ) -> Result<clausters_plugin_host::format::EditorState, String> {
        if self.pending() {
            return Err("plugin state operation already pending".into());
        }
        self.control
            .lock()
            .map_err(|_| "plugin control poisoned")?
            .editor(command)
    }
    /// Capture the live state while the instance keeps processing: saving
    /// is a main-thread call both formats allow during processing.
    pub fn save(&self) -> Result<Vec<u8>, String> {
        if self.pending() {
            return Err("plugin state operation already pending".into());
        }
        self.control
            .lock()
            .map_err(|_| "plugin state control poisoned")?
            .save_state()
    }
}

#[derive(Clone, Copy)]
struct HeldNote {
    source: u128,
    id: i32,
    native: i32,
    port: i16,
    channel: i16,
    key: i16,
    released: bool,
}
#[derive(Clone, Copy)]
struct Pedal {
    source: u128,
    port: i16,
    channel: i16,
    down: bool,
}
/// Maximum concurrently owned native notes in one persistent instance.
pub const HELD_NOTES: usize = 256;
/// Most parameter changes one job carries; the rest of the list is the notes'.
const PARAMETER_EVENTS: usize = clausters_core::plugin_shm::EVENTS * 3 / 4;
/// One persistent helper, one bounded producer and preallocated block staging.
/// Dropped through the engine's garbage FIFO, never inside the audio callback.
pub struct PluginSynth {
    def: Arc<PluginDef>,
    audio: AudioInstance,
    admin: Arc<PluginAdmin>,
    state_loaded: bool,
    values: Vec<f32>,
    sent: Vec<f32>,
    maps: Vec<ControlMap>,
    input: Vec<f32>,
    output: Vec<f32>,
    job: Job,
    open: bool,
    offline: bool,
    fault: u32,
    reported: bool,
    pending: EventList,
    note_ports: Vec<i16>,
    dialects: Vec<u32>,
    notes: Vec<Option<HeldNote>>,
    pedals: Vec<Option<Pedal>>,
    next_note: i32,
    tempo: f64,
    ended: [i32; clausters_core::plugin_shm::EVENTS],
    ended_count: usize,
    control_events: EventList,
    deferred: [Event; HELD_NOTES],
    deferred_count: usize,
}
impl PluginSynth {
    /// A live instance whose pipeline covers device callbacks of up to
    /// `device_frames` frames (zero before the first callback).
    pub fn new(def: Arc<PluginDef>, sample_rate: f32, device_frames: u32) -> Result<Self, String> {
        let depth = def.live_depth(device_frames);
        Self::with_mode(def, sample_rate, depth, false)
    }
    /// Build a synchronous helper endpoint exclusively for the offline renderer.
    pub(crate) fn offline(def: Arc<PluginDef>, sample_rate: f32) -> Result<Self, String> {
        let depth = def.reference.depth;
        Self::with_mode(def, sample_rate, depth, true)
    }
    fn with_mode(
        def: Arc<PluginDef>,
        sample_rate: f32,
        depth: u32,
        offline: bool,
    ) -> Result<Self, String> {
        let instance = def.instance(sample_rate, depth)?;
        let admin = Arc::new(PluginAdmin {
            phase: AtomicU32::new(0),
            control: std::sync::Mutex::new(instance.administration()?),
        });
        let audio = AudioInstance::new(instance)?;
        let note_ports = def
            .info
            .note_inputs
            .iter()
            .map(|port| {
                audio
                    .metadata()
                    .note_inputs
                    .iter()
                    .position(|native| native.id == port.id && native.dialects == port.dialects)
                    .map(|index| index as i16)
                    .ok_or("plugin note-port surface changed")
            })
            .collect::<Result<Vec<_>, _>>()?;
        if audio.metadata().note_inputs.len() != def.info.note_inputs.len() {
            return Err("plugin note-port surface changed".into());
        }
        let dialects = audio
            .metadata()
            .note_inputs
            .iter()
            .map(|port| port.dialects)
            .collect();
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
            admin,
            state_loaded: false,
            input: vec![0.0; inputs * FRAMES],
            output: vec![0.0; outputs * FRAMES],
            job: Job::default(),
            open: false,
            offline,
            fault: 0,
            reported: false,
            pending: EventList::default(),
            note_ports,
            dialects,
            notes: vec![None; HELD_NOTES],
            pedals: vec![None; 64],
            next_note: 0,
            tempo: 120.0,
            ended: [0; clausters_core::plugin_shm::EVENTS],
            ended_count: 0,
            control_events: EventList::default(),
            deferred: [Event::default(); HELD_NOTES],
            deferred_count: 0,
        })
    }
    /// Pipeline delay of this instance, in engine blocks.
    pub fn depth(&self) -> u32 {
        self.audio.depth()
    }
    /// Administrative handle for this live generation, never shared by a def.
    pub fn administration(&self) -> Arc<PluginAdmin> {
        Arc::clone(&self.admin)
    }
    /// A load replaces parameter values but keeps routing and live maps.
    pub fn loaded_state(&mut self) {
        self.state_loaded = true;
    }
    fn fail(&mut self, failure: Failure) {
        self.audio.fail(failure);
        self.output.fill(0.0);
    }
    fn emit(&mut self, event: Event) {
        let ok = if self.open {
            self.job.events.push(event)
        } else {
            self.pending.push(event)
        };
        if ok {
            return;
        }
        // A release is never lost and never fails the instance for want of
        // room: it goes out first in the next job. A stop with a full
        // keyboard held is spread over a few blocks instead.
        let release = event.kind == EventKind::NoteOff as u32
            || event.kind == EventKind::NoteChoke as u32
            || (event.kind == EventKind::Midi as u32 && event.id & 0xf0 == 0x80);
        if release && self.deferred_count < self.deferred.len() {
            self.deferred[self.deferred_count] = Event { offset: 0, ..event };
            self.deferred_count += 1;
        } else {
            self.fail(Failure::Capacity);
        }
    }
    fn native_note(&mut self, event: Event) {
        let dialects = self.dialects[event.port as usize];
        if dialects & 1 != 0 {
            self.emit(event);
        } else {
            let status = if event.kind == EventKind::NoteOn as u32 {
                0x90
            } else {
                0x80
            };
            self.emit(Event {
                kind: EventKind::Midi as u32,
                id: (status | event.channel as u32)
                    | ((event.key as u32) << 8)
                    | (((event.value * 127.0).round() as u32) << 16),
                ..event
            });
        }
    }
    fn owned(&self, note: HeldNote, source: u128, event: Event) -> bool {
        note.source == source
            && note.port == event.port
            && (event.channel == -1 || note.channel == event.channel)
            && (event.key == -1 || note.key == event.key)
            && (event.note_id == -1 || note.id == event.note_id)
    }
    fn note_event(&mut self, source: u128, mut event: Event) {
        if event.kind == EventKind::NoteOn as u32 {
            if self.notes.iter().flatten().any(|n| {
                (event.note_id >= 0
                    && n.source == source
                    && n.id == event.note_id
                    && n.port == event.port)
                    || (self.dialects[event.port as usize] & 1 == 0
                        && n.port == event.port
                        && n.channel == event.channel
                        && n.key == event.key)
            }) {
                self.fail(Failure::Protocol);
                return;
            }
            let Some(slot) = self.notes.iter_mut().find(|n| n.is_none()) else {
                self.fail(Failure::Capacity);
                return;
            };
            let Some(next) = self.next_note.checked_add(1) else {
                self.fail(Failure::Capacity);
                return;
            };
            self.next_note = next;
            *slot = Some(HeldNote {
                source,
                id: event.note_id,
                native: next,
                port: event.port,
                channel: event.channel,
                key: event.key,
                released: false,
            });
            event.note_id = next;
            self.native_note(event);
            return;
        }
        for i in 0..self.notes.len() {
            let Some(note) = self.notes[i] else {
                continue;
            };
            if !self.owned(note, source, event)
                || (event.kind == EventKind::NoteOff as u32 && note.released)
            {
                continue;
            }
            if event.kind == EventKind::NoteOff as u32
                && self.pedals.iter().flatten().any(|p| {
                    p.source == source
                        && p.port == note.port
                        && (p.channel == -1 || p.channel == note.channel)
                        && p.down
                })
            {
                self.notes[i].as_mut().unwrap().released = true;
                break;
            }
            let native = Event {
                note_id: note.native,
                channel: note.channel,
                key: note.key,
                ..event
            };
            if event.kind == EventKind::Expression as u32 {
                self.emit(native);
            } else {
                self.notes[i] = None;
                self.native_note(native);
            }
            // A MIDI release without an identity releases the oldest matching attack.
            if event.kind == EventKind::NoteOff as u32 {
                break;
            }
        }
    }
    fn midi_event(&mut self, source: u128, event: Event) {
        let bytes = [
            event.id as u8,
            (event.id >> 8) as u8,
            (event.id >> 16) as u8,
        ];
        let channel = if event.channel == -1 {
            -1
        } else {
            (bytes[0] & 15) as i16
        };
        let address = Event {
            channel,
            note_id: -1,
            key: -1,
            ..event
        };
        match bytes[0] & 0xf0 {
            0x80 | 0x90 => self.note_event(
                source,
                Event {
                    kind: if bytes[0] & 0xf0 == 0x90 && bytes[2] != 0 {
                        EventKind::NoteOn
                    } else {
                        EventKind::NoteOff
                    } as u32,
                    key: bytes[1] as i16,
                    value: bytes[2] as f64 / 127.0,
                    ..address
                },
            ),
            0xb0 if bytes[1] == 64 => {
                let index = self
                    .pedals
                    .iter()
                    .position(|p| {
                        p.is_some_and(|p| {
                            p.source == source && p.port == event.port && p.channel == channel
                        })
                    })
                    .or_else(|| self.pedals.iter().position(Option::is_none));
                let Some(index) = index else {
                    self.fail(Failure::Capacity);
                    return;
                };
                let down = bytes[2] >= 64;
                self.pedals[index] = if down {
                    Some(Pedal {
                        source,
                        port: event.port,
                        channel,
                        down,
                    })
                } else {
                    None
                };
                if self.dialects[event.port as usize] & 6 != 0 {
                    self.emit(event);
                }
                if !down {
                    for i in 0..self.notes.len() {
                        if let Some(note) = self.notes[i]
                            && note.source == source
                            && note.port == event.port
                            && (channel == -1 || note.channel == channel)
                            && note.released
                        {
                            self.notes[i] = None;
                            self.native_note(Event {
                                kind: EventKind::NoteOff as u32,
                                note_id: note.native,
                                key: note.key,
                                value: 0.0,
                                ..address
                            });
                        }
                    }
                }
            }
            0xb0 if matches!(bytes[1], 120 | 123) => self.note_event(
                source,
                Event {
                    kind: EventKind::NoteChoke as u32,
                    ..address
                },
            ),
            _ if self.dialects[event.port as usize] & 6 != 0 => self.emit(event),
            status => {
                let (id, key, value) = match status {
                    0xa0 => (6, bytes[1] as i16, bytes[2] as f64 / 127.0),
                    0xd0 => (6, -1, bytes[1] as f64 / 127.0),
                    0xe0 => (
                        2,
                        -1,
                        ((bytes[1] as u32 | ((bytes[2] as u32) << 7)) as f64 - 8192.0) / 4096.0,
                    ),
                    0xb0 => match bytes[1] {
                        7 => (0, -1, bytes[2] as f64 / 127.0),
                        10 => (1, -1, bytes[2] as f64 / 63.5 - 1.0),
                        11 => (4, -1, bytes[2] as f64 / 127.0),
                        74 => (5, -1, bytes[2] as f64 / 127.0),
                        _ => {
                            self.fail(Failure::Protocol);
                            return;
                        }
                    },
                    _ => {
                        self.fail(Failure::Protocol);
                        return;
                    }
                };
                self.note_event(
                    source,
                    Event {
                        kind: EventKind::Expression as u32,
                        id,
                        key,
                        value,
                        ..address
                    },
                );
            }
        }
    }
    /// Choke every held note `released` selects. Bounded scan over the fixed
    /// note table; no allocation.
    fn release_where(&mut self, offset: usize, released: impl Fn(&HeldNote) -> bool) {
        for i in 0..self.notes.len() {
            let Some(note) = self.notes[i] else {
                continue;
            };
            if !released(&note) {
                continue;
            }
            self.notes[i] = None;
            self.native_note(Event {
                offset: offset as u32,
                kind: EventKind::NoteChoke as u32,
                note_id: note.native,
                port: note.port,
                channel: note.channel,
                key: note.key,
                ..Event::default()
            });
        }
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
        // A burst of edits waits for room rather than failing the instance:
        // what is not sent stays unsent and goes out in a later slice, and
        // a quarter of the list stays free for notes.
        if self.job.events.count as usize >= PARAMETER_EVENTS {
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
        if self.admin.phase.load(Ordering::Acquire) >= 2 {
            return;
        }
        if !self.open {
            self.open = true;
            self.job = Job::default();
            // Deferred releases first, at the job's first sample, then what
            // arrived while no job was open.
            let mut sent = 0;
            while sent < self.deferred_count && self.job.events.push(self.deferred[sent]) {
                sent += 1;
            }
            self.deferred.copy_within(sent..self.deferred_count, 0);
            self.deferred_count -= sent;
            let pending = std::mem::take(&mut self.pending);
            for event in pending.values.iter().take(pending.count as usize) {
                self.emit(*event);
            }
            self.input.fill(0.0);
            let mut events = EventList::default();
            self.audio.receive(&mut self.output, &mut events);
            for event in events.values.iter().take(events.count as usize) {
                if matches!(event.kind, 4 | 8 | 9 | 10)
                    && event.note_id == -1
                    && event.port == -1
                    && event.channel == -1
                    && event.key == -1
                {
                    let Some(index) = self
                        .def
                        .info
                        .parameters
                        .iter()
                        .position(|p| p.id == event.id)
                    else {
                        self.fail(Failure::Protocol);
                        break;
                    };
                    let parameter = &self.def.info.parameters[index];
                    if !event.value.is_finite()
                        || (event.kind == 4
                            && !(parameter.min..=parameter.max).contains(&event.value))
                    {
                        self.fail(Failure::Protocol);
                        break;
                    }
                    if event.kind == 4 && self.maps[index].bus < 0 {
                        self.values[index] = event.value as f32;
                        self.sent[index] = event.value as f32;
                    } else if event.kind == 4 {
                        // A bus map remains authoritative after an editor edit.
                        self.sent[index] = f32::NAN;
                    }
                    if !self.control_events.push(*event) {
                        self.fail(Failure::Capacity);
                        break;
                    }
                }
                if event.kind == EventKind::NoteEnd as u32 {
                    for index in 0..self.notes.len() {
                        if let Some(note) = self.notes[index]
                            && (event.note_id == note.native
                                || (event.note_id == -1
                                    && (event.port == -1 || note.port == event.port)
                                    && (event.channel == -1 || note.channel == event.channel)
                                    && (event.key == -1 || note.key == event.key)))
                        {
                            self.notes[index] = None;
                            if note.id < -1 {
                                if self.ended_count == self.ended.len() {
                                    self.fail(Failure::Capacity);
                                } else {
                                    self.ended[self.ended_count] = note.id;
                                    self.ended_count += 1;
                                }
                            }
                        }
                    }
                }
            }
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
            tempo: self.tempo,
        };
        self.job.span_count += 1;
    }
    fn finish_block(&mut self) {
        if self.open {
            if self.offline {
                self.audio.publish_offline(&self.job, &self.input);
            } else {
                self.audio.publish(&self.job, &self.input);
            }
            self.open = false;
        }
        if self.admin.phase.load(Ordering::Acquire) == 1 {
            self.admin.phase.store(2, Ordering::Release);
        }
        if self.admin.phase.load(Ordering::Acquire) == 3 {
            self.audio.fail(Failure::Protocol);
        }
        self.fault = self.audio.failure();
    }
    fn process(&mut self, ctx: &mut ProcessCtx) {
        if self.admin.phase.load(Ordering::Acquire) >= 2 {
            return;
        }
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
            // An unconnected (negative) or missing bus reads silence.
            let connected = self.values[control] >= 0.0;
            let first = self.values[control].max(0.0) as usize;
            for ch in 0..port.channels as usize {
                let staged =
                    &mut self.input[channel * FRAMES + range.start..channel * FRAMES + range.end];
                if connected && first.saturating_add(ch) < ctx.buses.audio_count() {
                    staged
                        .copy_from_slice(&ctx.buses.audio(first.saturating_add(ch))[range.clone()]);
                } else {
                    staged.fill(0.0);
                }
                channel += 1;
            }
            control += 1;
        }
        channel = 0;
        for port in &self.def.info.outputs {
            let connected = self.values[control] >= 0.0;
            let first = self.values[control].max(0.0) as usize;
            for ch in 0..port.channels as usize {
                if connected && first.saturating_add(ch) < ctx.buses.audio_count() {
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
    fn plugin_tempo(&mut self, bpm: f64) {
        self.tempo = bpm;
    }
    fn plugin_event(&mut self, source: u128, mut event: Event) {
        if event.port < 0 || event.port as usize >= self.def.info.note_inputs.len() {
            self.fail(Failure::Protocol);
            return;
        }
        event.port = self.note_ports[event.port as usize];
        if event.kind == EventKind::Midi as u32 {
            self.midi_event(source, event);
        } else {
            self.note_event(source, event);
        }
    }
    fn release_plugin_notes(&mut self, lane: Option<i32>, offset: usize) {
        let from_lane = |source: u128| {
            lane.is_none_or(|lane| {
                source >> 96 == 1 && ((source >> 32) & 0xffff_ffff) == lane as u32 as u128
            })
        };
        self.release_where(offset, |note| from_lane(note.source));
        for pedal in &mut self.pedals {
            if pedal.is_some_and(|p| from_lane(p.source)) {
                *pedal = None;
            }
        }
    }
    fn release_plugin_transport(&mut self, transport: usize, offset: usize) {
        let governed = |source: u128| {
            source >> 96 == 1 && ((source >> 64) & u32::MAX as u128) == transport as u128
        };
        self.release_where(offset, |note| governed(note.source));
        for pedal in &mut self.pedals {
            if pedal.is_some_and(|p| governed(p.source)) {
                *pedal = None;
            }
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
                if !self.state_loaded || i >= self.sent.len() {
                    self.values[i] = value;
                }
                self.maps[i] = map;
            }
        }
    }
    fn ugen_count(&self) -> usize {
        1
    }
    fn latency(&self) -> usize {
        self.audio.depth() as usize * FRAMES + self.audio.metadata().latency as usize
    }
    fn has_replies(&self) -> bool {
        true
    }
    fn drain_replies(&mut self, node_id: i32, sink: &mut dyn FnMut(ReplyMsg)) {
        for event in self
            .control_events
            .values
            .iter()
            .take(self.control_events.count as usize)
        {
            let mut msg = ReplyMsg::new(ReplyKind::PluginControl, event.id as i32, "");
            msg.node_id = node_id;
            msg.push_value(event.value as f32);
            msg.push_value(event.kind as f32);
            let generation = self.audio.generation();
            for shift in [0, 16, 32, 48] {
                msg.push_value(((generation >> shift) & 65535) as f32);
            }
            sink(msg);
        }
        self.control_events.count = 0;
        for &token in &self.ended[..self.ended_count] {
            let mut msg = ReplyMsg::new(ReplyKind::PluginNoteEnd, token, "");
            msg.node_id = node_id;
            sink(msg);
        }
        self.ended_count = 0;
        if self.fault != 0 && !self.reported {
            let mut msg = ReplyMsg::new(ReplyKind::Fault, self.fault as i32, "Plugin");
            msg.node_id = node_id;
            sink(msg);
            self.reported = true;
        }
    }
}

#[cfg(all(test, feature = "plugin-clap"))]
mod offline_tests {
    use super::*;
    use crate::dsp::{Buses, ControlBuses, TransportCtx};

    fn instrument(name: &str) -> Arc<PluginDef> {
        let directory =
            std::env::temp_dir().join(format!("clausters-plugin-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("fixture.clap");
        assert!(
            std::process::Command::new("cc")
                .args([
                    "-std=c11",
                    "-O2",
                    "-fPIC",
                    "-shared",
                    "-Ithird_party/clap/include",
                    "tools/plugin-bridge/fixture.c",
                    "-o"
                ])
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        let reference = serde_json::from_value(serde_json::json!({"format":"clap", "path":path, "id":"org.clausters.fixture.instrument", "inputs":[], "outputs":[{"id":0,"channels":1,"bus":0}]})).unwrap();
        Arc::new(PluginDef::probe(reference, 48000.0).unwrap())
    }

    fn context(buses: &Buses, offset: usize, frames: usize) -> ProcessCtx<'_> {
        ProcessCtx {
            sample_rate: 48000.0,
            full_sample_rate: 48000.0,
            buses,
            buffers: &[],
            offset,
            frames,
            transport: TransportCtx::default(),
        }
    }

    #[test]
    fn a_release_burst_larger_than_a_job_is_deferred_rather_than_failed() {
        let mut synth = PluginSynth::new(instrument("releases"), 48000.0, 0).unwrap();
        let buses = Buses::new(ControlBuses::new(16), 16);
        let held = clausters_core::plugin_shm::EVENTS + 26;
        for block in 0..6 {
            synth.prepare_slice(&context(&buses, 0, FRAMES), true);
            if block < 3 {
                for key in (block * held / 3)..((block + 1) * held / 3) {
                    synth.plugin_event(
                        1,
                        Event {
                            kind: EventKind::NoteOn as u32,
                            port: 0,
                            channel: 0,
                            key: key as i16,
                            note_id: key as i32,
                            value: 0.5,
                            ..Default::default()
                        },
                    );
                }
            }
            if block == 3 {
                assert_eq!(synth.notes.iter().flatten().count(), held);
                synth.release_plugin_notes(None, 0);
                assert_eq!(
                    synth.deferred_count,
                    held - clausters_core::plugin_shm::EVENTS
                );
            }
            synth.finish_block();
            std::thread::sleep(std::time::Duration::from_millis(5));
            assert_eq!(synth.fault, 0, "block {block}");
        }
        assert_eq!(synth.deferred_count, 0);
        assert!(synth.notes.iter().all(Option::is_none));
    }

    /// `/synth_new` takes the def's spare -- the very instance, already
    /// running -- and asks for the next; a spare of a replaced def is not
    /// taken and is handed back.
    #[test]
    fn a_new_node_takes_the_spare_and_a_replaced_def_drops_it() {
        use crate::osc::translate::CmdTranslator;
        let def = instrument("spare");
        let translator = {
            let mut t = CmdTranslator::new(48000.0);
            t.plugin_defs.insert("keys".into(), Arc::clone(&def));
            t
        };
        let spare = Box::new(PluginSynth::new(Arc::clone(&def), 48000.0, 0).unwrap());
        let generation = spare.incarnation();
        assert!(
            translator
                .offer_spare("keys".into(), Arc::clone(&def), spare)
                .is_none()
        );
        assert!(translator.has_spare("keys"));
        let (synth, _) = translator.make_synth("keys").unwrap();
        assert_eq!(synth.incarnation(), generation);
        assert!(!translator.has_spare("keys"));
        assert_eq!(translator.take_spare_requests(), ["keys"]);
        // Without a spare the node is still built, synchronously.
        let (other, _) = translator.make_synth("keys").unwrap();
        assert_ne!(other.incarnation(), generation);
        // A spare built for a def that has since been replaced is refused.
        let old = instrument("spare-old");
        let stale = Box::new(PluginSynth::new(Arc::clone(&old), 48000.0, 0).unwrap());
        assert!(translator.offer_spare("keys".into(), old, stale).is_some());
    }

    /// A 2048-frame device callback runs 32 blocks back to back: a result due
    /// inside it can only come from the previous callback. Sized for the
    /// callback, the pipeline delivers; sized for single blocks, it starves.
    #[test]
    fn the_pipeline_covers_a_device_callback_of_many_blocks() {
        let def = instrument("callback");
        assert_eq!(def.live_depth(0), clausters_core::plugin_shm::DEFAULT_DEPTH);
        assert_eq!(def.live_depth(2048), 32);
        assert_eq!(def.live_depth(1 << 20), MAX_DEPTH);
        let buses = Buses::new(ControlBuses::new(16), 16);
        for (device_frames, starves) in [(2048, false), (0, true)] {
            let mut synth = PluginSynth::new(Arc::clone(&def), 48000.0, device_frames).unwrap();
            for _ in 0..8 {
                for _ in 0..32 {
                    synth.prepare_slice(&context(&buses, 0, FRAMES), true);
                    synth.finish_block();
                }
                std::thread::sleep(std::time::Duration::from_millis(40));
            }
            assert_eq!(
                synth.fault == Failure::Starved as u32,
                starves,
                "device callback of {device_frames} frames"
            );
            if !starves {
                assert_eq!(synth.fault, 0);
            }
        }
    }

    #[test]
    fn offline_and_live_helpers_agree_on_frozen_spans_and_transport_jumps() {
        let def = instrument("slices");
        let mut live = PluginSynth::new(Arc::clone(&def), 48000.0, 0).unwrap();
        let mut offline = PluginSynth::offline(def, 48000.0).unwrap();
        let buses = Buses::new(ControlBuses::new(16), 16);
        let mut position = 0;
        let mut heard = false;
        for block in 0..20 {
            if block == 7 {
                position = 48000;
            }
            for (offset, frames) in [(0, 13), (13, 18), (31, 33)] {
                let active = !(3..6).contains(&block) && !(block == 2 && offset == 13);
                let ctx = ProcessCtx {
                    sample_rate: 48000.0,
                    full_sample_rate: 48000.0,
                    buses: &buses,
                    buffers: &[],
                    offset,
                    frames,
                    transport: TransportCtx {
                        position,
                        rolling: active,
                        ..Default::default()
                    },
                };
                for synth in [&mut live, &mut offline] {
                    synth.plugin_tempo(if block < 7 { 90.0 } else { 150.0 });
                    synth.prepare_slice(&ctx, active);
                    if block == 0 && offset == 13 {
                        synth.plugin_event(
                            1,
                            Event {
                                offset: 13,
                                kind: EventKind::NoteOn as u32,
                                port: 0,
                                channel: 0,
                                key: 60,
                                note_id: 1,
                                value: 0.5,
                                ..Default::default()
                            },
                        );
                    }
                    if block == 3 && offset == 0 {
                        synth.release_plugin_notes(None, 0);
                    }
                    if block == 5 && offset == 31 {
                        synth.plugin_event(
                            2,
                            Event {
                                offset: 31,
                                kind: EventKind::NoteOn as u32,
                                port: 0,
                                channel: 0,
                                key: 64,
                                note_id: 2,
                                value: 0.5,
                                ..Default::default()
                            },
                        );
                    }
                }
                if active {
                    position += frames as u64;
                }
            }
            live.finish_block();
            offline.finish_block();
            std::thread::sleep(std::time::Duration::from_millis(5));
            assert_eq!(live.fault, 0);
            assert_eq!(offline.fault, 0);
            assert_eq!(live.output, offline.output);
            heard |= offline.output.iter().any(|value| *value > 1.0);
        }
        assert!(heard, "the resumed note must reflect the transport jump");
    }
}
