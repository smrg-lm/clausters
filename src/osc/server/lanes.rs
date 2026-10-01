//! **Event lanes**: sequences of notes, messages and MIDI the transport plays, the
//! way a reader plays a take.
//!
//! A take follows the transport because its reader reads the position: a
//! locate, a loop and a stop are the transport's and nothing is sent per
//! pass. A lane is the same thing for events. It holds its notes and messages
//! as data, in samples of one transport's position, and this side keeps the
//! stretch of it just ahead of the position **built** on the engine's lane
//! queue (`Cmd::EventLaneEntry`), where an entry fires when the position reaches
//! it. Keyed by position, an entry needs nothing re-stamped when the transport
//! jumps: one the position left behind waits for it to come back.
//!
//! The feed is here and not on the audio thread because a note is a node, and
//! a node is built on this one: what reaches the engine is what a timed bundle
//! carries, built by the same translator. Each turn of the loop tops the lane
//! queue up to [`LOOKAHEAD_SECS`] ahead of where the transport stands, in
//! playback order -- through a loop's wrap -- and a locate, a loop set or
//! cleared, a pass that ended on its mark, and new data clear what the lane
//! had queued and feed it again.
//!
//! **Node ids are the server's**, minted per pass from the auto range as a
//! `/synth_new -1` would be, because a lane plays again and again and a note's
//! start and its release must name one node. An entry that leaves unrun comes
//! back through the garbage FIFO with its commands, and the nodes it would have
//! made are forgotten there ([`OscServer::forget_unrun`]).
//!
//! **A lane's MIDI plays as the live input does.** Its messages go through
//! the channel's `/midi_bind` binding and the translator's own MIDI builders,
//! as though they had reached the port at their position. What the live path
//! decides as a message arrives -- which voice a note-off ends, which voices a
//! controller moves, which instrument a program change picks -- the lane
//! decides from its data when it is set ([`midi_events`]), since it builds its
//! entries ahead of the position: a note-on and its note-off are one entry
//! with a release, like a note, and its voice id is the MIDI range's.

use std::collections::{HashMap, HashSet};

use clausters_core::event::render::Arg;
use clausters_core::event_lane::{
    EventLaneData, EventLaneMidi, EventLaneUmp, EventLaneVoice, Release,
};
use clausters_midi::mpe::{Decoder, MpeEvent};

use super::*;
use crate::midi::ump::{PerNote, UmpMessage, parse_ump};
use crate::midi::{ChannelVoiceMessage, parse_midi1};
use crate::server::engine::EventLaneTag;

/// How far ahead of the position a lane is kept built, in seconds. Far more
/// than a turn of the loop (10 ms while a lane exists), so a turn spent on
/// something slow -- a def compiled, a file read -- does not let the position
/// overtake the feed.
pub(in crate::osc::server) const LOOKAHEAD_SECS: f64 = 0.5;

/// One event of a lane, at a position of its transport.
#[derive(Clone, Debug, PartialEq)]
enum EventLaneEvent {
    /// A note: a synth of a def, or one more of a graph's slot, with
    /// `controls`, released `length` samples after it starts.
    Note {
        voice: EventLaneVoice,
        controls: Vec<OscType>,
        length: u64,
        release: Release,
    },
    /// A command the server takes in a timed bundle, run as it is written.
    Message(OscMessage),
    /// A MIDI note-on and its note-off, paired on the lane's data: a voice
    /// through the channel's binding, released `length` samples after it
    /// starts. `voice` names it to the expressive messages that reach it, and
    /// `program` is the lane's last program change on the channel before it.
    MidiNote {
        channel: u8,
        note: u8,
        velocity: u16,
        length: u64,
        program: Option<u8>,
        voice: u32,
    },
    /// An expressive MIDI message (a controller, bend, pressure), on the
    /// lane's `voices` sounding on its channel -- one for poly pressure.
    MidiSet {
        message: ChannelVoiceMessage,
        voices: Vec<u32>,
    },
    /// A note in an MPE zone (by its `master`), decoded from the lane's data:
    /// a voice at `key` plus `bend` semitones, with the pressure and timbre it
    /// starts with, released `length` samples after it starts.
    ZoneNote {
        master: u8,
        key: u8,
        velocity: u16,
        bend: f32,
        pressure: u32,
        timbre: u32,
        length: u64,
        voice: u32,
    },
    /// A zone note's expression, on that note's `voice`.
    ZoneSet {
        master: u8,
        key: u8,
        voice: u32,
        event: MpeEvent,
    },
    /// A MIDI 2.0 per-note message, on the `voice` of the note `note` of
    /// `channel` sounding at its position.
    PerNote {
        channel: u8,
        note: u8,
        message: PerNote,
        voice: u32,
    },
}

