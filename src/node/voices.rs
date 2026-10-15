//! The `Voices` node: N voices of one def, played by notes.
//!
//! An instrument is a persistent node, and a note never makes one. A `Voices`
//! node holds its voices built -- N instances of a SynthDef or a FaustDef,
//! made when the node is -- and gives each note one on the audio thread: a
//! note takes a free voice, resets it on its own memory
//! ([`SynthNode::reset`]) and starts it with the note's controls; a voice
//! whose def frees itself (a done action) is free again; a free voice is not
//! processed, so it costs nothing; and when every voice is busy the note
//! takes the oldest released one, else the oldest held. In the tree it is one
//! node like any other.
//!
//! **Its controls are its def's**, and a `set` on the node reaches every
//! voice, as do its `in` and `out`; `notes` is the event bus it reads. What
//! a note sets on its own voice comes from the note through the node's
//! **voice map** ([`VoiceMap`]): the key as the frequency control, the
//! velocity as the amplitude one, the gate closed at the note's end (or the
//! voice freed, for a def with no gate), and the expression, pressure,
//! timbre, release velocity and controllers each to the control it names. A
//! channel's controller, bend and pressure are the node's, set like `set`
//! on every voice and kept for the voices to come; a note's own expression
//! is that voice's.

use crate::dsp::events::{BusEvent, Kind};
use crate::dsp::{DoneAction, ProcessCtx, ReplyMsg, UGenCmd};
use crate::node::{ControlMap, SynthNode};
use clausters_core::event_bus::Expression;

/// What a note sets on a voice, as indices of the def's controls.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoiceMap {
    /// The note's frequency, in Hz, from its key and tuning (`freq`).
    pub freq: Option<u32>,
    /// The note's velocity, `0..=1` (`amp`).
    pub amp: Option<u32>,
    /// Opened at the note's start and closed at its end (`gate`). Without
    /// one, the note's end frees the voice.
    pub gate: Option<u32>,
    /// A channel's bend, `-1..=1`, and a note's tuning, in semitones.
    pub bend: Option<u32>,
    /// A channel's pressure, and a note's when `poly` is not mapped.
    pub pressure: Option<u32>,
    /// A note's own pressure.
    pub poly: Option<u32>,
    /// A note's brightness -- MPE's timbre.
    pub timbre: Option<u32>,
    /// The note's release velocity, set as it ends.
    pub lift: Option<u32>,
    /// Each controller's control.
    pub cc: [Option<u32>; 128],
}

impl VoiceMap {
    /// Nothing mapped.
    pub const EMPTY: Self = Self {
        freq: None,
        amp: None,
        gate: None,
        bend: None,
        pressure: None,
        poly: None,
        timbre: None,
        lift: None,
        cc: [None; 128],
    };

    /// One entry of the map, set (`Some`) or cleared.
    pub fn set(&mut self, slot: VoiceSlot, index: Option<u32>) {
        match slot {
            VoiceSlot::Freq => self.freq = index,
            VoiceSlot::Amp => self.amp = index,
            VoiceSlot::Gate => self.gate = index,
            VoiceSlot::Bend => self.bend = index,
            VoiceSlot::Pressure => self.pressure = index,
            VoiceSlot::Poly => self.poly = index,
            VoiceSlot::Timbre => self.timbre = index,
            VoiceSlot::Lift => self.lift = index,
            VoiceSlot::Cc(n) => self.cc[n as usize & 127] = index,
        }
    }
}

/// An entry of a [`VoiceMap`], as `/voices_map` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceSlot {
    Freq,
    Amp,
    Gate,
    Bend,
    Pressure,
    Poly,
    Timbre,
    Lift,
    Cc(u8),
}

impl VoiceSlot {
    /// The slot a selector names: `note`, `vel` (`velocity`), `gate`,
    /// `bend`, `pressure`, `poly`, `timbre`, `lift` or `ccN`.
    pub fn parse(selector: &str) -> Option<Self> {
        Some(match selector {
            "note" => Self::Freq,
            "vel" | "velocity" => Self::Amp,
            "gate" => Self::Gate,
            "bend" => Self::Bend,
            "pressure" => Self::Pressure,
            "poly" => Self::Poly,
            "timbre" => Self::Timbre,
            "lift" => Self::Lift,
            s => Self::Cc(
                s.strip_prefix("cc")?
                    .parse()
                    .ok()
                    .filter(|n: &u8| *n < 128)?,
            ),
        })
    }
}

