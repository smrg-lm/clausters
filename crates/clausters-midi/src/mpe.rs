//! MPE: the expression belongs to the note.
//!
//! MIDI Polyphonic Expression is a convention over MIDI 1.0's channel-voice
//! messages: a **zone** is a master channel and a run of member channels, a
//! controller puts one note on each member, and a bend, a pressure and a
//! third-dimension controller ("timbre", CC 74 unless the zone says otherwise)
//! on a member reach that one note. On the master they reach every note of the
//! zone. A zone is declared by RPN 6 (the MPE Configuration Message) on the
//! master, and a bend's range by RPN 0.
//!
//! Two halves live here, one per direction, both pure computation (no port, no
//! thread), so they build for wasm and are shared by the server, the GUI host
//! and the clients:
//!
//! - [`Decoder`] turns an incoming stream into **per-note** messages
//!   ([`MpeEvent`]), each naming a note by an id it mints at the note-on --
//!   the per-note model MIDI 2.0 has natively and MPE emulates by spending a
//!   channel per note. What is outside every zone comes back untouched as
//!   [`MpeEvent::Plain`], so a plain keyboard on a channel no zone covers
//!   arrives as it always did.
//! - [`Assigner`] picks the member channel an outgoing note goes on, and
//!   [`zone_messages`] writes the RPN that declares a zone at the head of a
//!   file or a port.
//!
//! Channels are 0-based here, as on the wire: the lower zone's master is
//! channel 0 (the specification's channel 1) with members ascending from 1;
//! the upper zone's master is channel 15 with members descending from 14.
//!
//! The C ABI shell is at the bottom: the decoder and the assigner cross as
//! opaque handles, so what they remember stays private (`docs/decisions.md`,
//! "The MPE decoder crosses as a handle").

use std::collections::VecDeque;

use clausters_core::midi::{widen_7_to_16, widen_7_to_32};

/// The member bend range a zone starts with, in semitones (the
/// specification's default).
pub const MEMBER_BEND_RANGE: f32 = 48.0;
/// The master bend range a zone starts with, in semitones.
pub const MASTER_BEND_RANGE: f32 = 2.0;
/// The controller that carries the third dimension unless a zone says
/// otherwise.
pub const TIMBRE_CC: u8 = 74;

/// Which of the two zones: the lower one's master is channel 0, the upper
/// one's channel 15.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    Lower,
    Upper,
}

impl Side {
    /// The master channel of this zone.
    #[must_use]
    pub fn master(self) -> u8 {
        match self {
            Self::Lower => 0,
            Self::Upper => 15,
        }
    }

    /// The zone whose master `channel` is, if it is one.
    #[must_use]
    pub fn of_master(channel: u8) -> Option<Self> {
        match channel {
            0 => Some(Self::Lower),
            15 => Some(Self::Upper),
            _ => None,
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Lower => 0,
            Self::Upper => 1,
        }
    }

    /// The member channels of a zone of `members`, in the order a note is
    /// assigned to them.
    #[must_use]
    pub fn members(self, members: u8) -> Vec<u8> {
        let n = members.min(15);
        match self {
            Self::Lower => (1..=n).collect(),
            Self::Upper => (0..n).map(|i| 14 - i).collect(),
        }
    }
}

/// A zone as the decoder holds it: how many members, the two bend ranges and
/// the controller that carries the third dimension.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zone {
    pub members: u8,
    pub member_range: f32,
    pub master_range: f32,
    pub timbre_cc: u8,
}

impl Zone {
    /// A zone of `members` with the specification's ranges and CC 74.
    #[must_use]
    pub fn new(members: u8) -> Self {
        Self {
            members: members.min(15),
            member_range: MEMBER_BEND_RANGE,
            master_range: MASTER_BEND_RANGE,
            timbre_cc: TIMBRE_CC,
        }
    }
}

/// One decoded message. A note is named by `note`, an id minted at its
/// note-on and never reused while the decoder lives.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MpeEvent {
    /// A note starts, with the state its channel was in: `bend` in semitones
    /// (its member bend and the zone's master bend, each through its range),
    /// and the pressure and timbre at MIDI 2.0 resolution.
    NoteOn {
        note: u32,
        side: Side,
        channel: u8,
        key: u8,
        velocity: u16,
        bend: f32,
        pressure: u32,
        timbre: u32,
    },
    /// A note ends, with its release velocity (the "lift").
    NoteOff { note: u32, velocity: u16 },
    /// A note's bend moved, in semitones from its key.
    Bend { note: u32, semitones: f32 },
    /// A note's pressure moved.
    Pressure { note: u32, value: u32 },
    /// A note's third dimension moved.
    Timbre { note: u32, value: u32 },
    /// Another controller on a note's channel (or on the master, for every
    /// note of the zone).
    Controller {
        note: u32,
        controller: u8,
        value: u32,
    },
    /// A message outside every zone, or one a zone does not decode (a program
    /// change), passed through as its three bytes.
    Plain([u8; 3]),
}