/// A lane's MIDI as its data holds it: MIDI 1.0 bytes, or a MIDI 2.0 packet.
enum Raw {
    Bytes(Vec<u8>),
    Packet(Vec<u32>),
}

/// What a lane's MIDI reads as, in position order: what the zones' decoder
/// makes of MIDI 1.0 bytes, a MIDI 2.0 channel voice message at its own
/// resolution, or a MIDI 2.0 per-note message.
enum Heard {
    Mpe(MpeEvent),
    Voice(ChannelVoiceMessage),
    PerNote {
        channel: u8,
        note: u8,
        message: PerNote,
    },
}

/// One lane, as this side holds it.
pub(in crate::osc::server) struct EventLane {
    transport: usize,
    target: i32,
    /// `(position, event)`, sorted by position.
    events: Vec<(u64, EventLaneEvent)>,
    /// The generation of `events`, bumped by every new data and every clear,
    /// so an entry spent from an older one does not unmark a newer one.
    generation: u32,
    /// The events of this generation that are on the engine's lane queue.
    queued: HashSet<u32>,
    /// The node of each MIDI voice built in this generation, by its `voice`,
    /// for the expressive messages after it.
    voices: HashMap<u32, i32>,
}

/// Reads a lane's data (`clausters_core::event_lane::EventLaneData`, the JSON
/// `/lane_set` carries) into its events, sorted by position.
fn parse_events(json: &[u8], zones: Decoder) -> Result<Vec<(u64, EventLaneEvent)>, String> {
    let data = EventLaneData::from_json(json)?;
    let mut events = Vec::new();
    for note in data.notes {
        let controls = note
            .controls
            .into_iter()
            .flat_map(|(name, value)| [OscType::String(name), OscType::Float(value as f32)])
            .collect();
        events.push((
            note.start,
            EventLaneEvent::Note {
                voice: note.voice,
                controls,
                length: note.end.saturating_sub(note.start),
                release: note.release,
            },
        ));
    }
    for message in data.messages {
        let args = message
            .args
            .into_iter()
            .map(|arg| match arg {
                Arg::Int(i) => OscType::Int(i),
                Arg::Float(f) => OscType::Float(f),
                Arg::Str(s) => OscType::String(s),
            })
            .collect();
        events.push((
            message.position,
            EventLaneEvent::Message(OscMessage {
                addr: message.addr,
                args,
            }),
        ));
    }
    events.extend(midi_events(data.midi, data.ump, zones));
    events.sort_by_key(|(position, _)| *position);
    Ok(events)
}

