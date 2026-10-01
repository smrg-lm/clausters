//! **A roll over an event sequence**: what it draws, and what a hand's gesture
//! on it means -- note by note, by id.
//!
//! A roll is a plane of events: time on X, a **domain** on Y. What the domain is
//! -- which key of an event the Y axis reads and writes, on what scale, over
//! what window -- is data ([`YDomain`]), so a roll over MIDI notes and one over
//! frequencies are the same projection and the same intake with a different
//! domain. What a moved note *means* is then one key written with its family's
//! coherence (`clausters_core::event::render::set_key`): a note dragged a
//! semitone up in a roll of `midinote`s also moves the `freq` and the `degree`
//! it was written with.
//!
//! # By id, not by order
//!
//! Every note on the wire carries the id its event has in the
//! [`EventSequence`], so an edit is matched to the note it names: removing one
//! note leaves every other note its own data, which the whole-list reading
//! before this ([`crate::events`], matched by order) did not. A note the hand
//! made arrives with id 0 and becomes a new event.
//!
//! # The wire
//!
//! - `notes`: flat quintuples `start dur y velocity channel`, in the view's
//!   units -- an [`Axis`]: the sequence's beats through its tempo map to
//!   seconds, and seconds to the samples the roll counts -- as the roll has
//!   always drawn them, `y` in the domain's key.
//! - `note_ids`: the id of each, in the same order.
//! - A `notes` report comes back as sextuples, `id start dur y velocity channel`,
//!   every note as the hand left it.
//! - `osc`: the marker lane, `time label` pairs, and its report the same --
//!   matched by label, since a marker is the message it sends.
//! - `curves`: the sequence's automation (CC, bend, pressure, a control, each on
//!   one channel or on every one), flat
//!   `name label min max height` quintuples, a row each under the plane;
//!   `layers`: each note's own curves, flat `name note label min max pitch`
//!   sextuples (`pitch` for a bend, drawn in the plane over its range);
//!   `points`: every curve's break-points, flat `name at value shape curve`,
//!   `at` in view units -- a note's curve measured from the note's start, and
//!   free to run past its off into the release. A curve's name is its id. A
//!   `points` report comes back in the same shape, every curve's points as the
//!   hand left them.
//! - `midi`: the MIDI spec the sequence is written for, as a reader names it
//!   (`MIDI 1.0`, `MPE`, `MIDI 2.0`), empty for a sequence for the server --
//!   the roll shows it beside its ruler.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use clausters_core::event::amp_of_velocity;
use clausters_core::event::render::{self, Type};
use clausters_core::tempomap::TempoMap;
use clausters_document::events::{Event, EventSequence, EventsIntent};
use clausters_document::multitrack::Automation;
use clausters_document::{Beat, Opaque};

use crate::events::{PAIR, label_of};
use crate::intake::{Intake, groups, number, text};

/// What the `notes` widget reports per note, with its id first.
pub const SEXTUPLE: usize = 6;

/// How a domain maps its values onto the axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scale {
    /// Equal steps: semitones, a control's value.
    #[default]
    Linear,
    /// Equal ratios: frequency in Hz.
    Log,
    /// Named rows with no order between them: a drum map.
    Categorical,
}

/// **What the Y axis of a roll is**: the key it reads and writes, the scale it
/// is drawn on, the ruler that labels it, the window it shows, and the height
/// of one step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct YDomain {
    /// The event key the axis is -- `midinote`, `freq`, any numeric key.
    pub key: String,
    /// How values are laid out.
    #[serde(default)]
    pub scale: Scale,
    /// What labels the axis: `keyboard`, `hz`, `names`, or a caller's word.
    #[serde(default)]
    pub ruler: String,
    /// The lowest value shown.
    pub min: f64,
    /// The highest.
    pub max: f64,
    /// One step: a semitone, a row. What a drag snaps to vertically.
    #[serde(default = "one")]
    pub quantum: f64,
}

fn one() -> f64 {
    1.0
}

impl Default for YDomain {
    fn default() -> Self {
        Self::midi()
    }
}

