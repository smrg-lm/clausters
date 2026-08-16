//! the shared-memory IPC segment — transport and data plane.
//!
//! OSC stays the only **encoding**; this module adds two **transports**
//! beside UDP, both built on one memory segment:
//!
//! - **Two processes, one machine** (`clausters --shm <path>`): the segment
//!   is a memory-mapped file. A local client maps the same file: no socket,
//!   no packet loss (the ring gives backpressure instead), and the data
//!   plane below costs a memory read instead of an OSC round trip.
//! - **In-process / embedded** (`src/embed.rs`, feature `embed`): the same
//!   layout over plain heap memory — the "client" is the host application
//!   calling into the cdylib.
//!
//! The segment carries:
//!
//! - a **versioned header** (magic + ABI version checked on attach: the
//!   scsynth plugin-ABI lesson — never trust an unversioned binary
//!   boundary);
//! - the **data plane**: the engine's sample clock, mirrored block-accurately
//!   by the audio thread (one extra atomic store per block — a client anchors with
//!   zero UDP jitter), the 1024 control buses as raw atomics (the engine
//!   reads *these very words* through `InCtl`: a client write is live on the
//!   next block, no command involved), the **audio taps** (ABI v3): a
//!   fixed set of single-channel sample rings the audio thread appends a
//!   block to whenever `/bus_tap` routes an audio bus into one, read lock-free by
//!   a peer each display frame — the audio-rate sibling of the control buses
//!   (SuperCollider's `ScopeOut2` scope buffers play this role) — and the
//!   **audio-bus region** (ABI v4): two words per audio bus, the bus → tap
//!   directory and the block level, both keyed by the bus so a reader names a
//!   bus and never a ring, and a meter costs one number per block instead of
//!   a whole tap;
//! - the **command plane**: two SPSC byte rings (client→server and
//!   server→client) carrying ordinary length-prefixed OSC packets. The
//!   network thread drains the inbound ring in its loop and routes replies
//!   back by [`ClientId::Ring`](crate::osc::ClientId); ring bytes are as
//!   untrusted as UDP bytes (`osc::decode_packet` validates).
//!
//!   Each frame carries a **peer tag** beside its length: on the inbound ring
//!   it says who *authored* the packet, on the outbound one who the reply is
//!   *for*. Every frame is addressed — a notification reaches several clients
//!   as several frames, one per client registered with `/server_notify` —
//!   which is what lets one segment serve several independent clients, a
//!   script and a GUI host in one page, each with its own `/bus_stream`
//!   subscription and its own replies instead of the single `ClientId::Ring`
//!   they used to share and overwrite.
//!   The rings stay SPSC: the tag says who wrote the *packet*, not who wrote
//!   the ring, and a multi-peer embedder funnels its sends through one
//!   producer (the page's worklet does exactly this).
//!
//! Synchronization is index-based: each ring has a `head` (producer) and
//! `tail` (consumer) cursor, monotonically increasing `u32`s used modulo the
//! capacity. The producer copies the packet first and publishes `head` with
//! Release; the consumer Acquire-loads `head`. A malformed length resyncs
//! the ring by dropping its contents (the producer is outside our trust
//! boundary). The server polls the ring on a short socket timeout instead of
//! a semaphore — a documented trade-off in `docs/ipc.md`.

#[cfg(unix)]
use std::fs::OpenOptions;
#[cfg(unix)]
use std::io;
#[cfg(unix)]
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};

use crate::dsp::{BLOCK_SIZE, ControlBuses, NUM_AUDIO_BUSES, NUM_CONTROL_BUSES};

/// "CLAU" little-endian.
pub const MAGIC: u32 = 0x5541_4C43;
/// Bump on **any** layout change: attaching rejects mismatches. v5 changed the
/// embed C ABI rather than the segment: `clausters_render` grew a `seed` in
/// pointer form (NULL for a fresh take, a seed to repeat one) and out pointers
/// for the score's event count and for the seed the render actually used. v6
/// added the transport clock beside the sample clock, so a local peer reads
/// the piece's own position with a load. v7 gave ring frames a peer tag, so one
/// segment carries several independent clients (see the module docs); no field
/// of the header or the data plane moved, only the framing inside the rings. v8
/// added the transport **position** beside the transport clock — two different
/// quantities, see [`Segment::transport_position`] — in the last of the
/// header's reserved space, so again no offset moved.
pub const ABI_VERSION: u32 = 8;

