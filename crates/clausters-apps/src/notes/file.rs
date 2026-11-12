//! **The roll's file**: a sequence written in its format, and read from one.
//!
//! A sequence is edited as a file of one format
//! (`clausters_document::events::format`), so its file is what that format
//! is: an MEI file for a page, a Standard MIDI File for MIDI 1.0 and MPE, a
//! MIDI 2.0 Clip File for MIDI 2.0 -- and a sequence for the server, which
//! states no spec, a Standard MIDI File, the spelling every reader has. A
//! file is read back by what it is: a MIDI file by its header, and any other
//! as a score, through the engraver the application holds
//! ([`ScoreReader`]), since this crate links none.

use std::sync::Arc;

use clausters_document::EventSequence;
use clausters_document::events::{Format, MidiSpec};

/// **A score's text read as the sequence it plays as**: the engraver the
/// application holds (MEI, MusicXML, ABC -- whatever it reads), handed in as
/// a mark's outlines are ([`super::OutlineSource`]).
pub type ScoreReader = Arc<dyn Fn(&str) -> Result<EventSequence, String> + Send + Sync>;

/// **The engraver's reading, where the application holds one**, kept by a
/// roll that reads its own files.
#[derive(Clone, Default)]
pub struct Scores(pub Option<ScoreReader>);

impl std::fmt::Debug for Scores {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Scores").field(&self.0.is_some()).finish()
    }
}

/// The ticks per beat a MIDI file is written at: what a client's `to_clip`
/// and `to_smf` write at.
const PPQ: u16 = 480;

/// **The extensions a roll opens**, a MIDI file's and a score's.
pub const OPENS: &[&str] = &[
    "mid", "midi", "smf", "midi2", "mei", "musicxml", "xml", "mxl", "abc",
];

/// **The extension a file of `format` is written with.**
#[must_use]
pub fn extension(format: Format) -> &'static str {
    match format {
        Format::Notation => "mei",
        Format::Midi(MidiSpec::Midi2) => "midi2",
        Format::Midi(_) | Format::Events => "mid",
    }
}

/// **The sequence a file's bytes hold**: a Standard MIDI File (`MThd`) or a
/// MIDI 2.0 Clip File (`SMF2CLIP`) by its header, read as a client's
/// `from_smf` and `from_clip` read one, and any other file as a score through
/// `scores`.
///
/// # Errors
/// When the bytes are no file of these, or a score with no engraver to read
/// it.
pub fn read(bytes: &[u8], scores: Option<&ScoreReader>) -> Result<EventSequence, String> {
    if bytes.starts_with(b"SMF2CLIP") {
        let read = clausters_midi::read_clip(bytes)?;
        return EventSequence::from_ump(read.ppq, &read.events);
    }
    if bytes.starts_with(b"MThd") {
        let read = clausters_midi::read_smf(bytes)?;
        let tempo: Vec<(u32, u32)> = read.tempo.iter().map(|m| (m.tick, m.micros)).collect();
        return EventSequence::from_midi(read.ppq, &read.events, &tempo);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "neither a MIDI file nor a score")?;
    let scores = scores
        .ok_or("not a MIDI file, and this application holds no engraver, so it reads no score")?;
    scores(text)
}

/// **`sequence` as the file of its format** ([`extension`] says which).
///
/// # Errors
/// When a page cannot be written: always, in a build without the notation
/// layer.
pub fn write(sequence: &EventSequence) -> Result<Vec<u8>, String> {
    match sequence.format() {
        Format::Notation => page(sequence),
        Format::Midi(MidiSpec::Midi2) => Ok(midi_file(sequence, true)),
        Format::Midi(_) | Format::Events => Ok(midi_file(sequence, false)),
    }
}

