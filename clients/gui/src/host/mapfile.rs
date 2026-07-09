//! A read-only memory map of a local file: the bulk-data path that bypasses OSC.
//!
//! G7's decision — large payloads move between processes through **local shared
//! resources**, not the network — needs the host to read a multi-megabyte
//! buffer (or its prebuilt peak cache) that a client wrote, or the audio server
//! exported, without it crossing a UDP datagram or being re-sent per frame. A
//! `waveform` names a file (`path` = raw little-endian `f32` samples, `cache` =
//! a `peaks` pyramid built by [`clausters_core::peaks`]); the host maps it
//! read-only here and reads it once, zero-copy. This is the same `libc::mmap`
//! the shared-segment reader uses ([`super::shm`]), over an arbitrary file
//! rather than the server's segment.
//!
//! Unix-only, matching the rest of the host's memory-mapping; a wasm/browser
//! build cannot map files and uses binary WS frames instead (a later milestone).

#![cfg(unix)]

use std::fs::OpenOptions;
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;

/// A read-only mapping of a file. The bytes stay valid until this is dropped
/// (which unmaps). An empty file is rejected (nothing to map).
pub struct MappedFile {
    ptr: *mut u8,
    len: usize,
}

// SAFETY: the mapping is read-only and stays valid until `Drop`.
unsafe impl Send for MappedFile {}
unsafe impl Sync for MappedFile {}

impl Drop for MappedFile {
    fn drop(&mut self) {
        // SAFETY: the exact mapping created in `open`.
        unsafe { libc::munmap(self.ptr as *mut libc::c_void, self.len) };
    }
}

impl MappedFile {
    /// Maps `path` read-only. Errors if the file is empty or cannot be opened.
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).open(path)?;
        let len = file.metadata()?.len() as usize;
        if len == 0 {
            return Err(io::Error::other("cannot map an empty file"));
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
        Ok(Self {
            ptr: raw as *mut u8,
            len,
        })
    }

    /// The mapped bytes.
    pub fn bytes(&self) -> &[u8] {
        // SAFETY: `ptr`/`len` describe the live mapping until `Drop`.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }

    /// The mapped bytes as little-endian `f32`s, taking channel 0 of `channels`
    /// interleaved channels (no-op de-interleave when `channels <= 1`). A
    /// trailing partial frame is ignored. Copies into an owned buffer the
    /// waveform renderer can hold and read at fine zoom; the map can then be
    /// released. (A zero-copy hold of the map itself is a later refinement.)
    pub fn channel0_f32(&self, channels: usize) -> Vec<f32> {
        let channels = channels.max(1);
        let frames = (self.len / 4) / channels;
        let b = self.bytes();
        (0..frames)
            .map(|f| {
                let i = f * channels * 4;
                f32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
            })
            .collect()
    }

    /// The mapped bytes as little-endian `f32`s, de-interleaved into **all**
    /// `channels` channels (a trailing partial frame is ignored) — the
    /// multichannel read the editor-grade views build their lanes from.
    pub fn channels_f32(&self, channels: usize) -> Vec<Vec<f32>> {
        let channels = channels.max(1);
        let frames = (self.len / 4) / channels;
        let b = self.bytes();
        let at = |f: usize, ch: usize| {
            let i = (f * channels + ch) * 4;
            f32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
        };
        (0..channels)
            .map(|ch| (0..frames).map(|f| at(f, ch)).collect())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("clausters_mapfile_{name}_{}", std::process::id()));
        std::fs::File::create(&path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
        path
    }

    #[test]
    fn maps_raw_f32_and_deinterleaves_channel0() {
        // Interleaved stereo: L,R,L,R...
        let frames = [(0.0f32, 1.0f32), (0.25, -1.0), (0.5, 0.0)];
        let mut bytes = Vec::new();
        for (l, r) in frames {
            bytes.extend_from_slice(&l.to_le_bytes());
            bytes.extend_from_slice(&r.to_le_bytes());
        }
        let path = write_temp("stereo", &bytes);
        let map = MappedFile::open(&path).unwrap();
        assert_eq!(map.channel0_f32(2), vec![0.0, 0.25, 0.5], "channel 0 only");
        assert_eq!(
            map.channel0_f32(1).len(),
            6,
            "as mono, all samples are kept"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn empty_file_is_rejected() {
        let path = write_temp("empty", &[]);
        assert!(MappedFile::open(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