/// The peer tag an embedder gets when it never asks for one: the single client
/// a segment has always had, so a peer built against the old single-client
/// behaviour keeps working unchanged.
pub const DEFAULT_PEER: u32 = 0;
/// Byte capacity of each ring (power of two).
pub const RING_CAPACITY: usize = 64 * 1024;
/// Default audio-tap count (`--taps`).
pub const DEFAULT_TAPS: usize = 8;
/// Default per-tap ring capacity in samples (`--tap-frames`): a power of two,
/// ~341 ms at 48 kHz — comfortably more than twice any oscilloscope window.
pub const DEFAULT_TAP_FRAMES: usize = 16384;
/// Each tap slot starts 64-byte aligned: the cursor gets its own cache line
/// (the audio thread stores it every block; readers poll it), and the sample
/// ring follows without straddling the cursor's line.
const TAP_ALIGN: usize = 64;

#[repr(C)]
struct Header {
    magic: u32,
    abi_version: u32,
    /// `f64` bits; filled by the server once the device rate is known.
    sample_rate_bits: AtomicU64,
    /// Mirrored by the audio thread every block (samples processed).
    sample_clock: AtomicU64,
    ring_capacity: u32,
    control_buses: u32,
    /// Audio-tap count and per-tap ring capacity in samples (ABI v3); the tap
    /// region trails the control buses (see `Segment::tap_region_offset`).
    taps: u32,
    tap_frames: u32,
    /// Audio-bus count (ABI v4): the length of the per-bus directory and level
    /// table that sit between the control slots and the tap region.
    audio_buses: u32,
    /// Kept for 8-byte alignment of `transport_clock` below.
    _pad: u32,
    /// The transport clock (ABI v6): samples elapsed *under the transport*,
    /// frozen while it is stopped. The sample clock above never stops, so a
    /// reader pacing on the device wants that one — but this one is monotonic
    /// too, which is what a scheduler needs and what a **playhead does not**:
    /// for where the piece *is*, read `transport_position` below.
    ///
    /// It sits in what was reserved space rather than beside `sample_clock`,
    /// which is where it belongs by meaning: putting it there would shift every
    /// field after it, and out-of-process readers pin those offsets by hand
    /// (`clients/python/clausters/ipc.py`). Reserved space exists precisely so
    /// a counter can be added without moving anything.
    transport_clock: AtomicU64,
    /// The transport **position** (ABI v8): the sample of the *piece* the
    /// engine is playing, in the material's own axis.
    ///
    /// It is not the clock above and the difference is the whole reason both
    /// exist. `transport_clock` counts samples **elapsed** under the transport
    /// and is monotonic by construction, so a locate cannot move it; the
    /// scheduler's transport queue needs exactly that, since "due" is only
    /// meaningful on an axis that does not jump. This one is where the piece
    /// *is*: it advances with the clock while rolling, holds while stopped,
    /// **jumps on `/transport_locate`** and wraps at a loop's end. A playhead
    /// and a buffer reader want this one; a scheduled bundle wants that one.
    ///
    /// Non-negative: a locate before the start of the piece clamps to 0, the
    /// same floor `/transport_set` puts on `originSample`.
    ///
    /// This spends the last of the header's reserved space. The next counter
    /// added here moves offsets, and out-of-process readers pin those by hand
    /// (`clients/python/clausters/ipc.py`, `clients/gui/src/host/shm.rs`), so
    /// it costs more than a version bump.
    transport_position: AtomicU64,
}

#[repr(C)]
struct Ring {
    /// Write cursor, owned by the producer. Monotonic, wraps at u32::MAX
    /// (capacity divides 2³², so the modulo arithmetic stays consistent).
    head: AtomicU32,
    /// Read cursor, owned by the consumer.
    tail: AtomicU32,
    _pad: [u32; 14],
    data: [u8; RING_CAPACITY],
}

