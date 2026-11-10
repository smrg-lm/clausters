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
//! - `osc`: the OSC markers, `time label` pairs, and its report the same --
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
//!   (`MIDI 1.0`, `MPE`, `MIDI 2.0`), empty for a sequence for the server
//!   and in the notation mode, which shows what a page writes -- the roll
//!   shows it beside its ruler.
//! - In the roll's **notation mode** ([`Mode::Notation`]) a note is drawn as
//!   long as it is written (its `dur`), and four lists ride beside `notes`:
//!   `held`, how long each sounds, in view units; `voices`, flat `staff
//!   voice` pairs; `dimmed`, `1` for a note of a voice other than the one
//!   edited, which a press does not take; `labels`, the dynamic each
//!   states, or `""`; `spelled`, how each is written as a reader says it
//!   (its written value, its spelling and its marks); and `marks`, the marks
//!   a page writes on each by name (its articulations, then its breath);
//!   and `tints`, each note's voice, staff or part as the editing says
//!   ([`ColorBy`]). Only the notes of the staves shown are drawn.
//! - `joins`: which note goes on into which, flat `from into` pairs of
//!   indices into `notes` -- a note's `joins`, which tells the roll its owner
//!   speaks the joins of a line.
//! - `lines`: the pitch a line sounds over time where it says more than its
//!   boxes (a run of joined notes, a note a bend moves), a list of flat `at
//!   value` runs drawn over the boxes ([`Projection::lines`]).
//! - A `split` report is `at id...`: the cursor, in view units, and the
//!   selected notes, which the document cuts there (`EventsIntent::Split`) --
//!   their keys and curves on both halves, as a region is split.
//! - `bars`: where the sequence's bars fall, flat `beat length number`
//!   triples in beats ([`bars`]), each a run of bars of one length from its
//!   beat until the next run; empty where the sequence states no meter, and
//!   the ruler then counts bars of its own, which the plane bars as well.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use clausters_core::event::amp_of_velocity;
use clausters_core::event::render::{self, Type};
use clausters_core::tempomap::TempoMap;
use clausters_document::events::{Event, EventSequence, EventsIntent};
use clausters_document::multitrack::Automation;
use clausters_document::timebase::Axis as TimeAxis;
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

    /// A place on the axis, as a beat -- settled on the line it was meant
    /// for when it is a grid line read back through seconds
    /// ([`TempoMap::settle_beat`]), so a quantized note is written into the
    /// sequence on its beat exactly.
    pub fn beat(&self, units: f64) -> f64 {
        TempoMap::settle_beat(self.map.beats_at(units / self.rate))
    }
}

/// What a roll over a sequence draws.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Projection {
    /// Flat `start dur y velocity channel` quintuples, in view units.
    pub notes: Vec<f64>,
    /// The id of each note, in the same order.
    pub note_ids: Vec<u64>,
    /// The OSC markers: `time label` pairs.
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
    /// Where its bars fall: `beat length number` triples ([`bars`]).
    pub bars: Vec<f64>,
    /// Which note goes on into which: `from into` pairs of indices into
    /// `notes`.
    pub joins: Vec<usize>,
    /// **The pitch each line sounds over time** where it says more than its
    /// boxes do -- a run of joined notes, or a note a bend moves: flat `at
    /// value` pairs per run, `at` in view units and `value` on the Y axis
    /// ([`pitch_lines`]).
    pub lines: Vec<Vec<f64>>,
    /// In notation mode, how long each note sounds, in view units.
    pub held: Vec<f64>,
    /// In notation mode, each note's `staff voice`.
    pub voices: Vec<usize>,
    /// In notation mode, `1` for a note of a voice other than the one edited.
    pub dimmed: Vec<u8>,
    /// In notation mode, the dynamic each note states, or `""`.
    pub labels: Vec<String>,
    /// In notation mode, how each note is written, as a reader says it: its
    /// written value, its spelling and its marks (`1/8  F#4  stacc`).
    pub spelled: Vec<String>,
    /// In notation mode, **each note's tint**: its voice, its staff or its
    /// part as [`Editing::color_by`] says, counted from 0 -- the first
    /// untinted.
    pub tints: Vec<usize>,
    /// In notation mode, **the marks a page writes on each note**, by name:
    /// its articulations (MEI's), then its breath (`breath`, `caesura`),
    /// space-separated -- what the roll draws over the box ([`marks_of`]).
    pub marks: Vec<String>,
}

