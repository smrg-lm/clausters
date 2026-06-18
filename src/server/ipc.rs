//! M14: the shared-memory IPC segment — transport and data plane.
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
//!   by the audio thread (one extra atomic store per block — M8 anchors with
//!   zero UDP jitter), and the 1024 control buses as raw atomics (the engine
//!   reads *these very words* through `InCtl`: a client write is live on the
//!   next block, no command involved);
//! - the **command plane**: two SPSC byte rings (client→server and
//!   server→client) carrying ordinary length-prefixed OSC packets. The
//!   network thread drains the inbound ring in its loop and routes replies
//!   back by [`ClientId::Ring`](crate::osc::ClientId); ring bytes are as
//!   untrusted as UDP bytes (`osc::decode_packet` validates).
//!
//! Synchronization is index-based: each ring has a `head` (producer) and
//! `tail` (consumer) cursor, monotonically increasing `u32`s used modulo the
//! capacity. The producer copies the packet first and publishes `head` with
//! Release; the consumer Acquire-loads `head`. A malformed length resyncs
//! the ring by dropping its contents (the producer is outside our trust
//! boundary). v1 keeps exactly **one** ring client per segment and the
//! server polls the ring on a short socket timeout instead of a semaphore —
//! documented trade-offs in `docs/ipc.md`.

use std::fs::OpenOptions;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::dsp::{ControlBuses, NUM_CONTROL_BUSES};

/// "CLAU" little-endian.
pub const MAGIC: u32 = 0x5541_4C43;
/// Bump on **any** layout change: attaching rejects mismatches.
pub const ABI_VERSION: u32 = 1;
/// Byte capacity of each ring (power of two).
pub const RING_CAPACITY: usize = 64 * 1024;

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
    _reserved: [u32; 8],
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

#[repr(C)]
struct Layout {
    header: Header,
    controls: [AtomicU32; NUM_CONTROL_BUSES],
    /// Client → server commands.
    c2s: Ring,
    /// Server → client replies.
    s2c: Ring,
}

pub const SEGMENT_SIZE: usize = size_of::<Layout>();

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
    /// A heap-backed segment for the in-process (embed) transport.
    pub fn in_memory() -> Arc<Self> {
        let mut words = vec![0u128; SEGMENT_SIZE.div_ceil(16)].into_boxed_slice();
        let layout = words.as_mut_ptr() as *mut Layout;
        let seg = Self {
            layout,
            _backing: Backing::Heap(words),
        };
        seg.init_header();
        Arc::new(seg)
    }

    /// Creates (or truncates) the segment file and maps it shared. Put it on
    /// a memory filesystem — `/dev/shm/...` on Linux — to avoid disk writes.
    #[cfg(unix)]
    pub fn create(path: &Path) -> io::Result<Arc<Self>> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.set_len(SEGMENT_SIZE as u64)?;
        let seg = Self::map_file(&file)?;
        seg.init_header();
        Ok(Arc::new(seg))
    }

    /// Maps an existing segment (the client side) and validates the header.
    #[cfg(unix)]
    pub fn open(path: &Path) -> io::Result<Arc<Self>> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        if file.metadata()?.len() != SEGMENT_SIZE as u64 {
            return Err(io::Error::other("segment size mismatch"));
        }
        let seg = Self::map_file(&file)?;
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
        Ok(Arc::new(seg))
    }

    #[cfg(unix)]
    fn map_file(file: &std::fs::File) -> io::Result<Self> {
        use std::os::fd::AsRawFd;
        // SAFETY: anonymous-address shared mapping of a file we just sized.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                SEGMENT_SIZE,
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
                len: SEGMENT_SIZE,
            },
        })
    }

    /// Runs at creation time, before any other process can have attached
    /// (the file was just created/truncated), so plain writes are sound.
    fn init_header(&self) {
        let header = unsafe { &mut (*self.layout).header };
        header.ring_capacity = RING_CAPACITY as u32;
        header.control_buses = NUM_CONTROL_BUSES as u32;
        header.abi_version = ABI_VERSION;
        // Written last: a client that sees the magic sees a full header.
        header.magic = MAGIC;
    }

    fn layout(&self) -> &Layout {
        unsafe { &*self.layout }
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

    /// Control buses living inside the segment; hand this to
    /// `engine_pair_full` so `InCtl` and `/c_set` operate on shared memory.
    pub fn control_buses(self: &Arc<Self>) -> ControlBuses {
        let ptr = self.layout().controls.as_ptr();
        // SAFETY: the array is part of the segment, kept alive by the Arc.
        unsafe { ControlBuses::from_raw(ptr, Arc::clone(self) as _) }
    }
}

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

    /// Appends one packet to the outbound ring. `false` when the ring lacks
    /// space — backpressure, the caller may retry (nothing is dropped).
    pub fn push(&self, packet: &[u8]) -> bool {
        let ring = self.outbound();
        let padded = packet.len().div_ceil(4) * 4;
        let total = 4 + padded;
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
        write_ring(ring, head.wrapping_add(4), packet);
        ring.head
            .store(head.wrapping_add(total as u32), Ordering::Release);
        true
    }

    /// Pops one packet from the inbound ring into `buf`, returning its
    /// length. Malformed lengths (a hostile or crashed peer) drop the whole
    /// ring contents and return `None`.
    pub fn try_pop(&self, buf: &mut [u8]) -> Option<usize> {
        let ring = self.inbound();
        let head = ring.head.load(Ordering::Acquire);
        let tail = ring.tail.load(Ordering::Relaxed);
        let used = head.wrapping_sub(tail) as usize;
        if used == 0 {
            return None;
        }
        let mut len_bytes = [0u8; 4];
        read_ring(ring, tail, &mut len_bytes);
        let len = u32::from_le_bytes(len_bytes) as usize;
        let padded = len.div_ceil(4) * 4;
        let total = 4 + padded;
        if len == 0 || total > used || len > buf.len() {
            // Untrusted peer wrote garbage (or a packet bigger than our
            // buffer): resync by discarding everything buffered.
            ring.tail.store(head, Ordering::Release);
            return None;
        }
        read_ring(ring, tail.wrapping_add(4), &mut buf[..len]);
        ring.tail
            .store(tail.wrapping_add(total as u32), Ordering::Release);
        Some(len)
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
