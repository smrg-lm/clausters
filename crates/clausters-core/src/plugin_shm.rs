//! Versioned per-instance plugin audio plane, independent of the server IPC ABI.
//!
//! Allocation/mapping and administrative control stay outside this module.
//! A producer only copies preallocated data and accesses lock-free atomics.
//! One producer and one consumer may attach to a segment; memory must remain
//! mapped until both endpoints have stopped. Late audio never moves in time.

use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::mem::{align_of, size_of};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Plugin segment ABI, checked independently of the embed and core C ABIs.
pub const PLUGIN_ABI_VERSION: u32 = 1;
/// Engine frames per published job.
pub const FRAMES: usize = 64;
/// Fixed slot count: no reuse while a helper owns a slot.
pub const SLOTS: usize = 32;
/// Maximum events per job, result and frozen-event backlog.
pub const EVENTS: usize = 16;
/// Maximum transport/active spans per job.
pub const SPANS: usize = 8;
/// Maximum total channels per direction, negotiated before activation.
pub const CHANNELS: u32 = 32;
/// Initial pipeline delay selected by the feasibility experiment.
pub const DEFAULT_DEPTH: u32 = 4;
const MAGIC: u32 = 0x504c_434c;
const FREE: u32 = 0;
const READY: u32 = 1;
const BUSY: u32 = 2;
const DONE: u32 = 3;

/// Latched failure reason; a failed producer emits silence without submitting.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// No failure.
    None = 0,
    /// A bounded queue, event list or negotiated payload was exceeded.
    Capacity = 1,
    /// Eight consecutive missing results (including process death/hang).
    Starved = 2,
    /// Plugin processing failed or returned an invalid result.
    Process = 3,
    /// Segment, sequence or generation mismatch.
    Protocol = 4,
}

/// Immutable segment configuration. Samples are channel-major, 64 per channel.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    /// Nonzero incarnation identifier, never changed in a live mapping.
    pub generation: u64,
    /// Actual processing sample rate.
    pub sample_rate: f64,
    /// Device sample origin for relative job sequence zero.
    pub sample_origin: u64,
    /// Total input channels over all ports.
    pub inputs: u32,
    /// Total output channels over all ports.
    pub outputs: u32,
    /// Pipeline delay in complete engine blocks, 1 through 16.
    pub depth: u32,
    /// Reserved; must be zero.
    pub reserved: u32,
}
impl Config {
    /// Validate capacities and arithmetic before allocating or attaching.
    pub fn valid(self) -> bool {
        self.generation != 0
            && self.sample_rate.is_finite()
            && self.sample_rate > 0.0
            && self.inputs <= CHANNELS
            && self.outputs > 0
            && self.outputs <= CHANNELS
            && (1..=16).contains(&self.depth)
            && self.reserved == 0
            && self.sample_origin <= i64::MAX as u64 - FRAMES as u64
    }
    /// Mapped byte count for this exact configuration, including alignment.
    pub fn bytes(self) -> Option<usize> {
        self.valid()
            .then(|| size_of::<Header>() + SLOTS * self.stride())
    }
    fn stride(self) -> usize {
        let bytes = size_of::<Slot>() + (self.inputs + self.outputs) as usize * FRAMES * 4;
        (bytes + 63) & !63
    }
}