/// One sounding note.
#[derive(Debug, Clone, Copy)]
struct Sounding {
    note: u32,
    side: Side,
    channel: u8,
    key: u8,
}

/// What a channel carries between notes: the state a new note on it starts
/// with, and the RPN being assembled over CC 101/100/6/38.
#[derive(Debug, Clone, Copy)]
struct Channel {
    /// The bend, -1..1.
    bend: f32,
    pressure: u32,
    timbre: u32,
    rpn: Option<(u8, u8)>,
    data_msb: u8,
}

impl Default for Channel {
    fn default() -> Self {
        Self {
            bend: 0.0,
            pressure: 0,
            timbre: 0,
            rpn: None,
            data_msb: 0,
        }
    }
}

/// **The incoming half**: an MPE stream in, per-note messages out.
///
/// A zone is heard only where it has been configured ([`Decoder::set_zone`]):
/// the RPN that declares or resizes one is read on a configured master and
/// nowhere else, so a channel no zone covers passes every controller through
/// raw -- a binding that maps CC 6 keeps receiving it. A device's own RPN 6
/// then wins over the configured size, and a zone of zero members still plays
/// the notes on its master (with the master's bend as pitch), which is what a
/// device means by "plain MIDI again".
#[derive(Debug, Clone)]
pub struct Decoder {
    /// Each side's zone when configured.
    zones: [Option<Zone>; 2],
    /// The channels a zone may cover; an RPN that would widen one over another
    /// is refused.
    allowed: u16,
    channels: [Channel; 16],
    sounding: Vec<Sounding>,
    next_note: u32,
    out: VecDeque<MpeEvent>,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    /// A decoder with no zone: everything passes through.
    #[must_use]
    pub fn new() -> Self {
        Self {
            zones: [None, None],
            allowed: 0xFFFF,
            channels: [Channel::default(); 16],
            sounding: Vec::new(),
            next_note: 1,
            out: VecDeque::new(),
        }
    }

    /// A fresh decoder with this one's layout -- its zones and the channels
    /// they may reach -- and none of its state: no sounding note, no RPN half
    /// read. What decodes a stream written for the same zones from its start.
    #[must_use]
    pub fn with_layout(&self) -> Self {
        Self {
            zones: self.zones,
            allowed: self.allowed,
            ..Self::new()
        }
    }

    /// Configures a zone of `members` on `side` (0 waits for the device's own
    /// RPN 6, and plays notes on the master meanwhile), or removes it with
    /// `None`. The notes the zone was sounding end.
    pub fn set_zone(&mut self, side: Side, members: Option<u8>) {
        self.end_zone_notes(side);
        self.zones[side.index()] = members.map(Zone::new);
        if let Some(n) = members {
            self.shrink_other(side, n);
        }
    }

    /// Sets the controller a zone reads its third dimension from.
    pub fn set_timbre_cc(&mut self, side: Side, cc: u8) {
        if let Some(zone) = self.zones[side.index()].as_mut() {
            zone.timbre_cc = cc & 0x7F;
        }
    }

    /// The channels a zone may cover, as a bit per channel. An RPN 6 whose
    /// members would reach one outside it is refused, and the layout stands.
    pub fn set_allowed(&mut self, mask: u16) {
        self.allowed = mask;
    }

    /// The zone on `side`, if configured.
    #[must_use]
    pub fn zone(&self, side: Side) -> Option<Zone> {
        self.zones[side.index()]
    }

    /// The zone `channel` belongs to, as its master or a member.
    #[must_use]
    pub fn zone_of(&self, channel: u8) -> Option<Side> {
        [Side::Lower, Side::Upper].into_iter().find(|side| {
            self.zones[side.index()].is_some_and(|zone| {
                channel == side.master() || side.members(zone.members).contains(&channel)
            })
        })
    }