/// Where a voice is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Waiting for a note; not processed.
    Free,
    /// Sounding its note.
    Held,
    /// Its note ended; sounding its release until its def frees it.
    Released,
}

/// One voice: its synth and the note it plays.
struct Voice {
    synth: Box<dyn SynthNode>,
    state: State,
    /// The note, as its writer named it.
    note: BusEvent,
    /// The note's tuning, in semitones from its key.
    tuning: f64,
    /// When the voice was given its note, for the steal order.
    since: u64,
}

/// N voices of one def. Built off the audio thread with every voice it will
/// ever have; nothing here allocates afterwards.
pub struct VoicesNode {
    voices: Vec<Voice>,
    /// The def's controls as set on the node, and their bus maps.
    shared: Vec<f32>,
    maps: Vec<ControlMap>,
    /// The event bus the node reads (`notes`), or negative for none.
    notes: f32,
    map: VoiceMap,
    clock: u64,
    /// Notes named by their key that a lane's or a transport's release ended,
    /// as `(source, channel, key)`: their writer's late note-off is theirs.
    orphans: [Option<(u128, i16, i16)>; ORPHANS],
    next_orphan: usize,
}

/// Ended notes whose late note-off is remembered.
const ORPHANS: usize = 64;

impl VoicesNode {
    /// A node over `voices`, all of one def whose controls start at
    /// `defaults`, its notes set as `map` says.
    pub fn new(voices: Vec<Box<dyn SynthNode>>, defaults: Vec<f32>, map: VoiceMap) -> Self {
        let maps = vec![ControlMap::UNMAPPED; defaults.len()];
        Self {
            voices: voices
                .into_iter()
                .map(|synth| Voice {
                    synth,
                    state: State::Free,
                    note: BusEvent::default(),
                    tuning: 0.0,
                    since: 0,
                })
                .collect(),
            shared: defaults,
            maps,
            notes: -1.0,
            map,
            clock: 0,
            orphans: [None; ORPHANS],
            next_orphan: 0,
        }
    }

    /// How many voices are sounding (held or releasing).
    pub fn sounding(&self) -> usize {
        self.voices
            .iter()
            .filter(|v| v.state != State::Free)
            .count()
    }

    /// The control index of `notes`: just past the def's.
    fn notes_index(&self) -> usize {
        self.shared.len()
    }

    /// The voice a new note takes: a free one, else the oldest released,
    /// else the oldest held.
    fn take(&self) -> Option<usize> {
        if let Some(i) = self.voices.iter().position(|v| v.state == State::Free) {
            return Some(i);
        }
        let oldest = |state| {
            self.voices
                .iter()
                .enumerate()
                .filter(|(_, v)| v.state == state)
                .min_by_key(|(_, v)| v.since)
                .map(|(i, _)| i)
        };
        oldest(State::Released).or_else(|| oldest(State::Held))
    }

    fn set(&mut self, i: usize, control: Option<u32>, value: f32) {
        if let Some(index) = control {
            self.voices[i].synth.set_control(index, value);
        }
    }

    fn freq(&mut self, i: usize) {
        let pitch = f64::from(self.voices[i].note.key) + self.voices[i].tuning;
        let hz = crate::midi::convert::midi2freq(pitch as f32);
        self.set(i, self.map.freq, hz);
    }

    /// Starts voice `i` on note `e`: reset, the node's controls, the note's.
    fn start(&mut self, i: usize, e: &BusEvent) {
        let voice = &mut self.voices[i];
        voice.synth.reset();
        for (index, value) in self.shared.iter().enumerate() {
            voice.synth.set_control(index as u32, *value);
            let map = self.maps[index];
            if map.bus >= 0 {
                voice.synth.map_control(index as u32, map.bus, map.audio);
            }
        }
        voice.state = State::Held;
        voice.note = *e;
        voice.tuning = 0.0;
        self.clock += 1;
        voice.since = self.clock;
        self.freq(i);
        self.set(i, self.map.amp, e.value as f32);
        self.set(i, self.map.gate, 1.0);
    }