/// **How a roll edits**: the options of whoever edits, which are the
/// window's and never the sequence's -- the sequence holds none of them, and
/// changing one writes nothing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Editing {
    /// **One note at a time in a line**: a gesture never leaves two notes of
    /// one line overlapping, and shortens or removes what it covers
    /// ([`monophonic`]). Off, notes overlap as the hand leaves them.
    pub monophonic: bool,
    /// The roll's mode: MIDI, or notation.
    pub mode: Mode,
    /// What the roll shows in notation mode: the whole system, a part or a
    /// staff.
    pub scope: Scope,
    /// The voice edited in notation mode, `[staff, voice]`: what a note
    /// drawn is written in, and the one voice a press takes.
    pub voice: [usize; 2],
    /// **What a note's tint tells apart in notation mode**: its voice, its
    /// staff or its part.
    pub color_by: ColorBy,
}

/// What a note's tint tells apart, in notation mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorBy {
    /// The voices of a staff: the first untinted, each other its own.
    #[default]
    Voice,
    /// The staves.
    Staff,
    /// The parts.
    Part,
}

/// **The roll's two modes** (`crates/clausters-document/PLAN.md`, "The
/// notation mode"), the roll's and never the sequence's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// The events as MIDI says them: notes on and off, velocities, channels,
    /// curves, joins. Nothing of how a page writes them.
    #[default]
    Midi,
    /// A view of the notation: staves and voices, written values, and a
    /// performance derived from what is written.
    Notation,
}

/// What a roll in notation mode shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "show", content = "index")]
pub enum Scope {
    /// Every staff: the system.
    #[default]
    System,
    /// The staves of one part, by its index.
    Part(usize),
    /// One staff, by its index.
    Staff(usize),
}

impl Editing {
    /// **Whether the roll draws and takes a note with these keys**: every
    /// note in MIDI mode; in notation mode, one on a staff the scope shows.
    #[must_use]
    pub fn shows(&self, sequence: &EventSequence, keys: &Map<String, Value>) -> bool {
        if self.mode == Mode::Midi {
            return true;
        }
        let staff = staff_voice(keys)[0];
        match self.scope {
            Scope::System => true,
            Scope::Staff(shown) => staff == shown,
            Scope::Part(part) => {
                let parts = sequence.parts_read();
                let first: usize = parts.iter().take(part).map(|p| p.staves.max(1)).sum();
                let staves = parts.get(part).map_or(0, |p| p.staves.max(1));
                (first..first + staves).contains(&staff)
            }
        }
    }
}

/// The `[staff, voice]` a note says it is written in, `[0, 0]` where it says
/// neither.
#[must_use]
pub fn staff_voice(keys: &Map<String, Value>) -> [usize; 2] {
    let read = |key: &str| {
        keys.get(key)
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            .max(0.0) as usize
    };
    [read("staff"), read("voice")]
}

/// How tall a sequence curve's row under the plane is drawn.
pub const CURVE_H: f64 = 40.0;

/// **What a curve is drawn over**: its label, its value range
/// ([`crate::points::range`]), and whether it is a bend (drawn in the plane
/// when it is a note's). The target says what it moves -- `{"cc": n}`,
/// `{"bend": ...}`, `{"pressure": ...}`, `{"timbre": ...}`, `{"control":
/// name}` -- and a name on the curve wins over the label it gives. A sequence curve's `channel` (the notes' own count,
/// from 0) is the channel it acts on, and none is every channel.
fn curve_look<T: TimeAxis>(curve: &Automation<T>) -> (String, f64, f64, bool) {
    let target = curve.target.0.as_object();
    let has = |key: &str| target.is_some_and(|t| t.contains_key(key));
    let read = |key: &str| target.and_then(|t| t.get(key)).and_then(Value::as_f64);
    let (label, pitch) = if let Some(cc) = read("cc") {
        (format!("CC {cc}"), false)
    } else if has("bend") {
        ("bend".to_string(), true)
    } else if has("pressure") {
        ("pressure".to_string(), false)
    } else if has("timbre") {
        ("timbre".to_string(), false)
    } else {
        let name = target
            .and_then(|t| t.get("control"))
            .and_then(Value::as_str)
            .unwrap_or("curve");
        (name.to_string(), false)
    };
    // The range is the parameter's, read by the one rule every curve's view
    // reads it by.
    let (min, max) = crate::points::range(&curve.target.0);
    // A curve's channel is part of what it is on, as in MIDI, and shown the
    // way MIDI shows a channel: counted from 1.
    let label = match read("channel") {
        Some(channel) => format!("{label} ch {}", channel + 1.0),
        None => label,
    };
    (curve.name.clone().unwrap_or(label), min, max, pitch)
}