/// **A lane's MIDI messages as its events**, read in position order as the
/// live input would have read them, through a decoder with the MPE zones'
/// layout: in a zone, a note-on and its note-off are one zone note and its
/// bend, pressure and timbre reach it alone. Outside every zone, a note-on
/// and the note-off after it on its channel and key are one note (a note-on over one already sounding ends
/// it there, as a live retrigger does); an expressive message reaches the
/// notes sounding on its channel at its position, or the one on its key; a
/// program change picks the instrument of the notes after it. A note-on the
/// lane never turns off is not played, and a message that is not a channel
/// voice message is dropped. MIDI 2.0's packets read among the bytes, in
/// position order: a channel voice message as its MIDI 1.0 twin reads, at its
/// own resolution, and a per-note message reaches the note sounding on its
/// channel and key.
fn midi_events(
    midi: Vec<EventLaneMidi>,
    ump: Vec<EventLaneUmp>,
    mut zones: Decoder,
) -> Vec<(u64, EventLaneEvent)> {
    // Both lists in one order, bytes before packets at a position.
    let mut raw: Vec<(u64, Raw)> = midi
        .into_iter()
        .map(|m| (m.position, Raw::Bytes(m.bytes)))
        .chain(ump.into_iter().map(|p| (p.position, Raw::Packet(p.words))))
        .collect();
    raw.sort_by_key(|(position, _)| *position);
    // Each zone note's slot in `out`, its start, key and master, while it sounds.
    let mut zone_notes: HashMap<u32, (usize, u64, u8, u8)> = HashMap::new();
    // Each note's slot in `out`, with its start, while it sounds.
    let mut sounding: HashMap<(u8, u8), (usize, u64)> = HashMap::new();
    let mut programs: HashMap<u8, u8> = HashMap::new();
    let mut out: Vec<Option<(u64, EventLaneEvent)>> = Vec::new();
    let mut voices = 0u32;
    let voice_of = |out: &[Option<(u64, EventLaneEvent)>], slot: usize| match out[slot] {
        Some((_, EventLaneEvent::MidiNote { voice, .. })) => Some(voice),
        _ => None,
    };
    let mut decoded: Vec<(u64, Heard)> = Vec::new();
    let feed = |zones: &mut Decoder, decoded: &mut Vec<(u64, Heard)>, at: u64, bytes: &[u8]| {
        zones.feed(bytes);
        while let Some(event) = zones.poll() {
            decoded.push((at, Heard::Mpe(event)));
        }
    };
    for (at, item) in raw {
        match item {
            Raw::Bytes(bytes) => feed(&mut zones, &mut decoded, at, &bytes),
            Raw::Packet(words) => {
                for message in parse_ump(&words) {
                    match message {
                        UmpMessage::Voice(voice) => decoded.push((at, Heard::Voice(voice))),
                        UmpMessage::PerNote {
                            channel,
                            note,
                            message,
                        } => decoded.push((
                            at,
                            Heard::PerNote {
                                channel,
                                note,
                                message,
                            },
                        )),
                        UmpMessage::Midi1(bytes) => feed(&mut zones, &mut decoded, at, &bytes),
                    }
                }
            }
        }
    }
    for (at, heard) in decoded {
        let event = match heard {
            Heard::Mpe(event) => event,
            Heard::Voice(message) => {
                push_voice(
                    &mut out,
                    &mut sounding,
                    &mut programs,
                    &mut voices,
                    at,
                    message,
                );
                continue;
            }
            Heard::PerNote {
                channel,
                note,
                message,
            } => {
                if let Some(voice) = sounding
                    .get(&(channel, note))
                    .and_then(|(slot, _)| voice_of(&out, *slot))
                {
                    out.push(Some((
                        at,
                        EventLaneEvent::PerNote {
                            channel,
                            note,
                            message,
                            voice,
                        },
                    )));
                }
                continue;
            }
        };
        let message = match event {
            MpeEvent::Plain([status, d1, d2]) => match parse_midi1(status, d1, d2) {
                Some(message) => message,
                None => continue,
            },
            MpeEvent::NoteOn {
                note,
                side,
                key,
                velocity,
                bend,
                pressure,
                timbre,
                ..
            } => {
                zone_notes.insert(note, (out.len(), at, key, side.master()));
                out.push(Some((
                    at,
                    EventLaneEvent::ZoneNote {
                        master: side.master(),
                        key,
                        velocity,
                        bend,
                        pressure,
                        timbre,
                        length: u64::MAX,
                        voice: voices,
                    },
                )));
                voices = voices.wrapping_add(1);
                continue;
            }
            MpeEvent::NoteOff { note, .. } => {
                if let Some((slot, from, _, _)) = zone_notes.remove(&note)
                    && let Some((_, EventLaneEvent::ZoneNote { length, .. })) = out[slot].as_mut()
                {
                    *length = at - from;
                }
                continue;
            }
            expression => {
                let note = match expression {
                    MpeEvent::Bend { note, .. }
                    | MpeEvent::Pressure { note, .. }
                    | MpeEvent::Timbre { note, .. }
                    | MpeEvent::Controller { note, .. } => note,
                    _ => continue,
                };
                if let Some(&(slot, _, key, master)) = zone_notes.get(&note)
                    && let Some((_, EventLaneEvent::ZoneNote { voice, .. })) = out[slot]
                {
                    out.push(Some((
                        at,
                        EventLaneEvent::ZoneSet {
                            master,
                            key,
                            voice,
                            event: expression,
                        },
                    )));
                }
                continue;
            }
        };
        push_voice(
            &mut out,
            &mut sounding,
            &mut programs,
            &mut voices,
            at,
            message,
        );
    }
    for (slot, _) in sounding.into_values() {
        out[slot] = None;
    }
    for (slot, ..) in zone_notes.into_values() {
        out[slot] = None;
    }
    out.into_iter().flatten().collect()
}