    /// Ends voice `i`'s note: its release velocity, then the gate closed, or
    /// the voice freed when the def has no gate.
    fn release(&mut self, i: usize, velocity: f64) {
        if self.voices[i].state != State::Held {
            return;
        }
        self.set(i, self.map.lift, velocity as f32);
        if self.map.gate.is_some() {
            self.set(i, self.map.gate, 0.0);
            self.voices[i].state = State::Released;
        } else {
            self.voices[i].state = State::Free;
        }
    }

    /// The held voices an event names (its source, and every address it
    /// gives).
    fn named(&self, e: &BusEvent) -> impl Iterator<Item = usize> + '_ {
        let e = *e;
        self.voices
            .iter()
            .enumerate()
            .filter(move |(_, v)| v.state == State::Held && v.note.named_by(&e))
            .map(|(i, _)| i)
    }

    /// A control set on the node: every voice, and the voices to come.
    fn set_shared(&mut self, index: u32, value: f32) {
        let i = index as usize;
        if i == self.notes_index() {
            self.notes = value;
            return;
        }
        if let Some(slot) = self.shared.get_mut(i) {
            *slot = value;
            self.maps[i] = ControlMap::UNMAPPED;
        }
        for voice in &mut self.voices {
            voice.synth.set_control(index, value);
        }
    }

    /// **One event**, read from the node's bus or addressed to it.
    fn event(&mut self, e: &BusEvent) {
        match e.kind {
            Kind::NoteOn => {
                if e.joins >= 0 {
                    let joined = BusEvent {
                        note_id: e.joins,
                        channel: -1,
                        key: -1,
                        ..*e
                    };
                    let held = self.named(&joined).next();
                    if let Some(i) = held {
                        // The line goes on: the held voice takes the new key
                        // and velocity, and the note's identity.
                        let voice = &mut self.voices[i];
                        voice.note = BusEvent { joins: -1, ..*e };
                        voice.tuning = 0.0;
                        self.freq(i);
                        self.set(i, self.map.amp, e.value as f32);
                        return;
                    }
                }
                // A note the writer already holds ends first: the same id, or
                // the same channel and key for a note named by its key.
                let clash = BusEvent {
                    channel: if e.note_id >= 0 { -1 } else { e.channel },
                    key: if e.note_id >= 0 { -1 } else { e.key },
                    ..*e
                };
                for i in 0..self.voices.len() {
                    if self.voices[i].state == State::Held && self.voices[i].note.named_by(&clash) {
                        self.release(i, 0.0);
                    }
                }
                if let Some(i) = self.take() {
                    self.start(i, e);
                }
            }
            Kind::NoteOff => {
                if e.note_id < 0
                    && let Some(slot) = self.orphans.iter_mut().find(|o| {
                        o.is_some_and(|(s, c, k)| s == e.source && c == e.channel && k == e.key)
                    })
                {
                    *slot = None;
                    return;
                }
                // A note named by its key ends the source's oldest one.
                let mut ends: Option<usize> = None;
                for i in self.named(e) {
                    if e.note_id >= 0 {
                        ends = Some(i);
                        break;
                    }
                    if ends.is_none_or(|j| self.voices[i].since < self.voices[j].since) {
                        ends = Some(i);
                    }
                }
                if let Some(i) = ends {
                    self.release(i, e.value);
                }
            }
            Kind::Choke => {
                for i in 0..self.voices.len() {
                    if self.voices[i].state != State::Free && self.voices[i].note.named_by(e) {
                        self.voices[i].state = State::Free;
                    }
                }
            }
            Kind::Expression => {
                for i in 0..self.voices.len() {
                    if self.voices[i].state != State::Held || !self.voices[i].note.named_by(e) {
                        continue;
                    }
                    match Expression::from_u32(e.index) {
                        Some(Expression::Tuning) => {
                            self.voices[i].tuning = e.value;
                            self.freq(i);
                            self.set(i, self.map.bend, e.value as f32);
                        }
                        Some(Expression::Pressure) => {
                            let control = self.map.poly.or(self.map.pressure);
                            self.set(i, control, e.value as f32);
                        }
                        Some(Expression::Brightness) => {
                            self.set(i, self.map.timbre, e.value as f32);
                        }
                        _ => {}
                    }
                }
            }
            Kind::Controller => {
                if let Some(index) = self.map.cc[e.index as usize & 127] {
                    self.set_shared(index, e.value as f32);
                }
            }
            Kind::Bend => {
                if let Some(index) = self.map.bend {
                    self.set_shared(index, e.value as f32);
                }
            }
            Kind::Pressure => {
                if let Some(index) = self.map.pressure {
                    self.set_shared(index, e.value as f32);
                }
            }
            Kind::Program => {}
            Kind::Control => {
                if e.note_id < 0 {
                    self.set_shared(e.index, e.value as f32);
                } else if (e.index as usize) < self.shared.len() {
                    for i in 0..self.voices.len() {
                        if self.voices[i].state != State::Free && self.voices[i].note.named_by(e) {
                            self.voices[i].synth.set_control(e.index, e.value as f32);
                        }
                    }
                }
            }
        }
    }

    /// Releases every held voice whose source `released` selects.
    fn release_where(&mut self, released: impl Fn(u128) -> bool) {
        for i in 0..self.voices.len() {
            if self.voices[i].state == State::Held && released(self.voices[i].note.source) {
                let note = self.voices[i].note;
                if note.note_id < 0 {
                    self.orphans[self.next_orphan] = Some((note.source, note.channel, note.key));
                    self.next_orphan = (self.next_orphan + 1) % ORPHANS;
                }
                self.release(i, 0.0);
            }
        }
    }
}