/// **`sequence` as a MIDI file**, a MIDI 2.0 Clip File when `clip`, else a
/// Standard MIDI File: what a client's `to_clip` and `to_smf` write, at the
/// ticks per beat they write it at, with the writer they bind.
#[must_use]
pub fn midi_file(sequence: &EventSequence, clip: bool) -> Vec<u8> {
    if clip {
        return clausters_midi::write_clip_ump(&sequence.to_ump(PPQ), PPQ);
    }
    let (events, tempo) = sequence.to_midi(PPQ);
    let events: Vec<clausters_midi::TimedMessage> = events
        .into_iter()
        .map(|(tick, message)| {
            let mut bytes = [0u8; 3];
            let n = message.len().min(3);
            bytes[..n].copy_from_slice(&message[..n]);
            clausters_midi::TimedMessage { tick, bytes }
        })
        .collect();
    let tempo: Vec<clausters_midi::TempoMark> = tempo
        .into_iter()
        .map(|(tick, micros)| clausters_midi::TempoMark { tick, micros })
        .collect();
    clausters_midi::write_smf_with_tempo(&events, PPQ, &tempo)
}

/// **A page as MEI**: the sequence read into its score as the score editor
/// over it reads one, and the score written.
#[cfg(feature = "notation")]
fn page(sequence: &EventSequence) -> Result<Vec<u8>, String> {
    use clausters_document::events::reading::Transcription;
    let read = clausters_document::events::transcription::read(
        sequence,
        &Transcription::of(sequence),
        &clausters_notation::default_interpretation(),
    )?;
    clausters_notation::sheet_to_mei(&read.sheet).map(String::into_bytes)
}

/// Without the notation layer there is no page to write.
#[cfg(not(feature = "notation"))]
fn page(_: &EventSequence) -> Result<Vec<u8>, String> {
    Err("writing a page needs the notation layer, which this build leaves out".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_document::events::Event;
    use serde_json::json;

    fn two_notes(midi: Option<MidiSpec>) -> EventSequence {
        let mut sequence = EventSequence::new(vec![
            Event::new(0.0, json!({"midinote": 60, "dur": 1.0})),
            Event::new(1.0, json!({"midinote": 64, "dur": 1.0})),
        ]);
        sequence.midi = midi;
        sequence
    }

    /// **A MIDI sequence is written as its format's file and read back**,
    /// each by its header: a Standard MIDI File for MIDI 1.0, a Clip File
    /// for MIDI 2.0.
    #[test]
    fn a_midi_sequence_is_its_format_s_file_and_reads_back() {
        for (spec, header) in [
            (MidiSpec::Midi1, &b"MThd"[..]),
            (MidiSpec::Midi2, &b"SMF2CLIP"[..]),
        ] {
            let sequence = two_notes(Some(spec));
            let bytes = write(&sequence).unwrap();
            assert!(bytes.starts_with(header));
            let back = read(&bytes, None).unwrap();
            assert_eq!(back.events.len(), 2);
            assert_eq!(back.format(), Format::Midi(spec));
        }
        assert_eq!(extension(Format::Midi(MidiSpec::Midi2)), "midi2");
    }

    /// **A score is read through the engraver handed in**, and without one
    /// says so.
    #[test]
    fn a_score_is_read_through_the_engraver_or_refused() {
        let refused = read(b"<mei/>", None).unwrap_err();
        assert!(refused.contains("engraver"), "{refused}");
        let reader: ScoreReader = Arc::new(|text: &str| {
            assert_eq!(text, "<mei/>");
            Ok(two_notes(None))
        });
        assert_eq!(read(b"<mei/>", Some(&reader)).unwrap().events.len(), 2);
    }

    /// **A page is written as MEI**, which reads back as a page.
    #[cfg(feature = "notation")]
    #[test]
    fn a_page_is_written_as_mei() {
        let page = two_notes(None).notated(None, None, None).unwrap();
        let bytes = write(&page).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("<mei"), "{text}");
        assert_eq!(extension(page.format()), "mei");
    }
}