/// A point as the wire carries it, `at` already in view units.
fn point_values<T: TimeAxis>(
    name: &str,
    at: f64,
    point: &clausters_document::Point<T>,
) -> [Value; 5] {
    let [at, value, shape, curve] = crate::points::quad(at, point);
    [
        json!(name),
        json!(at),
        json!(value),
        json!(shape),
        json!(curve),
    ]
}

/// **What a roll draws of a sequence**: every event the domain places, as a
/// note, and every raw message as a marker.
pub fn project(
    sequence: &EventSequence,
    domain: &YDomain,
    axis: &Axis,
    editing: &Editing,
) -> Projection {
    let notation = editing.mode == Mode::Notation;
    // the staves of each part, for a tint by part
    let mut first = 0;
    let part_of: Vec<std::ops::Range<usize>> = if notation {
        sequence
            .parts_read()
            .iter()
            .map(|part| {
                let staves = first..first + part.staves.max(1);
                first = staves.end;
                staves
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut out = Projection {
        // which MIDI the notes are written for says nothing of a page: in
        // notation mode the roll shows what is written, and no spec
        midi: sequence
            .midi
            .filter(|_| !notation)
            .map(|spec| spec.label().to_string())
            .unwrap_or_default(),
        bars: bars(sequence),
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
        if !editing.shows(sequence, &keys) {
            continue;
        }
        let level = render::level_of(&keys);
        let start = axis.units(event.at.0);
        let sounding = render::sustain_of(&keys).max(0.0);
        // in notation mode a note is as long as it is written, with what it
        // sounds inside it
        let length = if notation {
            keys.get("dur")
                .and_then(Value::as_f64)
                .unwrap_or(sounding)
                .max(0.0)
        } else {
            sounding
        };
        let end = axis.units(event.at.0 + length);
        if notation {
            out.held
                .push(axis.units(event.at.0 + sounding.min(length)) - start);
            let [staff, voice] = staff_voice(&keys);
            out.voices.extend([staff, voice]);
            out.dimmed.push(u8::from([staff, voice] != editing.voice));
            out.labels.push(
                keys.get("dynamic")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            );
            out.spelled.push(spelled(&keys));
            out.marks.push(marks_of(&keys));
            out.tints.push(match editing.color_by {
                ColorBy::Voice => voice,
                ColorBy::Staff => staff,
                ColorBy::Part => part_of
                    .iter()
                    .position(|staves| staves.contains(&staff))
                    .unwrap_or(0),
            });
        }
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
                let at = axis.units(event.at.0 + point.at.0) - from;
                out.points.extend(point_values(&name, at, point));
            }
        }
    }
    // what goes on into what, by where each is drawn
    let at: std::collections::BTreeMap<u64, usize> = out
        .note_ids
        .iter()
        .enumerate()
        .map(|(i, id)| (*id, i))
        .collect();
    for (into, id) in out.note_ids.iter().enumerate() {
        let from = sequence
            .get(*id)
            .and_then(|event| event.keys().get("joins").and_then(Value::as_u64))
            .and_then(|from| at.get(&from));
        if let Some(from) = from {
            out.joins.extend([*from, into]);
        }
    }
    out.lines = pitch_lines(sequence, domain, axis, &out.note_ids);
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
                .extend(point_values(&name, axis.units(point.at.0), point));
        }
    }
    out
}

