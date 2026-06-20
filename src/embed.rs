//! M14: the embed C ABI (feature `embed`) — Clausters as a library.
//!
//! The cdylib (`libclausters.so` / `.dylib` / `.dll`) is the **canonical
//! language-agnostic surface**: thin bindings in any language sit on top of
//! it (Python via stdlib `ctypes` in `clients/python/clausters.py`,
//! JavaScript via Node/Deno FFI later). The boundary follows the project
//! rule: only **basic structures** cross it — flat `f32` arrays as
//! pointer + length, integers, NUL-terminated error strings. Never a
//! library type: a numpy array can *view* the returned pointer without
//! copying, but that is the client's choice, not a dependency.
//!
//! Two entry points:
//!
//! - [`clausters_render`]: the synchronous "scientific" call — render a
//!   binary score offline and get the interleaved samples back. No audio
//!   device, no threads, no asynchrony; blocks the *caller* only.
//! - [`clausters_open`]/[`clausters_send`]/[`clausters_poll`]: a full live
//!   server in-process. Commands are ordinary OSC packets delivered by
//!   function call through the same heap-backed ring the `--shm` transport
//!   uses (`server::ipc`); replies are polled. The data plane is direct:
//!   [`clausters_clock`] and [`clausters_ctl_set`]/[`clausters_ctl_get`]
//!   touch the segment atomics with no command round trip at all.
//!
//! Versioning: check [`clausters_abi_version`] before anything else; the
//! constant moves in lockstep with the segment layout version (the scsynth
//! plugin-ABI lesson: every binary boundary is versioned and checked).

#![cfg(feature = "embed")]

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;

use crate::server::ipc::{ABI_VERSION, IpcPeer, Role, Segment};
use crate::server::render::{RenderConfig, Score, render_to_vec};

/// The C ABI version (== the IPC segment layout version).
#[unsafe(no_mangle)]
pub extern "C" fn clausters_abi_version() -> u32 {
    ABI_VERSION
}

/// Writes `msg` into (`buf`, `cap`) as a NUL-terminated C string.
fn write_error(msg: &str, buf: *mut u8, cap: usize) {
    if buf.is_null() || cap == 0 {
        return;
    }
    let bytes = msg.as_bytes();
    let n = bytes.len().min(cap - 1);
    // SAFETY: caller-provided buffer of at least `cap` bytes.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, n);
        *buf.add(n) = 0;
    }
}

/// Renders a binary score (the `--nrt` format: length-prefixed OSC packets,
/// timetags in seconds from the start) synchronously.
///
/// On success returns a malloc'd interleaved `f32` buffer and writes the
/// frame count to `out_frames` (total samples = frames × channels); free it
/// with [`clausters_free_samples`]. On failure returns NULL and writes a
/// human-readable message into (`err`, `err_cap`).
///
/// # Safety
/// `score`/`score_len` must describe a readable byte range; `out_frames`
/// must be writable; `err` either NULL or writable for `err_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_render(
    score: *const u8,
    score_len: usize,
    sample_rate: f64,
    channels: u32,
    workers: u32,
    out_frames: *mut u64,
    err: *mut u8,
    err_cap: usize,
) -> *mut f32 {
    let bytes = if score.is_null() {
        &[][..]
    } else {
        // SAFETY: caller contract.
        unsafe { std::slice::from_raw_parts(score, score_len) }
    };
    let result = Score::from_bytes(bytes).and_then(|score| {
        let cfg = RenderConfig {
            sample_rate,
            channels: channels as usize,
            workers: workers as usize,
        };
        render_to_vec(&score, &cfg)
    });
    match result {
        Ok((samples, stats)) => {
            // SAFETY: caller contract.
            unsafe { *out_frames = stats.frames };
            let mut samples = samples.into_boxed_slice();
            let ptr = samples.as_mut_ptr();
            std::mem::forget(samples);
            ptr
        }
        Err(e) => {
            write_error(&e, err, err_cap);
            std::ptr::null_mut()
        }
    }
}

/// Frees a buffer returned by [`clausters_render`]. `samples` is
/// frames × channels (the full length, not per channel).
///
/// # Safety
/// Must be called exactly once with the pointer and total sample count of
/// one successful `clausters_render`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_free_samples(ptr: *mut f32, samples: u64) {
    if !ptr.is_null() {
        // SAFETY: reconstructs the Box from clausters_render.
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, samples as usize)) });
    }
}

/// An in-process live server: audio device + engine + network loop, with
/// the host as the single ring client.
#[cfg(feature = "realtime")]
pub struct Clausters {
    peer: IpcPeer,
    /// The cpal stream lives here; dropping it stops audio.
    _backend: crate::server::backend::AudioBackend,
    server: Option<JoinHandle<std::io::Result<()>>>,
    segment: Arc<Segment>,
}

