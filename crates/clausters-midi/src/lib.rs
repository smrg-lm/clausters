//! Reusable MIDI file I/O for clausters clients (and, later, the server).
//!
//! Same contract as `clausters-ffi` and the server's embed ABI: **only flat
//! data crosses** — integers and pointer+length arrays in, a malloc'd byte
//! buffer out (the same shape as `clausters_render`/`clausters_free_samples`),
//! never a library type. A per-language wrapper (Python `ctypes` now) sits on
//! top; check [`clausters_midi_abi_version`] first.
//!
//! Scope (M17 client sub-part 1): write a **Standard MIDI File** (`.mid`, SMF
//! type 0) from a flat list of timed channel-voice messages — the interop
//! format every DAW reads. The MIDI 2.0 **Clip File** (full 16/32-bit
//! resolution via `midi2-clip`) is the planned follow-up behind the same ABI;
//! SMF deliberately quantizes to MIDI 1.0's 7 bits on write.

use midly::live::LiveEvent;
use midly::{Format, Header, MetaMessage, Smf, Timing, Track, TrackEvent, TrackEventKind};

/// The C ABI version of this surface. Bump on any incompatible change.
pub const MIDI_ABI_VERSION: u32 = 1;

/// One timed MIDI event: an absolute `tick` (in the file's PPQ time base) and
/// up to three raw channel-voice bytes (`status`, `data1`, `data2`). The byte
/// count is derived from the status nibble, so two-byte messages (program
/// change, channel pressure) ignore the third byte.
#[derive(Clone, Copy)]
pub struct TimedMessage {
    pub tick: u32,
    pub bytes: [u8; 3],
}

/// Number of data bytes a channel-voice status byte carries (1 for program
/// change `0xC` and channel pressure `0xD`, else 2). Non-channel-voice status
/// bytes return `None`.
fn data_len(status: u8) -> Option<usize> {
    match status & 0xF0 {
        0x80 | 0x90 | 0xA0 | 0xB0 | 0xE0 => Some(2),
        0xC0 | 0xD0 => Some(1),
        _ => None,
    }
}

/// Builds a type-0 Standard MIDI File from `events` at `ppq` ticks per quarter
/// note. Events are sorted by tick (a stable sort keeps same-tick order, e.g. a
/// note-off before a re-triggered note-on); malformed status bytes are skipped.
pub fn write_smf(events: &[TimedMessage], ppq: u16) -> Vec<u8> {
    let mut events: Vec<TimedMessage> = events.to_vec();
    events.sort_by_key(|e| e.tick);

    let mut track: Track = Vec::with_capacity(events.len() + 1);
    let mut last_tick = 0u32;
    for ev in &events {
        let Some(n) = data_len(ev.bytes[0]) else {
            continue;
        };
        let Ok(LiveEvent::Midi { channel, message }) = LiveEvent::parse(&ev.bytes[..1 + n]) else {
            continue;
        };
        let delta = ev.tick.saturating_sub(last_tick);
        last_tick = ev.tick;
        track.push(TrackEvent {
            delta: delta.into(),
            kind: TrackEventKind::Midi { channel, message },
        });
    }
    track.push(TrackEvent {
        delta: 0.into(),
        kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
    });

    let smf = Smf {
        header: Header::new(Format::SingleTrack, Timing::Metrical(ppq.into())),
        tracks: vec![track],
    };
    let mut out = Vec::new();
    smf.write(&mut out)
        .expect("writing SMF to a Vec cannot fail");
    out
}

// ---- C ABI ----

/// Returns [`MIDI_ABI_VERSION`]; call before anything else.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_midi_abi_version() -> u32 {
    MIDI_ABI_VERSION
}

/// Writes a Standard MIDI File from `n` events and returns a malloc'd byte
/// buffer (its length in `out_len`), or null on a null/zero-size input. Free it
/// with [`clausters_midi_free`].
///
/// `ticks` points to `n` `u32` absolute ticks; `msgs` points to `3 * n` bytes
/// (status, data1, data2 per event). `ppq` is ticks per quarter note.
///
/// # Safety
/// `ticks` must be readable for `n` `u32`s, `msgs` for `3 * n` bytes, and
/// `out_len` must be a valid `*mut usize`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_midi_write_smf(
    ticks: *const u32,
    msgs: *const u8,
    n: usize,
    ppq: u16,
    out_len: *mut usize,
) -> *mut u8 {
    if ticks.is_null() || msgs.is_null() || out_len.is_null() || n == 0 {
        return std::ptr::null_mut();
    }
    // SAFETY: caller guarantees the ranges above.
    let ticks = unsafe { std::slice::from_raw_parts(ticks, n) };
    let msgs = unsafe { std::slice::from_raw_parts(msgs, 3 * n) };
    let events: Vec<TimedMessage> = (0..n)
        .map(|i| TimedMessage {
            tick: ticks[i],
            bytes: [msgs[3 * i], msgs[3 * i + 1], msgs[3 * i + 2]],
        })
        .collect();
    let bytes = write_smf(&events, ppq);
    // Hand ownership to the caller as a boxed slice; reclaimed by the free fn.
    let boxed = bytes.into_boxed_slice();
    let len = boxed.len();
    // SAFETY: out_len is non-null per the contract.
    unsafe { *out_len = len };
    Box::into_raw(boxed) as *mut u8
}