/// **Where a sequence's bars fall**, in its beats: flat `beat length number`
/// triples, each a run of bars `length` beats long from `beat` until the next
/// run, the last one going on -- the first of them numbered `number`, as a
/// page numbers it (an anacrusis is bar 0, the first whole bar 1).
///
/// The bars are the page's: the meters by measure and the bars of another
/// length of the sequence's `notation.grid`, or the meter its reading names
/// (`notation.reading.meter`), counted in the reading's beat unit. A sequence
/// that states neither has none, and the roll's ruler counts bars of its own.
pub fn bars(sequence: &EventSequence) -> Vec<f64> {
    let section = sequence.notation.as_ref();
    let reading = clausters_document::events::reading::Transcription::of(sequence);
    let beats = reading.beat_unit.max(1) as f64;
    // (measure, count, unit) of each meter, and (measure, length in whole
    // notes) of each bar of another length
    let ratio = |value: &Value| -> Option<f64> {
        let pair = value.as_array()?;
        let (n, d) = (pair.first()?.as_f64()?, pair.get(1)?.as_f64()?);
        (d != 0.0).then(|| n / d)
    };
    let grid = section.and_then(|n| n.get("grid"));
    let mut meters: Vec<(usize, f64)> = grid
        .and_then(|g| g.get("meters"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let count = m.get("count")?.as_f64()?;
            let unit = m.get("unit")?.as_f64()?;
            let measure = m.get("measure").and_then(Value::as_u64).unwrap_or(0) as usize;
            (count > 0.0 && unit > 0.0).then(|| (measure, count / unit))
        })
        .collect();
    if meters.is_empty() {
        let stated = reading.meter.as_deref().and_then(|meter| {
            let (n, d) = meter.split_once('/')?;
            let (n, d): (f64, f64) = (n.trim().parse().ok()?, d.trim().parse().ok()?);
            (n > 0.0 && d > 0.0).then(|| n / d)
        });
        match stated {
            Some(bar) => meters.push((0, bar)),
            None => return Vec::new(),
        }
    }
    meters.sort_by_key(|(measure, _)| *measure);
    let irregular: Vec<(usize, f64)> = grid
        .and_then(|g| g.get("irregular"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|pair| {
            let pair = pair.as_array()?;
            let len = ratio(pair.get(1)?)?;
            let measure = pair.first()?.as_u64()? as usize;
            (len > 0.0).then_some((measure, len))
        })
        .collect();
    let meter_at = |measure: usize| {
        meters
            .iter()
            .rev()
            .find(|(m, _)| *m <= measure)
            .map_or(meters[0].1, |(_, bar)| *bar)
    };
    let length = |measure: usize| {
        irregular
            .iter()
            .find(|(m, _)| *m == measure)
            .map_or_else(|| meter_at(measure), |(_, len)| *len)
    };
    // a bar 0 shorter than its meter is an anacrusis, and the first whole bar
    // is bar 1
    let pickup = irregular
        .iter()
        .any(|(m, len)| *m == 0 && *len < meter_at(0));
    let mut changes: Vec<usize> = std::iter::once(0)
        .chain(meters.iter().map(|(m, _)| *m))
        .chain(irregular.iter().flat_map(|(m, _)| [*m, m + 1]))
        .collect();
    changes.sort_unstable();
    changes.dedup();
    let mut out = Vec::new();
    let mut at = 0.0;
    let mut measure = 0;
    for (k, change) in changes.iter().enumerate() {
        // the beats from where the last run started to this one
        while measure < *change {
            at += length(measure) * beats;
            measure += 1;
        }
        let number = if pickup { measure } else { measure + 1 };
        let bar = length(measure) * beats;
        // a run that changes nothing of the one before it is that run
        let same = out.len() >= 3 && out[out.len() - 2] == bar;
        if !same || k == 0 {
            out.extend([at, bar, number as f64]);
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
/// `axis` where the roll drew the beats, `editing` how the roll edits. The
/// strip the hand did not touch is carried through untouched.
///
/// `join` and `unjoin` with the ids of the selected notes are the roll's own
/// two verbs over a line (`Join`, `Unjoin`).
pub fn intake(
    sequence: &EventSequence,
    tag: &str,
    values: &[Value],
    axis: &Axis,
    domain: &YDomain,
    editing: &Editing,
) -> Intake {
    if tag == "points" {
        return curves(sequence, values, axis);
    }
    if matches!(tag, "join" | "unjoin") {
        let ids: Vec<u64> = values.iter().map(|v| number(v).max(0.0) as u64).collect();
        if ids.is_empty() {
            return Intake::refused("nothing is selected");
        }
        let (intent, label) = if tag == "join" {
            (EventsIntent::Join { ids }, "join")
        } else {
            (EventsIntent::Unjoin { ids }, "unjoin")
        };
        return Intake::edit(serde_json::to_value(&intent).unwrap_or(Value::Null), label);
    }
    if tag == "split" {
        // the cut, on the view's axis, and the notes it is asked of
        let Some((at, ids)) = values.split_first() else {
            return Intake::refused("nothing is selected");
        };
        let ids: Vec<u64> = ids.iter().map(|v| number(v).max(0.0) as u64).collect();
        if ids.is_empty() {
            return Intake::refused("nothing is selected");
        }
        let at = Beat(axis.beat(number(at).max(0.0)));
        let intent = EventsIntent::Split { ids, at };
        return Intake::edit(
            serde_json::to_value(&intent).unwrap_or(Value::Null),
            "split",
        );
    }
    if TRANSPOSITIONS.contains(&tag) {
        return transpose(sequence, tag, values, domain, editing);
    }
    if editing.mode == Mode::Notation {
        return notation_intake(sequence, tag, values, axis, domain, editing);
    }
    if notation::ARTICULATIONS.contains(&tag) {
        return Intake::refused(
            "an articulation is written on a page: switch the roll to notation to write one",
        );
    }
    let events = match tag {
        "notes" => {
            let mut events = notes(sequence, values, axis, domain, editing);
            if editing.monophonic {
                monophonic(sequence, &mut events, false);
            }
            events
        }
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

/// **How a note is written, as a reader says it**: its written value as a
/// fraction of a whole note, its pitches spelled (a letter, its sign, its
/// octave) and its marks, each where it has one.
/// **The marks a page writes on a note, by name**: its articulations, in
/// the order it states them, then its breath -- space-separated, empty for
/// none.
#[must_use]
pub fn marks_of(keys: &Map<String, Value>) -> String {
    let mut names: Vec<&str> = keys
        .get("articulations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if let Some(breath) = keys.get("breath").and_then(Value::as_str) {
        names.push(breath);
    }
    names.join(" ")
}

fn spelled(keys: &Map<String, Value>) -> String {
    let mut parts = Vec::new();
    if let Some([n, d]) = keys
        .get("value")
        .and_then(Value::as_array)
        .and_then(|v| Some([v.first()?.as_i64()?, v.get(1)?.as_i64()?]))
    {
        parts.push(format!("{n}/{d}"));
    }
    let pitches: Vec<String> = keys
        .get("pitches")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let step = p.get("step")?.as_str()?.to_uppercase();
            let alter = p.get("alter").and_then(Value::as_f64).unwrap_or(0.0) as i64;
            let sign = match alter {
                2 => "##",
                1 => "#",
                -1 => "b",
                -2 => "bb",
                _ => "",
            };
            Some(format!("{step}{sign}{}", p.get("octave")?.as_i64()?))
        })
        .collect();
    if !pitches.is_empty() {
        parts.push(pitches.join(" "));
    }
    let marks: Vec<&str> = keys
        .get("articulations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if !marks.is_empty() {
        parts.push(marks.join(" "));
    }
    parts.join("  ")
}

/// **The roll's selections by data**: every note the roll takes that shares
/// a pitch, a channel, a voice or a staff with one of the selected notes.
pub const SELECTIONS: &[&str] = &[
    "select_pitch",
    "select_channel",
    "select_voice",
    "select_staff",
];

/// **The notes a selection by data marks** (see [`SELECTIONS`]): of every
/// note the roll takes -- drawn, and in notation mode of the voice edited --
/// those that share with a note of `ids` its pitch (as a MIDI number), its
/// channel, its staff and voice, or its staff, by `tag`. Empty for a tag
/// that is none of them or no note of `ids`.
#[must_use]
pub fn select_like(
    sequence: &EventSequence,
    tag: &str,
    ids: &[u64],
    domain: &YDomain,
    editing: &Editing,
) -> Vec<u64> {
    let what = |keys: &Map<String, Value>| -> Option<Vec<i64>> {
        let [staff, voice] = staff_voice(keys);
        Some(match tag {
            "select_pitch" => {
                vec![
                    render::pitch_of(keys)
                        .midinote(&render::scale_of(keys))
                        .round() as i64,
                ]
            }
            "select_channel" => {
                vec![keys.get("channel").and_then(Value::as_f64).unwrap_or(0.0) as i64]
            }
            "select_voice" => vec![staff as i64, voice as i64],
            "select_staff" => vec![staff as i64],
            _ => return None,
        })
    };
    let taken = |event: &Event| {
        let keys = event.keys();
        domain.value(&keys).is_some()
            && label_of(&event.data.0).is_none()
            && editing.shows(sequence, &keys)
            && (editing.mode == Mode::Midi || staff_voice(&keys) == editing.voice)
    };
    let wanted: Vec<Vec<i64>> = ids
        .iter()
        .filter_map(|id| sequence.get(*id))
        .filter(|event| taken(event))
        .filter_map(|event| what(&event.keys()))
        .collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    sequence
        .events
        .iter()
        .filter(|event| taken(event))
        .filter(|event| what(&event.keys()).is_some_and(|w| wanted.contains(&w)))
        .map(|event| event.id)
        .collect()
}

/// **The roll's transpositions**, over the selected notes or, with none
/// selected, every note the roll would take: up or down a semitone in MIDI
/// mode and a step of the key in notation mode, and an octave in both.
pub const TRANSPOSITIONS: &[&str] = &["step_up", "step_down", "octave_up", "octave_down"];

/// **A transposition** (see [`TRANSPOSITIONS`]): `values` are the ids of the
/// notes, none for every note drawn -- in notation mode, of the voice
/// edited. A semitone or an octave moves what a note sounds, and its written
/// `pitches` follow by its family's coherence; a step of the key moves what
/// it is written as, to the next letter with the sign the key writes on it
/// (`clausters_core::event::notation::stepped_in`), and what it sounds
/// follows. One edit.
fn transpose(
    sequence: &EventSequence,
    tag: &str,
    values: &[Value],
    domain: &YDomain,
    editing: &Editing,
) -> Intake {
    use clausters_core::event::notation::{fifths_of, spell_in, stepped_in};

    let ids: Vec<u64> = values.iter().map(|v| number(v).max(0.0) as u64).collect();
    let sign = if tag.ends_with("_up") { 1 } else { -1 };
    let octave = tag.starts_with("octave");
    let written = editing.mode == Mode::Notation;
    let key = clausters_document::events::reading::Transcription::of(sequence)
        .key
        .or_else(|| {
            sequence
                .notation
                .as_ref()
                .and_then(|n| n.get("key"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "C".into());
    let fifths = fifths_of(&key);
    let mut moved = false;
    let events: Vec<Event> = sequence
        .events
        .iter()
        .map(|event| {
            let mut keys = event.keys();
            let taken = if ids.is_empty() {
                domain.value(&keys).is_some()
                    && label_of(&event.data.0).is_none()
                    && editing.shows(sequence, &keys)
                    && (!written || staff_voice(&keys) == editing.voice)
            } else {
                ids.contains(&event.id) && Type::of(&keys) == Type::Note
            };
            if !taken {
                return event.clone();
            }
            let midi = render::pitch_of(&keys).midinote(&render::scale_of(&keys));
            if octave || !written {
                let by = if octave { 12.0 } else { 1.0 };
                render::set_key(
                    &mut keys,
                    "midinote",
                    json!(midi.round() + sign as f64 * by),
                );
            } else {
                let pitches = keys.get("pitches").cloned().unwrap_or_else(|| {
                    let (step, alter, octave) = spell_in(midi.round() as i32, fifths);
                    let letter = ["c", "d", "e", "f", "g", "a", "b"][step];
                    json!([{"step": letter, "alter": alter, "octave": octave}])
                });
                render::set_key(&mut keys, "pitches", stepped_in(&pitches, sign, fifths));
            }
            moved = true;
            Event {
                data: Opaque(Value::Object(keys)),
                ..event.clone()
            }
        })
        .collect();
    if !moved {
        return Intake::refused("there is no note to transpose");
    }
    let intent = EventsIntent::SetEvents { events };
    Intake::edit(
        serde_json::to_value(&intent).unwrap_or(Value::Null),
        "transpose",
    )
}

/// **A gesture of the roll in notation mode**, read by the rules of notation
/// (`notation`), or refused where the build has no notation layer.
fn notation_intake(
    sequence: &EventSequence,
    tag: &str,
    values: &[Value],
    axis: &Axis,
    domain: &YDomain,
    editing: &Editing,
) -> Intake {
    #[cfg(feature = "notation")]
    {
        notation::intake(sequence, tag, values, axis, domain, editing)
    }
    #[cfg(not(feature = "notation"))]
    {
        let _ = (sequence, tag, values, axis, domain, editing);
        Intake::refused(
            "the roll's notation mode needs the notation layer, which this build leaves out",
        )
    }
}

/// The sequence after a `notes` gesture: each reported note written onto the
/// event its id names -- only the keys the roll changed, each with its family's
/// coherence -- a note with no id as a new event, a note that is no longer
/// reported removed, and everything that is not a note the roll draws kept.
///
/// **An edit in MIDI mode drops the notation keys it contradicts**: a new
/// length drops the written `value`, a new velocity the `dynamic`, a new
/// channel the `staff` and `voice`, so the page reads the performance there
/// and keeps the rest as written.
fn notes(
    sequence: &EventSequence,
    values: &[Value],
    axis: &Axis,
    domain: &YDomain,
    editing: &Editing,
) -> Vec<Event> {
    let drawn = |event: &Event| {
        let keys = event.keys();
        domain.value(&keys).is_some()
            && label_of(&event.data.0).is_none()
            && editing.shows(sequence, &keys)
    };
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
                    // the written value says another length: the page reads
                    // the one played
                    if keys.remove("value").is_some() {
                        keys.insert("dur".into(), json!(length));
                    }
                }
                if render::level_of(&keys).velocity() != velocity {
                    // The velocity joins the keys the event holds, so the
                    // level family keeps the amplitude in step with it.
                    keys.entry("velocity").or_insert(json!(velocity));
                    render::set_key(&mut keys, "velocity", json!(velocity));
                    keys.remove("dynamic");
                }
                let held_channel = keys.get("channel").and_then(Value::as_f64).unwrap_or(0.0);
                if held_channel != channel {
                    if channel == 0.0 {
                        keys.remove("channel");
                    } else {
                        keys.insert("channel".into(), json!(channel));
                    }
                    keys.remove("staff");
                    keys.remove("voice");
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

/// **One note at a time in each line**, after a gesture: every note the
/// gesture changed or made wins over the notes of its line it covers. One it
/// starts inside is shortened to end where it starts; one that starts inside
/// it is shortened from its start, its onset moved to where it ends and its
/// end kept, and removed where it is covered whole. A note's line is its staff
/// and voice, else its channel (`clausters_document::events::articulation`).
/// **The pitch lines a roll draws over its boxes**: the document's runs
/// ([`clausters_document::events::sounding::pitch_runs`]) of the notes drawn,
/// those that join two notes or bend, as flat `at value` pairs -- `at` in view
/// units, `value` on the domain's axis, hertz on a roll in hertz. A domain
/// that is not a pitch draws none.
fn pitch_lines(
    sequence: &EventSequence,
    domain: &YDomain,
    axis: &Axis,
    drawn: &[u64],
) -> Vec<Vec<f64>> {
    let value = |midi: f64| match domain.key.as_str() {
        "midinote" => Some(midi),
        "freq" => Some(clausters_core::scale::midi_to_hz(midi)),
        _ => None,
    };
    if value(0.0).is_none() {
        return Vec::new();
    }
    clausters_document::events::sounding::pitch_runs(sequence)
        .into_iter()
        .filter(|run| run.notes.iter().all(|id| drawn.contains(id)))
        .filter(|run| {
            run.notes.len() > 1
                || run
                    .points
                    .windows(2)
                    .any(|pair| (pair[0][1] - pair[1][1]).abs() > 1e-9)
        })
        .map(|run| {
            run.points
                .iter()
                .flat_map(|&[beat, midi]| [axis.units(beat), value(midi).unwrap_or(midi)])
                .collect()
        })
        .collect()
}

/// A length that reaches the next note joins nothing: it is a length.
///
/// `written` measures and shortens a note by its written length (`dur`, with
/// its `value`), as the roll in notation mode draws it; else by how long it
/// sounds (`sustain`).
pub fn monophonic(sequence: &EventSequence, events: &mut Vec<Event>, written: bool) {
    use clausters_document::events::articulation::line_of;

    let length = |event: &Event| {
        let keys = event.keys();
        let sounding = render::sustain_of(&keys).max(0.0);
        if written {
            keys.get("dur")
                .and_then(Value::as_f64)
                .unwrap_or(sounding)
                .max(0.0)
        } else {
            sounding
        }
    };
    let beat_unit = clausters_document::events::reading::Transcription::of(sequence).beat_unit;
    let shorten = |keys: &mut Map<String, Value>, to: f64| {
        if written {
            keys.insert("dur".into(), json!(to));
            if let Some(value) = notation::value_of(to, beat_unit) {
                keys.insert("value".into(), value);
            }
        } else {
            render::set_key(keys, "sustain", json!(to));
            keys.remove("value");
        }
    };
    let edited: Vec<bool> = events
        .iter()
        .map(
            |event| match sequence.get(event.id).filter(|_| event.id != 0) {
                Some(was) => was.at != event.at || was.data != event.data,
                None => true,
            },
        )
        .collect();
    let lines: Vec<_> = events.iter().map(|event| line_of(&event.keys())).collect();
    let notes: Vec<bool> = events
        .iter()
        .map(|event| Type::of(&event.keys()) == Type::Note)
        .collect();
    let mut gone = vec![false; events.len()];
    for e in (0..events.len()).filter(|e| edited[*e] && notes[*e]) {
        let (s, end) = (events[e].at.0, events[e].at.0 + length(&events[e]));
        for o in 0..events.len() {
            if o == e || edited[o] || gone[o] || !notes[o] || lines[o] != lines[e] {
                continue;
            }
            let (os, oe) = (events[o].at.0, events[o].at.0 + length(&events[o]));
            let mut keys = events[o].keys();
            if os < s && oe > s {
                // it was sounding where the edited one starts: it ends there
                shorten(&mut keys, s - os);
            } else if os >= s && os < end {
                if oe <= end {
                    gone[o] = true;
                    continue;
                }
                // it starts inside the edited one: it starts where that ends
                if !written && let Some(dur) = keys.get("dur").and_then(Value::as_f64) {
                    keys.insert("dur".into(), json!((dur - (end - os)).max(oe - end)));
                }
                shorten(&mut keys, oe - end);
                events[o].at = Beat(end);
            } else {
                continue;
            }
            events[o].data = Opaque(Value::Object(keys));
        }
    }
    let mut k = 0;
    events.retain(|_| {
        k += 1;
        !gone[k - 1]
    });
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
    let points_of =
        |name: &str, beat_at: &dyn Fn(f64) -> f64| -> Vec<clausters_document::Point<Beat>> {
            reported
                .get(name)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|p| {
                    let p = p.as_array()?;
                    let f = |i: usize| p.get(i).and_then(Value::as_f64).unwrap_or(0.0);
                    Some(crate::points::point(Beat(beat_at(f(0))), f(1), f(2), f(3)))
                })
                .collect()
        };
    let mut after = sequence.clone();
    let mut intents = Vec::new();
    for curve in &mut after.automation {
        let points = points_of(&curve.id.0.to_string(), &|units| axis.beat(units));
        if !crate::points::same(&curve.points, &points) {
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
            if !crate::points::same(&curve.points, &points) {
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

pub mod notation;

#[cfg(test)]
mod tests;
