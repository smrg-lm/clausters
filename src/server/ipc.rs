//! the shared-memory IPC segment — transport and data plane.
//!
//! OSC stays the only **encoding**; this module adds two **transports**
//! beside UDP, both built on one memory segment:
//!
//! - **Two processes, one machine** (`clausters --shm <path>`): the segment
//!   is a memory-mapped file. A local client maps the same file: no socket,
//!   no packet loss (the ring gives backpressure instead), and the data
//!   plane costs a memory read instead of an OSC round trip.
//! - **In-process / embedded** (`src/embed.rs`, feature `embed`): the same
//!   layout over plain heap memory — the "client" is the host application
//!   calling into the cdylib.
//!
//! **The layout itself is not here.** It is
//! [`clausters_core::shm`], because four processes read
//! it and a layout mirrored by hand in each of them is a layout that drifts —
//! which it did, twice, in ways a version number cannot catch. What this module
//! owns is what is genuinely the *server's*: getting the memory (a mapped file
//! or a heap allocation), the pool buffers behind the directory's rows, and the
//! ring endpoint the run loop drains. Everything else is one call into the
//! shared reader.
//!
//! What the segment carries, and the rules each part follows, is in
//! `docs/ipc.md` and in the core module's own docs; the short version is: a
//! versioned header, the control buses **as the very words the engine reads**,
//! the audio taps, the per-bus levels, the buffer directory, and two SPSC byte
//! rings of ordinary OSC packets. Ring bytes are as untrusted as UDP bytes
//! (`osc::decode_packet` validates), and the server polls the ring on a short
//! socket timeout instead of a semaphore — a documented trade-off in
//! `docs/ipc.md`.

#[cfg(unix)]
use std::fs::OpenOptions;
#[cfg(unix)]
use std::io;
// `Path` is not gated: `remove_segment` takes one on every target, and only
// its unix face has a filesystem to unlink from.
use std::path::Path;

use crate::dsp::buffer::Frontier;
#[cfg(unix)]
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use clausters_core::shm::{self, View};

use crate::dsp::{BLOCK_SIZE, ControlBuses, NUM_AUDIO_BUSES, NUM_CONTROL_BUSES};

pub use clausters_core::shm::{
    ABI_VERSION, BufferShape, DEFAULT_PEER, DEFAULT_TAP_FRAMES, DEFAULT_TAPS, MAGIC, RING_CAPACITY,
    Role,
};

/// The shared layout is sized for constants the engine also declares, so the
/// two must agree — and here they are checked rather than trusted.
const _: () = assert!(NUM_AUDIO_BUSES == shm::AUDIO_BUS_SLOTS);
const _: () = assert!(BLOCK_SIZE == shm::BLOCK);
const _: () = assert!(crate::dsp::buffer::NUM_BUFFERS == shm::DEFAULT_BUFFER_ROWS);

/// Default buffer-directory rows — scsynth's own default buffer count, so a
/// segment created with no `--max-buffers` describes every buffer the server
/// can allocate.
pub const DEFAULT_BUFFERS: usize = crate::dsp::buffer::NUM_BUFFERS;

/// Default segment size (the `--control-buses`/`--taps`/`--tap-frames`
/// default counts).
pub const SEGMENT_SIZE: usize = shm::segment_size(
    NUM_CONTROL_BUSES,
    DEFAULT_TAPS,
    DEFAULT_TAP_FRAMES,
    DEFAULT_BUFFERS,
);

/// Whether `pid` is a process that still exists — what tells a stale
/// control-plane claim from a live one. Signal 0 checks for the process
/// without touching it; `EPERM` means it is there and not ours, which is still
/// "alive". Off Unix nothing can be asked, so a claim is believed.
fn process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: `kill` with signal 0 sends nothing; it only reports whether
        // the pid exists.
        let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
        rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// Removes a segment's name and the name of every region beside it.