/// Frees a buffer returned by [`clausters_midi_write_smf`].
///
/// # Safety
/// `ptr`/`len` must be exactly what a `clausters_midi_*` call returned, freed
/// once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_midi_free(ptr: *mut u8, len: usize) {
    if ptr.is_null() {
        return;
    }
    // SAFETY: reconstitute the same boxed slice we leaked, then drop it.
    let slice = unsafe { std::slice::from_raw_parts_mut(ptr, len) };
    drop(unsafe { Box::from_raw(slice as *mut [u8]) });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note_on(tick: u32, channel: u8, key: u8, vel: u8) -> TimedMessage {
        TimedMessage {
            tick,
            bytes: [0x90 | channel, key, vel],
        }
    }
    fn note_off(tick: u32, channel: u8, key: u8) -> TimedMessage {
        TimedMessage {
            tick,
            bytes: [0x80 | channel, key, 0],
        }
    }

    /// Write a couple of notes, read the file back, and check the timing and
    /// the messages survive the round trip.
    #[test]
    fn smf_round_trips() {
        let events = [
            note_on(0, 0, 60, 100),
            note_off(96, 0, 60),
            note_on(96, 0, 67, 80),
            note_off(192, 0, 67),
        ];
        let bytes = write_smf(&events, 96);
        let smf = Smf::parse(&bytes).expect("valid SMF");
        assert_eq!(smf.header.timing, Timing::Metrical(96.into()));
        assert_eq!(smf.tracks.len(), 1);

        // Reconstruct (absolute tick, kind) and compare against what we wrote.
        let mut abs = 0u32;
        let mut notes = Vec::new();
        for ev in &smf.tracks[0] {
            abs += u32::from(ev.delta);
            if let TrackEventKind::Midi { message, .. } = ev.kind {
                notes.push((abs, message));
            }
        }
        assert_eq!(notes.len(), 4);
        assert_eq!(notes[0].0, 0); // first note-on at tick 0
        assert_eq!(notes[1].0, 96); // note-off
        assert_eq!(notes[2].0, 96); // re-trigger same tick
        assert_eq!(notes[3].0, 192);
        assert!(matches!(
            notes[0].1,
            midly::MidiMessage::NoteOn { key, vel } if key == 60 && vel == 100
        ));
    }

    /// Two-byte messages (program change) ignore the third byte and still
    /// serialize.
    #[test]
    fn two_byte_message_is_accepted() {
        let events = [TimedMessage {
            tick: 0,
            bytes: [0xC0, 5, 0],
        }];
        let bytes = write_smf(&events, 480);
        let smf = Smf::parse(&bytes).expect("valid SMF");
        let prog = smf.tracks[0].iter().find_map(|ev| match ev.kind {
            TrackEventKind::Midi {
                message: midly::MidiMessage::ProgramChange { program },
                ..
            } => Some(program),
            _ => None,
        });
        assert_eq!(prog.map(u8::from), Some(5));
    }

    /// The C ABI path produces the same bytes and frees cleanly.
    #[test]
    fn c_abi_matches_and_frees() {
        let events = [note_on(0, 0, 60, 100), note_off(48, 0, 60)];
        let direct = write_smf(&events, 96);

        let ticks: Vec<u32> = events.iter().map(|e| e.tick).collect();
        let msgs: Vec<u8> = events.iter().flat_map(|e| e.bytes).collect();
        let mut len = 0usize;
        let ptr = unsafe {
            clausters_midi_write_smf(ticks.as_ptr(), msgs.as_ptr(), events.len(), 96, &mut len)
        };
        assert!(!ptr.is_null());
        let via_abi = unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec();
        unsafe { clausters_midi_free(ptr, len) };
        assert_eq!(direct, via_abi);
    }
}