/// Opens the default audio device and starts a full server in-process.
/// Returns NULL on failure (the error goes to `err`). Close with
/// [`clausters_close`].
///
/// # Safety
/// `err` either NULL or writable for `err_cap` bytes.
#[cfg(feature = "realtime")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_open(
    workers: u32,
    err: *mut u8,
    err_cap: usize,
) -> *mut Clausters {
    use crate::osc::server::{OscServer, ServerInfo};

    let segment = Segment::in_memory();
    // Embedded hosts follow the device's default rate (None); they can resample
    // on their side if they need a specific rate.
    let opened = crate::server::backend::start(workers as usize, Some(Arc::clone(&segment)), None)
        .map_err(|e| e.to_string())
        .and_then(|(backend, handle)| {
            let info = ServerInfo {
                nominal_sample_rate: backend.sample_rate as f64,
                actual_sample_rate: backend.sample_rate as f64,
            };
            // The socket is an ephemeral localhost port: unused by the embed
            // client (commands go through the ring), it just drives the
            // loop's tick — and doubles as an escape hatch for debugging.
            let mut server =
                OscServer::bind(("127.0.0.1", 0), info, handle).map_err(|e| e.to_string())?;
            server
                .attach_ipc(IpcPeer::new(Arc::clone(&segment), Role::Server))
                .map_err(|e| e.to_string())?;
            Ok((backend, server))
        });
    match opened {
        Ok((backend, mut server)) => {
            let thread = std::thread::Builder::new()
                .name("clausters-embed-server".into())
                .spawn(move || server.run())
                .expect("failed to spawn the embedded server thread");
            Box::into_raw(Box::new(Clausters {
                peer: IpcPeer::new(Arc::clone(&segment), Role::Client),
                _backend: backend,
                server: Some(thread),
                segment,
            }))
        }
        Err(e) => {
            write_error(&e, err, err_cap);
            std::ptr::null_mut()
        }
    }
}

/// Delivers one complete OSC packet (message or bundle). Returns 0 on
/// success, -1 when the command ring is full (backpressure: retry).
///
/// # Safety
/// `handle` from [`clausters_open`]; `packet`/`len` a readable byte range.
#[cfg(feature = "realtime")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_send(
    handle: *mut Clausters,
    packet: *const u8,
    len: usize,
) -> i32 {
    let Some(h) = (unsafe { handle.as_ref() }) else {
        return -1;
    };
    let bytes = unsafe { std::slice::from_raw_parts(packet, len) };
    if h.peer.push(bytes) { 0 } else { -1 }
}

/// Pops one pending reply into (`buf`, `cap`). Returns the packet length,
/// 0 when none is pending, or -1 on error. Replies bigger than `cap` are
/// dropped (use 64 KiB to be safe).
///
/// # Safety
/// `handle` from [`clausters_open`]; `buf` writable for `cap` bytes.
#[cfg(feature = "realtime")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_poll(handle: *mut Clausters, buf: *mut u8, cap: usize) -> i64 {
    let Some(h) = (unsafe { handle.as_ref() }) else {
        return -1;
    };
    let slice = unsafe { std::slice::from_raw_parts_mut(buf, cap) };
    match h.peer.try_pop(slice) {
        Some(len) => len as i64,
        None => 0,
    }
}

/// The engine's sample counter (block-accurate, written by the audio
/// thread) — the M8 sample clock with zero transport jitter.
///
/// # Safety
/// `handle` from [`clausters_open`].
#[cfg(feature = "realtime")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_clock(handle: *mut Clausters) -> u64 {
    match unsafe { handle.as_ref() } {
        Some(h) => h.segment.clock().load(Ordering::Acquire),
        None => 0,
    }
}

/// The device sample rate.
///
/// # Safety
/// `handle` from [`clausters_open`].
#[cfg(feature = "realtime")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_sample_rate(handle: *mut Clausters) -> f64 {
    match unsafe { handle.as_ref() } {
        Some(h) => h.segment.sample_rate(),
        None => 0.0,
    }
}

/// Writes a control bus directly in the data plane: the engine's `InCtl`
/// reads this very atomic on the next block — no command, no round trip.
///
/// # Safety
/// `handle` from [`clausters_open`].
#[cfg(feature = "realtime")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_ctl_set(handle: *mut Clausters, index: u32, value: f32) {
    if let Some(h) = unsafe { handle.as_ref() } {
        h.segment.control_buses().set(index as usize, value);
    }
}

/// Reads a control bus from the data plane.
///
/// # Safety
/// `handle` from [`clausters_open`].
#[cfg(feature = "realtime")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_ctl_get(handle: *mut Clausters, index: u32) -> f32 {
    match unsafe { handle.as_ref() } {
        Some(h) => h.segment.control_buses().get(index as usize),
        None => 0.0,
    }
}

/// Shuts the embedded server down (sends `/quit` through the ring, joins
/// the network thread, stops the audio stream) and frees the handle.
///
/// # Safety
/// `handle` from [`clausters_open`], used at most once.
#[cfg(feature = "realtime")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_close(handle: *mut Clausters) {
    if handle.is_null() {
        return;
    }
    // SAFETY: ownership returns from clausters_open's Box::into_raw.
    let mut h = unsafe { Box::from_raw(handle) };
    // ",s" type tag, no args: a minimal /quit message.
    let quit = rosc::encoder::encode(&rosc::OscPacket::Message(rosc::OscMessage {
        addr: "/quit".into(),
        args: vec![],
    }))
    .expect("static /quit message encodes");
    let _ = h.peer.push(&quit);
    if let Some(thread) = h.server.take() {
        let _ = thread.join();
    }
}