    /// Feeds one raw message (a status byte and up to two data bytes).
    pub fn feed(&mut self, bytes: &[u8]) {
        let Some(&status) = bytes.first() else {
            return;
        };
        let d1 = bytes.get(1).copied().unwrap_or(0) & 0x7F;
        let d2 = bytes.get(2).copied().unwrap_or(0) & 0x7F;
        let channel = status & 0x0F;
        let plain = MpeEvent::Plain([status, d1, d2]);
        if !(0x80..0xF0).contains(&status) {
            self.out.push_back(plain);
            return;
        }
        let Some(side) = self.zone_of(channel) else {
            self.out.push_back(plain);
            return;
        };
        let master = channel == side.master();
        match status & 0xF0 {
            0x90 if d2 > 0 => self.note_on(side, channel, d1, d2),
            0x80 | 0x90 => self.note_off(channel, d1, if status & 0xF0 == 0x80 { d2 } else { 0 }),
            0xE0 => {
                let raw = (u16::from(d2) << 7) | u16::from(d1);
                let bend = if raw >= 8192 {
                    f32::from(raw - 8192) / 8191.0
                } else {
                    -f32::from(8192 - raw) / 8192.0
                };
                self.channels[channel as usize].bend = bend;
                for s in self.reached(side, channel, master) {
                    let semitones = self.bend_of(s);
                    self.out.push_back(MpeEvent::Bend {
                        note: s.note,
                        semitones,
                    });
                }
            }
            0xD0 => {
                let value = widen_7_to_32(d1);
                self.channels[channel as usize].pressure = value;
                for s in self.reached(side, channel, master) {
                    self.out.push_back(MpeEvent::Pressure {
                        note: s.note,
                        value,
                    });
                }
            }
            0xA0 => {
                let value = widen_7_to_32(d2);
                if let Some(s) = self.newest(channel, d1) {
                    self.out.push_back(MpeEvent::Pressure {
                        note: s.note,
                        value,
                    });
                }
            }
            0xB0 => self.controller(side, channel, master, d1, d2),
            _ => self.out.push_back(plain),
        }
    }

    /// The next decoded message, or `None` when the queue is drained.
    pub fn poll(&mut self) -> Option<MpeEvent> {
        self.out.pop_front()
    }

    fn note_on(&mut self, side: Side, channel: u8, key: u8, velocity: u8) {
        // A key a channel already sounds is struck again: the old note ends.
        if let Some(s) = self.newest(channel, key) {
            self.remove(s.note);
            self.out.push_back(MpeEvent::NoteOff {
                note: s.note,
                velocity: 0,
            });
        }
        let note = self.next_note;
        self.next_note = self.next_note.wrapping_add(1).max(1);
        let sounding = Sounding {
            note,
            side,
            channel,
            key,
        };
        self.sounding.push(sounding);
        let state = self.channels[channel as usize];
        self.out.push_back(MpeEvent::NoteOn {
            note,
            side,
            channel,
            key,
            velocity: widen_7_to_16(velocity),
            bend: self.bend_of(sounding),
            pressure: state.pressure,
            timbre: state.timbre,
        });
    }

    fn note_off(&mut self, channel: u8, key: u8, velocity: u8) {
        if let Some(s) = self.newest(channel, key) {
            self.remove(s.note);
            self.out.push_back(MpeEvent::NoteOff {
                note: s.note,
                velocity: widen_7_to_16(velocity),
            });
        }
    }

    fn controller(&mut self, side: Side, channel: u8, master: bool, cc: u8, value: u8) {
        let ch = &mut self.channels[channel as usize];
        match cc {
            101 => ch.rpn = Some((value, ch.rpn.map_or(127, |(_, lsb)| lsb))),
            100 => ch.rpn = Some((ch.rpn.map_or(127, |(msb, _)| msb), value)),
            // An NRPN selection deselects the RPN.
            98 | 99 => ch.rpn = None,
            6 => {
                ch.data_msb = value;
                let rpn = ch.rpn;
                self.rpn(side, master, rpn, value, 0);
            }
            38 => {
                let (rpn, msb) = (ch.rpn, ch.data_msb);
                self.rpn(side, master, rpn, msb, value);
            }
            _ => {
                let zone = self.zones[side.index()].unwrap_or(Zone::new(0));
                let wide = widen_7_to_32(value);
                if cc == zone.timbre_cc {
                    self.channels[channel as usize].timbre = wide;
                }
                for s in self.reached(side, channel, master) {
                    self.out.push_back(if cc == zone.timbre_cc {
                        MpeEvent::Timbre {
                            note: s.note,
                            value: wide,
                        }
                    } else {
                        MpeEvent::Controller {
                            note: s.note,
                            controller: cc,
                            value: wide,
                        }
                    });
                }
            }
        }
    }

