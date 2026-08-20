//! The shared-memory segment: **one definition of the layout**, and the reader
//! every process uses.
//!
//! The segment is the local data plane — the clocks, the control buses as the
//! very words the engine reads, the audio taps, the per-bus levels, the buffer
//! directory — plus a pair of byte rings carrying OSC. Four processes look at
//! it: the server that writes it, the GUI host, the Python client, and any
//! later peer.
//!
//! **Why it lives here and not in the server.** Every one of those readers used
//! to mirror the `#[repr(C)]` layout by hand, pinning offsets against a comment,
//! and the version counter was the only thing tying them together. That is
//! exactly as fragile as it sounds, and it failed twice in one week in two
//! different ways: a mirror that agreed on the version number and not on the
//! size check refused every valid segment, and another declared 1024 control
//! buses against a server that had had 16 384 for months. A number cannot check
//! a layout. So the layout is one piece of code, in the crate every process
//! already links, and a reader is [`View`] rather than a copy of the arithmetic.
//!
//! **What stays outside.** Getting the memory: `mmap` of a file, a heap
//! allocation, a `memoryview` over Python's `mmap`. That is the one genuinely
//! platform-shaped part, so each process does it and hands the address here —
//! which is also what keeps this module compiling for wasm, where there is no
//! mapping at all and a page keeps talking OSC.
//!
//! # Layout
//!
//! ```text
//! Header (64 B) | c2s Ring | s2c Ring | control buses | audio-bus region | tap rings | buffer directory
//! ```
//!
//! Everything after the two rings is a **trailing region sized at run time**,
//! and every offset is derived from the header rather than fixed — which is
//! what lets `--control-buses`, `--taps` and `--tap-frames` be options instead
//! of recompiles. The buffer directory is the tail, and *its* row count is what
//! remains of the mapped length rather than a header field: the header has no
//! reserved space left, and a count there would move every offset after it.
//!
//! `docs/ipc.md` is the prose version of all of this, and the reference a
//! non-Rust peer reads.

use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};

/// "CLAU" little-endian.
pub const MAGIC: u32 = 0x5541_4C43;

/// The segment layout version, checked on attach. Every binary boundary is
/// versioned and refused on mismatch (the scsynth plugin-ABI lesson); what each
/// version changed is recorded in `docs/ipc.md`, not here — a changelog in a
/// constant is a changelog nobody updates.
pub const ABI_VERSION: u32 = 10;

/// The peer tag an embedder gets when it never asks for one: the single client
/// a segment has always had.
pub const DEFAULT_PEER: u32 = 0;

/// Byte capacity of each ring (a power of two).
pub const RING_CAPACITY: usize = 64 * 1024;

/// Bytes before a ring frame's payload: its length, then its peer tag.
pub const FRAME_HEADER: usize = 8;

/// Default audio-tap count (`--taps`).
pub const DEFAULT_TAPS: usize = 8;

/// Default per-tap ring capacity in samples (`--tap-frames`): a power of two,
/// ~341 ms at 48 kHz — comfortably more than twice any oscilloscope window.
pub const DEFAULT_TAP_FRAMES: usize = 16384;

/// Each tap slot starts 64-byte aligned: the cursor gets its own cache line
/// (the audio thread stores it every block; readers poll it), and the sample
/// ring follows without straddling the cursor's line.
pub const TAP_ALIGN: usize = 64;

/// Audio-bus slots the bus region is always sized for. The region is two words
/// per bus and 1 KiB in total, small enough that making it a runtime parameter
/// would buy nothing — so it is the server's compile-time cap, and the server
/// asserts the two agree.
pub const AUDIO_BUS_SLOTS: usize = 128;

/// The engine's block size, which the tap rings are written in whole multiples
/// of. Here so a reader can check a window without linking the engine; the
/// server asserts the two agree.
pub const BLOCK: usize = 64;

/// Directory rows a segment gets when nobody says otherwise — the server's own
/// default buffer count, so a segment created with no `--max-buffers` describes
/// every buffer it can allocate.
pub const DEFAULT_BUFFER_ROWS: usize = 4096;

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
    /// Audio-tap count and per-tap ring capacity in samples; the tap region
    /// trails the audio-bus one.
    taps: u32,
    tap_frames: u32,
    /// Audio-bus count: the length of the per-bus directory and level table
    /// between the control slots and the tap region.
    audio_buses: u32,
    /// **Who serves the command plane** — the pid that claimed the rings, or 0
    /// while they are free. It occupies the word that kept `transport_clock`
    /// 8-byte aligned, so it is a meaning given to space that was already there
    /// rather than a field anything had to move for.
    ///
    /// A segment has **one** ring pair and it is SPSC, so exactly one process
    /// may drain the inbound one: a second server that also popped it would
    /// steal half the commands, silently. See [`View::claim_control`].
    control_owner: AtomicU32,
    /// Samples elapsed *under the transport*, frozen while it is stopped. The
    /// sample clock above never stops, so a reader pacing on the device wants
    /// that one — but this one is monotonic too, which is what a scheduler
    /// needs and what a **playhead does not**: for where the piece *is*, read
    /// `transport_position`.
    transport_clock: AtomicU64,
    /// The sample of the *piece* the engine is playing, in the piece's own
    /// axis. It advances with the clock while rolling, holds while stopped,
    /// jumps on a locate and wraps at a loop's end. A playhead wants this one;
    /// a scheduled bundle wants the clock above.
    ///
    /// This spends the last of the header's reserved space.
    transport_position: AtomicU64,
}