/// A lane's events so far, as `Option`s so an unended note can be dropped.
type Slots = Vec<Option<(u64, EventLaneEvent)>>;

/// **One channel voice message into a lane's events**, at `at`: a note-on
/// starts a note (ending one already sounding on its key), a note-off ends
/// it, a program change is remembered for the notes after it, and an
/// expressive message reaches the notes it is for.
fn push_voice(
    out: &mut Slots,
    sounding: &mut HashMap<(u8, u8), (usize, u64)>,
    programs: &mut HashMap<u8, u8>,
    voices: &mut u32,
    at: u64,
    message: ChannelVoiceMessage,
) {
    let end = |out: &mut Slots, slot: usize, at: u64, from: u64| {
        if let Some((_, EventLaneEvent::MidiNote { length, .. })) = out[slot].as_mut() {
            *length = at - from;
        }
    };
    let voice_of = |out: &Slots, slot: usize| match out[slot] {
        Some((_, EventLaneEvent::MidiNote { voice, .. })) => Some(voice),
        _ => None,
    };
    match message {
        ChannelVoiceMessage::NoteOn {
            channel,
            note,
            velocity,
        } => {
            if let Some((slot, from)) = sounding.remove(&(channel, note)) {
                end(out, slot, at, from);
            }
            sounding.insert((channel, note), (out.len(), at));
            out.push(Some((
                at,
                EventLaneEvent::MidiNote {
                    channel,
                    note,
                    velocity,
                    // Unset until its note-off, and dropped without one.
                    length: u64::MAX,
                    program: programs.get(&channel).copied(),
                    voice: *voices,
                },
            )));
            *voices = voices.wrapping_add(1);
        }
        ChannelVoiceMessage::NoteOff { channel, note, .. } => {
            if let Some((slot, from)) = sounding.remove(&(channel, note)) {
                end(out, slot, at, from);
            }
        }
        ChannelVoiceMessage::ProgramChange { channel, program } => {
            programs.insert(channel, program);
        }
        ChannelVoiceMessage::PolyAftertouch { channel, note, .. } => {
            let voices = sounding
                .get(&(channel, note))
                .and_then(|(slot, _)| voice_of(out, *slot))
                .into_iter()
                .collect();
            out.push(Some((at, EventLaneEvent::MidiSet { message, voices })));
        }
        ChannelVoiceMessage::ChannelAftertouch { channel, .. }
        | ChannelVoiceMessage::ControlChange { channel, .. }
        | ChannelVoiceMessage::PitchBend { channel, .. } => {
            let mut voices: Vec<u32> = sounding
                .iter()
                .filter(|((c, _), _)| *c == channel)
                .filter_map(|(_, (slot, _))| voice_of(out, *slot))
                .collect();
            voices.sort_unstable();
            out.push(Some((at, EventLaneEvent::MidiSet { message, voices })));
        }
    }
}