    /// An RPN's data arrived: RPN 0 sets a bend range (the member range on a
    /// member, the master's on the master) and RPN 6 on the master sizes the
    /// zone.
    fn rpn(&mut self, side: Side, master: bool, rpn: Option<(u8, u8)>, msb: u8, lsb: u8) {
        match rpn {
            Some((0, 0)) => {
                let range = f32::from(msb) + f32::from(lsb) / 100.0;
                if let Some(zone) = self.zones[side.index()].as_mut() {
                    if master {
                        zone.master_range = range;
                    } else {
                        zone.member_range = range;
                    }
                }
            }
            Some((0, 6)) if master && lsb == 0 => {
                let members = msb.min(15);
                let reach = side
                    .members(members)
                    .iter()
                    .all(|c| self.allowed & (1 << c) != 0);
                if reach {
                    self.end_zone_notes(side);
                    let timbre_cc = self.zones[side.index()].map_or(TIMBRE_CC, |z| z.timbre_cc);
                    self.zones[side.index()] = Some(Zone {
                        timbre_cc,
                        ..Zone::new(members)
                    });
                    self.shrink_other(side, members);
                }
            }
            _ => {}
        }
    }

    /// A zone grew to `members`: the other one keeps only the channels left
    /// (the two share fifteen), and loses its notes if it shrank.
    fn shrink_other(&mut self, side: Side, members: u8) {
        let other = match side {
            Side::Lower => Side::Upper,
            Side::Upper => Side::Lower,
        };
        let room = 14u8.saturating_sub(members);
        if let Some(zone) = self.zones[other.index()]
            && zone.members > room
        {
            self.end_zone_notes(other);
            self.zones[other.index()] = if members >= 15 {
                None
            } else {
                Some(Zone {
                    members: room,
                    ..zone
                })
            };
        }
    }

    fn end_zone_notes(&mut self, side: Side) {
        let ending: Vec<u32> = self
            .sounding
            .iter()
            .filter(|s| s.side == side)
            .map(|s| s.note)
            .collect();
        for note in ending {
            self.remove(note);
            self.out.push_back(MpeEvent::NoteOff { note, velocity: 0 });
        }
    }

    /// The notes a message on `channel` reaches: its own, or every note of the
    /// zone when it is the master.
    fn reached(&self, side: Side, channel: u8, master: bool) -> Vec<Sounding> {
        self.sounding
            .iter()
            .filter(|s| {
                if master {
                    s.side == side
                } else {
                    s.channel == channel
                }
            })
            .copied()
            .collect()
    }

    /// The newest note sounding `key` on `channel`.
    fn newest(&self, channel: u8, key: u8) -> Option<Sounding> {
        self.sounding
            .iter()
            .rev()
            .find(|s| s.channel == channel && s.key == key)
            .copied()
    }

    fn remove(&mut self, note: u32) {
        self.sounding.retain(|s| s.note != note);
    }

    /// A note's bend in semitones: its channel's bend through the member
    /// range (the master's own range for a note on the master), plus the
    /// master's bend through the master range.
    fn bend_of(&self, s: Sounding) -> f32 {
        let zone = self.zones[s.side.index()].unwrap_or(Zone::new(0));
        let master = self.channels[s.side.master() as usize].bend * zone.master_range;
        if s.channel == s.side.master() {
            master
        } else {
            self.channels[s.channel as usize].bend * zone.member_range + master
        }
    }
}

/// **The outgoing half**: which member channel a note goes on.
///
/// Round robin over the members, preferring a channel with nothing sounding;
/// when every member is busy, the one whose note has been held longest is
/// reused.
#[derive(Debug, Clone)]
pub struct Assigner {
    members: Vec<u8>,
    /// Per member: what it sounds, as `(key, stamp)`.
    sounding: Vec<Vec<(u8, u64)>>,
    next: usize,
    stamp: u64,
}

impl Assigner {
    /// An assigner over the members of a zone of `members` on `side`.
    #[must_use]
    pub fn new(side: Side, members: u8) -> Self {
        let members = side.members(members);
        let sounding = vec![Vec::new(); members.len()];
        Self {
            members,
            sounding,
            next: 0,
            stamp: 0,
        }
    }

