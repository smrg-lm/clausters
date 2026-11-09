//! **A sequence's parts**: what is written on which staves and played on
//! which channels.
//!
//! A part is what a page names an instrument by and what a roll plays it on:
//! its name, how many consecutive staves it is written on, the channels (or
//! the MPE zone) its notes are played on, how its polyphony is written, and
//! its transposition. It is the document's because a page and a roll over
//! one sequence read the same part, and neither holds it.
//!
//! **A part holds nothing of an instrument's physics** -- how many notes it
//! sounds at once, its range, its strings, its pedal. Those belong to a model
//! of instruments, which is not this.
//!
//! A sequence that states no parts reads as one part per staff
//! ([`EventSequence::parts_read`]): derived each time it is asked, and never
//! written back, so a sequence saved before parts existed is saved unchanged.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use clausters_core::event::render::Type;

use super::{EventSequence, MidiSpec};

/// One part of a sequence. Every field has a default, so `{}` is a part: one
/// staff, no name, played wherever its staff's notes are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Part {
    /// What it is called at its first system.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// ...and at every one after, the short name.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub abbr: String,
    /// How many consecutive staves it is written on, counted from where the
    /// part before it ends. Each staff's clef and lines stay in the sequence's
    /// `notation.staves`.
    #[serde(skip_serializing_if = "is_one")]
    pub staves: usize,
    /// The MIDI channels its notes are played on (counted from zero).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<i64>,
    /// Or the MPE zone they are played on, where a channel is one note's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zone: Option<Zone>,
    /// How its polyphony is written.
    #[serde(skip_serializing_if = "Writing::is_default")]
    pub writing: Writing,
    /// How many semitones what sounds is from what is written: `-2` for a
    /// clarinet in B flat.
    #[serde(skip_serializing_if = "is_zero")]
    pub transpose: i32,
}

impl Default for Part {
    fn default() -> Self {
        Self {
            name: String::new(),
            abbr: String::new(),
            staves: 1,
            channels: Vec::new(),
            zone: None,
            writing: Writing::default(),
            transpose: 0,
        }
    }
}

/// An MPE zone, as a part is played on one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Zone {
    /// The upper zone (master channel 16) rather than the lower (1).
    #[serde(default)]
    pub upper: bool,
    /// How many member channels the zone has.
    #[serde(default = "fifteen")]
    pub members: u8,
}

fn fifteen() -> u8 {
    15
}

/// **How a part's polyphony is written**: notes that sound together in
/// several voices, or in one voice whose notes are cut and tied wherever
/// another starts or ends. The two are the same music, notated two ways.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Writing {
    /// Voices, or ties.
    pub polyphony: Polyphony,
    /// The most voices on one of its staves: `1`, or `2` (stems up and
    /// down).
    pub voices_per_staff: u8,
}

impl Default for Writing {
    fn default() -> Self {
        Self {
            polyphony: Polyphony::Voices,
            voices_per_staff: 2,
        }
    }
}

impl Writing {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Voices, or one voice with ties.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Polyphony {
    /// Notes that sound together are written in voices.
    Voices,
    /// In one voice, each note cut and tied where another starts or ends.
    Ties,
}

fn is_one(n: &usize) -> bool {
    *n == 1
}

fn is_zero(n: &i32) -> bool {
    *n == 0
}

impl EventSequence {
    /// **The channels a staff is read on**, in the order of the staves: every
    /// channel a note sounds on, from the lowest -- in MIDI 1.0 and 2.0 a
    /// channel is a line. The rule a reading without parts places a note by.
    pub fn note_channels(&self) -> Vec<i64> {
        self.events
            .iter()
            .filter_map(|event| {
                let keys = event.keys();
                (Type::of(&keys) == Type::Note).then(|| channel_of(&keys))
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// **The parts as they are read**: the ones the sequence states, else one
    /// part per staff, derived -- named as its staff is in the `notation`
    /// section, played on the channel its staff is read from, or on the
    /// sequence's MPE zone. A derived part is never written back.
    pub fn parts_read(&self) -> Vec<Part> {
        if !self.parts.is_empty() {
            return self.parts.clone();
        }
        let said: Vec<Value> = self
            .notation
            .as_ref()
            .and_then(|n| n.get("staves"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let notes: Vec<_> = self
            .events
            .iter()
            .map(|e| e.keys())
            .filter(|keys| Type::of(keys) == Type::Note)
            .collect();
        let stated = notes.iter().filter_map(|keys| keys.get("staff")?.as_f64());
        let stated_count = stated.map(|s| s.max(0.0) as usize + 1).max();
        let zone = match self.midi {
            Some(MidiSpec::Mpe { upper, members }) => Some(Zone { upper, members }),
            _ => None,
        };
        let channels = self.note_channels();
        // where no note says its staff, a staff is a channel (one for a zone)
        let by_channel = stated_count.is_none() && zone.is_none();
        let count = stated_count
            .unwrap_or(if by_channel { channels.len() } else { 1 })
            .max(said.len())
            .max(1);
        (0..count)
            .map(|staff| {
                let text = |key: &str| {
                    said.get(staff)
                        .and_then(|s| s.get(key))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                };
                Part {
                    name: text("label"),
                    abbr: text("abbr"),
                    channels: if by_channel {
                        channels.get(staff).copied().into_iter().collect()
                    } else {
                        Vec::new()
                    },
                    zone,
                    transpose: said
                        .get(staff)
                        .and_then(|s| s.get("transpose"))
                        .and_then(Value::as_i64)
                        .unwrap_or(0) as i32,
                    ..Part::default()
                }
            })
            .collect()
    }
}

/// A note's channel, `0` when it names none.
fn channel_of(keys: &serde_json::Map<String, Value>) -> i64 {
    keys.get("channel").and_then(Value::as_f64).unwrap_or(0.0) as i64
}

/// **Why a list of parts cannot be a sequence's**, or `None`: a part on no
/// staff, a polyphony of more than two voices a staff, a part on both
/// channels and a zone, a channel outside MIDI's sixteen, or a channel two
/// parts claim.
pub fn refusal(parts: &[Part]) -> Option<String> {
    let mut claimed = BTreeSet::new();
    for (i, part) in parts.iter().enumerate() {
        let named = if part.name.is_empty() {
            format!("part {}", i + 1)
        } else {
            format!("the part {:?}", part.name)
        };
        if part.staves == 0 {
            return Some(format!("{named} is written on no staff"));
        }
        if !(1..=2).contains(&part.writing.voices_per_staff) {
            return Some(format!(
                "{named} writes {} voices on a staff; a staff holds one or two",
                part.writing.voices_per_staff
            ));
        }
        if part.zone.is_some() && !part.channels.is_empty() {
            return Some(format!(
                "{named} is played on channels and on an MPE zone; it is one or the other"
            ));
        }
        for channel in &part.channels {
            if !(0..16).contains(channel) {
                return Some(format!(
                    "{named} names channel {channel}; MIDI's are 0 to 15"
                ));
            }
            if !claimed.insert(*channel) {
                return Some(format!("channel {channel} is claimed by two parts"));
            }
        }
    }
    None
}
