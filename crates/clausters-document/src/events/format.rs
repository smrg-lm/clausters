//! **The format a sequence is written in**, and the conversions between
//! formats, each a copy.
//!
//! A sequence is edited as a file of one format: MIDI 1.0, MPE, MIDI 2.0 or
//! notation. Each holds what the others do not -- a page's written values and
//! voices, a performance's joins, a note's own curves in the specs that say
//! them -- so a view never switches a sequence from one to another: the
//! format is read from what the sequence holds ([`EventSequence::format`]),
//! and going to another is a conversion into a new sequence, the one it was
//! made from left as it was ([`EventSequence::as_midi`], and
//! [`EventSequence::notated`] for the page).

use serde_json::Value;

use super::{EventSequence, MidiSpec};

/// **What a sequence is written as.**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// A page: its notes say their written values, staves and voices, and
    /// what they sound is derived from them.
    Notation,
    /// A performance written for one MIDI spec.
    Midi(MidiSpec),
    /// A performance for the server, which states no MIDI spec, and where
    /// every curve is legal.
    Events,
}

impl Format {
    /// **The format's name**, as both clients spell it: `"notation"`,
    /// `"1.0"`, `"mpe"`, `"2.0"` or `"events"`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Format::Notation => "notation",
            Format::Midi(MidiSpec::Midi1) => "1.0",
            Format::Midi(MidiSpec::Mpe { .. }) => "mpe",
            Format::Midi(MidiSpec::Midi2) => "2.0",
            Format::Events => "events",
        }
    }
}

/// The keys that state what a note sounds, one of which a note keeps when its
/// written pitches are left out.
const SOUNDING_PITCH: &[&str] = &["midinote", "freq", "degree", "note"];

impl EventSequence {
    /// **The format the sequence is written in**: notation when it is written
    /// as a page ([`EventSequence::written_as_notation`]), else the MIDI spec
    /// it states, else [`Format::Events`].
    #[must_use]
    pub fn format(&self) -> Format {
        if self.written_as_notation() {
            return Format::Notation;
        }
        self.midi.map_or(Format::Events, Format::Midi)
    }

    /// **A copy of the sequence as its performance, written for `spec`**:
    /// every notation key of its events (`clausters_core::event::notation`)
    /// and its `notation` section left out, and what sounds kept -- a note
    /// that sounded its written pitch states it as a `midinote`. This sequence
    /// is not changed.
    ///
    /// # Errors
    /// When a curve the sequence holds has no spelling in `spec`, as
    /// [`super::EventsIntent::Midi`] refuses one.
    pub fn as_midi(&self, spec: MidiSpec) -> Result<Self, String> {
        let notation = clausters_core::event::notation::names();
        let mut copy = self.clone();
        for event in &mut copy.events {
            let Value::Object(keys) = &mut event.data.0 else {
                continue;
            };
            if !SOUNDING_PITCH.iter().any(|key| keys.contains_key(*key))
                && let Some(midinote) = keys
                    .get("pitches")
                    .and_then(clausters_core::event::notation::written_midinote)
            {
                keys.insert("midinote".into(), midinote.into());
            }
            keys.retain(|key, _| !notation.contains(&key.as_str()));
        }
        copy.notation = None;
        if let Some(why) = copy.unsayable(spec) {
            return Err(why);
        }
        copy.midi = Some(spec);
        Ok(copy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Event;
    use serde_json::json;

    fn written() -> EventSequence {
        EventSequence {
            events: vec![
                Event::new(
                    0.0,
                    json!({"dur": 1.0, "amp": 0.2, "value": [1, 4], "staff": 0, "voice": 0,
                           "pitches": [{"step": "e", "alter": 0, "octave": 4}]}),
                ),
                Event::new(1.0, json!({"dur": 1.0, "midinote": 67, "value": [1, 4]})),
            ],
            notation: Some(json!({"grid": {"meters": []}})),
            ..EventSequence::default()
        }
    }

    /// **The format is read from what the sequence holds**: a page, a spec,
    /// or none.
    #[test]
    fn a_sequence_says_its_format() {
        let page = written();
        assert_eq!(page.format(), Format::Notation);
        let mut performance = EventSequence::default();
        assert_eq!(performance.format().name(), "events");
        performance.midi = Some(MidiSpec::Midi2);
        assert_eq!(performance.format().name(), "2.0");
    }

    /// **A page as MIDI is a copy that sounds what the page sounded**: no
    /// notation key and no section, a written pitch stated as the note it
    /// sounds, and the page itself unchanged.
    #[test]
    fn a_page_as_midi_is_its_performance_and_the_page_stays() {
        let page = written();
        let midi = page.as_midi(MidiSpec::Midi1).unwrap();
        assert_eq!(midi.format(), Format::Midi(MidiSpec::Midi1));
        let first = midi.events[0].keys();
        assert_eq!(first.get("midinote"), Some(&json!(64.0)));
        assert_eq!(first.get("amp"), Some(&json!(0.2)));
        for key in ["value", "staff", "voice", "pitches"] {
            assert!(!first.contains_key(key), "{key} was kept");
        }
        assert_eq!(midi.events[1].keys().get("midinote"), Some(&json!(67)));
        assert_eq!(page.format(), Format::Notation, "the page is not changed");
    }

    /// **A spec that cannot say a curve the sequence holds is refused**, as
    /// writing the sequence for it is.
    #[test]
    fn a_spec_that_cannot_say_a_curve_is_refused() {
        let mut sequence = written();
        sequence.events[0].automation = vec![
            serde_json::from_value(json!({"id": 9, "target": {"bend": true}, "points": []}))
                .unwrap(),
        ];
        assert!(sequence.as_midi(MidiSpec::Midi1).is_err());
        assert!(sequence.as_midi(MidiSpec::Midi2).is_ok());
    }
}