#[repr(C)]
struct Ring {
    /// Write cursor, owned by the producer. Monotonic, wraps at `u32::MAX`
    /// (the capacity divides 2³², so the modulo arithmetic stays consistent).
    head: AtomicU32,
    /// Read cursor, owned by the consumer.
    tail: AtomicU32,
    _pad: [u32; 14],
    data: [u8; RING_CAPACITY],
}

/// The fixed prefix: the header and the two rings. Everything else is a
/// trailing region whose offset is derived from the header.
#[repr(C)]
struct Layout {
    header: Header,
    /// Client → server commands.
    c2s: Ring,
    /// Server → client replies.
    s2c: Ring,
}

/// One pool buffer, as a peer finds it: what shape it is, and **which** buffer
/// it is.
///
/// The `generation` does three jobs with one number, which is why it is the
/// only field that is not a shape. It is **odd while a buffer is live** and
/// even when the slot is empty. It **names the region file**, so a freed buffer
/// and its replacement can never share a name and a stale mapping can never be
/// aliased onto new samples. And it is a **seqlock**: a writer bumps it,
/// writes the shape and bumps it again, so a reader that sees it move between
/// its two loads knows it read a torn row and re-reads.
#[repr(C)]
struct BufferRow {
    generation: AtomicU64,
    frames: AtomicU32,
    channels: AtomicU32,
    sample_rate_bits: AtomicU64,
    /// **The write frontier**: the highest frame any writer has filled, or 0.
    ///
    /// It is a *hint* and not a promise. A buffer may have several writers and
    /// nothing here says which of them wrote what, or that everything before
    /// it is final — what it answers is the one question a picture of a
    /// recording has to ask and cannot otherwise: how far does the samples go
    /// now. Outside the seqlock deliberately: it moves every block while the
    /// shape does not move at all, and folding it into the row's version would
    /// spin every reader of the shape against a recorder.
    frontier: AtomicU64,
}

/// Which end of the ring pair a peer holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Server,
    Client,
}

/// Byte offset of the control-bus array: straight after the fixed prefix.
pub const fn controls_offset() -> usize {
    size_of::<Layout>()
}

/// Byte offset of the **audio-bus region**: the control slots' end. Two arrays
/// of one word per audio bus follow, the bus → tap directory then the levels.
pub const fn bus_region_offset(control_buses: usize) -> usize {
    controls_offset() + control_buses * size_of::<AtomicU32>()
}

/// Byte offset of the tap region: the audio-bus region's end, rounded up to
/// [`TAP_ALIGN`] so the first tap cursor is cache-line aligned.
pub const fn tap_region_offset(control_buses: usize) -> usize {
    let end = bus_region_offset(control_buses) + 2 * AUDIO_BUS_SLOTS * size_of::<AtomicU32>();
    end.div_ceil(TAP_ALIGN) * TAP_ALIGN
}

/// Byte size of one tap slot: a cache line for the cursor, then the ring.
pub const fn tap_slot_size(tap_frames: usize) -> usize {
    TAP_ALIGN + tap_frames * size_of::<f32>()
}

/// Byte offset of the **buffer directory**: the tap region's end, and the
/// segment's tail.
pub const fn buffer_region_offset(control_buses: usize, taps: usize, tap_frames: usize) -> usize {
    tap_region_offset(control_buses) + taps * tap_slot_size(tap_frames)
}

/// Bytes of one directory row.
pub const fn buffer_row_size() -> usize {
    size_of::<BufferRow>()
}

/// Total byte size of a segment carrying `control_buses` control slots, the
/// fixed audio-bus region, `taps` rings of `tap_frames` samples, and a
/// directory of `buffers` rows.
pub const fn segment_size(
    control_buses: usize,
    taps: usize,
    tap_frames: usize,
    buffers: usize,
) -> usize {
    buffer_region_offset(control_buses, taps, tap_frames) + buffers * size_of::<BufferRow>()
}

/// Default segment size (the default counts).
pub const SEGMENT_SIZE: usize =
    segment_size(16384, DEFAULT_TAPS, DEFAULT_TAP_FRAMES, DEFAULT_BUFFER_ROWS);