    /// The channel for a new note on `key`; `None` for a zone of no members.
    pub fn note_on(&mut self, key: u8) -> Option<u8> {
        let n = self.members.len();
        if n == 0 {
            return None;
        }
        let free = (0..n)
            .map(|i| (self.next + i) % n)
            .find(|&i| self.sounding[i].is_empty());
        let slot = free.unwrap_or_else(|| {
            (0..n)
                .min_by_key(|&i| self.sounding[i].iter().map(|(_, t)| *t).min().unwrap_or(0))
                .unwrap_or(0)
        });
        self.stamp += 1;
        self.sounding[slot].push((key, self.stamp));
        self.next = (slot + 1) % n;
        Some(self.members[slot])
    }

    /// A note on `key` ended on `channel`.
    pub fn note_off(&mut self, channel: u8, key: u8) {
        if let Some(i) = self.members.iter().position(|&c| c == channel) {
            let held = &mut self.sounding[i];
            if let Some(j) = held.iter().rposition(|(k, _)| *k == key) {
                held.remove(j);
            }
        }
    }
}

/// The messages that declare a zone of `members` on `side` (RPN 6 on its
/// master, then the null RPN), for the head of a file or a port.
#[must_use]
pub fn zone_messages(side: Side, members: u8) -> Vec<[u8; 3]> {
    let cc = 0xB0 | side.master();
    vec![
        [cc, 101, 0],
        [cc, 100, 6],
        [cc, 6, members.min(15)],
        [cc, 101, 127],
        [cc, 100, 127],
    ]
}

/// A bend of `semitones` on `channel` through `range`, as the 14-bit pitch
/// bend message.
#[must_use]
pub fn bend_message(channel: u8, semitones: f32, range: f32) -> [u8; 3] {
    let norm = if range > 0.0 {
        (semitones / range).clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let raw = if norm >= 0.0 {
        8192.0 + norm * 8191.0
    } else {
        8192.0 + norm * 8192.0
    }
    .round() as u16;
    [
        0xE0 | (channel & 0x0F),
        (raw & 0x7F) as u8,
        (raw >> 7) as u8,
    ]
}

/// **A note's starting expression** on its member `channel`, sent just
/// before its note-on so a reused channel does not carry the last note's: the
/// bend in semitones through the member `range`, the pressure and the timbre
/// (on `timbre_cc`), each 0..1. A dimension the note does not state (`None`)
/// goes back to its rest -- no bend, no pressure, the timbre centred.
#[must_use]
pub fn expression_messages(
    channel: u8,
    bend: f32,
    range: f32,
    pressure: Option<f32>,
    timbre: Option<f32>,
    timbre_cc: u8,
) -> Vec<[u8; 3]> {
    let ch = channel & 0x0F;
    let seven = |v: f32| (v.clamp(0.0, 1.0) * 127.0).round() as u8;
    vec![
        bend_message(ch, bend, range),
        [0xD0 | ch, pressure.map_or(0, seven), 0],
        [0xB0 | ch, timbre_cc & 0x7F, timbre.map_or(64, seven)],
    ]
}

// ---- C ABI ----

fn side_of(upper: i32) -> Side {
    if upper != 0 { Side::Upper } else { Side::Lower }
}

/// A new decoder with no zone. Free it with [`clausters_mpe_decoder_free`].
#[unsafe(no_mangle)]
pub extern "C" fn clausters_mpe_decoder_new() -> *mut Decoder {
    Box::into_raw(Box::new(Decoder::new()))
}

/// Frees a decoder.
///
/// # Safety
/// `handle` must come from [`clausters_mpe_decoder_new`] and not be used after.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_mpe_decoder_free(handle: *mut Decoder) {
    if !handle.is_null() {
        // SAFETY: per the contract.
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// Configures the lower (`upper` 0) or upper zone with `members` (0 waits for
/// the device's RPN), or removes it when `members` is negative.
///
/// # Safety
/// `handle` must be a live decoder.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_mpe_decoder_set_zone(
    handle: *mut Decoder,
    upper: i32,
    members: i32,
) {
    if handle.is_null() {
        return;
    }
    // SAFETY: per the contract.
    let decoder = unsafe { &mut *handle };
    let members = u8::try_from(members).ok().map(|m| m.min(15));
    decoder.set_zone(side_of(upper), members);
}

/// Feeds one raw message of `len` bytes.
///
/// # Safety
/// `handle` must be a live decoder and `bytes` readable for `len`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_mpe_decoder_feed(
    handle: *mut Decoder,
    bytes: *const u8,
    len: usize,
) {
    if handle.is_null() || bytes.is_null() {
        return;
    }
    // SAFETY: per the contract.
    let (decoder, bytes) = unsafe { (&mut *handle, std::slice::from_raw_parts(bytes, len)) };
    decoder.feed(bytes);
}