impl YDomain {
    /// MIDI notes in semitone rows, labelled by a keyboard: the roll's first
    /// domain, and every roll's until it says otherwise.
    pub fn midi() -> Self {
        Self {
            key: "midinote".into(),
            scale: Scale::Linear,
            ruler: "keyboard".into(),
            min: 0.0,
            max: 127.0,
            quantum: 1.0,
        }
    }

    /// Frequency in Hz on a log scale: the same pitch in another coordinate.
    pub fn hz(min: f64, max: f64) -> Self {
        Self {
            key: "freq".into(),
            scale: Scale::Log,
            ruler: "hz".into(),
            min,
            max,
            quantum: 0.0,
        }
    }

    /// Where an event sits on this axis, or `None` for one this roll does not
    /// draw as a note -- a rest, a raw message, an event without the key.
    pub fn value(&self, keys: &Map<String, Value>) -> Option<f64> {
        if Type::of(keys) != Type::Note {
            return None;
        }
        let pitch = || (render::pitch_of(keys), render::scale_of(keys));
        match self.key.as_str() {
            // The pitch family is read through its own resolution, so a note
            // written by degree or in Hz draws where it sounds.
            "midinote" => {
                let (p, s) = pitch();
                Some(p.midinote(&s))
            }
            "freq" => {
                let (p, s) = pitch();
                Some(p.freq(&s))
            }
            key => keys.get(key).and_then(Value::as_f64),
        }
    }
}

/// **Where a beat is on the roll's axis**: through the tempo map to seconds,
/// and seconds times the rate the roll counts in. A tempo that changes along
/// the sequence is what makes this a map and not a factor.
#[derive(Debug, Clone, PartialEq)]
pub struct Axis {
    /// The beat-to-second map.
    pub map: TempoMap,
    /// View units a second: the sample rate the roll's axis reads.
    pub rate: f64,
}

impl Axis {
    /// The axis a sequence is drawn on: its own tempo map (one beat a second
    /// when it states none) at `rate` units a second.
    pub fn of(sequence: &EventSequence, rate: f64) -> Self {
        Self {
            map: sequence
                .tempo_map
                .clone()
                .unwrap_or_else(|| TempoMap::new(1.0)),
            rate: if rate > 0.0 { rate } else { 1.0 },
        }
    }

    /// A constant axis of `units` a beat -- one beat a second, `units` a
    /// second.
    pub fn constant(units: f64) -> Self {
        Self {
            map: TempoMap::new(1.0),
            rate: if units > 0.0 { units } else { 1.0 },
        }
    }

    /// A beat, on the axis.
    pub fn units(&self, beat: f64) -> f64 {
        self.map.secs_at(beat) * self.rate
    }

    /// A place on the axis, as a beat.
    pub fn beat(&self, units: f64) -> f64 {
        self.map.beats_at(units / self.rate)
    }
}

/// What a roll over a sequence draws.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Projection {
    /// Flat `start dur y velocity channel` quintuples, in view units.
    pub notes: Vec<f64>,
    /// The id of each note, in the same order.
    pub note_ids: Vec<u64>,
    /// The marker lane: `time label` pairs.
    pub osc: Vec<Value>,
    /// The sequence's automation: `name label min max height` quintuples.
    pub curves: Vec<Value>,
    /// The notes' curves: `name note label min max pitch` sextuples.
    pub layers: Vec<Value>,
    /// Every curve's points: `name at value shape curve` quintuples.
    pub points: Vec<Value>,
    /// The MIDI spec the sequence is written for, as a reader names it
    /// (`MIDI 1.0`, `MPE`, `MIDI 2.0`), or empty for a sequence for the server.
    pub midi: String,
}

/// How tall a sequence curve's row under the plane is drawn.
pub const CURVE_H: f64 = 40.0;