/// Pointer-free event type, shared by all native format adapters.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    /// Note attack.
    NoteOn = 1,
    /// Note release.
    NoteOff = 2,
    /// Immediate note release.
    NoteChoke = 3,
    /// Parameter in its native plain-value units.
    Parameter = 4,
    /// Three-byte MIDI message.
    Midi = 5,
    /// Per-note expression, identified by `id`.
    Expression = 6,
    /// Native note ownership ended (plugin output).
    NoteEnd = 7,
    /// Native parameter modulation.
    ParameterMod = 8,
    /// Native parameter edit gesture began.
    GestureBegin = 9,
    /// Native parameter edit gesture ended.
    GestureEnd = 10,
}
/// An event contains values only: native pointers/cookies are resolved by helpers.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Event {
    /// Sample offset within the engine block (rebased by the adapter per span).
    pub offset: u32,
    /// Numeric `EventKind`; unknown values are rejected before processing.
    pub kind: u32,
    /// Stable native parameter/expression identity, or packed MIDI bytes.
    pub id: u32,
    /// Native note identity; -1 means wildcard when the dialect permits it.
    pub note_id: i32,
    /// Event port, or -1 for all ports.
    pub port: i16,
    /// Channel, or -1 for all channels.
    pub channel: i16,
    /// Key, or -1 for all keys.
    pub key: i16,
    /// Reserved; must be zero.
    pub reserved: i16,
    /// Velocity, parameter or expression value.
    pub value: f64,
}
impl Event {
    fn valid(self) -> bool {
        self.offset < FRAMES as u32
            && (1..=10).contains(&self.kind)
            && self.reserved == 0
            && self.value.is_finite()
            && self.port >= -1
            && (-1..=15).contains(&self.channel)
            && (-1..=127).contains(&self.key)
            && self.note_id >= -1
    }
}
/// Fixed-capacity event list, used for both directions and deferred events.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct EventList {
    /// Number of valid entries; never greater than `EVENTS`.
    pub count: u32,
    /// Sample-ordered event records.
    pub values: [Event; EVENTS],
}
impl EventList {
    /// Reject full, invalid or out-of-order input without allocating.
    pub fn push(&mut self, event: Event) -> bool {
        let n = self.count as usize;
        if n >= EVENTS || !event.valid() || (n > 0 && self.values[n - 1].offset > event.offset) {
            return false;
        }
        self.values[n] = event;
        self.count += 1;
        true
    }
    fn valid(self) -> bool {
        let mut list = Self::default();
        self.count <= EVENTS as u32
            && self.values[..self.count.min(EVENTS as u32) as usize]
                .iter()
                .all(|event| list.push(*event))
    }
}
/// A contiguous span carries both processing validity and governing transport.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Span {
    /// First sample within the job.
    pub offset: u32,
    /// Number of samples; spans cover the block exactly once.
    pub frames: u32,
    /// 1 to process; 0 to freeze DSP and emit silence.
    pub active: u32,
    /// 1 if transport is playing, 0 if stopped.
    pub playing: u32,
    /// Transport position in samples (may jump independently of device time).
    pub position: i64,
    /// Tempo in beats per minute.
    pub tempo: f64,
}
/// Immutable job metadata. Its audio is in the negotiated trailing slot region.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Job {
    /// Relative engine block index, assigned by the producer.
    pub sequence: u64,
    /// Incarnation identifier, assigned by the producer.
    pub generation: u64,
    /// Valid span count.
    pub span_count: u32,
    /// Reserved; must be zero.
    pub reserved: u32,
    /// Contiguous, ordered spans.
    pub spans: [Span; SPANS],
    /// Sorted input events with original block offsets.
    pub events: EventList,
}
impl Job {
    /// A single active full block, without events.
    pub fn full(position: i64) -> Self {
        let mut job = Self {
            span_count: 1,
            ..Self::default()
        };
        job.spans[0] = Span {
            frames: FRAMES as u32,
            active: 1,
            playing: 1,
            position,
            tempo: 120.0,
            ..Span::default()
        };
        job
    }
    /// Validate spans, event capacity/order and numeric values, without allocation.
    pub fn valid(&self) -> bool {
        if self.reserved != 0
            || self.span_count == 0
            || self.span_count > SPANS as u32
            || !self.events.valid()
        {
            return false;
        }
        let mut offset = 0;
        for span in &self.spans[..self.span_count as usize] {
            if span.offset != offset
                || span.frames == 0
                || span.frames > FRAMES as u32
                || span.active > 1
                || span.playing > 1
                || !span.tempo.is_finite()
                || span.tempo <= 0.0
            {
                return false;
            }
            offset += span.frames;
            if offset > FRAMES as u32 {
                return false;
            }
        }
        offset == FRAMES as u32
    }
}

