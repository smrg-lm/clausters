//! Reusable MIDI file I/O for clausters clients (and, later, the server).
//!
//! Same contract as `clausters-ffi` and the server's embed ABI: **only flat
//! data crosses** — integers and pointer+length arrays in, a malloc'd byte
//! buffer out (the same shape as `clausters_render`/`clausters_free_samples`),
//! never a library type. A per-language wrapper (Python `ctypes` now) sits on
//! top; check [`clausters_midi_abi_version`] first.
//!
//! Scope (M17 client output): write a **Standard MIDI File** (`.mid`, SMF type
//! 0, via `midly`) — the interop format every DAW reads — and a **MIDI 2.0
//! Clip File** (SMF2CLIP, assembled from `midi2`'s typed UMP messages) that
//! carries note velocities at 16-bit resolution instead of SMF's 7 bits. With
//! the `live` feature, also a virtual MIDI **output port** (midir/ALSA) for
//! real-time playback, and — for the client's responder layer — a virtual MIDI
//! **input port** that other apps/devices route into, drained by polling (no
//! callback crosses the boundary, keeping the flat-data contract). (The planned
//! `midi2-clip` crate is a v0.1.0 stub — `write_clip_file` is `todo!()` — so
//! the clip container is built here.)

use midi2::channel_voice2::{NoteOff, NoteOn};
use midi2::prelude::*;
use midi2::ump_stream::{EndOfClip, StartOfClip};
use midi2::utility::{DeltaClockstamp, DeltaClockstampTpq};
use midly::live::LiveEvent;
use midly::{Format, Header, MetaMessage, Smf, Timing, Track, TrackEvent, TrackEventKind};

/// The C ABI version of this surface. Bump on any incompatible change.
///
/// v2 added the live virtual MIDI **input** port (`clausters_midi_input_*`) for
/// the client's responder layer; the v1 surface (file writers + live output)
/// is unchanged.
pub const MIDI_ABI_VERSION: u32 = 2;

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

// ---- MIDI 2.0 Clip File (SMF2CLIP) ----
//
// The full-resolution format the plan wanted. The planned `midi2-clip` crate
// (v0.1.0) turned out to be a stub — its `write_clip_file` is `todo!()` — so
// the file is assembled here from `midi2`'s typed UMP messages (the message
// layer the plan pinned, which *is* functional): the 8-byte `SMF2CLIP` header,
// then a UMP stream of DCTPQ + Start of Clip + (Delta Clockstamp + Channel
// Voice 2) per event + End of Clip, words big-endian. MIDI 1.0 note velocities
// are widened to 16 bits, so a clip carries them at full resolution.

/// Widen a 7-bit value to 16 bits (bit-repeat fill, 0→0 and 127→65535) — the
/// same scaling the server uses for live MIDI 1.0 input.
fn scale_7_to_16(v: u8) -> u16 {
    let v = (v & 0x7f) as u16;
    (v << 9) | (v << 2) | (v >> 5)
}

/// The two UMP words of a MIDI 2.0 Channel Voice note on/off from a MIDI 1.0
/// status byte and data. `None` for non-note status (the client's MIDI
/// destination only emits notes; other message types can be added here).
fn note_cv2_words(status: u8, d1: u8, d2: u8) -> Option<[u32; 2]> {
    let channel = u4::new(status & 0x0f);
    let note = u7::new(d1 & 0x7f);
    let velocity = scale_7_to_16(d2);
    match status & 0xf0 {
        0x90 if d2 != 0 => {
            let mut m = NoteOn::<[u32; 2]>::new();
            m.set_channel(channel);
            m.set_note_number(note);
            m.set_velocity(velocity);
            Some([m.data()[0], m.data()[1]])
        }
        0x80 | 0x90 => {
            let mut m = NoteOff::<[u32; 2]>::new();
            m.set_channel(channel);
            m.set_note_number(note);
            m.set_velocity(velocity);
            Some([m.data()[0], m.data()[1]])
        }
        _ => None,
    }
}