/// **What a curve is drawn over**: its label, its value range, and whether
/// it is a bend (drawn in the plane when it is a note's). The target says
/// what it moves -- `{"cc": n}` (0 to 127), `{"bend": ...}` (semitones, 2
/// either way unless the target says), `{"pressure": ...}` or `{"timbre":
/// ...}` (0 to 1), `{"control": name}` -- and an explicit `min`/`max` on it,
/// or a name on the curve, wins. A sequence curve's `channel` (the notes' own count,
/// from 0) is the channel it acts on, and none is every channel.
fn curve_look(curve: &Automation) -> (String, f64, f64, bool) {
    let target = curve.target.0.as_object();
    let has = |key: &str| target.is_some_and(|t| t.contains_key(key));
    let read = |key: &str| target.and_then(|t| t.get(key)).and_then(Value::as_f64);
    let (label, min, max, pitch) = if let Some(cc) = read("cc") {
        (format!("CC {cc}"), 0.0, 127.0, false)
    } else if has("bend") {
        ("bend".to_string(), -2.0, 2.0, true)
    } else if has("pressure") {
        ("pressure".to_string(), 0.0, 1.0, false)
    } else if has("timbre") {
        ("timbre".to_string(), 0.0, 1.0, false)
    } else {
        let name = target
            .and_then(|t| t.get("control"))
            .and_then(Value::as_str)
            .unwrap_or("curve");
        (name.to_string(), 0.0, 1.0, false)
    };
    // A curve's channel is part of what it is on, as in MIDI, and shown the
    // way MIDI shows a channel: counted from 1.
    let label = match read("channel") {
        Some(channel) => format!("{label} ch {}", channel + 1.0),
        None => label,
    };
    (
        curve.name.clone().unwrap_or(label),
        read("min").unwrap_or(min),
        read("max").unwrap_or(max),
        pitch,
    )
}

/// A point as the wire carries it, `at` already in view units.
fn point_values(name: &str, at: f64, point: &clausters_document::Point) -> [Value; 5] {
    let data = point.data.0.as_object();
    let read = |key: &str, default: f64| {
        data.and_then(|d| d.get(key))
            .and_then(Value::as_f64)
            .unwrap_or(default)
    };
    [
        json!(name),
        json!(at),
        json!(point.value),
        json!(read("shape", 1.0)),
        json!(read("curve", 0.0)),
    ]
}

/// **What a roll draws of a sequence**: every event the domain places, as a
/// note, and every raw message as a marker.
pub fn project(sequence: &EventSequence, domain: &YDomain, axis: &Axis) -> Projection {
    let mut out = Projection {
        midi: sequence
            .midi
            .map(|spec| spec.label().to_string())
            .unwrap_or_default(),
        ..Projection::default()
    };
    for event in &sequence.events {
        let keys = event.keys();
        if let Some(label) = label_of(&event.data.0) {
            out.osc.push(json!(axis.units(event.at.0)));
            out.osc.push(json!(label));
            continue;
        }
        let Some(y) = domain.value(&keys) else {
            continue;
        };
        let level = render::level_of(&keys);
        let start = axis.units(event.at.0);
        let end = axis.units(event.at.0 + render::sustain_of(&keys).max(0.0));
        out.notes.extend([
            start,
            end - start,
            y,
            level.velocity(),
            keys.get("channel").and_then(Value::as_f64).unwrap_or(0.0),
        ]);
        out.note_ids.push(event.id);
        let from = axis.units(event.at.0);
        for curve in &event.automation {
            let name = curve.id.0.to_string();
            let (label, min, max, pitch) = curve_look(curve);
            out.layers.extend([
                json!(name),
                json!(event.id),
                json!(label),
                json!(min),
                json!(max),
                json!(pitch),
            ]);
            for point in &curve.points {
                let at = axis.units(event.at.0 + point.at) - from;
                out.points.extend(point_values(&name, at, point));
            }
        }
    }
    for curve in &sequence.automation {
        let name = curve.id.0.to_string();
        let (label, min, max, _) = curve_look(curve);
        out.curves.extend([
            json!(name),
            json!(label),
            json!(min),
            json!(max),
            json!(CURVE_H),
        ]);
        for point in &curve.points {
            out.points
                .extend(point_values(&name, axis.units(point.at), point));
        }
    }
    out
}

/// Whether two values on the axis are the same, as a roll that holds them as
/// `f32` hands them back.
fn same(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-4 * a.abs().max(1.0)
}