/// The size of one polled record, in bytes.
pub const MPE_RECORD: usize = 12;

/// Dequeues one decoded message into `out` as a 12-byte record and returns 1,
/// or 0 when the queue is empty (-1 on a bad argument). The record is `kind`
/// (0 plain, 1 note-on, 2 note-off, 3 bend, 4 pressure, 5 timbre, 6
/// controller), `channel`, `key`, `controller` (for a plain message these
/// three are its bytes), the note id as a little-endian `u32`, and a
/// little-endian `f32` value: the velocity 0..1 for a note-on or off, the
/// semitones for a bend, 0..1 for the rest.
///
/// # Safety
/// `handle` must be a live decoder and `out` writable for `cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_mpe_decoder_poll(
    handle: *mut Decoder,
    out: *mut u8,
    cap: usize,
) -> i32 {
    if handle.is_null() || out.is_null() || cap < MPE_RECORD {
        return -1;
    }
    // SAFETY: per the contract.
    let decoder = unsafe { &mut *handle };
    let Some(event) = decoder.poll() else {
        return 0;
    };
    let record = record(event);
    // SAFETY: `out` holds at least MPE_RECORD bytes.
    unsafe { std::ptr::copy_nonoverlapping(record.as_ptr(), out, MPE_RECORD) };
    1
}

fn record(event: MpeEvent) -> [u8; MPE_RECORD] {
    let unit16 = |v: u16| f32::from(v) / f32::from(u16::MAX);
    let unit32 = |v: u32| (f64::from(v) / f64::from(u32::MAX)) as f32;
    let (head, note, value) = match event {
        MpeEvent::Plain(bytes) => ([0, bytes[0], bytes[1], bytes[2]], 0, 0.0),
        MpeEvent::NoteOn {
            note,
            channel,
            key,
            velocity,
            ..
        } => ([1, channel, key, 0], note, unit16(velocity)),
        MpeEvent::NoteOff { note, velocity } => ([2, 0, 0, 0], note, unit16(velocity)),
        MpeEvent::Bend { note, semitones } => ([3, 0, 0, 0], note, semitones),
        MpeEvent::Pressure { note, value } => ([4, 0, 0, 0], note, unit32(value)),
        MpeEvent::Timbre { note, value } => ([5, 0, 0, 0], note, unit32(value)),
        MpeEvent::Controller {
            note,
            controller,
            value,
        } => ([6, 0, 0, controller], note, unit32(value)),
    };
    let mut out = [0u8; MPE_RECORD];
    out[..4].copy_from_slice(&head);
    out[4..8].copy_from_slice(&note.to_le_bytes());
    out[8..].copy_from_slice(&value.to_le_bytes());
    out
}

/// A new assigner over the lower (`upper` 0) or upper zone of `members`. Free
/// it with [`clausters_mpe_assigner_free`].
#[unsafe(no_mangle)]
pub extern "C" fn clausters_mpe_assigner_new(upper: i32, members: i32) -> *mut Assigner {
    let members = u8::try_from(members.clamp(0, 15)).unwrap_or(0);
    Box::into_raw(Box::new(Assigner::new(side_of(upper), members)))
}

/// Frees an assigner.
///
/// # Safety
/// `handle` must come from [`clausters_mpe_assigner_new`] and not be used after.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_mpe_assigner_free(handle: *mut Assigner) {
    if !handle.is_null() {
        // SAFETY: per the contract.
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// The channel a new note on `key` goes on, or -1 for a zone of no members.
///
/// # Safety
/// `handle` must be a live assigner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_mpe_assigner_note_on(handle: *mut Assigner, key: u8) -> i32 {
    if handle.is_null() {
        return -1;
    }
    // SAFETY: per the contract.
    let assigner = unsafe { &mut *handle };
    assigner.note_on(key).map_or(-1, i32::from)
}

/// A note on `key` ended on `channel`.
///
/// # Safety
/// `handle` must be a live assigner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_mpe_assigner_note_off(
    handle: *mut Assigner,
    channel: u8,
    key: u8,
) {
    if handle.is_null() {
        return;
    }
    // SAFETY: per the contract.
    let assigner = unsafe { &mut *handle };
    assigner.note_off(channel, key);
}