/// Fixed prefix of the segment. The `header.control_buses` control slots
/// (`AtomicU32`) are a **trailing dynamically-sized array** right after this
/// struct, so the control-bus count is a runtime parameter (`--control-buses`)
/// without the rings moving — they stay fixed fields here.
#[repr(C)]
struct Layout {
    header: Header,
    /// Client → server commands.
    c2s: Ring,
    /// Server → client replies.
    s2c: Ring,
}

/// Byte offset of the **audio-bus region** (ABI v4): the control slots' end.
/// Two arrays of one word per audio bus follow, the directory then the levels
/// (see [`Segment::tap_of_bus`] and [`Segment::level`]).
const fn bus_region_offset(control_buses: usize) -> usize {
    size_of::<Layout>() + control_buses * size_of::<AtomicU32>()
}

/// Byte size of the audio-bus region: the bus → tap directory (`AtomicI32`
/// each) followed by the per-bus block levels (`AtomicU32`, `f32` bits). It is
/// always sized for the compile-time cap ([`NUM_AUDIO_BUSES`]) — 1 KiB, small
/// enough that making it a runtime parameter would buy nothing.
const fn bus_region_size(audio_buses: usize) -> usize {
    2 * audio_buses * size_of::<AtomicU32>()
}

/// Byte offset of the tap region: the audio-bus region's end, rounded up to
/// [`TAP_ALIGN`] so the first tap cursor is cache-line aligned.
const fn tap_region_offset(control_buses: usize) -> usize {
    let buses_end = bus_region_offset(control_buses) + bus_region_size(NUM_AUDIO_BUSES);
    buses_end.div_ceil(TAP_ALIGN) * TAP_ALIGN
}

/// Byte size of one tap slot: a cache line for the cursor, then the sample
/// ring. `tap_frames` is a power of two ≥ [`BLOCK_SIZE`], so the ring's byte
/// size is a multiple of [`TAP_ALIGN`] and every slot stays aligned.
const fn tap_slot_size(tap_frames: usize) -> usize {
    TAP_ALIGN + tap_frames * size_of::<f32>()
}

/// Total byte size of a segment carrying `control_buses` control slots, the
/// fixed audio-bus region, and `taps` audio-tap rings of `tap_frames` samples
/// each.
const fn segment_size(control_buses: usize, taps: usize, tap_frames: usize) -> usize {
    tap_region_offset(control_buses) + taps * tap_slot_size(tap_frames)
}

/// Default segment size (the `--control-buses`/`--taps`/`--tap-frames`
/// default counts).
pub const SEGMENT_SIZE: usize = segment_size(NUM_CONTROL_BUSES, DEFAULT_TAPS, DEFAULT_TAP_FRAMES);