/// **The spans of position a transport plays next**, in playback order,
/// `window` samples long in all: straight on from `position`, or up to a
/// loop's end and on from its start -- never more than one pass of the loop,
/// so an event is fed once per pass -- and never past an end mark, which the
/// transport stops on.
fn ahead(
    position: u64,
    window: u64,
    looping: Option<(u64, u64)>,
    end: Option<u64>,
) -> Vec<(u64, u64)> {
    match looping {
        Some((start, stop)) if position < stop && start < stop => {
            let first = (position, stop.min(position + window));
            let left = window.saturating_sub(stop - position);
            // What is left wraps, up to where this pass began, or the loop's
            // end when the position is before the loop.
            let limit = if position >= start { position } else { stop };
            let second = (start, (start + left).min(limit));
            [first, second].into_iter().filter(|(a, b)| a < b).collect()
        }
        _ => {
            let stop = end.map_or(position + window, |e| e.min(position + window));
            if position < stop {
                vec![(position, stop)]
            } else {
                Vec::new()
            }
        }
    }
}

impl OscServer {
    /// `/lane_new <transport:int32> <lane:int32> <target:int32>` -- makes lane
    /// `lane` on transport `transport`, its notes made at the tail of group
    /// `target` (the group that transport governs, for a pause to freeze
    /// them). An existing lane of that id is freed first. Replies `/done`.
    pub(in crate::osc::server) fn handle_lane_new(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let transport = self.transport_arg(&mut args)?;
        let lane = args.int()?;
        let target = args.int()?;
        if self.lanes.contains_key(&lane) {
            self.free_lane(lane)?;
        }
        self.lanes.insert(
            lane,
            EventLane {
                transport,
                target,
                events: Vec::new(),
                generation: 0,
                queued: HashSet::new(),
                voices: HashMap::new(),
            },
        );
        self.retune_timeout();
        self.done(from, "/lane_new");
        Ok(())
    }

    /// `/lane_set <lane:int32> <json>` -- replaces lane `lane`'s data whole
    /// (the JSON as a string or a blob, like a def's). What it had queued is
    /// cleared and fed again from the position; the notes it is sounding keep
    /// their releases. Replies `/done`.
    pub(in crate::osc::server) fn handle_lane_set(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let id = args.int()?;
        let zones = self.translator.midi.decoder.with_layout();
        let events = parse_events(crate::osc::args::json_payload(args.rest())?, zones)?;
        if !self.lanes.contains_key(&id) {
            return Err(format!("no lane {id}"));
        }
        self.clear_lane(id, false)?;
        if let Some(lane) = self.lanes.get_mut(&id) {
            lane.events = events;
        }
        self.feed_lane(id);
        self.done(from, "/lane_set");
        Ok(())
    }

    /// `/lane_free <lane:int32>` -- frees lane `lane`: what it queued is
    /// dropped and the notes it is sounding are released now. Replies `/done`.
    pub(in crate::osc::server) fn handle_lane_free(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let id = args.int()?;
        if !self.lanes.contains_key(&id) {
            return Err(format!("no lane {id}"));
        }
        self.free_lane(id)?;
        self.retune_timeout();
        self.done(from, "/lane_free");
        Ok(())
    }

    fn free_lane(&mut self, id: i32) -> Answer {
        self.clear_lane(id, true)?;
        self.lanes.remove(&id);
        Ok(())
    }