impl SynthNode for VoicesNode {
    fn process(&mut self, ctx: &mut ProcessCtx) {
        if self.notes >= 0.0 {
            let buses = ctx.buses;
            for e in buses
                .events()
                .read(self.notes as u32, ctx.offset, ctx.frames)
            {
                self.event(e);
            }
        }
        for voice in &mut self.voices {
            if voice.state == State::Free {
                continue;
            }
            voice.synth.process(ctx);
            // A voice whose def frees itself is the node's to give again.
            if !matches!(voice.synth.done_action(), DoneAction::None) {
                voice.state = State::Free;
            }
        }
    }

    fn reads_notes(&self) -> bool {
        true
    }

    fn note_event(&mut self, event: &BusEvent) {
        self.event(event);
    }

    fn release_notes(&mut self, lane: Option<i32>, _offset: usize) {
        // Every note with no lane named: the node is under a group its
        // transport has frozen.
        self.release_where(|source| {
            lane.is_none_or(|n| clausters_core::event_bus::lane_of(source) == Some(n))
        });
    }

    fn release_transport_notes(&mut self, transport: usize, _offset: usize) {
        self.release_where(|source| {
            clausters_core::event_bus::transport_of(source) == Some(transport)
        });
    }

    fn voice_map(&mut self, slot: VoiceSlot, index: Option<u32>) {
        self.map.set(slot, index);
    }

    fn control_state(&self, index: usize) -> Option<(f32, ControlMap)> {
        if index == self.notes_index() {
            return Some((self.notes, ControlMap::UNMAPPED));
        }
        Some((*self.shared.get(index)?, self.maps[index]))
    }

    fn set_control(&mut self, index: u32, value: f32) {
        self.set_shared(index, value);
    }

    fn map_control(&mut self, index: u32, bus: i32, audio: bool) {
        let i = index as usize;
        if let Some(map) = self.maps.get_mut(i) {
            *map = ControlMap { bus, audio };
            for voice in &mut self.voices {
                voice.synth.map_control(index, bus, audio);
            }
        }
    }

    fn ugen_count(&self) -> usize {
        self.voices.iter().map(|v| v.synth.ugen_count()).sum()
    }

    fn ugen_command(&mut self, index: u32, cmd: &UGenCmd) {
        for voice in &mut self.voices {
            voice.synth.ugen_command(index, cmd);
        }
    }

    fn latency(&self) -> usize {
        self.voices.first().map_or(0, |v| v.synth.latency())
    }

    fn resume(&mut self) {
        for voice in &mut self.voices {
            voice.synth.resume();
        }
    }

    fn set_node_id(&mut self, id: i32) {
        for voice in &mut self.voices {
            voice.synth.set_node_id(id);
        }
    }

    fn has_replies(&self) -> bool {
        self.voices.iter().any(|v| v.synth.has_replies())
    }

    fn drain_replies(&mut self, node_id: i32, sink: &mut dyn FnMut(ReplyMsg)) {
        for voice in &mut self.voices {
            voice.synth.drain_replies(node_id, sink);
        }
    }
}