#[repr(C, align(64))]
struct Header {
    magic: u32,
    version: u32,
    bytes: u64,
    header_bytes: u32,
    slot_bytes: u32,
    event_bytes: u32,
    span_bytes: u32,
    slots: u32,
    events: u32,
    spans: u32,
    frames: u32,
    config: Config,
    ready: AtomicU32,
    stop: AtomicU32,
    failure: AtomicU32,
    reserved: u32,
    misses: AtomicU64,
    stale: AtomicU64,
}
#[repr(C, align(64))]
struct Slot {
    state: AtomicU32,
    job: UnsafeCell<Job>,
    result: UnsafeCell<EventList>,
}
/// A validated borrowed mapping. Getting memory is a native host concern.
pub struct View<'a> {
    base: NonNull<u8>,
    config: Config,
    _memory: PhantomData<&'a mut [u8]>,
}
// SAFETY: raw payload access is restricted to the SPSC ownership protocol.
unsafe impl Send for View<'_> {}
impl<'a> View<'a> {
    /// Initialize a fresh, writable, aligned mapping outside RT.
    ///
    /// # Safety
    /// `base` is 64-byte aligned, writable for `len` bytes, exclusively owned
    /// during initialization, and remains mapped for the returned lifetime.
    pub unsafe fn initialize(
        base: *mut u8,
        len: usize,
        config: Config,
    ) -> Result<Self, &'static str> {
        let bytes = config.bytes().ok_or("invalid plugin capacities")?;
        if len != bytes || !(base as usize).is_multiple_of(align_of::<Header>()) || base.is_null() {
            return Err("invalid plugin mapping");
        }
        // SAFETY: caller grants exclusive writable memory, with checked size/alignment.
        unsafe {
            std::ptr::write_bytes(base, 0, len);
            base.cast::<Header>().write(Header {
                magic: MAGIC,
                version: PLUGIN_ABI_VERSION,
                bytes: bytes as u64,
                header_bytes: size_of::<Header>() as u32,
                slot_bytes: config.stride() as u32,
                event_bytes: size_of::<Event>() as u32,
                span_bytes: size_of::<Span>() as u32,
                slots: SLOTS as u32,
                events: EVENTS as u32,
                spans: SPANS as u32,
                frames: FRAMES as u32,
                config,
                ready: AtomicU32::new(0),
                stop: AtomicU32::new(0),
                failure: AtomicU32::new(0),
                reserved: 0,
                misses: AtomicU64::new(0),
                stale: AtomicU64::new(0),
            });
            for i in 0..SLOTS {
                base.add(size_of::<Header>() + i * config.stride())
                    .cast::<Slot>()
                    .write(Slot {
                        state: AtomicU32::new(FREE),
                        job: UnsafeCell::new(Job::default()),
                        result: UnsafeCell::new(EventList::default()),
                    });
            }
            Self::attach(base, len, config.generation)
        }
    }
    /// Validate layout, capacities, sizes and generation before creating references.
    ///
    /// # Safety
    /// `base` is a readable/writable shared mapping for `len` bytes, initialized
    /// by a compatible producer and alive for `'a`. Its immutable header fields
    /// cannot be changed after publication. Only this module's protocol may
    /// access the audio plane; at most one endpoint of each kind may attach.
    pub unsafe fn attach(base: *mut u8, len: usize, generation: u64) -> Result<Self, &'static str> {
        if base.is_null()
            || !(base as usize).is_multiple_of(align_of::<Header>())
            || len < size_of::<Header>()
        {
            return Err("invalid plugin mapping");
        }
        // SAFETY: caller supplies initialized header bytes; alignment/length checked.
        let h = unsafe { &*base.cast::<Header>() };
        if h.magic != MAGIC
            || h.version != PLUGIN_ABI_VERSION
            || h.reserved != 0
            || h.config.generation != generation
            || h.config.bytes() != Some(len)
            || h.bytes != len as u64
            || h.header_bytes != size_of::<Header>() as u32
            || h.slot_bytes != h.config.stride() as u32
            || h.event_bytes != size_of::<Event>() as u32
            || h.span_bytes != size_of::<Span>() as u32
            || h.slots != SLOTS as u32
            || h.events != EVENTS as u32
            || h.spans != SPANS as u32
            || h.frames != FRAMES as u32
        {
            return Err("incompatible plugin segment");
        }
        Ok(Self {
            base: NonNull::new(base).ok_or("invalid plugin mapping")?,
            config: h.config,
            _memory: PhantomData,
        })
    }
    fn header(&self) -> &Header {
        // SAFETY: attach checked alignment/length and the mapping lifetime is borrowed.
        unsafe { self.base.cast::<Header>().as_ref() }
    }
    fn slot(&self, sequence: u64) -> &Slot {
        // SAFETY: index is bounded and stride/segment size were validated on attach.
        unsafe {
            &*self
                .base
                .as_ptr()
                .add(size_of::<Header>() + sequence as usize % SLOTS * self.config.stride())
                .cast::<Slot>()
        }
    }
    fn audio(&self, sequence: u64) -> *mut f32 {
        // SAFETY: caller uses only the negotiated payload after claiming ownership.
        unsafe {
            (self.slot(sequence) as *const Slot as *mut u8)
                .add(size_of::<Slot>())
                .cast()
        }
    }
    /// Immutable negotiated configuration.
    pub fn config(&self) -> Config {
        self.config
    }
    /// Current latched failure reason (zero means running).
    pub fn failure(&self) -> u32 {
        self.header().failure.load(Ordering::Acquire)
    }
    /// Latch failure; first reason wins, and RT never resets a live mapping.
    pub fn fail(&self, reason: Failure) {
        let _ = self.header().failure.compare_exchange(
            0,
            reason as u32,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
    /// Whether the helper completed initialization/activation.
    pub fn ready(&self) -> bool {
        self.header().ready.load(Ordering::Acquire) == 1
    }
    /// Helper activation publishes readiness outside RT.
    pub fn set_ready(&self) {
        self.header().ready.store(1, Ordering::Release);
    }
    /// Ask the helper to stop; no wake or syscall is made here.
    pub fn stop(&self) {
        self.header().stop.store(1, Ordering::Release);
    }
    /// Helper stop flag.
    pub fn stopping(&self) -> bool {
        self.header().stop.load(Ordering::Acquire) != 0
    }
    /// Missing scheduled output blocks and discarded late completions.
    pub fn counters(&self) -> (u64, u64) {
        (
            self.header().misses.load(Ordering::Relaxed),
            self.header().stale.load(Ordering::Relaxed),
        )
    }
    /// Consume this view as the sole producer endpoint.
    pub fn producer(self) -> Producer<'a> {
        Producer {
            view: self,
            sequence: 0,
            consecutive: 0,
        }
    }
    /// Consume this view as the sole helper endpoint.
    pub fn consumer(self) -> Consumer<'a> {
        Consumer {
            view: self,
            sequence: 0,
        }
    }
}
/// Sole producer state; dropping it does not free/unmap shared memory.
pub struct Producer<'a> {
    view: View<'a>,
    sequence: u64,
    consecutive: u32,
}
impl Producer<'_> {
    /// Access failure/counters/configuration without duplicating an endpoint.
    pub fn view(&self) -> &View<'_> {
        &self.view
    }
    /// Relative next block sequence.
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    /// Publish one completed engine block and copy only the output due now.
    /// Returns true only when a matching on-time result was delivered. All
    /// other cases produce silence. There are no allocations, frees or syscalls.
    pub fn process(
        &mut self,
        job: &Job,
        input: &[f32],
        output: &mut [f32],
        events: &mut EventList,
    ) -> bool {
        for value in output
            .iter_mut()
            .take(self.view.config.outputs as usize * FRAMES)
        {
            *value = 0.0;
        }
        events.count = 0;
        let seq = self.sequence;
        self.sequence = self.sequence.saturating_add(1);
        let c = self.view.config;
        if self.view.failure() != 0 {
            return false;
        }
        if input.len() != c.inputs as usize * FRAMES
            || output.len() != c.outputs as usize * FRAMES
            || !job.valid()
            || seq > (i64::MAX as u64 - c.sample_origin) / FRAMES as u64
        {
            self.view.fail(Failure::Capacity);
            return false;
        }
        let mut delivered = false;
        if seq >= c.depth as u64 {
            let due = seq - c.depth as u64;
            let slot = self.view.slot(due);
            if slot.state.load(Ordering::Acquire) == DONE {
                // SAFETY: DONE transfers immutable output ownership to this producer.
                let stored = unsafe { *slot.job.get() };
                if stored.sequence == due && stored.generation == c.generation {
                    // SAFETY: output follows input and fits the validated slot payload.
                    let audio = unsafe {
                        std::slice::from_raw_parts(
                            self.view.audio(due).add(input.len()),
                            output.len(),
                        )
                    };
                    output.copy_from_slice(audio);
                    // SAFETY: helper published this result before DONE.
                    *events = unsafe { *slot.result.get() };
                    if !events.valid() || output.iter().any(|x| !x.is_finite()) {
                        output.fill(0.0);
                        events.count = 0;
                        self.view.fail(Failure::Process);
                        return false;
                    }
                    slot.state.store(FREE, Ordering::Release);
                    self.consecutive = 0;
                    delivered = true;
                }
            }
            if !delivered {
                self.view.header().misses.fetch_add(1, Ordering::Relaxed);
                self.consecutive += 1;
                if self.consecutive == 8 {
                    self.view.fail(Failure::Starved);
                    return false;
                }
            }
        }
        for i in 0..SLOTS {
            let slot = self.view.slot(i as u64);
            if slot.state.load(Ordering::Acquire) == DONE {
                // SAFETY: DONE is immutable until the sole producer frees it.
                let old = unsafe { *slot.job.get() };
                if old.generation != c.generation
                    || (seq >= c.depth as u64 && old.sequence <= seq - c.depth as u64)
                {
                    slot.state.store(FREE, Ordering::Release);
                    self.view.header().stale.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        let slot = self.view.slot(seq);
        if slot.state.load(Ordering::Acquire) != FREE {
            self.view.fail(Failure::Capacity);
            output.fill(0.0);
            events.count = 0;
            return false;
        }
        let mut job = *job;
        job.sequence = seq;
        job.generation = c.generation;
        // SAFETY: FREE grants sole producer ownership of job and input payload.
        unsafe {
            *slot.job.get() = job;
            std::slice::from_raw_parts_mut(self.view.audio(seq), input.len())
                .copy_from_slice(input);
        }
        slot.state.store(READY, Ordering::Release);
        delivered
    }
}
/// Sole helper endpoint; consumes all accepted jobs, including late ones.
pub struct Consumer<'a> {
    view: View<'a>,
    sequence: u64,
}
impl Consumer<'_> {
    /// Shared lifecycle/failure controls.
    pub fn view(&self) -> &View<'_> {
        &self.view
    }
    /// Claim the next accepted job, or return immediately without waiting.
    pub fn next_job(&mut self) -> Option<Processing<'_>> {
        if self.view.failure() != 0 || self.view.stopping() {
            return None;
        }
        let slot = self.view.slot(self.sequence);
        if slot.state.load(Ordering::Acquire) != READY {
            return None;
        }
        // SAFETY: READY publishes immutable metadata/input exclusively to this helper.
        let job = unsafe { *slot.job.get() };
        if job.sequence != self.sequence
            || job.generation != self.view.config.generation
            || !job.valid()
        {
            self.view.fail(Failure::Protocol);
            return None;
        }
        slot.state.store(BUSY, Ordering::Relaxed);
        self.sequence += 1;
        Some(Processing {
            view: &self.view,
            sequence: job.sequence,
            job,
            finished: false,
        })
    }
}
/// Helper-owned slot guard. An unfinished guard fails the instance, never frees it.
pub struct Processing<'a> {
    view: &'a View<'a>,
    sequence: u64,
    job: Job,
    finished: bool,
}
impl Processing<'_> {
    /// Copied immutable job metadata.
    pub fn job(&self) -> &Job {
        &self.job
    }
    /// Disjoint input and writable output, while this guard owns BUSY.
    pub fn audio(&mut self) -> (&[f32], &mut [f32]) {
        let c = self.view.config;
        // SAFETY: BUSY is helper-exclusive, input/output are nonoverlapping validated regions.
        unsafe {
            let p = self.view.audio(self.sequence);
            (
                std::slice::from_raw_parts(p, c.inputs as usize * FRAMES),
                std::slice::from_raw_parts_mut(
                    p.add(c.inputs as usize * FRAMES),
                    c.outputs as usize * FRAMES,
                ),
            )
        }
    }
    /// Publish bounded, sorted output events and audio together.
    pub fn finish(mut self, events: EventList) {
        if !events.valid() {
            self.view.fail(Failure::Capacity);
            return;
        }
        let slot = self.view.slot(self.sequence);
        // SAFETY: this guard still owns BUSY and has exclusive result access.
        unsafe {
            *slot.result.get() = events;
        }
        slot.state.store(DONE, Ordering::Release);
        self.finished = true;
    }
    /// Explicit processing error, reported without racing slot ownership.
    pub fn fail(&self, reason: Failure) {
        self.view.fail(reason);
    }
}
impl Drop for Processing<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.view.fail(Failure::Process);
        }
    }
}