/// Writes the messages that declare a zone of `members` on the lower (`upper`
/// 0) or upper side into `out`, three bytes each, and returns how many bytes
/// it wrote (-1 when `cap` is too small).
///
/// # Safety
/// `out` must be writable for `cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_mpe_zone_messages(
    upper: i32,
    members: i32,
    out: *mut u8,
    cap: usize,
) -> i32 {
    let members = u8::try_from(members.clamp(0, 15)).unwrap_or(0);
    let bytes: Vec<u8> = zone_messages(side_of(upper), members).concat();
    if out.is_null() || cap < bytes.len() {
        return -1;
    }
    // SAFETY: `out` holds at least `bytes.len()` bytes.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len()) };
    bytes.len() as i32
}

/// The 14-bit bend message for `semitones` on `channel` through `range`, into
/// `out` (three bytes).
///
/// # Safety
/// `out` must be writable for three bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_mpe_bend_message(
    channel: u8,
    semitones: f32,
    range: f32,
    out: *mut u8,
) {
    if out.is_null() {
        return;
    }
    let msg = bend_message(channel, semitones, range);
    // SAFETY: `out` holds three bytes.
    unsafe { std::ptr::copy_nonoverlapping(msg.as_ptr(), out, 3) };
}

/// A note's starting expression (see [`expression_messages`]) into `out`,
/// three bytes a message, returning how many bytes it wrote (-1 when `cap` is
/// too small). A negative `pressure` or `timbre` is one the note does not
/// state.
///
/// # Safety
/// `out` must be writable for `cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_mpe_expression_messages(
    channel: u8,
    bend: f32,
    range: f32,
    pressure: f32,
    timbre: f32,
    timbre_cc: u8,
    out: *mut u8,
    cap: usize,
) -> i32 {
    let stated = |v: f32| (v >= 0.0).then_some(v);
    let bytes: Vec<u8> = expression_messages(
        channel,
        bend,
        range,
        stated(pressure),
        stated(timbre),
        timbre_cc,
    )
    .concat();
    if out.is_null() || cap < bytes.len() {
        return -1;
    }
    // SAFETY: `out` holds at least `bytes.len()` bytes.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len()) };
    bytes.len() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(d: &mut Decoder) -> Vec<MpeEvent> {
        std::iter::from_fn(|| d.poll()).collect()
    }

    fn declare(d: &mut Decoder, members: u8) {
        for msg in zone_messages(Side::Lower, members) {
            d.feed(&msg);
        }
    }

    #[test]
    fn without_a_zone_everything_passes_through() {
        let mut d = Decoder::new();
        d.feed(&[0x91, 60, 100]);
        d.feed(&[0xB1, 6, 12]);
        assert_eq!(
            drain(&mut d),
            vec![
                MpeEvent::Plain([0x91, 60, 100]),
                MpeEvent::Plain([0xB1, 6, 12])
            ]
        );
    }

    #[test]
    fn an_rpn_on_an_unconfigured_master_declares_nothing() {
        let mut d = Decoder::new();
        declare(&mut d, 15);
        assert_eq!(d.zone(Side::Lower), None);
        assert!(
            drain(&mut d)
                .iter()
                .all(|e| matches!(e, MpeEvent::Plain(_)))
        );
    }

    #[test]
    fn a_device_sizes_a_configured_zone_and_a_member_bend_moves_its_note_only() {
        let mut d = Decoder::new();
        d.set_zone(Side::Lower, Some(0));
        declare(&mut d, 4);
        assert_eq!(d.zone(Side::Lower).map(|z| z.members), Some(4));
        drain(&mut d);
        d.feed(&[0x91, 60, 100]);
        d.feed(&[0x92, 64, 100]);
        let ids: Vec<u32> = drain(&mut d)
            .into_iter()
            .filter_map(|e| match e {
                MpeEvent::NoteOn { note, .. } => Some(note),
                _ => None,
            })
            .collect();
        // Full bend up on channel 1: its member range, 48 semitones.
        d.feed(&[0xE1, 0x7F, 0x7F]);
        assert_eq!(
            drain(&mut d),
            vec![MpeEvent::Bend {
                note: ids[0],
                semitones: 48.0
            }]
        );
        // Full bend down on the master: every note, by the master range.
        d.feed(&[0xE0, 0, 0]);
        let bends = drain(&mut d);
        assert_eq!(
            bends,
            vec![
                MpeEvent::Bend {
                    note: ids[0],
                    semitones: 46.0
                },
                MpeEvent::Bend {
                    note: ids[1],
                    semitones: -2.0
                }
            ]
        );
    }

    #[test]
    fn pressure_timbre_and_a_note_off_reach_the_note_on_their_channel() {
        let mut d = Decoder::new();
        d.set_zone(Side::Lower, Some(15));
        d.feed(&[0xD3, 64, 0]);
        d.feed(&[0x93, 60, 100]);
        let on = drain(&mut d);
        let MpeEvent::NoteOn { note, pressure, .. } = on[0] else {
            panic!("{on:?}");
        };
        assert_eq!(
            pressure,
            widen_7_to_32(64),
            "a note starts with its channel's pressure"
        );
        d.feed(&[0xB3, 74, 127]);
        d.feed(&[0x83, 60, 30]);
        assert_eq!(
            drain(&mut d),
            vec![
                MpeEvent::Timbre {
                    note,
                    value: u32::MAX
                },
                MpeEvent::NoteOff {
                    note,
                    velocity: widen_7_to_16(30)
                }
            ]
        );
    }

    #[test]
    fn a_zone_of_zero_members_plays_its_master_with_the_master_bend() {
        let mut d = Decoder::new();
        d.set_zone(Side::Lower, Some(0));
        d.feed(&[0xE0, 0x7F, 0x7F]);
        d.feed(&[0x90, 60, 100]);
        let on = drain(&mut d);
        assert!(
            matches!(on[..], [MpeEvent::NoteOn { bend, .. }] if bend == 2.0),
            "{on:?}"
        );
        // Channel 1 is no member of a zone of none.
        d.feed(&[0x91, 60, 100]);
        assert_eq!(drain(&mut d), vec![MpeEvent::Plain([0x91, 60, 100])]);
    }

    #[test]
    fn an_rpn_that_would_widen_a_zone_over_a_refused_channel_is_ignored() {
        let mut d = Decoder::new();
        d.set_zone(Side::Lower, Some(2));
        d.set_allowed(!(1 << 5));
        declare(&mut d, 8);
        assert_eq!(d.zone(Side::Lower).map(|z| z.members), Some(2));
    }

    #[test]
    fn rpn_0_sets_the_member_range() {
        let mut d = Decoder::new();
        d.set_zone(Side::Lower, Some(15));
        for msg in [
            [0xB1, 101, 0],
            [0xB1, 100, 0],
            [0xB1, 6, 12],
            [0xB1, 38, 50],
        ] {
            d.feed(&msg);
        }
        assert_eq!(d.zone(Side::Lower).map(|z| z.member_range), Some(12.5));
    }

    #[test]
    fn the_assigner_prefers_a_free_channel_then_the_longest_held() {
        let mut a = Assigner::new(Side::Lower, 3);
        assert_eq!(
            [a.note_on(60), a.note_on(62), a.note_on(64)],
            [Some(1), Some(2), Some(3)]
        );
        // Every member busy: the longest held (channel 1) is reused.
        assert_eq!(a.note_on(65), Some(1));
        a.note_off(2, 62);
        assert_eq!(a.note_on(67), Some(2), "the free one first");
        assert_eq!(Assigner::new(Side::Upper, 2).note_on(60), Some(14));
    }

    #[test]
    fn a_notes_expression_reaches_it_through_the_decoder() {
        let mut d = Decoder::new();
        d.set_zone(Side::Lower, Some(15));
        for msg in expression_messages(3, -5.0, MEMBER_BEND_RANGE, Some(1.0), None, TIMBRE_CC) {
            d.feed(&msg);
        }
        d.feed(&[0x93, 60, 100]);
        let on = drain(&mut d);
        let MpeEvent::NoteOn {
            bend,
            pressure,
            timbre,
            ..
        } = on[0]
        else {
            panic!("{on:?}");
        };
        assert!((bend + 5.0).abs() < 0.01, "{bend}");
        assert_eq!(pressure, u32::MAX);
        assert_eq!(timbre, widen_7_to_32(64), "an unstated timbre is centred");
    }

    #[test]
    fn a_bend_message_round_trips_through_the_decoder() {
        let mut d = Decoder::new();
        d.set_zone(Side::Lower, Some(15));
        d.feed(&bend_message(1, 12.0, MEMBER_BEND_RANGE));
        d.feed(&[0x91, 60, 100]);
        let on = drain(&mut d);
        let MpeEvent::NoteOn { bend, .. } = on[0] else {
            panic!("{on:?}");
        };
        assert!((bend - 12.0).abs() < 0.01, "{bend}");
    }
}
