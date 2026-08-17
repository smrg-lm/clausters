//! Sample memory a second process can map: one **region per buffer**.
//!
//! The IPC segment carries the control plane and the small, fixed data plane
//! (the clocks, the buses, the taps), and it is sized once at boot — while a
//! buffer is sized at run time and can be enormous, a ten-minute stereo take
//! being 230 MB. So a shared buffer is not *in* the segment: it is its own
//! mapped file, and the segment carries only the **directory** that says where
//! to find it (`server::ipc::BufferDir`).
//!
//! **Why a file rather than anonymous shared memory.** Two properties this
//! design leans on, and both come from the filesystem rather than from us. A
//! peer can open it by **name**, which is what lets the directory be four
//! numbers and a generation instead of a handle-passing protocol. And an
//! unlinked file **stays alive until its last mapping goes**, which is the
//! whole answer to "what happens to a peer holding a buffer that was freed":
//! its memory stays valid, it learns the buffer is gone by reading the
//! directory, and the next allocation takes a new generation and therefore a
//! new name — so a stale mapping can never be aliased onto new material.
//!
//! The cells are [`AtomicU32`] exactly as an owned buffer's are, and for the
//! same reason (`dsp::buffer`): two threads — now two *processes* — touching
//! one location with a writer among them is a data race in any other shape.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU32;

/// One buffer's samples, mapped shared.
///
/// Dropping it unmaps; it does **not** unlink, because who owns the name is the
/// pool's business and a peer holding one must not be able to delete it.
#[derive(Debug)]
pub struct Region {
    ptr: *mut AtomicU32,
    cells: usize,
    path: PathBuf,
}

// SAFETY: the mapping is shared memory whose cells are atomic; the pointer is
// owned by this value and only ever handed out as `&[AtomicU32]`.
unsafe impl Send for Region {}
unsafe impl Sync for Region {}

impl Region {
    /// The name a buffer's region has: the segment's own path, the buffer
    /// number and the **generation** — so a freed buffer's file and its
    /// replacement never share a name.
    pub fn path_for(segment: &Path, bufnum: usize, generation: u64) -> PathBuf {
        let mut name = segment.as_os_str().to_os_string();
        name.push(format!(".buf{bufnum}.{generation}"));
        PathBuf::from(name)
    }

    /// Creates (or truncates) the region for `cells` samples and maps it.
    #[cfg(unix)]
    pub fn create(path: &Path, cells: usize) -> io::Result<Self> {
        use std::fs::OpenOptions;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.set_len((cells * 4) as u64)?;
        Self::map(&file, path, cells)
    }

    /// Maps an existing region — the peer's door, by the name the directory
    /// gave. `cells` is what the directory says the shape is; a file shorter
    /// than that is refused rather than read short.
    #[cfg(unix)]
    pub fn open(path: &Path, cells: usize) -> io::Result<Self> {
        use std::fs::OpenOptions;
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        if (file.metadata()?.len() as usize) < cells * 4 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "buffer region is shorter than the directory says",
            ));
        }
        Self::map(&file, path, cells)
    }

    #[cfg(unix)]
    fn map(file: &std::fs::File, path: &Path, cells: usize) -> io::Result<Self> {
        use std::os::fd::AsRawFd;
        let len = cells * 4;
        // SAFETY: a shared mapping of a file we have just sized (or checked).
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len.max(1),
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            ptr: ptr as *mut AtomicU32,
            cells,
            path: path.to_path_buf(),
        })
    }

    /// Removes the name, leaving every existing mapping valid until it is
    /// dropped — which is what makes freeing a buffer safe while a peer is
    /// still drawing it.
    pub fn unlink(path: &Path) {
        let _ = std::fs::remove_file(path);
    }

    /// The path this region was mapped from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The samples, as the same atomic cells an owned buffer holds.
    #[inline]
    pub fn cells(&self) -> &[AtomicU32] {
        // SAFETY: `ptr` maps `cells` initialized `u32`s (a fresh file reads as
        // zeros) and lives as long as `self`.
        unsafe { std::slice::from_raw_parts(self.ptr, self.cells) }
    }
}

impl Drop for Region {
    fn drop(&mut self) {
        // SAFETY: our own mapping, unmapped once.
        unsafe {
            libc::munmap(self.ptr as *mut libc::c_void, (self.cells * 4).max(1));
        }
    }
}
