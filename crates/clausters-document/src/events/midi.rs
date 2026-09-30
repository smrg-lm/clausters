//! **A MIDI file's messages and a sequence's curves**, each read into the
//! other.
//!
//! A MIDI file says a controller, a bend or a pressure as a stream of
//! messages, each the value from then on; a sequence says it as a curve.
//! Which curve depends on what the message is addressed to, which is what a
//! [`MidiSpec`] settles:
//!
//! - **a channel's stream** -- a CC, the bend, channel pressure -- is that
//!   channel's function, a **lane** whose target names the channel;
//! - **poly pressure** is one note's, the **expression** of the note sounding
//!   on its key;
//! - **in an MPE zone**, a member channel is one note's, so its bend,
//!   pressure and CC 74 are the expression of the note on it, and the master
//!   channel's streams are lanes over the whole zone.
//!
//! Every point is a step, since a message holds until the next. Writing goes
//! the other way: steps as they are and ramps sampled, emitting where the MIDI
//! value changes. The RPNs that configure a channel -- a bend range, an MPE
//! zone -- are read into the curves' values and the sequence's spec, and
//! written back from them, never kept as events.

use std::collections::HashMap;

use serde_json::{Value, json};

use clausters_core::envshape::SHAPE_STEP;
use clausters_core::event::render;

use super::{CurveKind, Event, EventSequence, MidiSpec};
use crate::multitrack::Automation;
use crate::multitrack::nodes::value_at;
use crate::{NodeId, Opaque, Point};

/// A channel's bend range, in semitones, when no RPN 0 sets it.
const CHANNEL_BEND: f64 = 2.0;

/// An MPE member channel's bend range, the specification's default.
const MEMBER_BEND: f64 = 48.0;

/// How finely a ramp is sampled when written, in beats.
const RAMP_STEP: f64 = 1.0 / 64.0;

/// The controllers that address a parameter (NRPN, RPN) and carry its value
/// (data entry): configuration, read into the curves and never an event.
const PARAMETER_CCS: [u8; 6] = [6, 38, 98, 99, 100, 101];

/// A point that holds until the next one.
fn step(at: f64, value: f64) -> Point {
    Point {
        at,
        value,
        data: Opaque(json!({"shape": SHAPE_STEP, "curve": 0.0})),
    }
}

/// The member channels of an MPE zone.
fn members(upper: bool, count: u8) -> Vec<u8> {
    let count = count.clamp(1, 15);
    if upper {
        (0..count).map(|i| 14 - i).collect()
    } else {
        (1..=count).collect()
    }
}

/// The master channel of an MPE zone.
fn master(upper: bool) -> u8 {
    if upper { 15 } else { 0 }
}

/// **The zone and the bend ranges a file's RPNs set**: the MPE Configuration
/// Message (RPN 6 on channel 1 or 16) declares a zone, RPN 0 a channel's
/// bend range. What is found first per channel is what holds.
fn configuration(messages: &[(f64, Vec<u8>)]) -> (Option<MidiSpec>, HashMap<u8, f64>) {
    let mut rpn: HashMap<u8, (u8, u8)> = HashMap::new();
    let mut zone = None;
    let mut ranges = HashMap::new();
    for (_, bytes) in messages {
        let [status, cc, value] = bytes.as_slice() else {
            continue;
        };
        if status & 0xF0 != 0xB0 {
            continue;
        }
        let channel = status & 0x0F;
        let entry = rpn.entry(channel).or_insert((127, 127));
        match cc {
            101 => entry.0 = *value,
            100 => entry.1 = *value,
            6 => match *entry {
                (0, 6) if (channel == 0 || channel == 15) && *value > 0 && zone.is_none() => {
                    zone = Some(MidiSpec::Mpe {
                        upper: channel == 15,
                        members: (*value).min(15),
                    });
                }
                (0, 0) => {
                    ranges.entry(channel).or_insert(f64::from(*value));
                }
                _ => {}
            },
            _ => {}
        }
    }
    (zone, ranges)
}