///
/// Unlinking leaves each mapping somebody still holds valid until it is
/// dropped — the same property freeing one buffer relies on — so this ends the
/// material rather than pulling it out from under a reader.
#[cfg(unix)]
pub fn remove_segment(path: &Path) {
    if let Some(dir) = path.parent()
        && let Some(prefix) = path.file_name().and_then(|n| n.to_str())
        && let Ok(entries) = std::fs::read_dir(dir)
    {
        let region = format!("{prefix}.buf");
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|n| n.starts_with(&region))
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    let _ = std::fs::remove_file(path);
}

/// Where there is no shared filesystem to unlink from, dropping the mapping is
/// the whole of it. The twin exists so `NrtSession`'s `Drop` reads the same on
/// every target -- wasm32 among them, where the engine is compiled for the
/// browser and a segment is a heap allocation nobody else can name.
#[cfg(not(unix))]
pub fn remove_segment(path: &Path) {
    let _ = path;
}

/// Collects the segments in `dir` that **no process is serving any more**,
/// regions and all, and answers with the paths it removed.
///
/// A segment is left behind whenever its owner dies without running `Drop` — a
/// `kill`, a crash, a `timeout` in a script — and one region is a whole take,
/// so a few crashes fill a memory filesystem with files nothing can tell live
/// from dead. The claim is what tells them apart: a header that is ours
/// (`MAGIC`/`ABI_VERSION`), naming a pid that no longer exists, is a segment
/// whose command plane nobody serves.
///
/// **Two rules make this safe to do to a file this process never created.**
/// A claim of *nobody* (a segment created a moment ago, or one released on a
/// clean exit so it can be adopted) is never swept — only a pid that answered
/// once and does not now. And `except` — the path the caller is about to open
/// — is never swept, which is what keeps a killed owner's material
/// recoverable: starting a server against **the same path** adopts what is
/// there, exactly as it did before this existed.
#[cfg(unix)]
pub fn sweep_dead_segments(dir: &Path, except: Option<&Path>) -> Vec<PathBuf> {
    let mut swept = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return swept;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if except.is_some_and(|keep| keep == path) || !entry.file_type().is_ok_and(|t| t.is_file())
        {
            continue;
        }
        // Anything that is not one of ours — another program's file, or one of
        // our own region files — fails the header check and is left alone.
        let Ok(segment) = Segment::open(&path) else {
            continue;
        };
        let Some(owner) = segment.control_owner() else {
            continue;
        };
        if process_alive(owner) {
            continue;
        }
        drop(segment);
        remove_segment(&path);
        swept.push(path);
    }
    swept
}

fn check_tap_params(taps: usize, tap_frames: usize) {
    assert!(
        taps == 0 || (tap_frames.is_power_of_two() && tap_frames >= BLOCK_SIZE),
        "tap_frames must be a power of two >= {BLOCK_SIZE} (got {tap_frames})"
    );
}

/// Who owns the memory a segment is laid over.
enum Backing {
    // `u128` words keep the heap allocation 16-aligned — the layout holds
    // 8-aligned atomics and a `&[u8]` box would only guarantee 1.
    Heap(#[allow(dead_code)] Box<[u128]>),
    #[cfg(unix)]
    Mapped {
        ptr: *mut u8,
        len: usize,
    },
}

impl Drop for Backing {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Backing::Mapped { ptr, len } = self {
            // SAFETY: the exact mapping created in `Segment::map_file`.
            unsafe { libc::munmap(*ptr as *mut libc::c_void, *len) };
        }
    }
}

/// One IPC segment: the memory, and the shared reader over it. Always handled
/// as `Arc<Segment>`; the engine, the server and any `ControlBuses` clone keep
/// it alive.
pub struct Segment {
    view: View,
    _backing: Backing,
}

// SAFETY: the shared state is atomics and ring bytes accessed under the SPSC
// cursor protocol (`clausters_core::shm`); the raw pointer inside `Backing` is
// the mapping this value owns and unmaps exactly once.
unsafe impl Send for Segment {}
unsafe impl Sync for Segment {}

impl Segment {
    /// A heap-backed segment for the in-process (embed) transport, with the
    /// default control-bus and tap counts.
    pub fn in_memory() -> Arc<Self> {
        Self::in_memory_with(NUM_CONTROL_BUSES)
    }