/// The next **odd** number past `counter`: a slot goes live on an odd
/// generation and empty on an even one, and every allocation takes a fresh one.
fn next_odd(counter: u64) -> u64 {
    counter.wrapping_add(if counter.is_multiple_of(2) { 1 } else { 2 })
}

/// The **shape** of a mapped segment, derived from its header once: how many of
/// each thing it holds, and where each region starts.
///
/// A peer that cannot call into this crate — a `ctypes` client reading the
/// mapping with `memoryview` — asks for this and does its own arithmetic
/// against *these* numbers instead of recomputing the layout, which is the
/// whole point.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shape {
    pub control_buses: u64,
    pub audio_buses: u64,
    pub taps: u64,
    pub tap_frames: u64,
    pub buffer_rows: u64,
    pub controls_offset: u64,
    pub buses_offset: u64,
    pub taps_offset: u64,
    pub buffers_offset: u64,
    /// Where each live header counter sits. They are constants of the layout
    /// rather than of a segment, and they are here for the same reason the
    /// region offsets are: a foreign reader should ask for every number it
    /// needs, not carry half of them and pin the other half.
    pub sample_rate_offset: u64,
    pub clock_offset: u64,
    pub transport_clock_offset: u64,
    pub transport_position_offset: u64,
    /// Bytes of one directory row, and of one tap slot: what a foreign reader
    /// strides by.
    pub buffer_row_size: u64,
    pub tap_slot_size: u64,
    /// The ring pair: where each one starts, its capacity, and the bytes each
    /// frame carries before its payload.
    pub c2s_offset: u64,
    pub s2c_offset: u64,
    pub ring_capacity: u64,
    pub ring_prefix: u64,
    pub frame_header: u64,
}

/// A mapped segment: the address, its length, and everything either end does
/// with it.
///
/// The accessors the **audio thread** reaches — the levels it publishes per
/// bus per block, the tap ring it appends to, the clocks it stores — are
/// `#[inline]`, because they were inlined inside one crate before this module
/// existed and a cross-crate call per bus per block is a cost the move should
/// not have introduced.
///
/// It owns no memory and frees none — whoever mapped it keeps it alive, which
/// is the one thing this type trusts its caller for. Every access is an atomic
/// load or store on a word the layout above places, so a `View` is safe to
/// share and safe to read while another process writes: per-cell atomicity, no
/// ordering between cells, a reader crossing a writer seeing some old and some
/// new. That is the contract the whole data plane has always had.
pub struct View {
    base: *mut u8,
    len: usize,
    shape: Shape,
}

// SAFETY: every field this reaches is an atomic, and the memory is kept alive
// by whoever mapped it for at least as long as the view.
unsafe impl Send for View {}
unsafe impl Sync for View {}