/// **A file's messages as a sequence's events and curves**, in beats: notes
/// paired as `render::from_midi_messages` pairs them, every stream a lane or a
/// note's expression, and the spec the file is written for.
pub(super) fn read(messages: &[(f64, Vec<u8>)]) -> (Vec<Event>, Vec<Automation>, MidiSpec) {
    let (zone, ranges) = configuration(messages);
    let spec = zone.unwrap_or(MidiSpec::Midi1);
    let zone_members = match spec {
        MidiSpec::Mpe { upper, members: n } => members(upper, n),
        _ => Vec::new(),
    };
    let zone_master = match spec {
        MidiSpec::Mpe { upper, .. } => Some(master(upper)),
        _ => None,
    };
    let range = |channel: u8| {
        ranges
            .get(&channel)
            .copied()
            .unwrap_or(if zone_members.contains(&channel) {
                MEMBER_BEND
            } else {
                CHANNEL_BEND
            })
    };
    let mut events: Vec<Event> = render::from_midi_messages(messages)
        .into_iter()
        .map(|(at, keys)| Event::new(at, Value::Object(keys)))
        .collect();
    // Each note's channel, key and span, to find the note a message is for.
    let spans: Vec<Option<(u8, u8, f64, f64)>> = events
        .iter()
        .map(|event| {
            let keys = event.keys();
            (render::Type::of(&keys) == render::Type::Note).then(|| {
                let channel = keys.get("channel").and_then(Value::as_f64).unwrap_or(0.0) as u8;
                let key = keys.get("midinote").and_then(Value::as_f64).unwrap_or(0.0) as u8;
                (
                    channel,
                    key,
                    event.at.0,
                    event.at.0 + render::sustain_of(&keys),
                )
            })
        })
        .collect();
    // The note a message at `at` is for: sounding on the channel (and key)
    // then, else the next to start there -- an MPE controller sends a note's
    // first bend and pressure just before its note-on.
    let note_for = |channel: u8, key: Option<u8>, at: f64| -> Option<usize> {
        let fits = |c: u8, k: u8| c == channel && key.is_none_or(|want| want == k);
        let sounding = spans.iter().enumerate().rev().find_map(|(i, span)| {
            let (c, k, from, to) = (*span)?;
            (fits(c, k) && from <= at && at < to.max(from + f64::EPSILON)).then_some(i)
        });
        sounding.or_else(|| {
            spans
                .iter()
                .enumerate()
                .filter_map(|(i, span)| {
                    let (c, k, from, _) = (*span)?;
                    (fits(c, k) && from >= at).then_some((i, from))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
        })
    };
    // Curves by what they are over: a lane by (channel or none, target), a
    // note's by (event index, target).
    let mut lanes: Vec<(Value, Vec<Point>)> = Vec::new();
    let mut notes: Vec<(usize, Value, Vec<Point>)> = Vec::new();
    let mut kept = Vec::with_capacity(events.len());
    let mut add_lane =
        |target: Value, point: Point| match lanes.iter_mut().find(|(t, _)| *t == target) {
            Some((_, points)) => points.push(point),
            None => lanes.push((target, vec![point])),
        };
    for (i, event) in events.iter().enumerate() {
        let keys = event.keys();
        let at = event.at.0;
        let command = keys.get("midicmd").and_then(Value::as_str).unwrap_or("");
        let channel = keys.get("channel").and_then(Value::as_f64).unwrap_or(0.0) as u8;
        let value = keys.get("value").and_then(Value::as_f64).unwrap_or(0.0);
        let cc = keys.get("cc").and_then(Value::as_f64).unwrap_or(-1.0);
        if command == "cc" && PARAMETER_CCS.contains(&(cc as u8)) {
            continue;
        }
        // What the message says, as a curve's target and value.
        let said = match command {
            "cc" if cc as u8 == 74 && zone_members.contains(&channel) => {
                Some((json!({"timbre": true}), value / 127.0))
            }
            "cc" => Some((json!({"cc": cc as u8}), value)),
            "bend" => Some((json!({"bend": true}), value / 8192.0 * range(channel))),
            "aftertouch" => Some((json!({"pressure": true}), value / 127.0)),
            "polytouch" => Some((json!({"pressure": true}), value / 127.0)),
            _ => None,
        };
        let Some((mut target, value)) = said else {
            kept.push(i);
            continue;
        };
        let per_note = command == "polytouch" || zone_members.contains(&channel);
        if per_note {
            let key = (command == "polytouch")
                .then(|| keys.get("midinote").and_then(Value::as_f64).unwrap_or(0.0) as u8);
            match note_for(channel, key, at) {
                Some(note) => {
                    let from = spans[note].map_or(0.0, |s| s.2);
                    let point = step((at - from).max(0.0), value);
                    match notes
                        .iter_mut()
                        .find(|(n, t, _)| *n == note && *t == target)
                    {
                        Some((_, _, points)) => points.push(point),
                        None => notes.push((note, target, vec![point])),
                    }
                }
                // A note's message with no note to be for says nothing a
                // curve can hold: it stays the message it was.
                None => kept.push(i),
            }
            continue;
        }
        // A channel's function; a zone's master is the whole zone's.
        if zone_master != Some(channel) {
            target["channel"] = json!(channel);
        }
        add_lane(target, step(at, value));
    }
    for (note, target, points) in notes {
        let mut curve = Automation::new(NodeId(0), Opaque(target));
        curve.points = points;
        events[note].expression.push(curve);
    }
    let events = events
        .into_iter()
        .enumerate()
        .filter(|(i, event)| {
            let keys = event.keys();
            render::Type::of(&keys) == render::Type::Note || kept.contains(i)
        })
        .map(|(_, event)| event)
        .collect();
    let lanes = lanes
        .into_iter()
        .map(|(target, points)| {
            let mut curve = Automation::new(NodeId(0), Opaque(target));
            curve.points = points;
            curve
        })
        .collect();
    (events, lanes, spec)
}

/// How a message sorts among those on its tick: configuration first, a
/// note's off before the next one's on, a note's first expression before its
/// on, then everything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Order {
    Configure,
    Off,
    Before,
    On,
    After,
}

/// The message a curve's value is on `channel`, for a target of `kind` -- a
/// note's `key` when it is poly pressure.
fn message(
    kind: CurveKind,
    channel: u8,
    key: Option<u8>,
    value: f64,
    range: f64,
) -> Option<Vec<u8>> {
    let seven = |v: f64| v.round().clamp(0.0, 127.0) as u8;
    let ch = channel & 0x0F;
    Some(match kind {
        CurveKind::Cc(n) => vec![0xB0 | ch, n, seven(value)],
        CurveKind::Timbre => vec![0xB0 | ch, 74, seven(value * 127.0)],
        CurveKind::Bend => {
            let wide = (value / range * 8192.0 + 8192.0)
                .round()
                .clamp(0.0, 16383.0) as u16;
            vec![0xE0 | ch, (wide & 0x7F) as u8, (wide >> 7) as u8]
        }
        CurveKind::Pressure => match key {
            Some(key) => vec![0xA0 | ch, key & 0x7F, seven(value * 127.0)],
            None => vec![0xD0 | ch, seven(value * 127.0)],
        },
        CurveKind::Control => return None,
    })
}

/// **A curve as the messages that say it**, from beat `from` to `to`: a
/// message at every point, and between two points of a ramp one wherever the
/// message it samples to changes.
fn sampled(
    points: &[Point],
    from: f64,
    to: f64,
    origin: f64,
    say: impl Fn(f64) -> Option<Vec<u8>>,
) -> Vec<(f64, Vec<u8>)> {
    let mut out: Vec<(f64, Vec<u8>)> = Vec::new();
    let mut emit = |at: f64, bytes: Option<Vec<u8>>| {
        if let Some(bytes) = bytes
            && out.last().is_none_or(|(_, last)| *last != bytes)
        {
            out.push((at, bytes));
        }
    };
    for (i, point) in points.iter().enumerate() {
        let at = origin + point.at;
        if at > to {
            break;
        }
        emit(at.max(from), say(point.value));
        let steps = point
            .data
            .0
            .get("shape")
            .and_then(Value::as_f64)
            .is_some_and(|s| s as i32 == SHAPE_STEP);
        let Some(next) = points.get(i + 1) else {
            continue;
        };
        if steps {
            continue;
        }
        let end = (origin + next.at).min(to);
        let mut t = at + RAMP_STEP;
        while t < end {
            emit(t, say(value_at(points, t - origin)));
            t += RAMP_STEP;
        }
    }
    out
}

/// **A sequence as the messages a file of it holds**, in beats, sorted: its
/// events, its lanes on their channels and its notes' expression -- as the
/// spec says it, MIDI 1.0 when it names none (and then a curve 1.0 cannot say
/// is left out, as an OSC event is). MIDI 2.0 has no voice in a 1.0 file of
/// its own, so it is written as MPE there: a member channel per note.
pub(super) fn write(sequence: &EventSequence) -> Vec<(f64, Vec<u8>)> {
    let spec = match sequence.midi {
        Some(MidiSpec::Midi2) => MidiSpec::Mpe {
            upper: false,
            members: 15,
        },
        Some(spec) => spec,
        None => MidiSpec::Midi1,
    };
    let mut out: Vec<(f64, Order, Vec<u8>)> = Vec::new();
    let (zone_members, zone_master) = match spec {
        MidiSpec::Mpe { upper, members: n } => {
            let master = master(upper);
            // The MPE Configuration Message: RPN 6 on the master, the member
            // count as its data.
            for (cc, value) in [(101, 0), (100, 6), (6, n.clamp(1, 15))] {
                out.push((0.0, Order::Configure, vec![0xB0 | master, cc, value]));
            }
            (members(upper, n), Some(master))
        }
        _ => (Vec::new(), None),
    };
    // A member channel per note, the one free longest -- a note's channel
    // is its own from its on to its off.
    let mut free_at: Vec<(u8, f64)> = zone_members.iter().map(|&c| (c, f64::MIN)).collect();
    let mut end = 0.0f64;
    let mut channels_used: Vec<u8> = Vec::new();
    for event in &sequence.events {
        let mut keys = event.keys();
        let at = event.at.0;
        let is_note = render::Type::of(&keys) == render::Type::Note;
        let sustain = render::sustain_of(&keys).max(0.0);
        let reach = event
            .expression
            .iter()
            .filter_map(|c| c.points.last())
            .map(|p| p.at)
            .fold(sustain, f64::max);
        end = end.max(at + reach);
        let channel = if is_note && !free_at.is_empty() {
            // The member free longest; with every one busy, the one free
            // soonest.
            let free = free_at
                .iter()
                .enumerate()
                .filter(|(_, (_, free))| *free <= at)
                .min_by(|a, b| a.1.1.total_cmp(&b.1.1))
                .or_else(|| {
                    free_at
                        .iter()
                        .enumerate()
                        .min_by(|a, b| a.1.1.total_cmp(&b.1.1))
                })
                .map_or(0, |(i, _)| i);
            free_at[free].1 = at + reach;
            let member = free_at[free].0;
            keys.insert("channel".into(), json!(member));
            member
        } else {
            keys.get("channel").and_then(Value::as_f64).unwrap_or(0.0) as u8
        };
        if is_note && !channels_used.contains(&channel) {
            channels_used.push(channel);
        }
        let Ok(messages) = render::midi(&keys, 0) else {
            continue;
        };
        for m in messages {
            let order = match (is_note, m.bytes.first().map(|s| s & 0xF0)) {
                (true, Some(0x90)) => Order::On,
                (true, Some(0x80)) => Order::Off,
                _ => Order::After,
            };
            out.push((at + m.at, order, m.bytes));
        }
        if !is_note {
            continue;
        }
        let key = keys
            .get("midinote")
            .and_then(Value::as_f64)
            .map(|k| k.round() as u8);
        for curve in event.expression.iter().filter(|c| c.enabled) {
            let kind = CurveKind::of(&curve.target.0);
            let (on, key) = match spec {
                MidiSpec::Midi1 if kind == CurveKind::Pressure => (channel, key),
                MidiSpec::Mpe { .. } if !matches!(kind, CurveKind::Cc(_) | CurveKind::Control) => {
                    (channel, None)
                }
                // What the spec cannot say of one note is left out.
                _ => continue,
            };
            let range = MEMBER_BEND;
            let said = sampled(&curve.points, at, at + reach, at, |v| {
                message(kind, on, key, v, range)
            });
            for (t, bytes) in said {
                // The first of a note's messages goes out before its on.
                let order = if t <= at { Order::Before } else { Order::After };
                out.push((t.max(at), order, bytes));
            }
        }
    }
    if channels_used.is_empty() {
        channels_used.push(0);
    }
    for lane in sequence.lanes.iter().filter(|l| l.enabled) {
        let kind = CurveKind::of(&lane.target.0);
        if kind == CurveKind::Control {
            continue;
        }
        let channels: Vec<u8> = match (zone_master, lane.target.0.get("channel")) {
            (Some(master), _) => vec![master],
            (None, Some(channel)) => vec![channel.as_f64().unwrap_or(0.0) as u8],
            (None, None) => channels_used.clone(),
        };
        let last = lane.points.last().map_or(0.0, |p| p.at);
        for channel in channels {
            let said = sampled(&lane.points, 0.0, last.max(end), 0.0, |v| {
                message(kind, channel, None, v, CHANNEL_BEND)
            });
            out.extend(said.into_iter().map(|(t, bytes)| (t, Order::After, bytes)));
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    out.into_iter().map(|(at, _, bytes)| (at, bytes)).collect()
}
