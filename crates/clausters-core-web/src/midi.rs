//! MIDI files, and MPE's outgoing half.
//!
//! A page has no virtual OS port, but it does have a score to write:
//! `MidiServer` over an NRT interface accumulates `(beat, message)` and
//! hands the ticked events here. The writers are `clausters-midi`'s -- the same
//! ones `clausters_midi_write_smf`/`_write_clip` open to the Python client -- so
//! a `.mid` saved from a tab and one saved from a script are the same bytes, and
//! a client that offers the take as a download re-implements nothing.
//!
//! The JS face takes the events flat, in the shape the C ABI already takes them:
//! `ticks` is n absolute ticks and `msgs` is 3n bytes (status, data1, data2).
//! Passing an array of objects would be the only other option and it would cost a
//! JS-side allocation per event on a path a bounce runs over every note.

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// Reads the flat `(ticks, msgs)` pair into events, checking the lengths agree.
#[cfg(target_arch = "wasm32")]
fn midi_events(ticks: &[u32], msgs: &[u8]) -> Result<Vec<clausters_midi::TimedMessage>, JsError> {
    if msgs.len() != 3 * ticks.len() {
        return Err(JsError::new(&format!(
            "msgs is {} bytes, not 3 per tick ({} ticks)",
            msgs.len(),
            ticks.len()
        )));
    }
    Ok(ticks
        .iter()
        .enumerate()
        .map(|(i, &tick)| clausters_midi::TimedMessage {
            tick,
            bytes: [msgs[3 * i], msgs[3 * i + 1], msgs[3 * i + 2]],
        })
        .collect())
}

/// Type-0 Standard MIDI File bytes from `n` events at `ppq` ticks per quarter
/// note.
///
/// JS face: `midiWriteSmf(Uint32Array, Uint8Array, ppq) -> Uint8Array`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = midiWriteSmf)]
pub fn midi_write_smf(ticks: &[u32], msgs: &[u8], ppq: u16) -> Result<Vec<u8>, JsError> {
    Ok(clausters_midi::write_smf(&midi_events(ticks, msgs)?, ppq))
}

/// [`midi_write_smf`] with the file's tempo: marks at `tempo_ticks`, each
/// `tempo_micros` microseconds per quarter note.
///
/// JS face: `midiWriteSmfTempo(Uint32Array, Uint8Array, ppq, Uint32Array,
/// Uint32Array) -> Uint8Array`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = midiWriteSmfTempo)]
pub fn midi_write_smf_tempo(
    ticks: &[u32],
    msgs: &[u8],
    ppq: u16,
    tempo_ticks: &[u32],
    tempo_micros: &[u32],
) -> Result<Vec<u8>, JsError> {
    let tempo: Vec<clausters_midi::TempoMark> = tempo_ticks
        .iter()
        .zip(tempo_micros)
        .map(|(&tick, &micros)| clausters_midi::TempoMark { tick, micros })
        .collect();
    Ok(clausters_midi::write_smf_with_tempo(
        &midi_events(ticks, msgs)?,
        ppq,
        &tempo,
    ))
}

/// A Standard MIDI File as JSON: `{"ppq", "events": [[tick, [bytes]]],
/// "tempo": [[tick, micros]]}` or `{"error"}`.
///
/// JS face: `midiReadSmf(Uint8Array) -> string`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = midiReadSmf)]
pub fn midi_read_smf(bytes: &[u8]) -> String {
    clausters_midi::read_smf_json(bytes)
}

/// MIDI 2.0 Clip File (SMF2CLIP) bytes from the same arguments, carrying note
/// velocities at 16-bit resolution.
///
/// JS face: `midiWriteClip(Uint32Array, Uint8Array, ppq) -> Uint8Array`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = midiWriteClip)]
pub fn midi_write_clip(ticks: &[u32], msgs: &[u8], ppq: u16) -> Result<Vec<u8>, JsError> {
    Ok(clausters_midi::write_clip(&midi_events(ticks, msgs)?, ppq))
}