impl View {
    /// Derives a view over memory that is **already a segment**, validating the
    /// magic, the version and the size.
    ///
    /// # Safety
    ///
    /// `base` must point at `len` readable, writable bytes that stay mapped and
    /// unmoved for as long as the view lives.
    pub unsafe fn attach(base: *mut u8, len: usize) -> Result<Self, &'static str> {
        if len < size_of::<Layout>() {
            return Err("segment size is smaller than its fixed prefix");
        }
        // SAFETY: the caller's range covers the prefix, checked above.
        let header = unsafe { &*(base as *const Header) };
        if header.magic != MAGIC {
            return Err("not a clausters segment (bad magic)");
        }
        if header.abi_version != ABI_VERSION {
            return Err("segment ABI version mismatch");
        }
        let shape = Self::shape_of(header, len)?;
        Ok(Self { base, len, shape })
    }

    /// Writes a fresh header over `base` and returns the view of it. For
    /// whoever **creates** a segment: it runs before any other process can have
    /// attached, which is what makes the plain writes sound.
    ///
    /// # Safety
    ///
    /// As [`Self::attach`], and `len` must be at least
    /// [`segment_size`] for the counts given.
    pub unsafe fn init(
        base: *mut u8,
        len: usize,
        control_buses: usize,
        taps: usize,
        tap_frames: usize,
    ) -> Self {
        // SAFETY: the caller's range covers the prefix.
        let header = unsafe { &mut *(base as *mut Header) };
        header.ring_capacity = RING_CAPACITY as u32;
        header.control_buses = control_buses as u32;
        header.taps = taps as u32;
        header.tap_frames = tap_frames as u32;
        header.audio_buses = AUDIO_BUS_SLOTS as u32;
        // Nobody serves the rings yet: creating a segment is not claiming it,
        // because the process that creates one is not always the one that
        // serves it (an editor creates, its session serves).
        *header.control_owner.get_mut() = 0;
        header.abi_version = ABI_VERSION;
        // Written last: a peer that sees the magic sees a full header.
        header.magic = MAGIC;
        let shape = Self::shape_of(header, len).expect("a segment we just wrote");
        let view = Self { base, len, shape };
        // A zeroed directory would read as "every bus is recorded by tap 0";
        // `-1` is the absent marker `/bus_tap` uses. The levels are fine
        // zeroed: those bits are `0.0`, which is silence.
        for bus in 0..AUDIO_BUS_SLOTS {
            view.set_tap_of_bus(bus, None);
        }
        view
    }

    fn shape_of(header: &Header, len: usize) -> Result<Shape, &'static str> {
        let control_buses = header.control_buses as usize;
        let taps = header.taps as usize;
        let tap_frames = header.tap_frames as usize;
        let buffers_offset = buffer_region_offset(control_buses, taps, tap_frames);
        // The mapped length must cover every region the header claims, plus
        // whole directory rows — the one region whose count is the segment's
        // own length rather than a field.
        if len < buffers_offset || !(len - buffers_offset).is_multiple_of(size_of::<BufferRow>()) {
            return Err("segment size does not match its header");
        }
        Ok(Shape {
            control_buses: control_buses as u64,
            audio_buses: header.audio_buses as u64,
            taps: taps as u64,
            tap_frames: tap_frames as u64,
            buffer_rows: ((len - buffers_offset) / size_of::<BufferRow>()) as u64,
            controls_offset: controls_offset() as u64,
            buses_offset: bus_region_offset(control_buses) as u64,
            taps_offset: tap_region_offset(control_buses) as u64,
            buffers_offset: buffers_offset as u64,
            sample_rate_offset: std::mem::offset_of!(Header, sample_rate_bits) as u64,
            clock_offset: std::mem::offset_of!(Header, sample_clock) as u64,
            transport_clock_offset: std::mem::offset_of!(Header, transport_clock) as u64,
            transport_position_offset: std::mem::offset_of!(Header, transport_position) as u64,
            buffer_row_size: size_of::<BufferRow>() as u64,
            tap_slot_size: tap_slot_size(tap_frames) as u64,
            c2s_offset: std::mem::offset_of!(Layout, c2s) as u64,
            s2c_offset: std::mem::offset_of!(Layout, s2c) as u64,
            ring_capacity: RING_CAPACITY as u64,
            ring_prefix: std::mem::offset_of!(Ring, data) as u64,
            frame_header: FRAME_HEADER as u64,
        })
    }

    /// What this segment holds and where: the numbers a foreign reader needs.
    pub fn shape(&self) -> Shape {
        self.shape
    }

    /// The address this view was built over, and its length.
    pub fn base(&self) -> *const u8 {
        self.base
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn header(&self) -> &Header {
        // SAFETY: validated at construction and kept alive by the caller.
        unsafe { &*(self.base as *const Header) }
    }

    fn layout(&self) -> &Layout {
        // SAFETY: as `header`.
        unsafe { &*(self.base as *const Layout) }
    }

    // ---- the clocks ----

    pub fn sample_rate(&self) -> f64 {
        f64::from_bits(self.header().sample_rate_bits.load(Ordering::Relaxed))
    }

    pub fn set_sample_rate(&self, rate: f64) {
        self.header()
            .sample_rate_bits
            .store(rate.to_bits(), Ordering::Relaxed);
    }

    /// The device clock: samples processed since boot, never stopping.
    #[inline]
    pub fn clock(&self) -> &AtomicU64 {
        &self.header().sample_clock
    }

    /// Samples elapsed under the transport, held while it is stopped.
    #[inline]
    pub fn transport_clock(&self) -> &AtomicU64 {
        &self.header().transport_clock
    }

    /// Where the piece *is*: the position a playhead draws.
    #[inline]
    pub fn transport_position(&self) -> &AtomicU64 {
        &self.header().transport_position
    }

    // ---- the control plane's owner ----

    /// Claims the command plane for `pid`, returning whether it got it.
    ///
    /// `alive` decides whether an existing claim still stands: a pid nothing
    /// answers to is stale and is taken over, which is what makes killing a
    /// server recoverable rather than terminal. Asking the *operating system*
    /// whether a process exists is the caller's job — this crate has no
    /// business knowing what a process is.
    pub fn claim_control(&self, pid: u32, alive: impl Fn(u32) -> bool) -> bool {
        let owner = &self.header().control_owner;
        loop {
            let held = owner.load(Ordering::Acquire);
            // A live holder refuses whoever asks second, this process
            // included: one ring pair, one drainer.
            if held != 0 && alive(held) {
                return false;
            }
            match owner.compare_exchange(held, pid, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return true,
                Err(_) => continue,
            }
        }
    }

    /// Gives the command plane back, if `pid` holds it.
    pub fn release_control(&self, pid: u32) {
        let _ = self.header().control_owner.compare_exchange(
            pid,
            0,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    /// The pid serving the command plane, or `None` while it is free.
    pub fn control_owner(&self) -> Option<u32> {
        match self.header().control_owner.load(Ordering::Acquire) {
            0 => None,
            pid => Some(pid),
        }
    }

    // ---- the control buses ----

    pub fn control_bus_count(&self) -> usize {
        self.shape.control_buses as usize
    }

    /// The control-bus array — *the* buses, the words `InCtl` reads.
    #[inline]
    pub fn controls(&self) -> &[AtomicU32] {
        // SAFETY: the region is `control_buses` words at `controls_offset`,
        // inside the mapping the shape was derived from.
        unsafe {
            std::slice::from_raw_parts(
                self.base.add(self.shape.controls_offset as usize) as *const AtomicU32,
                self.shape.control_buses as usize,
            )
        }
    }

    #[inline]
    pub fn control(&self, index: usize) -> f32 {
        self.controls()
            .get(index)
            .map_or(0.0, |cell| f32::from_bits(cell.load(Ordering::Relaxed)))
    }

    #[inline]
    pub fn set_control(&self, index: usize, value: f32) {
        if let Some(cell) = self.controls().get(index) {
            cell.store(value.to_bits(), Ordering::Relaxed);
        }
    }

    // ---- the audio-bus region ----

    pub fn audio_buses(&self) -> usize {
        self.shape.audio_buses as usize
    }

    fn bus_tap_cell(&self, bus: usize) -> Option<&AtomicI32> {
        (bus < self.audio_buses()).then(|| {
            // SAFETY: the directory is one word per bus at `buses_offset`.
            unsafe {
                &*(self.base.add(self.shape.buses_offset as usize) as *const AtomicI32).add(bus)
            }
        })
    }

    #[inline]
    fn bus_level_cell(&self, bus: usize) -> Option<&AtomicU32> {
        let count = self.audio_buses();
        (bus < count).then(|| {
            // SAFETY: the levels follow the directory, one word per bus.
            unsafe {
                &*(self.base.add(self.shape.buses_offset as usize) as *const AtomicU32)
                    .add(count + bus)
            }
        })
    }

    /// Which sample ring, if any, is recording audio bus `bus`.
    pub fn tap_of_bus(&self, bus: usize) -> Option<usize> {
        match self.bus_tap_cell(bus)?.load(Ordering::Acquire) {
            i if i >= 0 => Some(i as usize),
            _ => None,
        }
    }

    /// Publishes (or clears, with `None`) the ring recording audio bus `bus`.
    pub fn set_tap_of_bus(&self, bus: usize, tap: Option<usize>) {
        if let Some(cell) = self.bus_tap_cell(bus) {
            cell.store(tap.map_or(-1, |i| i as i32), Ordering::Release);
        }
    }

    /// Audio bus `bus`'s level: the peak magnitude of the engine's last block,
    /// or `0.0` where the bus is silent or out of range. What a meter reads —
    /// one number per block instead of a ring, so metering every bus costs no
    /// tap at all.
    #[inline]
    pub fn level(&self, bus: usize) -> f32 {
        self.bus_level_cell(bus)
            .map_or(0.0, |cell| f32::from_bits(cell.load(Ordering::Relaxed)))
    }

    /// Publishes audio bus `bus`'s block level. **Audio-thread safe**: one
    /// relaxed store, no allocation, no lock.
    #[inline]
    pub fn set_level(&self, bus: usize, peak: f32) {
        if let Some(cell) = self.bus_level_cell(bus) {
            cell.store(peak.to_bits(), Ordering::Relaxed);
        }
    }

    // ---- the audio taps ----

    pub fn taps(&self) -> usize {
        self.shape.taps as usize
    }

    pub fn tap_frames(&self) -> usize {
        self.shape.tap_frames as usize
    }

    #[inline]
    fn tap_slot_ptr(&self, i: usize) -> *const u8 {
        debug_assert!(i < self.taps());
        let at = self.shape.taps_offset as usize + i * tap_slot_size(self.tap_frames());
        // SAFETY: the shape was derived from a length covering the tap region.
        unsafe { self.base.add(at) }
    }

    /// Tap `i`'s cursor: total samples ever written (monotonic). The ring holds
    /// samples `[cursor - tap_frames, cursor)`.
    #[inline]
    fn tap_cursor(&self, i: usize) -> &AtomicU64 {
        // SAFETY: the slot starts 64-byte aligned and its first word is the
        // cursor.
        unsafe { &*(self.tap_slot_ptr(i) as *const AtomicU64) }
    }

    #[inline]
    fn tap_data_ptr(&self, i: usize) -> *const f32 {
        // SAFETY: the ring starts one alignment line into the slot.
        unsafe { self.tap_slot_ptr(i).add(TAP_ALIGN) as *const f32 }
    }

    /// Appends one block of samples to tap `i`'s ring. **Audio-thread safe**:
    /// one `memcpy` plus one Release store, no allocation, no lock. Single
    /// writer only. The block never wraps: the ring capacity is a power of two
    /// ≥ the block size and the cursor only advances by whole blocks, so every
    /// write lands block-aligned inside the ring.
    #[inline]
    pub fn tap_write(&self, i: usize, samples: &[f32]) {
        if i >= self.taps() {
            return;
        }
        let frames = self.tap_frames();
        debug_assert!(samples.len() == BLOCK && frames.is_multiple_of(BLOCK));
        let cursor = self.tap_cursor(i).load(Ordering::Relaxed);
        let start = (cursor as usize) % frames;
        // SAFETY: `start + BLOCK <= frames` (both multiples of the block
        // size), single writer, region sized by the constructor.
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
    /// end — `None` when the tap index is out of range, the window is empty or
    /// larger than half the ring, or the tap has not yet written a full window.
    ///
    /// The half-ring cap makes a torn read need the writer to lap half the ring
    /// during one `memcpy`; the cursor double-check turns "impossible in
    /// practice" into checked, and on the rare tear it retries with the fresh
    /// cursor.
    pub fn tap_read_latest(&self, i: usize, out: &mut [f32]) -> Option<u64> {
        let frames = self.tap_frames();
        let want = out.len();
        if i >= self.taps() || want == 0 || frames == 0 || want > frames / 2 {
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
            // SAFETY: both copies stay inside the ring; a concurrent writer's
            // overlap is detected by the cursor re-check below.
            unsafe {
                std::ptr::copy_nonoverlapping(data.add(s), out.as_mut_ptr(), first);
                std::ptr::copy_nonoverlapping(data, out.as_mut_ptr().add(first), want - first);
            }
            if self.tap_cursor(i).load(Ordering::Acquire) - start <= frames as u64 {
                return Some(end);
            }
        }
    }

    // ---- the buffer directory ----

    pub fn buffer_rows(&self) -> usize {
        self.shape.buffer_rows as usize
    }

    fn row(&self, bufnum: usize) -> Option<&BufferRow> {
        (bufnum < self.buffer_rows()).then(|| {
            // SAFETY: the directory is `buffer_rows` rows at `buffers_offset`.
            unsafe {
                &*(self.base.add(self.shape.buffers_offset as usize) as *const BufferRow)
                    .add(bufnum)
            }
        })
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
        let row = self.row(bufnum)?;
        let odd = next_odd(row.generation.load(Ordering::Relaxed));
        row.generation.store(odd.wrapping_sub(1), Ordering::Release);
        row.frames.store(frames as u32, Ordering::Relaxed);
        row.channels.store(channels as u32, Ordering::Relaxed);
        row.sample_rate_bits
            .store(sample_rate.to_bits(), Ordering::Relaxed);
        // A new take starts unwritten: the frontier is the *buffer's*, so
        // the previous tenant's would claim samples this one never got.
        row.frontier.store(0, Ordering::Relaxed);
        row.generation.store(odd, Ordering::Release);
        Some(odd)
    }

    /// Marks a slot empty. The region's file is unlinked by whoever owns it;
    /// this is what a peer reads to learn that what it holds is history.
    pub fn retire_buffer(&self, bufnum: usize) {
        let Some(row) = self.row(bufnum) else { return };
        let counter = row.generation.load(Ordering::Relaxed);
        if !counter.is_multiple_of(2) {
            row.generation
                .store(counter.wrapping_add(1), Ordering::Release);
        }
    }

    /// What a peer needs to map buffer `bufnum`: its generation (which names
    /// the region) and its shape — or `None` when the slot is empty or out of
    /// range. Read under the generation twice, so a row caught mid-write is
    /// re-read rather than believed.
    pub fn buffer_info(&self, bufnum: usize) -> Option<BufferShape> {
        let row = self.row(bufnum)?;
        for _ in 0..8 {
            let before = row.generation.load(Ordering::Acquire);
            if before.is_multiple_of(2) {
                return None; // an empty slot
            }
            let frames = row.frames.load(Ordering::Relaxed) as usize;
            let channels = row.channels.load(Ordering::Relaxed) as usize;
            let sample_rate = f64::from_bits(row.sample_rate_bits.load(Ordering::Relaxed));
            if row.generation.load(Ordering::Acquire) == before {
                return Some(BufferShape {
                    generation: before,
                    frames,
                    channels,
                    sample_rate,
                });
            }
        }
        None
    }

    // ---- the command plane ----

    fn inbound(&self, role: Role) -> &Ring {
        match role {
            Role::Server => &self.layout().c2s,
            Role::Client => &self.layout().s2c,
        }
    }

    fn outbound(&self, role: Role) -> &Ring {
        match role {
            Role::Server => &self.layout().s2c,
            Role::Client => &self.layout().c2s,
        }
    }

    /// Appends one packet to the outbound ring, tagged for `peer` — who wrote
    /// it (client → server) or who it is for (server → client). `false` when
    /// the ring lacks space: backpressure, and the caller may retry, since
    /// nothing was dropped.
    pub fn push(&self, role: Role, peer: u32, packet: &[u8]) -> bool {
        let ring = self.outbound(role);
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
    /// and its length. A malformed length — a hostile or crashed peer — drops
    /// the whole ring contents and returns `None`, which is the resync.
    pub fn try_pop(&self, role: Role, buf: &mut [u8]) -> Option<(u32, usize)> {
        let ring = self.inbound(role);
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
            // Garbage, or a packet bigger than the caller's buffer: resync by
            // discarding everything buffered.
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

/// A buffer as its directory row describes it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BufferShape {
    /// Odd, and the number the region file is named with.
    pub generation: u64,
    pub frames: usize,
    pub channels: usize,
    pub sample_rate: f64,
}

impl View {
    /// **How far a buffer has been written**: the frontier its writers
    /// published, in frames. Zero for a buffer nobody has recorded into, which
    /// is every buffer that arrived whole.
    pub fn buffer_frontier(&self, bufnum: usize) -> Option<u64> {
        Some(self.row(bufnum)?.frontier.load(Ordering::Relaxed))
    }

    /// Raises the frontier of `bufnum` to `frame` — **the highest wins**, so
    /// two writers on one buffer cannot pull it backwards and a looping
    /// recorder does not either.
    ///
    /// Called from the audio thread, so it is one relaxed read-modify-write
    /// and nothing else: no allocation, no lock, and no ordering with the
    /// samples it describes (a reader crossing a write sees some old and some
    /// new, which is what the cells promise anyway).
    pub fn raise_buffer_frontier(&self, bufnum: usize, frame: u64) {
        if let Some(row) = self.row(bufnum) {
            row.frontier.fetch_max(frame, Ordering::Relaxed);
        }
    }
}

/// The suffix a buffer's region file carries beside the segment's own path:
/// the buffer number and the generation, so a freed buffer's file and its
/// replacement can never share a name.
///
/// Here rather than in whoever maps it because **three processes name the same
/// file**, and a name computed twice is a name that can differ.
pub fn region_suffix(bufnum: usize, generation: u64) -> String {
    format!(".buf{bufnum}.{generation}")
}

fn write_ring(ring: &Ring, at: u32, bytes: &[u8]) {
    let data = ring.data.as_ptr() as *mut u8;
    let start = at as usize % RING_CAPACITY;
    let first = bytes.len().min(RING_CAPACITY - start);
    // SAFETY: SPSC — only the producer writes between head and tail, and the
    // range was checked to fit before calling.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A segment on the heap, aligned like a mapping: `u128` words, so the
    /// 8-byte atomics inside are aligned.
    struct Heap(Box<[u128]>, usize);

    impl Heap {
        fn new(control_buses: usize, taps: usize, tap_frames: usize, rows: usize) -> Self {
            let size = segment_size(control_buses, taps, tap_frames, rows);
            Heap(vec![0u128; size.div_ceil(16)].into_boxed_slice(), size)
        }
        fn view(&mut self, control_buses: usize, taps: usize, tap_frames: usize) -> View {
            let (ptr, len) = (self.0.as_mut_ptr() as *mut u8, self.1);
            unsafe { View::init(ptr, len, control_buses, taps, tap_frames) }
        }
        fn attach(&mut self) -> Result<View, &'static str> {
            let (ptr, len) = (self.0.as_mut_ptr() as *mut u8, self.1);
            unsafe { View::attach(ptr, len) }
        }
    }

    #[test]
    fn a_fresh_segment_attaches_and_reports_its_shape() {
        let mut heap = Heap::new(64, 2, 128, 4);
        let shape = heap.view(64, 2, 128).shape();
        assert_eq!(shape.control_buses, 64);
        assert_eq!(shape.taps, 2);
        assert_eq!(shape.tap_frames, 128);
        assert_eq!(shape.buffer_rows, 4);
        assert_eq!(shape.controls_offset, controls_offset() as u64);
        assert_eq!(shape.buses_offset, bus_region_offset(64) as u64);
        assert_eq!(shape.taps_offset, tap_region_offset(64) as u64);
        // Attaching again derives the same shape from the header alone, which
        // is the whole promise a second process relies on.
        assert_eq!(heap.attach().map(|v| v.shape()), Ok(shape));
    }

    #[test]
    fn a_segment_that_is_not_one_is_refused() {
        let mut heap = Heap::new(64, 0, 128, 1);
        assert_eq!(
            heap.attach().err(),
            Some("not a clausters segment (bad magic)")
        );
        heap.view(64, 0, 128);
        assert!(heap.attach().is_ok());
        // A length that is not the header's regions plus whole rows.
        let (ptr, len) = (heap.0.as_mut_ptr() as *mut u8, heap.1);
        assert!(unsafe { View::attach(ptr, len - 1) }.is_err());
    }

    #[test]
    fn the_buses_the_taps_and_the_levels_read_back() {
        let mut heap = Heap::new(8, 1, 128, 2);
        let view = heap.view(8, 1, 128);
        view.set_control(3, -0.25);
        assert_eq!(view.control(3), -0.25);
        assert_eq!(view.control(99), 0.0, "out of range reads silence");

        assert_eq!(view.tap_of_bus(2), None, "a fresh segment records nothing");
        view.set_tap_of_bus(2, Some(0));
        assert_eq!(view.tap_of_bus(2), Some(0));
        view.set_level(2, 0.5);
        assert_eq!(view.level(2), 0.5);

        let block = [0.75f32; BLOCK];
        view.tap_write(0, &block);
        let mut out = [0.0f32; 16];
        assert_eq!(view.tap_read_latest(0, &mut out), Some(BLOCK as u64));
        assert!(out.iter().all(|&s| s == 0.75));
    }

    #[test]
    fn a_frontier_only_rises_and_a_new_take_starts_at_none() {
        let mut heap = Heap::new(8, 0, 128, 3);
        let view = heap.view(8, 0, 128);
        let first = view.publish_buffer(1, 480_000, 1, 48_000.0).expect("a row");
        assert_eq!(view.buffer_frontier(1), Some(0), "nothing recorded yet");

        view.raise_buffer_frontier(1, 1_024);
        view.raise_buffer_frontier(1, 4_096);
        // A second writer behind the first, and a looping recorder that wrapped:
        // neither pulls the samples back.
        view.raise_buffer_frontier(1, 512);
        assert_eq!(view.buffer_frontier(1), Some(4_096));

        // The row's shape is unaffected by the frontier moving under it.
        assert_eq!(
            view.buffer_info(1).map(|s| (s.generation, s.frames)),
            Some((first, 480_000))
        );

        // A new take in the same slot is unwritten, whatever the last one did.
        view.publish_buffer(1, 96_000, 2, 48_000.0).expect("a row");
        assert_eq!(view.buffer_frontier(1), Some(0));
        assert_eq!(view.buffer_frontier(9), None, "no such row");
    }

    #[test]
    fn a_directory_row_is_live_while_its_generation_is_odd() {
        let mut heap = Heap::new(8, 0, 128, 3);
        let view = heap.view(8, 0, 128);
        assert_eq!(view.buffer_info(0), None);
        let first = view.publish_buffer(0, 64, 2, 44_100.0).expect("a row");
        assert!(!first.is_multiple_of(2));
        assert_eq!(
            view.buffer_info(0),
            Some(BufferShape {
                generation: first,
                frames: 64,
                channels: 2,
                sample_rate: 44_100.0,
            })
        );
        view.retire_buffer(0);
        assert_eq!(view.buffer_info(0), None);
        // A new allocation takes a new generation, so a stale mapping can
        // never be aliased onto new samples.
        let next = view.publish_buffer(0, 8, 1, 48_000.0).expect("a row");
        assert!(next > first);
        assert_ne!(region_suffix(0, first), region_suffix(0, next));
        assert_eq!(view.publish_buffer(3, 8, 1, 48_000.0), None, "no such row");
    }

    #[test]
    fn the_rings_carry_packets_both_ways_and_resync_on_garbage() {
        let mut heap = Heap::new(8, 0, 128, 1);
        let view = heap.view(8, 0, 128);
        let mut buf = [0u8; 64];
        assert_eq!(view.try_pop(Role::Server, &mut buf), None);
        assert!(view.push(Role::Client, 7, b"hello"));
        assert_eq!(view.try_pop(Role::Server, &mut buf), Some((7, 5)));
        assert_eq!(&buf[..5], b"hello");
        // The other direction is the other ring, not the same one.
        assert!(view.push(Role::Server, 1, b"reply"));
        assert_eq!(view.try_pop(Role::Server, &mut buf), None);
        assert_eq!(view.try_pop(Role::Client, &mut buf), Some((1, 5)));
        // A packet larger than the reader's buffer resyncs rather than wedges.
        assert!(view.push(Role::Client, 0, &[0u8; 128]));
        let mut small = [0u8; 8];
        assert_eq!(view.try_pop(Role::Server, &mut small), None);
        assert_eq!(view.try_pop(Role::Server, &mut buf), None, "ring emptied");
    }

    #[test]
    fn a_claim_is_taken_from_a_holder_that_is_gone() {
        let mut heap = Heap::new(8, 0, 128, 1);
        let view = heap.view(8, 0, 128);
        assert_eq!(view.control_owner(), None);
        assert!(view.claim_control(11, |_| true));
        assert_eq!(view.control_owner(), Some(11));
        assert!(!view.claim_control(22, |_| true), "a live holder refuses");
        assert!(view.claim_control(22, |_| false), "a dead one does not");
        assert_eq!(view.control_owner(), Some(22));
        view.release_control(11);
        assert_eq!(view.control_owner(), Some(22), "only the holder releases");
        view.release_control(22);
        assert_eq!(view.control_owner(), None);
    }
}
