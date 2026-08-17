//! The material a peer edits **in place**: a take's samples, mapped.
//!
//! The segment ([`super::shm`]) carries the small, fixed data plane and a
//! **directory** saying what each pool buffer is. It does not carry the
//! samples: a buffer is sized at run time and a ten-minute stereo take is
//! 230 MB, while the segment is sized once at boot. So a buffer's material is
//! its own file beside the segment, named from the segment's path, the buffer
//! number and the generation — and this module opens it.
//!
//! **What this changes for the host is the round trip, not the copy.** Drawing
//! a take used to be `/buffer_query` plus a chunked `/buffer_getRange`
//! conversation, and a stroke used to be a blob out, a job on the server, a
//! reply, and the host reconciling its own picture with what it had just sent.
//! With the take mapped, opening it is a local read and a stroke is a **store**:
//! the cells the engine reads on its next block are the cells the hand moved,
//! whether that engine is in this process or in the RT server attached to the
//! same segment.
//!
//! **It carries material, not computation.** A peer writes samples it already
//! holds — a drawn stroke, a pasted block, a take it loaded. Every *operation*
//! over samples (a gain, a fade, a reverse, a render) is still asked for over
//! the wire and performed by the server, which is the rule the whole system
//! rests on: one place performs audio processing. Mapped memory makes the
//! other thing easy, not correct.
//!
//! Unix-only, like the segment. A page has no equivalent and is not meant to:
//! a browser cannot map a file, so the web client keeps `/buffer_getRange` and
//! `/buffer_setRangeChannel`, which is the same split every bulk path already
//! has.

#![cfg(unix)]

use std::fs::OpenOptions;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use super::shm::SharedSegment;

/// The material of one segment: its directory, and the path its regions are
/// named from.
///
/// Held beside the segment rather than inside it because the two answer
/// different questions — the segment is *shape and time*, this is *samples* —
/// and because a host may read a segment (meters, scopes) without ever mapping
/// a take.
pub struct SharedMaterial {
    segment: Arc<SharedSegment>,
    path: PathBuf,
}

impl SharedMaterial {
    /// Reads `segment`'s material out of the regions beside `path`, which is
    /// the segment's own file — the `--shm` path the server was given.
    pub fn new(segment: Arc<SharedSegment>, path: PathBuf) -> Self {
        Self { segment, path }
    }

    /// The segment path the regions are named from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Maps pool buffer `bufnum`, or `None` when the directory has nothing
    /// live under that number or the region cannot be opened.
    ///
    /// The generation is re-read after the mapping succeeds: a row that moved
    /// meanwhile describes a buffer this mapping is not, and mapping a take
    /// that was freed under us is exactly the case the generation exists for.
    pub fn map(&self, bufnum: usize) -> Option<MappedTake> {
        let (generation, frames, channels, sample_rate) = self.segment.buffer_info(bufnum)?;
        let cells = frames.checked_mul(channels)?;
        let path = region_path(&self.path, bufnum, generation);
        let take = MappedTake::open(&path, cells, channels, frames, sample_rate).ok()?;
        if self.segment.buffer_info(bufnum)?.0 != generation {
            return None;
        }
        Some(take)
    }

    /// Whether the directory holds a live buffer under `bufnum` — the cheap
    /// question, asked before deciding whether a take needs fetching at all.
    pub fn holds(&self, bufnum: usize) -> bool {
        self.segment.buffer_info(bufnum).is_some()
    }
}

/// The name a buffer's region has (mirrors `dsp::region::Region::path_for`):
/// the segment's path, the buffer number and the generation, so a freed
/// buffer's file and its replacement can never share a name.
fn region_path(segment: &Path, bufnum: usize, generation: u64) -> PathBuf {
    let mut name = segment.as_os_str().to_os_string();
    name.push(format!(".buf{bufnum}.{generation}"));
    PathBuf::from(name)
}

/// One take's samples, mapped read/write: **the server's own memory**.
///
/// Interleaved, the layout every buffer has, and every cell an `AtomicU32`
/// holding `f32` bits — the same words the engine reads, so a store here is
/// audible on the next block with nothing sent. Concurrency is what the buffer
/// model has always promised and no more: per-sample atomicity, no ordering
/// between samples, a reader crossing a writer seeing some old and some new.
pub struct MappedTake {
    ptr: *mut AtomicU32,
    cells: usize,
    channels: usize,
    frames: usize,
    sample_rate: f64,
}

// SAFETY: every access goes through atomics on a shared mapping this value
// owns; the pointer is never handed out.
unsafe impl Send for MappedTake {}
unsafe impl Sync for MappedTake {}

impl Drop for MappedTake {
    fn drop(&mut self) {
        // SAFETY: the exact mapping made in `open`. Unmapping releases *this*
        // view; the file itself outlives it, and an unlinked one dies with the
        // last mapping — which is what makes freeing a take safe while
        // somebody is drawing it.
        unsafe { libc::munmap(self.ptr as *mut libc::c_void, (self.cells * 4).max(1)) };
    }
}

impl MappedTake {
    fn open(
        path: &Path,
        cells: usize,
        channels: usize,
        frames: usize,
        sample_rate: f64,
    ) -> std::io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        if (file.metadata()?.len() as usize) < cells * 4 {
            return Err(std::io::Error::other(
                "the region is shorter than the directory says",
            ));
        }
        // SAFETY: a shared read/write mapping of a file whose length we just
        // checked against the shape the directory reports.
        let raw = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                (cells * 4).max(1),
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if raw == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self {
            ptr: raw as *mut AtomicU32,
            cells,
            channels,
            frames,
            sample_rate,
        })
    }

    /// Channels, frames and sample rate — the take's shape, as the directory
    /// reported it when this was mapped.
    pub fn shape(&self) -> (usize, usize, f64) {
        (self.channels, self.frames, self.sample_rate)
    }

    /// Every sample, interleaved — one read of the whole take, which is what
    /// building a picture of it costs.
    pub fn read_all(&self) -> Vec<f32> {
        (0..self.cells).map(|i| self.at(i)).collect()
    }

    /// Writes `values` into `channel` starting at frame `start`. Out-of-range
    /// frames are dropped rather than wrapped: a stroke is clamped by whoever
    /// drew it, and silently writing into the next channel would be worse than
    /// writing nothing.
    pub fn write_channel(&self, channel: usize, start: u64, values: &[f32]) {
        if channel >= self.channels {
            return;
        }
        for (i, v) in values.iter().enumerate() {
            let frame = start as usize + i;
            if frame >= self.frames {
                break;
            }
            self.set_at(frame * self.channels + channel, *v);
        }
    }

    fn at(&self, cell: usize) -> f32 {
        if cell >= self.cells {
            return 0.0;
        }
        // SAFETY: in-range cell of the mapping this value owns.
        f32::from_bits(unsafe { (*self.ptr.add(cell)).load(Ordering::Relaxed) })
    }

    fn set_at(&self, cell: usize, value: f32) {
        if cell >= self.cells {
            return;
        }
        // SAFETY: in-range cell of the mapping this value owns.
        unsafe { (*self.ptr.add(cell)).store(value.to_bits(), Ordering::Relaxed) };
    }
}