/// **What a gesture on a roll means**, in the sequence's vocabulary: one
/// `setevents` naming every event by id, under the gesture's label.
///
/// `tag` is the strip the hand touched (`notes` or `osc`), `values` its report,
/// `axis` where the roll drew the beats. The strip the hand did not touch is
/// carried through untouched.
pub fn intake(
    sequence: &EventSequence,
    tag: &str,
    values: &[Value],
    axis: &Axis,
    domain: &YDomain,
) -> Intake {
    if tag == "points" {
        return curves(sequence, values, axis);
    }
    let events = match tag {
        "notes" => notes(sequence, values, axis, domain),
        "osc" => match markers(sequence, values, axis) {
            Some(events) => events,
            None => {
                return Intake::refused(
                    "a marker is the message it sends, and a roll cannot say which: \
                     add it to the timeline with its address, then drag it here",
                );
            }
        },
        _ => return Intake::nothing(),
    };
    let label = if tag == "notes" {
        "edit the notes"
    } else {
        "edit the markers"
    };
    let intent = EventsIntent::SetEvents { events };
    Intake::edit(serde_json::to_value(&intent).unwrap_or(Value::Null), label)
}

/// The sequence after a `notes` gesture: each reported note written onto the
/// event its id names -- only the keys the roll changed, each with its family's
/// coherence -- a note with no id as a new event, a note that is no longer
/// reported removed, and everything that is not a note of this domain kept.
fn notes(sequence: &EventSequence, values: &[Value], axis: &Axis, domain: &YDomain) -> Vec<Event> {
    let drawn =
        |event: &Event| domain.value(&event.keys()).is_some() && label_of(&event.data.0).is_none();
    let mut out: Vec<Event> = sequence
        .events
        .iter()
        .filter(|e| !drawn(e))
        .cloned()
        .collect();
    for sextuple in groups(values, SEXTUPLE) {
        let id = number(&sextuple[0]).max(0.0) as u64;
        let start = number(&sextuple[1]);
        let at = axis.beat(start);
        let length = axis.beat(start + number(&sextuple[2])) - at;
        let y = number(&sextuple[3]);
        let velocity = number(&sextuple[4]).round();
        let channel = number(&sextuple[5]).round();
        let held = (id != 0)
            .then(|| sequence.get(id))
            .flatten()
            .filter(|e| drawn(e));
        let event = match held {
            Some(was) => {
                let mut keys = was.keys();
                if domain.value(&keys).is_none_or(|v| !same(v, y)) {
                    render::set_key(&mut keys, &domain.key, json!(y));
                }
                if !same(render::sustain_of(&keys), length) {
                    render::set_key(&mut keys, "sustain", json!(length));
                }
                if render::level_of(&keys).velocity() != velocity {
                    // The velocity joins the keys the event holds, so the
                    // level family keeps the amplitude in step with it.
                    keys.entry("velocity").or_insert(json!(velocity));
                    render::set_key(&mut keys, "velocity", json!(velocity));
                }
                let held_channel = keys.get("channel").and_then(Value::as_f64).unwrap_or(0.0);
                if held_channel != channel {
                    if channel == 0.0 {
                        keys.remove("channel");
                    } else {
                        keys.insert("channel".into(), json!(channel));
                    }
                }
                Event {
                    at: Beat(at),
                    data: Opaque(Value::Object(keys)),
                    ..was.clone()
                }
            }
            None => {
                let mut keys = Map::new();
                keys.insert(domain.key.clone(), json!(y));
                keys.insert("dur".into(), json!(length));
                keys.insert("sustain".into(), json!(length));
                keys.insert("velocity".into(), json!(velocity));
                keys.insert("amp".into(), json!(amp_of_velocity(velocity)));
                if channel != 0.0 {
                    keys.insert("channel".into(), json!(channel));
                }
                Event::new(at, Value::Object(keys))
            }
        };
        out.push(event);
    }
    out
}

