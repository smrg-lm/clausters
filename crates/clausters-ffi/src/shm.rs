//! The shared-memory segment, for a peer that maps it from another language.
//!
//! A `ctypes` or N-API client maps the file itself — that part is the
//! language's, and there is nothing to share about it — and then needs to know
//! **where everything is**. That used to mean transcribing the layout into the
//! binding, which is how one client came to declare 1024 control buses against
//! a server that had had 16 384 for months: the number was wrong, unused, and
//! invisible.
//!
//! So the numbers come from here instead. [`clausters_core_shm_shape`] answers
//! every offset and count once, at attach; the rest of this module is the
//! things that are **logic rather than arithmetic** — the directory's seqlock,
//! the ring framing, the region file's name — where a second implementation is
//! a second set of bugs.
//!
//! Every entry takes the mapping as a pointer and a length and validates it
//! before touching anything, because a pointer from another language is not a
//! promise. What it cannot check is that the memory stays mapped for the
//! duration of the call: that is the caller's, and it is the only thing asked
//! of them.

use clausters_core::shm::{Role, Shape, View};

/// Attaching failed: the magic, the version or the size (see
/// [`clausters_core_shm_shape`]).
pub const SHM_INVALID: i32 = -1;
/// The slot, the tap or the frame asked for is not there.
pub const SHM_ABSENT: i32 = -2;
/// The caller's buffer is too small for what was asked.
pub const SHM_TOO_SMALL: i32 = -3;

/// The segment layout version this build speaks — the number a peer checks
/// against the header before anything else.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_shm_abi_version() -> u32 {
    clausters_core::shm::ABI_VERSION
}

/// Validates the segment at (`base`, `len`) and writes its **shape**: every
/// count and every byte offset a reader needs, in one call.
///
/// Returns 0, or [`SHM_INVALID`] when the memory is not a segment of this
/// version — bad magic, a different layout version, or a length that does not
/// match the regions the header claims.
///
/// # Safety
/// `base` must point at `len` readable bytes that stay mapped for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_shm_shape(
    base: *const u8,
    len: usize,
    out: *mut Shape,
) -> i32 {
    if base.is_null() || out.is_null() {
        return SHM_INVALID;
    }
    // SAFETY: caller contract.
    let Ok(view) = (unsafe { View::attach(base as *mut u8, len) }) else {
        return SHM_INVALID;
    };
    // SAFETY: caller contract.
    unsafe { *out = view.shape() };
    0
}

/// The byte size of a segment carrying these counts — what a peer sizes a file
/// to before creating one.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_shm_segment_size(
    control_buses: usize,
    taps: usize,
    tap_frames: usize,
    buffers: usize,
) -> usize {
    clausters_core::shm::segment_size(control_buses, taps, tap_frames, buffers)
}

/// Writes a fresh header over (`base`, `len`), making it a segment.
///
/// For a peer that **creates** one rather than attaching — which the editor's
/// arrangement makes an ordinary thing to be (the process that owns the
/// samples creates the segment, and every server attaches to it). `len` must
/// be at least [`clausters_core_shm_segment_size`] for the counts given, and
/// nothing else may have attached yet.
///
/// Returns 0 or [`SHM_TOO_SMALL`].
///
/// # Safety
/// `base` must point at `len` writable bytes that stay mapped for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_shm_init(
    base: *mut u8,
    len: usize,
    control_buses: usize,
    taps: usize,
    tap_frames: usize,
) -> i32 {
    if base.is_null() || len < clausters_core::shm::segment_size(control_buses, taps, tap_frames, 0)
    {
        return SHM_TOO_SMALL;
    }
    // SAFETY: caller contract, and the length was just checked.
    unsafe { View::init(base, len, control_buses, taps, tap_frames) };
    0
}

