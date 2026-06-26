//! Reading the audio server's shared-memory segment: the zero-message path for
//! meters and scopes.
//!
//! The audio server, started with `--shm <path>`, maps a memory-backed file
//! whose control-bus region is a flat array of atomics the engine reads and
//! writes directly. A GUI `meter`/`scope` widget reads those very atomics **each
//! frame**, so a live bus animates with no OSC traffic at all (the third leg of
//! the topology made cheap: the host is a client that reads the server's memory).
//!
//! This is a **read-only** view of a **versioned binary ABI**. Rather than depend
//! on the server crate (which would pull the engine, cpal and the rest into this
//! independent GUI crate), the reader mirrors the segment's `#[repr(C)]` layout
//! — the same role any independent peer plays against this boundary (the Python
//! `ctypes` client, a future JS one). The contract is the canonical definition in
//! the server's `server::ipc`; the safety net against drift is the version field:
//! [`SharedSegment::open`] rejects a segment whose magic or ABI version does not
//! match, so a layout change fails loudly here instead of reading stale memory.
//! Unix-only, as the server's segment is.

#![cfg(unix)]

use std::fs::OpenOptions;
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// "CLAU" little-endian (mirrors `server::ipc::MAGIC`).
const MAGIC: u32 = 0x5541_4C43;
/// The segment ABI version this reader understands (mirrors
/// `server::ipc::ABI_VERSION`). Bumped in lockstep with the server; a mismatch
/// is rejected on [`SharedSegment::open`].
const SUPPORTED_ABI_VERSION: u32 = 2;

// Byte offsets of the fields we read inside the `#[repr(C)]` Header.
const OFF_ABI: usize = 4;
const OFF_SAMPLE_RATE: usize = 8;
const OFF_SAMPLE_CLOCK: usize = 16;
const OFF_RING_CAPACITY: usize = 24;
const OFF_CONTROL_BUSES: usize = 28;
/// Size of the fixed Header struct.
const HEADER_SIZE: usize = 64;
/// Fixed prefix of each command ring before its `data` array (head/tail/pad).
const RING_PREFIX: usize = 64;

/// A read-only mapping of the audio server's shared-memory segment. Reading a
/// control bus is a single atomic load; the mapping is dropped (unmapped) when
/// this is.
pub struct SharedSegment {
    ptr: *mut u8,
    len: usize,
    control_count: usize,
    controls_offset: usize,
}

// SAFETY: the segment is only ever read here, through atomic loads of fields the
// server writes atomically; the mapping stays valid until `Drop`.
unsafe impl Send for SharedSegment {}
unsafe impl Sync for SharedSegment {}

impl Drop for SharedSegment {
    fn drop(&mut self) {
        // SAFETY: the exact mapping created in `open`.
        unsafe { libc::munmap(self.ptr as *mut libc::c_void, self.len) };
    }
}

impl SharedSegment {
    /// Maps the segment at `path` read-only and validates its header. Fails if
    /// the file is too small, the magic is wrong, the ABI version differs, or the
    /// file size does not match the layout the header describes.
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).open(path)?;
        let len = file.metadata()?.len() as usize;
        if len < HEADER_SIZE {
            return Err(io::Error::other("segment too small for a header"));
        }
        // SAFETY: a shared read-only mapping of a file we just sized.
        let raw = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if raw == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let ptr = raw as *mut u8;

        // Validate from the raw mapping before building the owning struct, so its
        // `Drop` (munmap) runs exactly once — for the segment we actually return.
        let fail = |ptr: *mut u8, msg: String| -> io::Result<Self> {
            // SAFETY: the mapping we just made; nothing else aliases it yet.
            unsafe { libc::munmap(ptr as *mut libc::c_void, len) };
            Err(io::Error::other(msg))
        };
        if header_u32(ptr, 0) != MAGIC {
            return fail(ptr, "not a clausters segment (bad magic)".into());
        }
        let abi = header_u32(ptr, OFF_ABI);
        if abi != SUPPORTED_ABI_VERSION {
            return fail(
                ptr,
                format!("segment ABI version {abi} != supported {SUPPORTED_ABI_VERSION}"),
            );
        }
        let ring_capacity = header_u32(ptr, OFF_RING_CAPACITY) as usize;
        let control_count = header_u32(ptr, OFF_CONTROL_BUSES) as usize;
        // The control region follows the header and the two command rings.
        let controls_offset = HEADER_SIZE + 2 * (RING_PREFIX + ring_capacity);
        let expected = controls_offset + control_count * size_of::<u32>();
        if len != expected {
            return fail(ptr, "segment size does not match its header".into());
        }
        Ok(SharedSegment {
            ptr,
            len,
            control_count,
            controls_offset,
        })
    }

    /// Number of control buses this segment carries.
    pub fn control_buses(&self) -> usize {
        self.control_count
    }

    /// The current value of control bus `index` (`0.0` for an out-of-range bus).
    /// A single atomic load of the same word the engine reads and writes.
    pub fn control(&self, index: usize) -> f32 {
        if index >= self.control_count {
            return 0.0;
        }
        let off = self.controls_offset + index * size_of::<u32>();
        // SAFETY: in-range offset into the control region; the word is an
        // `AtomicU32` the server keeps live.
        let bits = unsafe { (*(self.ptr.add(off) as *const AtomicU32)).load(Ordering::Relaxed) };
        f32::from_bits(bits)
    }

    /// The engine's block-accurate sample clock (samples processed since boot).
    pub fn sample_clock(&self) -> u64 {
        header_u64(self.ptr, OFF_SAMPLE_CLOCK)
    }

    /// The device sample rate the server published, or `0.0` before it is known.
    pub fn sample_rate(&self) -> f64 {
        f64::from_bits(header_u64(self.ptr, OFF_SAMPLE_RATE))
    }
}