enum Backing {
    // `u128` words keep the heap allocation 16-aligned — `Layout` holds
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

/// One IPC segment: header + data plane + ring pair. Always handled as
/// `Arc<Segment>`; the engine, the server and any `ControlBuses` clone keep
/// it alive.
pub struct Segment {
    layout: *mut Layout,
    _backing: Backing,
}

// SAFETY: all shared state inside `Layout` is atomics or ring bytes accessed
// under the SPSC cursor protocol.
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
        let size = segment_size(control_buses, taps, tap_frames);
        let mut words = vec![0u128; size.div_ceil(16)].into_boxed_slice();
        let layout = words.as_mut_ptr() as *mut Layout;
        let seg = Self {
            layout,
            _backing: Backing::Heap(words),
        };
        seg.init_header(control_buses, taps, tap_frames);
        Arc::new(seg)
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
        let size = segment_size(control_buses, taps, tap_frames);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.set_len(size as u64)?;
        let seg = Self::map_file(&file, size)?;
        seg.init_header(control_buses, taps, tap_frames);
        Ok(Arc::new(seg))
    }

    /// Maps an existing segment (the client side) and validates the header.
    #[cfg(unix)]
    pub fn open(path: &Path) -> io::Result<Arc<Self>> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let len = file.metadata()?.len() as usize;
        if len < size_of::<Layout>() {
            return Err(io::Error::other("segment size too small"));
        }
        let seg = Self::map_file(&file, len)?;
        let header = &seg.layout().header;
        if header.magic != MAGIC {
            return Err(io::Error::other("not a clausters segment (bad magic)"));
        }
        if header.abi_version != ABI_VERSION {
            return Err(io::Error::other(format!(
                "segment ABI version {} != supported {ABI_VERSION}",
                header.abi_version
            )));
        }
        // The mapped length must match the region sizes the header claims.
        if len
            != segment_size(
                header.control_buses as usize,
                header.taps as usize,
                header.tap_frames as usize,
            )
        {
            return Err(io::Error::other("segment size mismatch"));
        }
        Ok(Arc::new(seg))
    }

    #[cfg(unix)]
    fn map_file(file: &std::fs::File, len: usize) -> io::Result<Self> {
        use std::os::fd::AsRawFd;
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
        Ok(Self {
            layout: ptr as *mut Layout,
            _backing: Backing::Mapped {
                ptr: ptr as *mut u8,
                len,
            },
        })
    }

    /// Runs at creation time, before any other process can have attached
    /// (the file was just created/truncated), so plain writes are sound.
    fn init_header(&self, control_buses: usize, taps: usize, tap_frames: usize) {
        let header = unsafe { &mut (*self.layout).header };
        header.ring_capacity = RING_CAPACITY as u32;
        header.control_buses = control_buses as u32;
        header.taps = taps as u32;
        header.tap_frames = tap_frames as u32;
        header.audio_buses = NUM_AUDIO_BUSES as u32;
        // A zeroed directory would read as "every bus is recorded by tap 0";
        // `-1` is the absent marker, the same one `/bus_tap` uses. The levels are
        // fine zeroed: those bits are `0.0`, which is silence.
        for bus in 0..NUM_AUDIO_BUSES {
            self.set_tap_of_bus(bus, None);
        }
        header.abi_version = ABI_VERSION;
        // Written last: a client that sees the magic sees a full header.
        header.magic = MAGIC;
    }

    fn layout(&self) -> &Layout {
        unsafe { &*self.layout }
    }

    /// The control-bus array, a trailing region right after the fixed
    /// [`Layout`] prefix (see its doc comment).
    fn controls_ptr(&self) -> *const AtomicU32 {
        // SAFETY: `create`/`in_memory` sized the backing to hold the control
        // region immediately after the `Layout` prefix.
        unsafe { (self.layout as *const u8).add(size_of::<Layout>()) as *const AtomicU32 }
    }

    pub fn set_sample_rate(&self, rate: f64) {
        self.layout()
            .header
            .sample_rate_bits
            .store(rate.to_bits(), Ordering::Release);
    }

    pub fn sample_rate(&self) -> f64 {
        f64::from_bits(
            self.layout()
                .header
                .sample_rate_bits
                .load(Ordering::Acquire),
        )
    }

    /// The shared sample-clock cell (the engine's block-accurate mirror).
    pub fn clock(&self) -> &AtomicU64 {
        &self.layout().header.sample_clock
    }

    /// The transport clock: samples elapsed under the transport, held while it
    /// is stopped. Monotonic — see [`Self::transport_position`] for the one
    /// that moves with a locate.
    pub fn transport_clock(&self) -> &AtomicU64 {
        &self.layout().header.transport_clock
    }

    /// The transport position: the sample of the *piece* being played. Holds
    /// while stopped, jumps on a locate, wraps at a loop's end.
    pub fn transport_position(&self) -> &AtomicU64 {
        &self.layout().header.transport_position
    }

    /// The segment's base address and its **logical** size in bytes — what an
    /// in-process reader needs to map the same layout an out-of-process one
    /// gets from the file.
    ///
    /// The size is derived from the header rather than from the allocation: a
    /// heap-backed segment rounds its allocation up to whole `u128` words, and
    /// a reader validating `len == expected` would reject the extra bytes.
    ///
    /// The pointer is only valid while this `Segment` is alive, which is why
    /// every caller in the tree keeps the `Arc` beside it.
    pub fn base(&self) -> *const u8 {
        self.layout as *const u8
    }

    /// The size the layout occupies, in bytes. See [`Self::base`].
    pub fn size(&self) -> usize {
        let header = &self.layout().header;
        segment_size(
            header.control_buses as usize,
            header.taps as usize,
            header.tap_frames as usize,
        )
    }

    /// Control buses living inside the segment; hand this to
    /// `engine_pair_full` so `InCtl` and `/bus_set` operate on shared memory.
    pub fn control_buses(self: &Arc<Self>) -> ControlBuses {
        let count = self.layout().header.control_buses as usize;
        let ptr = self.controls_ptr();
        // SAFETY: the region is part of the segment, kept alive by the Arc.
        unsafe { ControlBuses::from_raw(ptr, count, Arc::clone(self) as _) }
    }

    /// Number of audio buses the segment's per-bus region covers.
    pub fn audio_buses(&self) -> usize {
        self.layout().header.audio_buses as usize
    }

    /// Base of the audio-bus region: the directory array, the levels right
    /// after it.
    fn bus_region_ptr(&self) -> *const AtomicI32 {
        let offset = bus_region_offset(self.layout().header.control_buses as usize);
        // SAFETY: the constructors sized the backing for the bus region.
        unsafe { (self.layout as *const u8).add(offset) as *const AtomicI32 }
    }

    /// The directory cell of audio bus `bus`: which tap ring is recording it,
    /// or `-1`. **The bus is the key** — a reader names the bus it wants to
    /// see and finds where the samples land, so the ring index stays an
    /// implementation detail of this segment and never reaches an API.
    fn bus_tap_cell(&self, bus: usize) -> Option<&AtomicI32> {
        (bus < self.audio_buses()).then(|| {
            // SAFETY: bounds-checked against the header's own count.
            unsafe { &*self.bus_region_ptr().add(bus) }
        })
    }

    /// The block-level cell of audio bus `bus` (`f32` bits).
    fn bus_level_cell(&self, bus: usize) -> Option<&AtomicU32> {
        let count = self.audio_buses();
        (bus < count).then(|| {
            // SAFETY: the levels are the second array of the bus region,
            // bounds-checked against the header's own count.
            unsafe { &*(self.bus_region_ptr().add(count + bus) as *const AtomicU32) }
        })
    }

    /// Which tap ring records audio bus `bus`, or `None` when nothing does.
    pub fn tap_of_bus(&self, bus: usize) -> Option<usize> {
        match self.bus_tap_cell(bus)?.load(Ordering::Acquire) {
            i if i >= 0 => Some(i as usize),
            _ => None,
        }
    }

    /// Publishes (or clears, with `None`) the ring recording audio bus `bus`.
    /// Written by the server when it starts or stops recording a bus.
    pub fn set_tap_of_bus(&self, bus: usize, tap: Option<usize>) {
        if let Some(cell) = self.bus_tap_cell(bus) {
            cell.store(tap.map_or(-1, |i| i as i32), Ordering::Release);
        }
    }

    /// Audio bus `bus`'s level: the peak magnitude of the last block the
    /// engine processed, or `0.0` where the bus is silent or out of range.
    /// This is what a meter reads — one number per block instead of a ring,
    /// so metering every bus costs no tap.
    pub fn level(&self, bus: usize) -> f32 {
        self.bus_level_cell(bus)
            .map_or(0.0, |cell| f32::from_bits(cell.load(Ordering::Relaxed)))
    }

    /// Publishes audio bus `bus`'s block level. **Audio-thread safe**: one
    /// relaxed store, no allocation, no lock.
    pub fn set_level(&self, bus: usize, peak: f32) {
        if let Some(cell) = self.bus_level_cell(bus) {
            cell.store(peak.to_bits(), Ordering::Relaxed);
        }
    }

    /// Number of audio-tap rings in the segment.
    pub fn taps(&self) -> usize {
        self.layout().header.taps as usize
    }

    /// Per-tap ring capacity in samples (a power of two).
    pub fn tap_frames(&self) -> usize {
        self.layout().header.tap_frames as usize
    }

    /// Base of tap `i`'s slot: a [`TAP_ALIGN`] header line holding the cursor,
    /// then `tap_frames` ring samples.
    fn tap_slot_ptr(&self, i: usize) -> *const u8 {
        debug_assert!(i < self.taps());
        let offset = tap_region_offset(self.layout().header.control_buses as usize)
            + i * tap_slot_size(self.tap_frames());
        // SAFETY: the constructors sized the backing for the tap region.
        unsafe { (self.layout as *const u8).add(offset) }
    }

    /// Tap `i`'s cursor: total samples ever written (monotonic). The ring
    /// holds samples `[cursor - tap_frames, cursor)`.
    fn tap_cursor(&self, i: usize) -> &AtomicU64 {
        // SAFETY: the slot starts 64-byte aligned and its first word is the
        // cursor; the mapping outlives `self`.
        unsafe { &*(self.tap_slot_ptr(i) as *const AtomicU64) }
    }

    fn tap_data_ptr(&self, i: usize) -> *const f32 {
        // SAFETY: the ring starts one alignment line into the slot.
        unsafe { self.tap_slot_ptr(i).add(TAP_ALIGN) as *const f32 }
    }

    /// Appends one block of samples to tap `i`'s ring. **Audio-thread safe**:
    /// one `memcpy` plus one Release store, no allocation, no lock. Single
    /// writer only (the engine). The block never wraps: the ring capacity is
    /// a power of two ≥ the block size and the cursor only advances by whole
    /// blocks, so every write lands block-aligned inside the ring.
    pub fn tap_write(&self, i: usize, samples: &[f32]) {
        let frames = self.tap_frames();
        debug_assert!(samples.len() == BLOCK_SIZE && frames.is_multiple_of(BLOCK_SIZE));
        let cursor = self.tap_cursor(i).load(Ordering::Relaxed);
        let start = (cursor as usize) % frames;
        // SAFETY: `start + BLOCK_SIZE <= frames` (both are multiples of the
        // block size), single writer, region sized by the constructors.
        unsafe {
            std::ptr::copy_nonoverlapping(
                samples.as_ptr(),
                self.tap_data_ptr(i).add(start) as *mut f32,
                samples.len(),
            );
        }
        self.tap_cursor(i)
            .store(cursor + samples.len() as u64, Ordering::Release);
    }

    /// Copies the **newest** `out.len()` samples of tap `i` into `out`,
    /// returning the stream position (total samples written) at the window's
    /// end — `None` when the tap index is out of range, the window is empty
    /// or larger than half the ring, or the tap has not yet written a full
    /// window. The half-ring cap makes a torn read need the writer to lap
    /// half the ring during one `memcpy`; the cursor double-check turns that
    /// "impossible in practice" into checked — on the rare tear it retries
    /// with the fresh cursor.
    pub fn tap_read_latest(&self, i: usize, out: &mut [f32]) -> Option<u64> {
        let frames = self.tap_frames();
        let want = out.len();
        if i >= self.taps() || want == 0 || want > frames / 2 {
            return None;
        }
        loop {
            let end = self.tap_cursor(i).load(Ordering::Acquire);
            if (end as usize) < want {
                return None;
            }
            let start = end - want as u64;
            let s = (start as usize) % frames;
            let first = want.min(frames - s);
            let data = self.tap_data_ptr(i);
            // SAFETY: both copies stay inside the ring; concurrent writer
            // overlap is detected by the cursor re-check below.
            unsafe {
                std::ptr::copy_nonoverlapping(data.add(s), out.as_mut_ptr(), first);
                std::ptr::copy_nonoverlapping(data, out.as_mut_ptr().add(first), want - first);
            }
            let end_after = self.tap_cursor(i).load(Ordering::Acquire);
            if end_after - start <= frames as u64 {
                return Some(end);
            }
        }
    }
}