/// What the directory says about pool buffer `bufnum` — its generation, frame
/// count, channel count and sample rate — read under the generation twice, so
/// a row caught mid-write is re-read rather than believed.
///
/// Returns 0, [`SHM_ABSENT`] when the slot is empty or out of range, or
/// [`SHM_INVALID`].
///
/// # Safety
/// As [`clausters_core_shm_shape`]; the four out pointers must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_shm_buffer_info(
    base: *const u8,
    len: usize,
    bufnum: usize,
    out_generation: *mut u64,
    out_frames: *mut u64,
    out_channels: *mut u64,
    out_sample_rate: *mut f64,
) -> i32 {
    if base.is_null() {
        return SHM_INVALID;
    }
    // SAFETY: caller contract.
    let Ok(view) = (unsafe { View::attach(base as *mut u8, len) }) else {
        return SHM_INVALID;
    };
    let Some(shape) = view.buffer_info(bufnum) else {
        return SHM_ABSENT;
    };
    // SAFETY: caller contract.
    unsafe {
        *out_generation = shape.generation;
        *out_frames = shape.frames as u64;
        *out_channels = shape.channels as u64;
        *out_sample_rate = shape.sample_rate;
    }
    0
}

/// Writes the suffix a buffer's region file carries beside the segment's own
/// path (`.buf<n>.<generation>`) into `out` as a NUL-terminated string.
///
/// Three processes name that file; a name built three times is a name that can
/// differ, which is why this is a call and not a format string in a comment.
///
/// Returns the number of bytes written (the NUL excluded) or
/// [`SHM_TOO_SMALL`].
///
/// # Safety
/// `out` must be writable for `cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_shm_region_suffix(
    bufnum: usize,
    generation: u64,
    out: *mut u8,
    cap: usize,
) -> i32 {
    let suffix = clausters_core::shm::region_suffix(bufnum, generation);
    let bytes = suffix.as_bytes();
    if out.is_null() || cap <= bytes.len() {
        return SHM_TOO_SMALL;
    }
    // SAFETY: caller contract, and the length was just checked.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len());
        *out.add(bytes.len()) = 0;
    }
    bytes.len() as i32
}

/// Pushes one OSC packet into the segment's outbound ring, tagged for `peer`.
/// `role` is 0 for a server (writing replies) and 1 for a client (writing
/// commands).
///
/// Returns 0, or [`SHM_TOO_SMALL`] when the ring is momentarily full — which is
/// backpressure and not an error: nothing was dropped and the caller may retry.
///
/// # Safety
/// As [`clausters_core_shm_shape`]; `packet` must be readable for `len`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_shm_push(
    base: *const u8,
    len: usize,
    role: u32,
    peer: u32,
    packet: *const u8,
    packet_len: usize,
) -> i32 {
    if base.is_null() || packet.is_null() {
        return SHM_INVALID;
    }
    // SAFETY: caller contract.
    let Ok(view) = (unsafe { View::attach(base as *mut u8, len) }) else {
        return SHM_INVALID;
    };
    // SAFETY: caller contract.
    let bytes = unsafe { std::slice::from_raw_parts(packet, packet_len) };
    if view.push(role_of(role), peer, bytes) {
        0
    } else {
        SHM_TOO_SMALL
    }
}

/// Pops one OSC packet from the segment's inbound ring into `out`, writing its
/// peer tag and its length.
///
/// Returns 0, [`SHM_ABSENT`] when the ring is empty (or held garbage, which
/// resyncs it), or [`SHM_INVALID`].
///
/// # Safety
/// As [`clausters_core_shm_shape`]; `out` must be writable for `cap`, and both
/// out pointers writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_shm_pop(
    base: *const u8,
    len: usize,
    role: u32,
    out: *mut u8,
    cap: usize,
    out_peer: *mut u32,
    out_len: *mut usize,
) -> i32 {
    if base.is_null() || out.is_null() {
        return SHM_INVALID;
    }
    // SAFETY: caller contract.
    let Ok(view) = (unsafe { View::attach(base as *mut u8, len) }) else {
        return SHM_INVALID;
    };
    // SAFETY: caller contract.
    let buf = unsafe { std::slice::from_raw_parts_mut(out, cap) };
    let Some((peer, n)) = view.try_pop(role_of(role), buf) else {
        return SHM_ABSENT;
    };
    // SAFETY: caller contract.
    unsafe {
        *out_peer = peer;
        *out_len = n;
    }
    0
}

/// 0 is the server's end of the pair, anything else the client's — the two
/// values a caller can hold without a header of enum constants.
fn role_of(role: u32) -> Role {
    if role == 0 {
        Role::Server
    } else {
        Role::Client
    }
}