impl super::BusSource for SharedSegment {
    fn control(&self, index: usize) -> f32 {
        SharedSegment::control(self, index)
    }
}

/// A constant header `u32` field (written once at creation, before any client
/// maps), read at byte offset `off`.
fn header_u32(ptr: *mut u8, off: usize) -> u32 {
    // SAFETY: `off + 4 <= HEADER_SIZE <= len`; the mmap base is page-aligned and
    // `off` is a multiple of 4, so the read is aligned and in range.
    unsafe { (ptr.add(off) as *const u32).read() }
}

/// A live header `u64` field (the server stores it atomically), read at `off`.
fn header_u64(ptr: *mut u8, off: usize) -> u64 {
    // SAFETY: aligned, in-range; the field is an `AtomicU64` in the layout.
    unsafe { (*(ptr.add(off) as *const AtomicU64)).load(Ordering::Relaxed) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Writes a segment file matching the documented `#[repr(C)]` layout, with a
    /// small ring capacity (the reader derives the control offset from the header
    /// field, so the real 64 KiB rings need not be present). Returns the path.
    fn fake_segment(name: &str, abi: u32, magic: u32, controls: &[f32]) -> std::path::PathBuf {
        let ring_capacity: u32 = 16;
        let controls_offset = HEADER_SIZE + 2 * (RING_PREFIX + ring_capacity as usize);
        let len = controls_offset + controls.len() * 4;
        let mut bytes = vec![0u8; len];
        bytes[0..4].copy_from_slice(&magic.to_le_bytes());
        bytes[OFF_ABI..OFF_ABI + 4].copy_from_slice(&abi.to_le_bytes());
        bytes[OFF_SAMPLE_RATE..OFF_SAMPLE_RATE + 8]
            .copy_from_slice(&48_000.0f64.to_bits().to_le_bytes());
        bytes[OFF_SAMPLE_CLOCK..OFF_SAMPLE_CLOCK + 8].copy_from_slice(&12_345u64.to_le_bytes());
        bytes[OFF_RING_CAPACITY..OFF_RING_CAPACITY + 4]
            .copy_from_slice(&ring_capacity.to_le_bytes());
        bytes[OFF_CONTROL_BUSES..OFF_CONTROL_BUSES + 4]
            .copy_from_slice(&(controls.len() as u32).to_le_bytes());
        for (i, v) in controls.iter().enumerate() {
            let at = controls_offset + i * 4;
            bytes[at..at + 4].copy_from_slice(&v.to_bits().to_le_bytes());
        }
        let path = std::env::temp_dir().join(format!(
            "clausters_gui_shm_{name}_{}.seg",
            std::process::id()
        ));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(&bytes).unwrap();
        path
    }

    #[test]
    fn reads_control_buses_and_header() {
        let path = fake_segment("ok", SUPPORTED_ABI_VERSION, MAGIC, &[0.0, 0.5, -0.25, 1.0]);
        let seg = SharedSegment::open(&path).unwrap();
        assert_eq!(seg.control_buses(), 4);
        assert_eq!(seg.control(0), 0.0);
        assert_eq!(seg.control(1), 0.5);
        assert_eq!(seg.control(2), -0.25);
        assert_eq!(seg.control(3), 1.0);
        assert_eq!(seg.control(99), 0.0, "out-of-range bus reads as 0");
        assert_eq!(seg.sample_rate(), 48_000.0);
        assert_eq!(seg.sample_clock(), 12_345);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_bad_magic_and_abi() {
        let bad_magic = fake_segment("magic", SUPPORTED_ABI_VERSION, 0xDEAD_BEEF, &[0.0]);
        assert!(SharedSegment::open(&bad_magic).is_err());
        let _ = std::fs::remove_file(&bad_magic);

        let bad_abi = fake_segment("abi", SUPPORTED_ABI_VERSION + 1, MAGIC, &[0.0]);
        assert!(SharedSegment::open(&bad_abi).is_err());
        let _ = std::fs::remove_file(&bad_abi);
    }
}