    /// Drops what lane `id` has queued, and with `release` the releases of
    /// the notes it is sounding run now. Its generation moves, so what comes
    /// back spent from before is not taken for what is queued after.
    fn clear_lane(&mut self, id: i32, release: bool) -> Answer {
        let Some(lane) = self.lanes.get_mut(&id) else {
            return Ok(());
        };
        lane.generation = lane.generation.wrapping_add(1);
        lane.queued.clear();
        lane.voices.clear();
        let transport = lane.transport;
        self.handle
            .send(Cmd::ClearLane {
                transport,
                lane: id,
                release,
            })
            .map_err(|_| "command FIFO full".to_string())
    }

    /// **Transport `k` jumped** -- a locate, a loop set or cleared, a pass
    /// that went back to its mark: every lane on it is cleared and fed again
    /// from where it now stands. The engine releases the sounding notes on a
    /// jump itself, so the clear leaves releases alone.
    pub(in crate::osc::server) fn lanes_jumped(&mut self, k: usize) {
        let ids: Vec<i32> = self
            .lanes
            .iter()
            .filter(|(_, lane)| lane.transport == k)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            if self.clear_lane(id, false).is_ok() {
                self.feed_lane(id);
            }
        }
    }

    /// Where transport `k` stands, as a reply would say: a locate sent and not
    /// yet applied counts as done.
    pub(in crate::osc::server) fn transport_position(&self, k: usize) -> u64 {
        let clock = self.handle.current_transport_samples(k);
        match self.transports[k].pending_locate {
            Some((position, sent_at)) if clock <= sent_at => position,
            _ => self.handle.current_transport_position(k),
        }
    }

    /// One turn's feed: every lane topped up ahead of its transport.
    pub(in crate::osc::server) fn feed_lanes(&mut self) {
        let ids: Vec<i32> = self.lanes.keys().copied().collect();
        for id in ids {
            self.feed_lane(id);
        }
    }

    /// Tops lane `id` up: every event within the lookahead of its transport's
    /// position, in playback order, that is not on the lane queue yet is built
    /// and sent there.
    fn feed_lane(&mut self, id: i32) {
        let Some(lane) = self.lanes.get(&id) else {
            return;
        };
        let k = lane.transport;
        let t = self.transports[k];
        let window = (LOOKAHEAD_SECS * self.info.actual_sample_rate) as u64;
        let looping = t.loop_span.map(|(a, b)| (a.max(0) as u64, b.max(0) as u64));
        let end = t.end_mark.map(|(e, _)| e.max(0) as u64);
        let spans = ahead(self.transport_position(k), window, looping, end);
        let mut due: Vec<u32> = Vec::new();
        for (from, to) in spans {
            let first = lane.events.partition_point(|(p, _)| *p < from);
            for (i, (position, _)) in lane.events.iter().enumerate().skip(first) {
                if *position >= to {
                    break;
                }
                if !lane.queued.contains(&(i as u32)) {
                    due.push(i as u32);
                }
            }
        }
        for i in due {
            match self.lane_entry(id, i) {
                Ok(Some(cmd)) => {
                    if self.handle.send(cmd).is_err() {
                        // A full FIFO: the next turn offers it again.
                        break;
                    }
                    if let Some(lane) = self.lanes.get_mut(&id) {
                        lane.queued.insert(i);
                    }
                }
                Ok(None) => {}
                Err(why) => {
                    warn!("lane {id}: event {i} was not built: {why}");
                    // Marked all the same, so a broken event is not rebuilt
                    // and warned about on every turn.
                    if let Some(lane) = self.lanes.get_mut(&id) {
                        lane.queued.insert(i);
                    }
                }
            }
        }
    }

    /// Builds lane `id`'s event `i` as the entry the engine takes: a note's
    /// `/synth_new` with a fresh id and its release, or a message.
    fn lane_entry(&mut self, id: i32, i: u32) -> Result<Option<Cmd>, String> {
        let Some(lane) = self.lanes.get(&id) else {
            return Ok(None);
        };
        let Some((position, event)) = lane.events.get(i as usize).cloned() else {
            return Ok(None);
        };
        let (transport, target) = (lane.transport, lane.target);
        let tag = EventLaneTag {
            lane: id,
            generation: lane.generation,
            event: i,
        };
        let mut start = Vec::new();
        let mut release = Vec::new();
        let mut length = 0;
        match event {
            EventLaneEvent::Note {
                voice,
                controls,
                length: held,
                release: how,
            } => {
                // A synth, or one more of a graph's slot -- whose group is the
                // note, released through the slot's `gate` port or freed whole.
                let message = match voice {
                    EventLaneVoice::Def(def) => {
                        let mut args = vec![
                            OscType::String(def),
                            OscType::Int(-1),
                            OscType::Int(1),
                            OscType::Int(target),
                        ];
                        args.extend(controls);
                        OscMessage {
                            addr: "/synth_new".into(),
                            args,
                        }
                    }
                    EventLaneVoice::Slot { graph, slot } => {
                        let mut args =
                            vec![OscType::Int(graph), OscType::String(slot), OscType::Int(-1)];
                        args.extend(controls);
                        OscMessage {
                            addr: "/graph_addSlot".into(),
                            args,
                        }
                    }
                };
                self.translator.translate(&message, &mut start)?;
                let Some(node) = start.iter().find_map(|cmd| match cmd {
                    Cmd::AddSynth { id, .. } | Cmd::AddGroup { id, .. } => Some(*id),
                    _ => None,
                }) else {
                    return Ok(None);
                };
                let message = match how {
                    Release::Gate => OscMessage {
                        addr: "/node_set".into(),
                        args: vec![
                            OscType::Int(node),
                            OscType::String("gate".into()),
                            OscType::Float(0.0),
                        ],
                    },
                    Release::Free => OscMessage {
                        addr: "/node_free".into(),
                        args: vec![OscType::Int(node)],
                    },
                };
                if let Err(why) = self.translator.translate(&message, &mut release) {
                    self.forget_unrun(&start);
                    return Err(why);
                }
                length = held;
            }
            EventLaneEvent::Message(message) => {
                self.translator.translate(&message, &mut start)?;
            }
            EventLaneEvent::MidiNote {
                channel,
                note,
                velocity,
                length: held,
                program,
                voice,
            } => {
                let Some(node) = self.translator.lane_midi_note(
                    channel,
                    note,
                    velocity,
                    program,
                    &mut start,
                    &mut release,
                )?
                else {
                    return Ok(None);
                };
                if let Some(lane) = self.lanes.get_mut(&id) {
                    lane.voices.insert(voice, node);
                }
                length = held;
            }
            EventLaneEvent::ZoneNote {
                master,
                key,
                velocity,
                bend,
                pressure,
                timbre,
                length: held,
                voice,
            } => {
                let Some(node) = self.translator.lane_zone_note(
                    master,
                    key,
                    velocity,
                    bend,
                    pressure,
                    timbre,
                    &mut start,
                    &mut release,
                )?
                else {
                    return Ok(None);
                };
                if let Some(lane) = self.lanes.get_mut(&id) {
                    lane.voices.insert(voice, node);
                }
                length = held;
            }
            EventLaneEvent::ZoneSet {
                master,
                key,
                voice,
                event,
            } => {
                let Some(node) = self
                    .lanes
                    .get(&id)
                    .and_then(|l| l.voices.get(&voice).copied())
                else {
                    return Ok(None);
                };
                self.translator
                    .lane_zone_set(master, key, node, event, &mut start);
                if start.is_empty() {
                    return Ok(None);
                }
            }
            EventLaneEvent::MidiSet { message, voices } => {
                let nodes: Vec<i32> = voices
                    .iter()
                    .filter_map(|v| self.lanes.get(&id)?.voices.get(v).copied())
                    .collect();
                self.translator.lane_midi_set(message, &nodes, &mut start);
                if start.is_empty() {
                    return Ok(None);
                }
            }
            EventLaneEvent::PerNote {
                channel,
                note,
                message,
                voice,
            } => {
                let Some(node) = self
                    .lanes
                    .get(&id)
                    .and_then(|lane| lane.voices.get(&voice).copied())
                else {
                    return Ok(None);
                };
                self.translator
                    .lane_per_note(node, channel, note, message, &mut start);
                if start.is_empty() {
                    return Ok(None);
                }
            }
        }
        Ok(Some(Cmd::EventLaneEntry {
            transport,
            position,
            tag,
            start,
            release,
            length,
        }))
    }

    /// **A lane entry left the engine**: it is no longer queued, and when it
    /// never ran, the nodes it would have made are forgotten.
    pub(in crate::osc::server) fn lane_spent(
        &mut self,
        tag: EventLaneTag,
        start: &[Cmd],
        fired: bool,
    ) {
        if !fired {
            self.forget_unrun(start);
        }
        if let Some(lane) = self.lanes.get_mut(&tag.lane)
            && lane.generation == tag.generation
        {
            lane.queued.remove(&tag.event);
        }
    }

    /// **Commands that were built and never ran** -- a cleared timed bundle, an
    /// unrun lane entry: the nodes they would have made never existed, so the
    /// mirror forgets them and a server-minted id goes back to its range. A
    /// shell that ran is empty and has nothing here.
    pub(in crate::osc::server) fn forget_unrun(&mut self, cmds: &[Cmd]) {
        for cmd in cmds {
            if let Cmd::AddSynth { id, .. } | Cmd::AddGroup { id, .. } = cmd {
                self.translator.forget_unrun_node(*id);
                self.translator.release_node_id(*id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_runs_straight_on_and_stops_at_the_end_mark() {
        assert_eq!(ahead(100, 50, None, None), vec![(100, 150)]);
        assert_eq!(ahead(100, 50, None, Some(120)), vec![(100, 120)]);
        assert!(ahead(130, 50, None, Some(120)).is_empty());
    }

    #[test]
    fn a_window_wraps_through_a_loop_and_covers_one_pass() {
        // 20 to the loop's end, and 30 more from its start.
        assert_eq!(
            ahead(80, 50, Some((0, 100)), None),
            vec![(80, 100), (0, 30)]
        );
        // A loop shorter than the window: one pass, never an event twice.
        assert_eq!(
            ahead(15, 50, Some((10, 30)), None),
            vec![(15, 30), (10, 15)]
        );
        // Past the loop's end the position runs on, as the engine's does.
        assert_eq!(ahead(120, 50, Some((0, 100)), None), vec![(120, 170)]);
    }

    #[test]
    fn a_lanes_data_reads_notes_and_messages_in_position_order() {
        let events = parse_events(
            br#"{"notes": [[480, 960, "sine", {"freq": 440, "amp": 0.1}, "free"]],
                 "messages": [[0, "/mark", 1, 2.5, "cue"]]}"#,
            Decoder::new(),
        )
        .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].0, 0);
        let EventLaneEvent::Message(m) = &events[0].1 else {
            panic!("the message first");
        };
        assert_eq!(
            m.args,
            vec![
                OscType::Int(1),
                OscType::Float(2.5),
                OscType::String("cue".into())
            ]
        );
        let EventLaneEvent::Note {
            length, release, ..
        } = &events[1].1
        else {
            panic!("then the note");
        };
        assert_eq!((*length, *release), (480, Release::Free));
        assert!(
            parse_events(br#"{"notes": [[0, 1, "x", {}, "slowly"]]}"#, Decoder::new()).is_err()
        );
    }
}