/// Builds a MIDI 2.0 Clip File (SMF2CLIP) from `events` at `ppq` ticks per
/// quarter note. Note velocities are carried at 16-bit resolution; unsupported
/// status bytes are skipped.
pub fn write_clip(events: &[TimedMessage], ppq: u16) -> Vec<u8> {
    let mut events: Vec<TimedMessage> = events.to_vec();
    events.sort_by_key(|e| e.tick);

    let mut words: Vec<u32> = Vec::new();
    let mut dctpq = DeltaClockstampTpq::<[u32; 1]>::new();
    dctpq.set_time_data(ppq);
    words.push(dctpq.data()[0]);
    words.extend_from_slice(StartOfClip::<[u32; 4]>::new().data());

    let mut last = 0u32;
    for ev in &events {
        let Some(cv2) = note_cv2_words(ev.bytes[0], ev.bytes[1], ev.bytes[2]) else {
            continue;
        };
        let delta = ev.tick.saturating_sub(last);
        last = ev.tick;
        if delta > 0 {
            let mut dc = DeltaClockstamp::<[u32; 1]>::new();
            dc.set_time_data(u20::new(delta & 0x000F_FFFF));
            words.push(dc.data()[0]);
        }
        words.extend_from_slice(&cv2);
    }
    words.extend_from_slice(EndOfClip::<[u32; 4]>::new().data());

    let mut out = Vec::with_capacity(8 + words.len() * 4);
    out.extend_from_slice(b"SMF2CLIP");
    for w in words {
        out.extend_from_slice(&w.to_be_bytes());
    }
    out
}

// ---- C ABI ----

/// Returns [`MIDI_ABI_VERSION`]; call before anything else.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_midi_abi_version() -> u32 {
    MIDI_ABI_VERSION
}

/// Collects `n` `(tick, 3-byte message)` events from the C arrays, or `None`
/// on a null/zero input.
///
/// # Safety
/// `ticks` must be readable for `n` `u32`s and `msgs` for `3 * n` bytes.
unsafe fn collect_events(
    ticks: *const u32,
    msgs: *const u8,
    n: usize,
) -> Option<Vec<TimedMessage>> {
    if ticks.is_null() || msgs.is_null() || n == 0 {
        return None;
    }
    // SAFETY: caller guarantees the ranges.
    let ticks = unsafe { std::slice::from_raw_parts(ticks, n) };
    let msgs = unsafe { std::slice::from_raw_parts(msgs, 3 * n) };
    Some(
        (0..n)
            .map(|i| TimedMessage {
                tick: ticks[i],
                bytes: [msgs[3 * i], msgs[3 * i + 1], msgs[3 * i + 2]],
            })
            .collect(),
    )
}

/// Hands `bytes` to the caller as a malloc'd buffer (length in `out_len`),
/// reclaimed by [`clausters_midi_free`].
fn leak_bytes(bytes: Vec<u8>, out_len: *mut usize) -> *mut u8 {
    let boxed = bytes.into_boxed_slice();
    let len = boxed.len();
    // SAFETY: out_len is non-null per each caller's contract.
    unsafe { *out_len = len };
    Box::into_raw(boxed) as *mut u8
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
    if out_len.is_null() {
        return std::ptr::null_mut();
    }
    let Some(events) = (unsafe { collect_events(ticks, msgs, n) }) else {
        return std::ptr::null_mut();
    };
    leak_bytes(write_smf(&events, ppq), out_len)
}

