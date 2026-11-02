//! **The score editor's MIDI files**, written here since the writer is a library
//! this host's build links and the applications crate does not.

/// **`sequence` as a MIDI file**, a MIDI 2.0 Clip File when `clip`, else a
/// Standard MIDI File: what a client's `to_clip` and `to_smf` write, at the
/// ticks per beat they write it at, with the writer they bind.
#[cfg(all(feature = "midi", not(target_arch = "wasm32")))]
pub(super) fn midi_file(
    sequence: &clausters_document::events::EventSequence,
    clip: bool,
) -> Option<Vec<u8>> {
    const PPQ: u16 = 480;
    if clip {
        return Some(clausters_midi::write_clip_ump(&sequence.to_ump(PPQ), PPQ));
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
    Some(clausters_midi::write_smf_with_tempo(&events, PPQ, &tempo))
}

/// A host without the MIDI crate writes no MIDI file. Only an application
/// asks for one, so a page without them has nothing to ask.
#[cfg(all(
    not(all(feature = "midi", not(target_arch = "wasm32"))),
    any(not(target_arch = "wasm32"), feature = "apps")
))]
pub(super) fn midi_file(_: &clausters_document::events::EventSequence, _: bool) -> Option<Vec<u8>> {
    None
}