    /// A heap-backed segment carrying `control_buses` control slots and the
    /// default tap region.
    pub fn in_memory_with(control_buses: usize) -> Arc<Self> {
        Self::in_memory_full(control_buses, DEFAULT_TAPS, DEFAULT_TAP_FRAMES)
    }

    /// A heap-backed segment with every region sized explicitly.
    pub fn in_memory_full(control_buses: usize, taps: usize, tap_frames: usize) -> Arc<Self> {
        check_tap_params(taps, tap_frames);
        let size = shm::segment_size(control_buses, taps, tap_frames, DEFAULT_BUFFERS);
        let mut words = vec![0u128; size.div_ceil(16)].into_boxed_slice();
        // SAFETY: the allocation is at least `size` bytes and 16-aligned, and
        // the box below keeps it alive for as long as the view.
        let view = unsafe {
            View::init(
                words.as_mut_ptr() as *mut u8,
                size,
                control_buses,
                taps,
                tap_frames,
            )
        };
        Arc::new(Self {
            view,
            _backing: Backing::Heap(words),
        })
    }

    /// Creates (or truncates) the segment file and maps it shared, with the
    /// default control-bus and tap counts. Put it on a memory filesystem —
    /// `/dev/shm/...` on Linux — to avoid disk writes.
    #[cfg(unix)]
    pub fn create(path: &Path) -> io::Result<Arc<Self>> {
        Self::create_with(path, NUM_CONTROL_BUSES)
    }

    /// Like [`create`](Self::create), sizing the control-bus region to
    /// `control_buses` (`--control-buses`).
    #[cfg(unix)]
    pub fn create_with(path: &Path, control_buses: usize) -> io::Result<Arc<Self>> {
        Self::create_full(path, control_buses, DEFAULT_TAPS, DEFAULT_TAP_FRAMES)
    }

    /// Like [`create`](Self::create), with every region sized explicitly
    /// (`--control-buses`, `--taps`, `--tap-frames`).
    #[cfg(unix)]
    pub fn create_full(
        path: &Path,
        control_buses: usize,
        taps: usize,
        tap_frames: usize,
    ) -> io::Result<Arc<Self>> {
        check_tap_params(taps, tap_frames);
        let size = shm::segment_size(control_buses, taps, tap_frames, DEFAULT_BUFFERS);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.set_len(size as u64)?;
        let (ptr, len) = Self::map_file(&file, size)?;
        // SAFETY: the mapping we just made, sized for the counts given.
        let view = unsafe { View::init(ptr, len, control_buses, taps, tap_frames) };
        Ok(Arc::new(Self {
            view,
            _backing: Backing::Mapped { ptr, len },
        }))
    }

    /// **Attaches to the segment at `path`, creating one only when there is
    /// none.** The door a server takes.
    ///
    /// [`create_full`](Self::create_full) truncates, which was right while a
    /// segment was one server's own transport and is wrong now that it indexes
    /// the **material**: the process most likely to be restarted — the one
    /// holding the audio device — would wipe what everybody else is editing.
    /// So a server opens what is there and creates only what is not, and the
    /// sizes it was asked for apply **to a segment it creates**: an existing
    /// one is described by its own header, and disagreeing with it is not a
    /// reason to destroy it.
    ///
    /// **Creating one sweeps the directory** of segments whose owner no longer
    /// exists ([`sweep_dead_segments`]), which is where the ones a killed
    /// editor leaves behind are collected.
    ///
    /// A file that exists and is *not* a valid segment is an error rather than
    /// something to overwrite. Racing creators are not arbitrated here: two
    /// servers started at the same instant against a path with nothing on it
    /// may both create, and the loser's material would be the one that
    /// vanishes — the arrangement this exists for starts the owner first (see
    /// `docs/ipc.md`).
    #[cfg(unix)]
    pub fn open_or_create_full(
        path: &Path,
        control_buses: usize,
        taps: usize,
        tap_frames: usize,
    ) -> io::Result<(Arc<Self>, bool)> {
        match Self::open(path) {
            Ok(seg) => Ok((seg, false)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // Creating one is also the moment to collect the ones nobody
                // serves any more: an editor killed rather than closed leaves
                // its segment and a file per take behind, and this is the only
                // point where a process is provably starting fresh. It never
                // touches `path` itself, so adopting a dead owner's material
                // by name keeps working.
                if let Some(dir) = path.parent() {
                    for swept in sweep_dead_segments(dir, Some(path)) {
                        tracing::info!(
                            "swept the shared segment at {} — its owner is gone",
                            swept.display()
                        );
                    }
                }
                Self::create_full(path, control_buses, taps, tap_frames).map(|seg| (seg, true))
            }
            Err(e) => Err(e),
        }
    }