/// **A `points` gesture**: each curve the report names, with its points as
/// the hand left them -- back to beats (a note's from its start) -- and the
/// shape of each segment kept in the point's data. The one curve that changed
/// is its own edit (`automation` or `eventautomation`, which coalesces per curve); if a
/// gesture changed more than one, the sequence is restated whole.
fn curves(sequence: &EventSequence, values: &[Value], axis: &Axis) -> Intake {
    let mut reported: Map<String, Value> = Map::new();
    for p in groups(values, 5) {
        let name = text(&p[0]);
        let entry = reported.entry(name).or_insert_with(|| json!([]));
        if let Some(list) = entry.as_array_mut() {
            list.push(json!([
                number(&p[1]),
                number(&p[2]),
                number(&p[3]),
                number(&p[4])
            ]));
        }
    }
    let points_of = |name: &str, beat_at: &dyn Fn(f64) -> f64| -> Vec<clausters_document::Point> {
        reported
            .get(name)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|p| {
                let p = p.as_array()?;
                let f = |i: usize| p.get(i).and_then(Value::as_f64).unwrap_or(0.0);
                Some(clausters_document::Point {
                    at: beat_at(f(0)),
                    value: f(1),
                    data: Opaque(json!({"shape": f(2), "curve": f(3)})),
                })
            })
            .collect()
    };
    let mut after = sequence.clone();
    let mut intents = Vec::new();
    for curve in &mut after.automation {
        let points = points_of(&curve.id.0.to_string(), &|units| axis.beat(units));
        if !same_points(&curve.points, &points) {
            curve.points = points;
            intents.push(EventsIntent::Automation {
                automation: curve.clone(),
            });
        }
    }
    for event in &mut after.events {
        let from = axis.units(event.at.0);
        let start = event.at.0;
        for curve in &mut event.automation {
            let points = points_of(&curve.id.0.to_string(), &|units| {
                axis.beat(from + units) - start
            });
            if !same_points(&curve.points, &points) {
                curve.points = points;
                intents.push(EventsIntent::EventAutomation {
                    id: event.id,
                    automation: curve.clone(),
                });
            }
        }
    }
    let intent = match intents.len() {
        0 => return Intake::nothing(),
        1 => intents.remove(0),
        _ => EventsIntent::Restore {
            sequence: Box::new(after),
        },
    };
    Intake::edit(
        serde_json::to_value(&intent).unwrap_or(Value::Null),
        "draw a curve",
    )
}

/// Whether two curves' points say the same thing, as a roll that holds them
/// as `f32` hands them back.
fn same_points(a: &[clausters_document::Point], b: &[clausters_document::Point]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(p, q)| {
            let shape = |p: &clausters_document::Point, key: &str, default: f64| {
                p.data.0.get(key).and_then(Value::as_f64).unwrap_or(default)
            };
            same(p.at, q.at)
                && same(p.value, q.value)
                && same(shape(p, "shape", 1.0), shape(q, "shape", 1.0))
                && same(shape(p, "curve", 0.0), shape(q, "curve", 0.0))
        })
}

/// The sequence after an `osc` gesture -- the notes untouched and the markers
/// as the lane now holds them -- or `None` when the gesture added one that has
/// no message to send. A marker is matched by its label, then by order among
/// the ones that share it.
fn markers(sequence: &EventSequence, values: &[Value], axis: &Axis) -> Option<Vec<Event>> {
    let held: Vec<&Event> = sequence
        .events
        .iter()
        .filter(|e| label_of(&e.data.0).is_some())
        .collect();
    let mut taken = vec![false; held.len()];
    let mut out: Vec<Event> = sequence
        .events
        .iter()
        .filter(|e| label_of(&e.data.0).is_none())
        .cloned()
        .collect();
    for pair in groups(values, PAIR) {
        let (time, label) = (number(&pair[0]), text(&pair[1]));
        let i = held
            .iter()
            .enumerate()
            .position(|(i, e)| !taken[i] && label_of(&e.data.0).as_deref() == Some(&label))?;
        taken[i] = true;
        out.push(Event {
            at: Beat(axis.beat(time)),
            ..held[i].clone()
        });
    }
    Some(out)
}

#[cfg(test)]
mod tests;
