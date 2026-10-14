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
    pending: EventList,
    note_ports: Vec<i16>,
    dialects: Vec<u32>,
    notes: Vec<Option<HeldNote>>,
    pedals: Vec<Option<Pedal>>,
    next_note: i32,
    tempo: f64,
    ended: [i32; clausters_core::plugin_shm::EVENTS],
    ended_count: usize,
}
impl PluginSynth {
    pub fn new(def: Arc<PluginDef>, sample_rate: f32) -> Result<Self, String> {
        let audio = AudioInstance::new(def.instance(sample_rate)?)?;
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
            input: vec![0.0; inputs * FRAMES],
            output: vec![0.0; outputs * FRAMES],
            job: Job::default(),
            open: false,
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
        })
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
        if !ok {
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
            self.job = Job {
                events: std::mem::take(&mut self.pending),
                ..Job::default()
            };
            self.input.fill(0.0);
            let mut events = EventList::default();
            self.audio.receive(&mut self.output, &mut events);
            for event in events.values.iter().take(events.count as usize) {
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
        for i in 0..self.notes.len() {
            let Some(note) = self.notes[i] else {
                continue;
            };
            if lane.is_some_and(|lane| {
                note.source >> 96 != 1 || ((note.source >> 32) & 0xffff_ffff) != lane as u32 as u128
            }) {
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
        for pedal in &mut self.pedals {
            if pedal.is_some_and(|p| {
                lane.is_none()
                    || (p.source >> 96 == 1
                        && ((p.source >> 32) & 0xffff_ffff) == lane.unwrap() as u32 as u128)
            }) {
                *pedal = None;
            }
        }
    }
    fn release_plugin_transport(&mut self, transport: usize, offset: usize) {
        for i in 0..self.notes.len() {
            let Some(note) = self.notes[i] else {
                continue;
            };
            if note.source >> 96 != 1
                || ((note.source >> 64) & u32::MAX as u128) != transport as u128
            {
                continue;
            }
            self.notes[i] = None;
            self.native_note(Event {
                offset: offset as u32,
                kind: 3,
                note_id: note.native,
                port: note.port,
                channel: note.channel,
                key: note.key,
                ..Event::default()
            });
        }
        for pedal in &mut self.pedals {
            if pedal.is_some_and(|p| {
                p.source >> 96 == 1 && ((p.source >> 64) & u32::MAX as u128) == transport as u128
            }) {
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