    /// Maps an existing segment (the client side) and validates the header.
    #[cfg(unix)]
    pub fn open(path: &Path) -> io::Result<Arc<Self>> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let len = file.metadata()?.len() as usize;
        let (ptr, len) = Self::map_file(&file, len)?;
        // SAFETY: the mapping we just made; validated inside, and unmapped
        // below if it is not a segment, so `Backing` never owns a bad one.
        match unsafe { View::attach(ptr, len) } {
            Ok(view) => Ok(Arc::new(Self {
                view,
                _backing: Backing::Mapped { ptr, len },
            })),
            Err(why) => {
                // SAFETY: the mapping we just made; nothing else aliases it.
                unsafe { libc::munmap(ptr as *mut libc::c_void, len) };
                Err(io::Error::other(why))
            }
        }
    }

    #[cfg(unix)]
    fn map_file(file: &std::fs::File, len: usize) -> io::Result<(*mut u8, usize)> {
        use std::os::fd::AsRawFd;
        if len == 0 {
            return Err(io::Error::other("segment size too small"));
        }
        // SAFETY: anonymous-address shared mapping of a file we just sized.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok((ptr as *mut u8, len))
    }

    /// The shared reader over this segment: everything the layout offers, and
    /// the one definition of it.
    pub fn view(&self) -> &View {
        &self.view
    }

    /// Claims the command plane for this process, returning whether it got it.
    ///
    /// The rings are SPSC and there is one pair, so a server that did **not**
    /// get the claim must not attach an [`IpcPeer`] as [`Role::Server`]: it
    /// reads and writes the data plane and serves its clients over sockets.
    /// An owner that died without releasing does not hold the segment hostage
    /// — a pid nothing answers to is stale and is taken over, which is what
    /// makes killing the RT server a recoverable event rather than a reboot.
    pub fn claim_control(&self) -> bool {
        self.view.claim_control(std::process::id(), process_alive)
    }

    /// Gives the command plane back, if this process holds it.
    pub fn release_control(&self) {
        self.view.release_control(std::process::id());
    }

    /// The pid serving the command plane, or `None` while it is free.
    pub fn control_owner(&self) -> Option<u32> {
        self.view.control_owner()
    }

    pub fn set_sample_rate(&self, rate: f64) {
        self.view.set_sample_rate(rate);
    }

    pub fn sample_rate(&self) -> f64 {
        self.view.sample_rate()
    }

    /// The engine's sample clock, mirrored block-accurately by the audio
    /// thread. A client anchors on it with zero transport jitter.
    pub fn clock(&self) -> &AtomicU64 {
        self.view.clock()
    }

    /// The transport clock: samples elapsed under the transport, held while it
    /// is stopped. Monotonic — see [`Self::transport_position`] for the one
    /// that moves with a locate.
    pub fn transport_clock(&self) -> &AtomicU64 {
        self.view.transport_clock()
    }

    /// The transport position: the sample of the *piece* being played. Holds
    /// while stopped, jumps on a locate, wraps at a loop's end.
    pub fn transport_position(&self) -> &AtomicU64 {
        self.view.transport_position()
    }

    /// The segment's base address and its **logical** size in bytes — what an
    /// in-process reader needs to map the same layout an out-of-process one
    /// gets from the file.
    ///
    /// The size is the layout's rather than the allocation's: a heap-backed
    /// segment rounds its allocation up to whole `u128` words, and a reader
    /// validating the length against the header would reject the extra bytes.
    ///
    /// The pointer is only valid while this `Segment` is alive, which is why
    /// every caller in the tree keeps the `Arc` beside it.
    pub fn base(&self) -> *const u8 {
        self.view.base()
    }

    /// The size the layout occupies, in bytes. See [`Self::base`].
    pub fn size(&self) -> usize {
        self.view.len()
    }

    /// **Publishes a buffer's shape and takes its generation**: the number the
    /// region's file is named with, and what tells a peer the slot is live.
    pub fn publish_buffer(
        &self,
        bufnum: usize,
        frames: usize,
        channels: usize,
        sample_rate: f64,
    ) -> Option<u64> {
        self.view
            .publish_buffer(bufnum, frames, channels, sample_rate)
    }

    /// **Raises a buffer's write frontier** — how far its writers have
    /// filled it, in frames, highest wins. Called from the audio thread.
    pub fn raise_buffer_frontier(&self, bufnum: usize, frame: u64) {
        self.view.raise_buffer_frontier(bufnum, frame);
    }

    /// How far buffer `bufnum` has been written, in frames, or `None` when the
    /// directory has no row for it.
    pub fn buffer_frontier(&self, bufnum: usize) -> Option<u64> {
        self.view.buffer_frontier(bufnum)
    }

    /// Marks a slot empty. The region's file is unlinked by whoever owns it;
    /// this is what a peer reads to learn that what it holds is history.
    pub fn retire_buffer(&self, bufnum: usize) {
        self.view.retire_buffer(bufnum);
    }

    /// What a peer needs to map buffer `bufnum`: its generation (which names
    /// the region) and its shape — or `None` when the slot is empty.
    pub fn buffer_info(&self, bufnum: usize) -> Option<BufferShape> {
        self.view.buffer_info(bufnum)
    }

    /// **Maps buffer `bufnum`'s samples** — the peer's door to the material.
    ///
    /// `at` is the segment's own path, which is what the region is named from.
    /// `None` when the slot is empty, when the row is out of range, or when the
    /// region cannot be opened — which is the ordinary answer for a buffer that
    /// was freed between reading the directory and opening the file, and is why
    /// this returns the generation it mapped: a caller that keeps the mapping
    /// compares it against the row to learn its material is history.
    #[cfg(unix)]
    pub fn map_buffer(
        self: &Arc<Self>,
        at: &Path,
        bufnum: usize,
    ) -> Option<(u64, Arc<crate::dsp::buffer::Buffer>)> {
        let shape = self.buffer_info(bufnum)?;
        let cells = shape.frames.checked_mul(shape.channels)?;
        let path = crate::dsp::region::Region::path_for(at, bufnum, shape.generation);
        let region = crate::dsp::region::Region::open(&path, cells).ok()?;
        // Re-read: a row that moved while the file was being opened describes
        // a buffer this mapping is not.
        if self.buffer_info(bufnum)?.generation != shape.generation {
            return None;
        }
        Some((
            shape.generation,
            Arc::new(
                crate::dsp::buffer::Buffer::shared(
                    Arc::new(region),
                    shape.channels,
                    shape.frames,
                    shape.sample_rate,
                )
                // **An attached server publishes the frontier too**, and it is
                // the one that matters most: the process holding the input
                // device is the only one that can record, and it is never the
                // owner of the material. The row belongs to the owner, the
                // number in it belongs to whoever wrote.
                .with_frontier(self.frontier_sink(bufnum)),
            ),
        ))
    }

    /// A [`Frontier`] sink over buffer `bufnum`'s directory row: where a
    /// writing UGen says how far it has filled the material.
    ///
    /// It holds the segment rather than a pointer into it, so the mapping
    /// cannot go out from under a buffer the engine is still writing — and the
    /// write itself is one relaxed read-modify-write, which is what makes it
    /// callable from the audio thread.
    pub fn frontier_sink(self: &Arc<Self>, bufnum: usize) -> Arc<dyn Frontier> {
        Arc::new(SegmentFrontier {
            segment: Arc::clone(self),
            bufnum,
        })
    }

    /// How many control buses this segment carries — the header's own count,
    /// which is what a server attaching to somebody else's segment must run
    /// with whatever its own command line asked for. The shape of a segment
    /// belongs to the process that created it.
    pub fn control_bus_count(&self) -> usize {
        self.view.control_bus_count()
    }

    /// Control buses living inside the segment; hand this to
    /// `engine_pair_full` so `InCtl` and `/bus_set` operate on shared memory.
    pub fn control_buses(self: &Arc<Self>) -> ControlBuses {
        let cells = self.view.controls();
        let (ptr, count) = (cells.as_ptr(), cells.len());
        // SAFETY: the region is part of the segment, kept alive by the Arc.
        unsafe { ControlBuses::from_raw(ptr, count, Arc::clone(self) as _) }
    }

    /// How many audio buses the segment's per-bus region covers.
    pub fn audio_buses(&self) -> usize {
        self.view.audio_buses()
    }

    /// Which sample ring, if any, is recording audio bus `bus`.
    pub fn tap_of_bus(&self, bus: usize) -> Option<usize> {
        self.view.tap_of_bus(bus)
    }

    /// Publishes (or clears, with `None`) the ring recording audio bus `bus`.
    pub fn set_tap_of_bus(&self, bus: usize, tap: Option<usize>) {
        self.view.set_tap_of_bus(bus, tap);
    }

    /// Audio bus `bus`'s level: the peak magnitude of the last block the
    /// engine processed. What a meter reads — one number per block instead of
    /// a ring, so metering every bus costs no tap.
    pub fn level(&self, bus: usize) -> f32 {
        self.view.level(bus)
    }

    /// Publishes audio bus `bus`'s block level. **Audio-thread safe**.
    pub fn set_level(&self, bus: usize, peak: f32) {
        self.view.set_level(bus, peak);
    }

    /// Number of audio-tap rings in the segment.
    pub fn taps(&self) -> usize {
        self.view.taps()
    }

    /// Per-tap ring capacity in samples (a power of two).
    pub fn tap_frames(&self) -> usize {
        self.view.tap_frames()
    }

    /// Appends one block of samples to tap `i`'s ring. **Audio-thread safe**:
    /// one `memcpy` plus one Release store, no allocation, no lock.
    pub fn tap_write(&self, i: usize, samples: &[f32]) {
        self.view.tap_write(i, samples);
    }

    /// Copies the **newest** `out.len()` samples of tap `i` into `out`,
    /// returning the stream position at the window's end.
    pub fn tap_read_latest(&self, i: usize, out: &mut [f32]) -> Option<u64> {
        self.view.tap_read_latest(i, out)
    }
}