/// A MIDI 2.0 Clip File from UMP packets: packet `i` at `ticks[i]`, `sizes[i]`
/// words of `words` in order -- the C ABI's `clausters_midi_write_clip_ump`.
///
/// JS face: `midiWriteClipUmp(Uint32Array, Uint8Array, Uint32Array, ppq) ->
/// Uint8Array`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = midiWriteClipUmp)]
pub fn midi_write_clip_ump(
    ticks: &[u32],
    sizes: &[u8],
    words: &[u32],
    ppq: u16,
) -> Result<Vec<u8>, JsError> {
    if sizes.len() != ticks.len() {
        return Err(JsError::new("a size per tick"));
    }
    let total: usize = sizes.iter().map(|&s| usize::from(s)).sum();
    if total != words.len() {
        return Err(JsError::new(&format!(
            "the sizes add to {total} words, and {} were given",
            words.len()
        )));
    }
    Ok(clausters_midi::write_clip_ump(
        &clausters_midi::ump_events(ticks, sizes, words),
        ppq,
    ))
}

/// A MIDI 2.0 Clip File read back as JSON -- the C ABI's
/// `clausters_midi_read_clip`.
///
/// JS face: `midiReadClip(Uint8Array) -> string`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = midiReadClip)]
pub fn midi_read_clip(bytes: &[u8]) -> String {
    clausters_midi::read_clip_json(bytes)
}

// ---- MPE: the outgoing half ----
//
// A `MidiServer` that is an MPE zone puts each note on a member channel of its
// own, with the note's expression ahead of its note-on. Which channel, and what
// bytes a zone and an expression are written as, are `clausters-midi`'s rules --
// the ones `clausters_mpe_assigner_*`, `clausters_mpe_zone_messages` and
// `clausters_mpe_expression_messages` open to the Python client -- so a pattern
// played to a zone from a page and from a script is the same messages on the
// same channels.

#[cfg(target_arch = "wasm32")]
fn mpe_side(upper: bool) -> clausters_midi::mpe::Side {
    if upper {
        clausters_midi::mpe::Side::Upper
    } else {
        clausters_midi::mpe::Side::Lower
    }
}

/// The messages that declare an MPE zone of `members` (the lower zone, master
/// channel 0, unless `upper`), flat, three bytes each.
///
/// JS face: `mpeZoneMessages(upper, members) -> Uint8Array`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = mpeZoneMessages)]
pub fn mpe_zone_messages(upper: bool, members: u8) -> Vec<u8> {
    clausters_midi::mpe::zone_messages(mpe_side(upper), members).concat()
}

/// A note's starting expression on its member `channel`, flat, three bytes
/// each: the bend in semitones through `range`, the pressure and the timbre
/// (0..1; absent for one the note does not state, which goes back to its
/// rest).
///
/// JS face: `mpeExpressionMessages(channel, bend, range, pressure?, timbre?,
/// timbreCc) -> Uint8Array`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = mpeExpressionMessages)]
pub fn mpe_expression_messages(
    channel: u8,
    bend: f32,
    range: f32,
    pressure: Option<f32>,
    timbre: Option<f32>,
    timbre_cc: u8,
) -> Vec<u8> {
    clausters_midi::mpe::expression_messages(channel, bend, range, pressure, timbre, timbre_cc)
        .concat()
}

/// Which member channel an outgoing note goes on, the JS face of
/// [`clausters_midi::mpe::Assigner`].
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = MpeAssigner)]
pub struct JsMpeAssigner(clausters_midi::mpe::Assigner);

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = MpeAssigner)]
impl JsMpeAssigner {
    /// An assigner over the members of a zone of `members`, the lower one
    /// unless `upper`.
    #[wasm_bindgen(constructor)]
    pub fn new(upper: bool, members: u8) -> JsMpeAssigner {
        JsMpeAssigner(clausters_midi::mpe::Assigner::new(mpe_side(upper), members))
    }

    /// The channel for a new note on `key`; `undefined` for a zone of no
    /// members.
    #[wasm_bindgen(js_name = noteOn)]
    pub fn note_on(&mut self, key: u8) -> Option<u8> {
        self.0.note_on(key & 0x7F)
    }

    /// A note on `key` ended on `channel`.
    #[wasm_bindgen(js_name = noteOff)]
    pub fn note_off(&mut self, channel: u8, key: u8) {
        self.0.note_off(channel, key & 0x7F);
    }
}
