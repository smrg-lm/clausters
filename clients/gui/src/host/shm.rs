//! Reading the audio server's shared-memory segment: the zero-message path for
//! meters, scopes and the playhead.
//!
//! The audio server, started with `--shm <path>`, maps a memory-backed file
//! whose control-bus region is a flat array of atomics the engine reads and
//! writes directly. A GUI `meter`/`scope` widget reads those very atomics
//! **each frame**, so a live bus animates with no OSC traffic at all — the
//! third leg of the topology made cheap: the host is a client that reads the
//! server's memory.
//!
//! **The layout is not mirrored here any more.** It is
//! [`clausters_core::shm`], the one definition every process links, and this
//! module is what is genuinely the *host's*: getting the memory (a mapped file,
//! or a borrow of an in-process server's own segment), and choosing which
//! counter a window's playhead draws. That split is not tidiness — a mirror of
//! the layout written by hand here refused every valid v9 segment for a week,
//! because it agreed with the server on the version number and not on the size
//! check, which is exactly the failure a shared definition cannot have.
//!
//! Unix-only, as the server's segment is.

#![cfg(unix)]

use std::any::Any;
use std::fs::OpenOptions;
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use clausters_core::shm::View;

/// Which of the segment's counters a window's **playhead** reads.
///
/// The segment publishes several and a widget draws one number, so the choice
/// is made once, where the source is built, rather than as a prop on every
/// widget that could carry a head.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum HeadClock {
    /// The device clock: samples processed since boot, never stopping. What a
    /// host attached to a live server wants — its meters, scopes and taps are
    /// all on that axis.
    #[default]
    Device,
    /// The transport's **position in the piece**: it holds while stopped,
    /// jumps on a locate and wraps in a loop. What an editor wants, because it
    /// is the time of the material rather than of the machine.
    Piece,
}

/// Who owns the memory this reads.
enum Backing {
    /// A mapping this made and must unmap.
    Mapped { ptr: *mut u8, len: usize },
    /// Somebody else's memory, kept alive by the handle held here — an
    /// in-process server's own segment, which is not ours to unmap and must
    /// not outlive its owner. Type-erased because the owner is the server
    /// crate's, which only a `standalone` build links.
    Borrowed(#[allow(dead_code)] Arc<dyn Any + Send + Sync>),
}

/// A view of the audio server's shared-memory segment. Reading a control bus is
/// a single atomic load; reading an audio-tap window is a lock-free copy with a
/// cursor double-check. A view it mapped itself is unmapped when this is
/// dropped; a borrowed one only releases its hold on the owner.
pub struct SharedSegment {
    view: View,
    backing: Backing,
    head: HeadClock,
}

// SAFETY: every access goes through the shared reader's atomics, and the
// mapping stays valid until `Drop`.
unsafe impl Send for SharedSegment {}
unsafe impl Sync for SharedSegment {}

impl Drop for SharedSegment {
    fn drop(&mut self) {
        if let Backing::Mapped { ptr, len } = self.backing {
            // SAFETY: the exact mapping created in `open`.
            unsafe { libc::munmap(ptr as *mut libc::c_void, len) };
        }
    }
}

impl SharedSegment {
    /// Maps the segment at `path` and validates its header. Fails if the file
    /// is too small, the magic is wrong, the ABI version differs, or the file
    /// size does not match the layout the header describes. Reads the
    /// **device** clock, which is what a host attached to a running server
    /// wants; see [`Self::with_head`].
    ///
    /// Mapped read/write, though the host only reads the segment itself: the
    /// shared reader is one type with one set of accessors, and a read-only
    /// mapping would turn a later zero-message write into a fault rather than
    /// a compile error. The file is the server's, and opening it read/write is
    /// the same permission a peer needs to edit the material beside it.
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let len = file.metadata()?.len() as usize;
        if len == 0 {
            return Err(io::Error::other("segment is empty"));
        }
        // SAFETY: a shared mapping of a file we just sized.
        let raw = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if raw == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let ptr = raw as *mut u8;
        // Validated from the raw mapping before the owning struct exists, so
        // its `Drop` (munmap) runs exactly once — for the segment we return.
        // SAFETY: the mapping we just made, of `len` bytes.
        match unsafe { View::attach(ptr, len) } {
            Ok(view) => Ok(SharedSegment {
                view,
                backing: Backing::Mapped { ptr, len },
                head: HeadClock::default(),
            }),
            Err(why) => {
                // SAFETY: the mapping we just made; nothing else aliases it.
                unsafe { libc::munmap(ptr as *mut libc::c_void, len) };
                Err(io::Error::other(why))
            }
        }
    }

    /// A view over a segment **somebody else owns** — an in-process server's
    /// own, where the host and the engine are one process and there is no file
    /// to map.
    ///
    /// `owner` is the handle that keeps the memory alive; it is held for as
    /// long as this view is, which is what makes the borrow safe rather than a
    /// promise. It is type-erased because the owner belongs to the server
    /// crate, which only a `standalone` build links and this module must not
    /// name.
    ///
    /// # Safety
    ///
    /// `ptr` and `len` must describe the segment `owner` keeps alive, and that
    /// memory must stay valid and unmoved for as long as `owner` is held.
    pub unsafe fn borrowed(
        ptr: *const u8,
        len: usize,
        owner: Arc<dyn Any + Send + Sync>,
    ) -> io::Result<Self> {
        // SAFETY: the caller's contract is exactly what `attach` asks for.
        let view = unsafe { View::attach(ptr as *mut u8, len) }.map_err(io::Error::other)?;
        Ok(SharedSegment {
            view,
            backing: Backing::Borrowed(owner),
            head: HeadClock::default(),
        })
    }