/// One end of a segment's ring pair: which direction this peer writes, and
/// which it drains.
pub struct IpcPeer {
    segment: Arc<Segment>,
    role: Role,
}

impl IpcPeer {
    pub fn new(segment: Arc<Segment>, role: Role) -> Self {
        Self { segment, role }
    }

    pub fn segment(&self) -> &Arc<Segment> {
        &self.segment
    }

    /// Appends one packet to the outbound ring, tagged for `peer` — who wrote
    /// it (client → server) or who it is for (server → client). `false` when
    /// the ring lacks space — backpressure, the caller may retry (nothing is
    /// dropped).
    pub fn push(&self, peer: u32, packet: &[u8]) -> bool {
        self.segment.view.push(self.role, peer, packet)
    }

    /// Pops one packet from the inbound ring into `buf`, returning its peer tag
    /// and its length. Malformed lengths (a hostile or crashed peer) drop the
    /// whole ring contents and return `None`.
    pub fn try_pop(&self, buf: &mut [u8]) -> Option<(u32, usize)> {
        self.segment.view.try_pop(self.role, buf)
    }
}

/// One buffer's row, as a [`Frontier`] sink.
struct SegmentFrontier {
    segment: Arc<Segment>,
    bufnum: usize,
}

impl Frontier for SegmentFrontier {
    fn raise(&self, frame: u64) {
        self.segment.raise_buffer_frontier(self.bufnum, frame);
    }
}