/// Tap parameters every constructor enforces: no taps, or a power-of-two ring
/// of at least one block (so `tap_write` never wraps mid-block).
fn check_tap_params(taps: usize, tap_frames: usize) {
    assert!(
        taps == 0 || (tap_frames.is_power_of_two() && tap_frames >= BLOCK_SIZE),
        "tap_frames must be a power of two >= {BLOCK_SIZE} (got {tap_frames})"
    );
}

/// Bytes each ring frame carries before its payload: the `u32` payload length
/// and the `u32` peer tag, both little-endian. The payload itself is padded to
/// 4 bytes, so a frame is always 4-aligned.
const FRAME_HEADER: usize = 8;

/// Which end of the ring pair this peer is.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Server,
    Client,
}

/// One endpoint of the command plane: pops packets from its inbound ring,
/// pushes packets to its outbound ring. Packets are length-prefixed
/// (`u32` LE) and padded to 4 bytes.
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

    fn inbound(&self) -> &Ring {
        match self.role {
            Role::Server => &self.segment.layout().c2s,
            Role::Client => &self.segment.layout().s2c,
        }
    }

    fn outbound(&self) -> &Ring {
        match self.role {
            Role::Server => &self.segment.layout().s2c,
            Role::Client => &self.segment.layout().c2s,
        }
    }

    /// Appends one packet to the outbound ring, tagged for `peer` — who wrote
    /// it (client → server) or who it is for (server → client). `false` when
    /// the ring lacks space — backpressure, the caller may retry (nothing is
    /// dropped).
    pub fn push(&self, peer: u32, packet: &[u8]) -> bool {
        let ring = self.outbound();
        let padded = packet.len().div_ceil(4) * 4;
        let total = FRAME_HEADER + padded;
        if packet.is_empty() || total > RING_CAPACITY {
            return false;
        }
        let head = ring.head.load(Ordering::Relaxed);
        let tail = ring.tail.load(Ordering::Acquire);
        let used = head.wrapping_sub(tail) as usize;
        if RING_CAPACITY - used < total {
            return false;
        }
        write_ring(ring, head, &(packet.len() as u32).to_le_bytes());
        write_ring(ring, head.wrapping_add(4), &peer.to_le_bytes());
        write_ring(ring, head.wrapping_add(FRAME_HEADER as u32), packet);
        ring.head
            .store(head.wrapping_add(total as u32), Ordering::Release);
        true
    }

    /// Pops one packet from the inbound ring into `buf`, returning its peer tag
    /// and its length. Malformed lengths (a hostile or crashed peer) drop the
    /// whole ring contents and return `None`.
    pub fn try_pop(&self, buf: &mut [u8]) -> Option<(u32, usize)> {
        let ring = self.inbound();
        let head = ring.head.load(Ordering::Acquire);
        let tail = ring.tail.load(Ordering::Relaxed);
        let used = head.wrapping_sub(tail) as usize;
        if used == 0 {
            return None;
        }
        let mut header = [0u8; FRAME_HEADER];
        read_ring(ring, tail, &mut header);
        let len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let peer = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
        let padded = len.div_ceil(4) * 4;
        let total = FRAME_HEADER + padded;
        if len == 0 || total > used || len > buf.len() {
            // Untrusted peer wrote garbage (or a packet bigger than our
            // buffer): resync by discarding everything buffered.
            ring.tail.store(head, Ordering::Release);
            return None;
        }
        read_ring(
            ring,
            tail.wrapping_add(FRAME_HEADER as u32),
            &mut buf[..len],
        );
        ring.tail
            .store(tail.wrapping_add(total as u32), Ordering::Release);
        Some((peer, len))
    }
}

fn write_ring(ring: &Ring, at: u32, bytes: &[u8]) {
    let data = ring.data.as_ptr() as *mut u8;
    let start = at as usize % RING_CAPACITY;
    let first = bytes.len().min(RING_CAPACITY - start);
    // SAFETY: SPSC — only the producer writes between head and tail, and
    // the range was checked to fit before calling.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), data.add(start), first);
        std::ptr::copy_nonoverlapping(bytes.as_ptr().add(first), data, bytes.len() - first);
    }
}

fn read_ring(ring: &Ring, at: u32, into: &mut [u8]) {
    let data = ring.data.as_ptr();
    let start = at as usize % RING_CAPACITY;
    let first = into.len().min(RING_CAPACITY - start);
    // SAFETY: SPSC — only the consumer reads between tail and head.
    unsafe {
        std::ptr::copy_nonoverlapping(data.add(start), into.as_mut_ptr(), first);
        std::ptr::copy_nonoverlapping(data, into.as_mut_ptr().add(first), into.len() - first);
    }
}