    /// Reads the piece's position rather than the device clock — the choice an
    /// editor makes, and the only place it is made (see [`HeadClock`]).
    pub fn with_head(mut self, head: HeadClock) -> Self {
        self.head = head;
        self
    }

    /// Number of control buses this segment carries.
    pub fn control_buses(&self) -> usize {
        self.view.control_bus_count()
    }

    /// The current value of control bus `index` (`0.0` for an out-of-range
    /// bus): one atomic load of the same word the engine reads and writes.
    pub fn control(&self, index: usize) -> f32 {
        self.view.control(index)
    }

    /// Audio bus `bus`'s level: the peak magnitude of the engine's last block.
    /// One atomic load — what a meter reads, and why a meter costs no tap ring.
    pub fn level(&self, bus: usize) -> f32 {
        self.view.level(bus)
    }

    /// Which tap ring is recording audio bus `bus`, or `None`. The server owns
    /// the assignment and publishes it here, so a caller asks by bus.
    pub fn tap_of_bus(&self, bus: usize) -> Option<usize> {
        self.view.tap_of_bus(bus)
    }

    /// The engine's block-accurate sample clock (samples processed since boot).
    pub fn sample_clock(&self) -> u64 {
        self.view.clock().load(Ordering::Relaxed)
    }

    /// Samples elapsed **under the transport**, held while it is stopped.
    ///
    /// [`Self::sample_clock`] never stops, so anything pacing on the device
    /// reads that one. This one is monotonic too — it holds, it never jumps —
    /// which is what a scheduler needs and what a **playhead does not**: for
    /// where the piece *is*, read [`Self::transport_position`].
    pub fn transport_clock(&self) -> u64 {
        self.view.transport_clock().load(Ordering::Relaxed)
    }

    /// Where the transport is **in the piece**, in samples of the material —
    /// what a playhead draws.
    ///
    /// Not a clock: it advances with the transport clock while rolling, holds
    /// while stopped, jumps to wherever `/transport_locate` puts it and wraps
    /// at the end of a loop. Reading the clock instead gives a head that
    /// ignores every seek and every loop, which is a picture of elapsed time
    /// rather than of the piece.
    pub fn transport_position(&self) -> u64 {
        self.view.transport_position().load(Ordering::Relaxed)
    }

    /// The device sample rate the server published, or `0.0` before it is
    /// known.
    pub fn sample_rate(&self) -> f64 {
        self.view.sample_rate()
    }

    /// Number of audio-tap rings in the segment.
    pub fn taps(&self) -> usize {
        self.view.taps()
    }

    /// Per-tap ring capacity in samples (a power of two).
    pub fn tap_frames(&self) -> usize {
        self.view.tap_frames()
    }

    /// **What the directory says about pool buffer `bufnum`**: its generation,
    /// its shape and its sample rate, or `None` when that slot holds nothing.
    ///
    /// The generation does three jobs with one number: it is *odd while the
    /// buffer is live*, it *names the region file* beside the segment (which is
    /// where the samples actually are — see [`crate::host::material`]), and it
    /// is a *seqlock* the shared reader retries under.
    pub fn buffer_info(&self, bufnum: usize) -> Option<clausters_core::shm::BufferShape> {
        self.view.buffer_info(bufnum)
    }

    /// **How far pool buffer `bufnum` has been written**, in frames: the
    /// frontier its writing UGens publish once per block (the server's S20).
    ///
    /// Zero for material nothing recorded into, which is every take that
    /// arrived whole. It is a hint and not a promise: several writers may
    /// share a buffer, and what it answers is only how far the material now
    /// goes.
    pub fn buffer_frontier(&self, bufnum: usize) -> Option<u64> {
        self.view.buffer_frontier(bufnum)
    }

    /// How many buffers the directory can describe — the pool's size, as the
    /// segment's own length reports it.
    pub fn buffer_rows(&self) -> usize {
        self.view.buffer_rows()
    }

    /// Copies the **newest** `out.len()` samples of tap `i` into `out`,
    /// returning the stream position at the window's end.
    pub fn tap_read_latest(&self, i: usize, out: &mut [f32]) -> Option<u64> {
        self.view.tap_read_latest(i, out)
    }
}

impl super::BusSource for SharedSegment {
    fn control(&self, index: usize) -> f32 {
        SharedSegment::control(self, index)
    }

    fn read_bus(&self, bus: i32, out: &mut [f32]) -> bool {
        self.read_bus_at(bus, out).is_some()
    }

    fn read_bus_at(&self, bus: i32, out: &mut [f32]) -> Option<u64> {
        // The bus is the key: the server publishes which ring records it.
        if bus < 0 {
            return None;
        }
        let tap = self.tap_of_bus(bus as usize)?;
        self.tap_read_latest(tap, out)
    }

    fn window_limit(&self) -> usize {
        // The reader's own cap: a window past half the ring cannot be copied
        // without racing the writer round it.
        self.tap_frames() / 2
    }

    fn level(&self, bus: i32) -> f32 {
        if bus < 0 {
            return 0.0;
        }
        SharedSegment::level(self, bus as usize)
    }

    fn sample_rate(&self) -> f64 {
        SharedSegment::sample_rate(self)
    }

    fn sample_clock(&self) -> f64 {
        // The one place the choice of counter is resolved: above here a
        // playhead reads "the clock" and never asks which.
        match self.head {
            HeadClock::Device => SharedSegment::sample_clock(self) as f64,
            HeadClock::Piece => SharedSegment::transport_position(self) as f64,
        }
    }
}