/// Writes a MIDI 2.0 Clip File (SMF2CLIP) from `n` events. Same arguments,
/// return and freeing as [`clausters_midi_write_smf`]; carries note velocities
/// at 16-bit resolution.
///
/// # Safety
/// Same as [`clausters_midi_write_smf`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_midi_write_clip(
    ticks: *const u32,
    msgs: *const u8,
    n: usize,
    ppq: u16,
    out_len: *mut usize,
) -> *mut u8 {
    if out_len.is_null() {
        return std::ptr::null_mut();
    }
    let Some(events) = (unsafe { collect_events(ticks, msgs, n) }) else {
        return std::ptr::null_mut();
    };
    leak_bytes(write_clip(&events, ppq), out_len)
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

// ---- Live MIDI output (feature `live`, ALSA seq on Linux via midir) ----
//
// The client sub-part 2 surface: a virtual MIDI output port other apps/devices
// subscribe to. An opaque handle (the same pattern as the embed ABI's
// `clausters_open`) crosses the boundary; raw channel-voice bytes go out.
#[cfg(all(feature = "live", unix))]
mod live {
    use midir::os::unix::VirtualOutput;
    use midir::{MidiOutput, MidiOutputConnection};

    /// Opaque live output handle.
    pub struct Output {
        conn: MidiOutputConnection,
    }

    /// Opens a virtual MIDI output port named `name` (UTF-8, `name_len` bytes).
    /// Returns an opaque handle or null on failure. Close with
    /// [`clausters_midi_output_close`].
    ///
    /// # Safety
    /// `name` must be readable for `name_len` bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn clausters_midi_output_open(
        name: *const u8,
        name_len: usize,
    ) -> *mut Output {
        if name.is_null() {
            return std::ptr::null_mut();
        }
        // SAFETY: caller guarantees the range.
        let bytes = unsafe { std::slice::from_raw_parts(name, name_len) };
        let Ok(name) = std::str::from_utf8(bytes) else {
            return std::ptr::null_mut();
        };
        let Ok(out) = MidiOutput::new("clausters") else {
            return std::ptr::null_mut();
        };
        match out.create_virtual(name) {
            Ok(conn) => Box::into_raw(Box::new(Output { conn })),
            Err(_) => std::ptr::null_mut(),
        }
    }

    /// Sends `len` raw MIDI bytes out the port now. Returns 0 on success, <0 on
    /// a null handle/buffer or a send error.
    ///
    /// # Safety
    /// `handle` must come from [`clausters_midi_output_open`] (not yet closed);
    /// `bytes` must be readable for `len`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn clausters_midi_output_send(
        handle: *mut Output,
        bytes: *const u8,
        len: usize,
    ) -> i32 {
        if handle.is_null() || bytes.is_null() || len == 0 {
            return -1;
        }
        // SAFETY: per the contract.
        let out = unsafe { &mut *handle };
        let msg = unsafe { std::slice::from_raw_parts(bytes, len) };
        match out.conn.send(msg) {
            Ok(()) => 0,
            Err(_) => -2,
        }
    }

    /// Closes a port opened with [`clausters_midi_output_open`].
    ///
    /// # Safety
    /// `handle` must come from [`clausters_midi_output_open`], closed once.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn clausters_midi_output_close(handle: *mut Output) {
        if handle.is_null() {
            return;
        }
        // SAFETY: reconstitute and drop the box we leaked.
        drop(unsafe { Box::from_raw(handle) });
    }

    use midir::os::unix::VirtualInput;
    use midir::{MidiInput, MidiInputConnection};
    use std::sync::mpsc::{Receiver, channel};

    /// Opaque live input handle. `midir` runs the input callback on its own
    /// thread; it pushes each raw message into an `mpsc` channel the caller
    /// drains by polling — so no callback ever crosses the C boundary (the
    /// flat-data contract) and the host language keeps control of its threads.
    /// Dropping the handle closes the virtual port and stops the input thread.
    pub struct Input {
        events: Receiver<Vec<u8>>,
        _conn: MidiInputConnection<()>,
    }

    /// Opens a virtual MIDI input port named `name` (UTF-8, `name_len` bytes)
    /// that other apps/devices route into. Returns an opaque handle or null on
    /// failure. Drain it with [`clausters_midi_input_poll`]; close with
    /// [`clausters_midi_input_close`].
    ///
    /// # Safety
    /// `name` must be readable for `name_len` bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn clausters_midi_input_open(
        name: *const u8,
        name_len: usize,
    ) -> *mut Input {
        if name.is_null() {
            return std::ptr::null_mut();
        }
        // SAFETY: caller guarantees the range.
        let bytes = unsafe { std::slice::from_raw_parts(name, name_len) };
        let Ok(name) = std::str::from_utf8(bytes) else {
            return std::ptr::null_mut();
        };
        let Ok(input) = MidiInput::new("clausters") else {
            return std::ptr::null_mut();
        };
        let (tx, events) = channel();
        match input.create_virtual(
            name,
            move |_timestamp, msg, _| {
                // Forward every raw message verbatim; the host decodes/filters.
                let _ = tx.send(msg.to_vec());
            },
            (),
        ) {
            Ok(conn) => Box::into_raw(Box::new(Input {
                events,
                _conn: conn,
            })),
            Err(_) => std::ptr::null_mut(),
        }
    }

    /// Dequeues the next pending input message into `out` (capacity `cap`
    /// bytes), writing its byte length to `out_len`. Returns 1 when a message
    /// was written, 0 when the queue is empty, or <0 on a null argument or a
    /// message longer than `cap`. Poll in a loop until it returns 0.
    ///
    /// # Safety
    /// `handle` must come from [`clausters_midi_input_open`] (not yet closed);
    /// `out` must be writable for `cap` bytes and `out_len` a valid
    /// `*mut usize`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn clausters_midi_input_poll(
        handle: *mut Input,
        out: *mut u8,
        cap: usize,
        out_len: *mut usize,
    ) -> i32 {
        if handle.is_null() || out.is_null() || out_len.is_null() {
            return -1;
        }
        // SAFETY: per the contract.
        let input = unsafe { &*handle };
        let Ok(msg) = input.events.try_recv() else {
            return 0;
        };
        if msg.len() > cap {
            return -2;
        }
        // SAFETY: msg.len() <= cap, out writable for cap bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(msg.as_ptr(), out, msg.len());
            *out_len = msg.len();
        }
        1
    }

    /// Closes a port opened with [`clausters_midi_input_open`].
    ///
    /// # Safety
    /// `handle` must come from [`clausters_midi_input_open`], closed once.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn clausters_midi_input_close(handle: *mut Input) {
        if handle.is_null() {
            return;
        }
        // SAFETY: reconstitute and drop the box we leaked.
        drop(unsafe { Box::from_raw(handle) });
    }
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

    /// Walk a SMF2CLIP UMP stream by message-type size, accumulating Delta
    /// Clockstamps, and return `(abs_tick, note, velocity16)` per note-on.
    fn clip_note_ons(bytes: &[u8]) -> Vec<(u32, u32, u32)> {
        assert_eq!(&bytes[..8], b"SMF2CLIP");
        let words: Vec<u32> = bytes[8..]
            .chunks(4)
            .map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        let mut ons = Vec::new();
        let mut tick = 0u32;
        let mut i = 0;
        while i < words.len() {
            let w = words[i];
            let mt = (w >> 28) & 0xF;
            let size = match mt {
                0x0 => 1, // utility
                0x4 => 2, // channel voice 2
                0xF => 4, // ump stream (start/end of clip)
                _ => 1,
            };
            if mt == 0x0 && (w >> 20) & 0xF == 0x4 {
                tick += w & 0x000F_FFFF; // delta clockstamp
            } else if mt == 0x4 && (w >> 20) & 0xF == 0x9 {
                let note = (w >> 8) & 0x7F;
                let velocity = (words[i + 1] >> 16) & 0xFFFF;
                ons.push((tick, note, velocity));
            }
            i += size;
        }
        ons
    }

    #[test]
    fn clip_round_trips_with_16bit_velocity() {
        let events = [
            note_on(0, 0, 60, 100),
            note_off(96, 0, 60),
            note_on(96, 0, 67, 80),
            note_off(192, 0, 67),
        ];
        let bytes = write_clip(&events, 96);
        let ons = clip_note_ons(&bytes);
        assert_eq!(ons.len(), 2);
        assert_eq!(ons[0], (0, 60, scale_7_to_16(100) as u32));
        assert_eq!(ons[1], (96, 67, scale_7_to_16(80) as u32));
        // Velocity really is widened past 7 bits (not just `vel << 9`).
        assert!(scale_7_to_16(100) > (100u16 << 8));
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
