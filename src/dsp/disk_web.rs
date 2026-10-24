//! Streaming disk I/O in a page: the same two UGens, fed by the host.
//!
//! The native module ([`crate::dsp::disk`]'s other half) gives each UGen a
//! background thread that opens a file and races the audio thread through a
//! lock-free ring. A page has neither half of that: no `std::thread`, and no
//! filesystem the engine can reach -- the private one it does have (OPFS) is a
//! JS API, and a synchronous handle on it exists only in a dedicated Worker.
//!
//! So the ring stays and the thread goes. **The host is the reader.** A stream
//! registers itself when the synth is built, the host asks what each open
//! stream wants ([`poll`]) and fills or drains it ([`push`], [`pull`]); the
//! `process` half is unchanged, and so is what it does when the host is late --
//! an underrun plays silence, an overrun drops samples, exactly as a slow disk
//! would. That is the honest shape of it: without shared memory the two threads
//! cannot share a ring, so the chunks are *moved* across and **how far ahead
//! the host reads is the design**, not a tuning constant.
//!
//! **A file's shape arrives late, and that is fine.** Natively `DiskIn::open`
//! opens the file and learns its channel count on the spot. Here it cannot --
//! reading is asynchronous and belongs to another thread -- so a stream is born
//! not knowing, plays silence while it does not, and is told ([`set_shape`])
//! when the host has looked. Nothing declares anything up front, which matters
//! for more than convenience: a declaration would be a call the other client
//! has no counterpart for, and this way the surface a script writes against is
//! the same one in both.
//!
//! The registry is a `thread_local`, and that is not a shortcut: everything
//! that touches it -- building a synth, dropping one, and the host's own calls --
//! happens on the one thread a page's engine has.

use std::cell::RefCell;
use std::sync::Arc;

use rtrb::{Consumer, Producer, RingBuffer};

use crate::dsp::buffer::Buffer;
use crate::dsp::registry::{BuildCtx, UGenConfig};
use crate::dsp::stitch::Part;
use crate::dsp::{CUE_LEAD, ListWrap, ProcessCtx, UGen, at};
use crate::server::meters::Meters;

/// Ring capacity in samples, as natively (~1.4 s of mono at 48 kHz).
const RING_SAMPLES: usize = 1 << 16;

/// Which way a stream runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    /// A file being read into the graph (`DiskIn`).
    In,
    /// The graph being written to a file (`DiskOut`).
    Out,
    /// A segment list played at the transport's position (`TransportDiskIn`):
    /// the host reads the span of a file one of its parts names, and the
    /// engine renders the part from it.
    List,
    /// The base buckets of a peaks file (`PeaksOut`), drained like a
    /// `DiskOut` and written by the host once the synth is freed: `start` is
    /// the frames summarized, `looping` says the synth is gone, and `format`
    /// is `"lost"` when the ring could not take a bucket.
    Peaks,
}

/// What one open stream wants from the host right now.
#[derive(Clone, Debug)]
pub struct StreamRequest {
    pub id: u32,
    pub direction: Direction,
    pub path: String,
    /// `DiskIn`: the file's channel count, as declared. `DiskOut`: 1 -- it
    /// writes a mono file, as it does natively.
    pub channels: usize,
    /// `DiskIn`: restart from the top at end of stream.
    pub looping: bool,
    /// `DiskIn`: the frame of the file the first read is from; a loop goes
    /// back to the top.
    pub start: u64,
    /// `DiskOut`: the WAV sample format (`int16` | `int24` | `float`).
    pub format: String,
    /// `DiskIn`: room in the ring, in samples -- how much the host may push.
    /// `DiskOut`: samples waiting to be pulled.
    pub samples: usize,
    /// `List`: which refill this span is for. A span answered for an older one
    /// is thrown away by [`list_push`].
    pub generation: u32,
    /// `List`: the frame of the list the span renders from, and how many.
    pub at: u64,
    pub list_frames: usize,
    /// `List`: how many frames of the file to read, from `start`.
    pub span: usize,
}

/// A list stream's side of the registry: what it plays and where the host
/// has rendered it to.
struct ListState {
    list: Option<Arc<Buffer>>,
    /// The next list frame to render; `None` before the first refill.
    next: Option<u64>,
    generation: u32,
    looping: bool,
    /// The transport's loop, rendered straight through.
    wrap: Option<ListWrap>,
}

enum End {
    Feeding(Producer<f32>),
    Draining(Consumer<f32>),
}

struct Slot {
    id: u32,
    direction: Direction,
    path: String,
    channels: usize,
    looping: bool,
    start: u64,
    format: String,
    end: End,
    list: Option<ListState>,
    /// `DiskIn` in an offline render whose host reads its files on the spot:
    /// the file, and the frame read to.
    offline: Option<OfflineRead>,
}

/// What an offline `DiskIn` has read of its file.
struct OfflineRead {
    layout: crate::dsp::pcm::Layout,
    next: u64,
}

thread_local! {
    static STREAMS: RefCell<Vec<Slot>> = const { RefCell::new(Vec::new()) };
    static NEXT_ID: RefCell<u32> = const { RefCell::new(0) };
    static FILES: RefCell<Option<std::rc::Rc<dyn PageFiles>>> = const { RefCell::new(None) };
}

/// **The files of an offline render in a page, read on the spot.**
///
/// Natively an offline render's streams wait for their threads: a frame not
/// read yet is waited for, never played as silence (`dsp::disk::wait_for`).
/// A page's engine cannot wait inside a block, but the thread an offline
/// render runs on can read a file synchronously (a Worker's sync access
/// handle, through `clausters_core::files`), so the render hands one of
/// these over for its length ([`install_files`], `server::render`'s
/// `page_streams`) and the input streams read through it the moment they
/// need to: a list stream its next span, a `DiskIn` its next run. What is
/// rendered is then what a native render renders. The outputs (`DiskOut`,
/// `PeaksOut`) need nothing of it: the render drains them between two runs,
/// as the host does live.
pub trait PageFiles {
    /// The length of the file at `path`, in bytes.
    fn size(&self, path: &str) -> Result<u64, String>;
    /// `len` bytes of the file at `path` from `offset`; fewer at its end.
    fn read(&self, path: &str, offset: u64, len: usize) -> Result<Vec<u8>, String>;
}

/// Hands the files an offline render reads over, or takes them back
/// (`None`) once it is done.
pub fn install_files(files: Option<std::rc::Rc<dyn PageFiles>>) {
    FILES.with(|f| *f.borrow_mut() = files);
}

fn files() -> Option<std::rc::Rc<dyn PageFiles>> {
    FILES.with(|f| f.borrow().clone())
}

/// **The layout of the file at `path`, read through the render's files**:
/// its head, and a larger one when the header runs past it.
pub fn layout_through_files(path: &str) -> Result<crate::dsp::pcm::Layout, String> {
    let files = files().ok_or_else(|| {
        format!(
            "{path}: a page's engine reaches no file outside an offline render whose host \
             reads them for it"
        )
    })?;
    let size = files.size(path)?;
    let mut head = files.read(path, 0, (1 << 16).min(size as usize))?;
    let parsed =
        crate::dsp::pcm::Layout::parse_within(&mut std::io::Cursor::new(&head), path, Some(size));
    if parsed.is_ok() || head.len() as u64 >= size {
        return parsed;
    }
    head = files.read(path, 0, (1 << 20).min(size as usize))?;
    crate::dsp::pcm::Layout::parse_within(&mut std::io::Cursor::new(&head), path, Some(size))
}

/// `frames` frames of `layout` from `first`, decoded and interleaved.
fn read_frames(
    files: &dyn PageFiles,
    layout: &crate::dsp::pcm::Layout,
    first: u64,
    frames: usize,
) -> Result<Vec<f32>, String> {
    let width = layout.frame_bytes();
    let bytes = files.read(
        &layout.path,
        layout.data_offset + first * width as u64,
        frames * width,
    )?;
    let one = layout.encoding.bytes();
    Ok(bytes
        .chunks_exact(one)
        .map(|b| layout.encoding.decode(b))
        .collect())
}

/// Tells a stream how many channels its file has, once the host has looked.
///
/// A `DiskIn` reports `channels: 0` until this arrives and plays silence
/// meanwhile; the host sees the zero in [`poll`] and knows the one thing it
/// still owes. `0` here is ignored -- a file with no channels is a file that
/// could not be read, and the stream stays silent rather than dividing by it.
pub fn set_shape(id: u32, channels: usize) {
    if channels == 0 {
        return;
    }
    STREAMS.with(|s| {
        if let Some(slot) = s.borrow_mut().iter_mut().find(|slot| slot.id == id) {
            slot.channels = channels;
        }
    });
}

/// What a stream's `process` reads out of the registry each block: how many
/// channels its file turned out to have, or 0 while nobody has said.
fn channels_of(id: u32) -> usize {
    STREAMS.with(|s| {
        s.borrow()
            .iter()
            .find(|slot| slot.id == id)
            .map_or(0, |slot| slot.channels)
    })
}

fn register(slot: Slot) -> u32 {
    let id = slot.id;
    STREAMS.with(|s| s.borrow_mut().push(slot));
    id
}

fn fresh_id() -> u32 {
    NEXT_ID.with(|n| {
        let mut n = n.borrow_mut();
        *n += 1;
        *n
    })
}

/// Forgets a stream: what the host calls once it has drained a `PeaksOut`
/// whose synth is gone and written its file.
pub fn release(id: u32) {
    STREAMS.with(|s| s.borrow_mut().retain(|slot| slot.id != id));
}

/// Every open stream and what it wants. The host walks this each turn: it is
/// the whole interface between the graph and whatever is reading files.
///
/// A stream whose synth has been freed is simply not here any more.
pub fn poll() -> Vec<StreamRequest> {
    STREAMS.with(|s| {
        let mut s = s.borrow_mut();
        let mut out = Vec::with_capacity(s.len());
        for slot in s.iter_mut() {
            if slot.direction == Direction::List {
                // Reported whether or not it wants a span (`span` 0 when it
                // does not), so the host can tell a live stream from a freed
                // one.
                let request = list_request(slot).unwrap_or_else(|| StreamRequest {
                    id: slot.id,
                    direction: Direction::List,
                    path: String::new(),
                    channels: 0,
                    looping: false,
                    start: 0,
                    format: String::new(),
                    samples: 0,
                    generation: 0,
                    at: 0,
                    list_frames: 0,
                    span: 0,
                });
                out.push(request);
                continue;
            }
            out.push(StreamRequest {
                id: slot.id,
                direction: slot.direction,
                path: slot.path.clone(),
                channels: slot.channels,
                looping: slot.looping,
                start: slot.start,
                format: slot.format.clone(),
                samples: match &slot.end {
                    End::Feeding(p) => p.slots(),
                    End::Draining(c) => c.slots(),
                },
                generation: 0,
                at: 0,
                list_frames: 0,
                span: 0,
            });
        }
        out
    })
}

/// The frames a list stream renders in one span: 0.4 s at 48 kHz, what a
/// `DiskIn` stream asks for at most. One span is in flight at a time, so a
/// span has to outlast the host's round trip to the page's files, or a
/// stream that has just been refilled chases the playhead and never catches
/// it.
const LIST_SPAN: usize = 19_200;

/// **What a list stream wants from the host**, after rendering what it can
/// without it: the parts over buffers are in memory and go straight into the
/// ring, and the first part over a file is the span the host is asked for.
/// `None` while the ring is full, the list is played out or nothing has been
/// asked for yet.
fn list_request(slot: &mut Slot) -> Option<StreamRequest> {
    let End::Feeding(producer) = &mut slot.end else {
        return None;
    };
    let state = slot.list.as_mut()?;
    let list = state.list.clone()?;
    let channels = list.channels().max(1);
    let total = list.frames() as u64;
    loop {
        let mut at = state.next?;
        if at >= total {
            if state.looping && total > 0 {
                at = 0;
                state.next = Some(0);
            } else {
                return None;
            }
        }
        let room = producer.slots() / channels;
        // A span stops at the loop's end, and the next one starts at its
        // start.
        let left = ListWrap::before(state.wrap, at).unwrap_or(total - at) as usize;
        let want = LIST_SPAN.min(room).min(left);
        if want == 0 || (room < LIST_SPAN && want < left) {
            return None;
        }
        let Some(stitch) = list.stitch() else {
            // A buffer that owns its samples, played through the same reader.
            let mut frames = Vec::with_capacity(want * channels);
            for f in 0..want {
                for c in 0..channels {
                    frames.push(list.sample(at as usize + f, c));
                }
            }
            push_frames(producer, &frames);
            state.next = Some(ListWrap::advance(state.wrap, at, want as u64));
            continue;
        };
        let (_, part) = stitch.part_at(at as usize)?;
        let inner = at as usize - part.start();
        let len = want.min(part.end() - at as usize);
        match part.src.file() {
            Some(layout) => {
                let (first, end) = part.source_span(inner, inner + len);
                return Some(StreamRequest {
                    id: slot.id,
                    direction: Direction::List,
                    path: layout.path.clone(),
                    channels: layout.channels,
                    looping: state.looping,
                    start: first as u64,
                    format: String::new(),
                    samples: producer.slots(),
                    generation: state.generation,
                    at,
                    list_frames: len,
                    span: end - first,
                });
            }
            None => {
                let mut frames = vec![0.0; len * channels];
                render_part(part, inner, channels, &mut frames, |f, c| {
                    part.src.sample(f, c)
                });
                push_frames(producer, &frames);
                state.next = Some(ListWrap::advance(state.wrap, at, len as u64));
            }
        }
    }
}

/// `len = out.len() / channels` frames of `part` from `inner` on, each read
/// through `frame` -- [`PartSpan::read_with`](clausters_core::segments::PartSpan::read_with), the read every part makes.
fn render_part(
    part: &Part,
    inner: usize,
    channels: usize,
    out: &mut [f32],
    frame: impl Fn(usize, usize) -> f32,
) {
    for (j, cells) in out.chunks_mut(channels).enumerate() {
        for (c, cell) in cells.iter_mut().enumerate() {
            *cell = part.read_with(inner + j, c, &frame);
        }
    }
}

fn push_frames(producer: &mut Producer<f32>, frames: &[f32]) {
    if let Ok(chunk) = producer.write_chunk_uninit(frames.len()) {
        chunk.fill_from_iter(frames.iter().copied());
    }
}

/// **Hands a list stream the span it asked for**: frames `start..start +
/// span` of the file, interleaved, for the `list_frames` list frames from
/// `at`. The part is rendered from them into the ring. Returns whether it was
/// taken: a span for an older refill, or for frames the stream is no longer
/// at, is thrown away.
pub fn list_push(id: u32, generation: u32, at: u64, list_frames: usize, samples: &[f32]) -> bool {
    STREAMS.with(|s| {
        let mut s = s.borrow_mut();
        let Some(slot) = s.iter_mut().find(|slot| slot.id == id) else {
            return false;
        };
        take_span(slot, generation, at, list_frames, samples)
    })
}

/// [`list_push`] into a stream already found.
fn take_span(
    slot: &mut Slot,
    generation: u32,
    at: u64,
    list_frames: usize,
    samples: &[f32],
) -> bool {
    {
        let End::Feeding(producer) = &mut slot.end else {
            return false;
        };
        let Some(state) = slot.list.as_mut() else {
            return false;
        };
        if state.generation != generation || state.next != Some(at) {
            return false;
        }
        let Some(list) = state.list.clone() else {
            return false;
        };
        let Some((_, part)) = list.stitch().and_then(|st| st.part_at(at as usize)) else {
            return false;
        };
        let Some(layout) = part.src.file() else {
            return false;
        };
        let channels = list.channels().max(1);
        if producer.slots() < list_frames * channels {
            return false;
        }
        let inner = at as usize - part.start();
        let (first, _) = part.source_span(inner, inner + list_frames);
        let width = layout.channels;
        let mut frames = vec![0.0; list_frames * channels];
        render_part(part, inner, channels, &mut frames, |f, c| {
            f.checked_sub(first)
                .and_then(|f| samples.get(f * width + c))
                .copied()
                .unwrap_or(0.0)
        });
        push_frames(producer, &frames);
        state.next = Some(ListWrap::advance(state.wrap, at, list_frames as u64));
        true
    }
}

/// **Serves a list stream on the spot**, through the files an offline
/// render's host handed over: every span it asks for, read and rendered,
/// until its ring has no room for another. Nothing without them.
fn serve_list(id: u32) {
    let Some(files) = files() else {
        return;
    };
    STREAMS.with(|s| {
        let mut s = s.borrow_mut();
        let Some(slot) = s.iter_mut().find(|slot| slot.id == id) else {
            return;
        };
        while let Some(request) = list_request(slot) {
            let layout = slot
                .list
                .as_ref()
                .and_then(|state| state.list.clone())
                .and_then(|list| {
                    let (_, part) = list.stitch()?.part_at(request.at as usize)?;
                    part.src.file().cloned()
                });
            let Some(layout) = layout else {
                return;
            };
            match read_frames(files.as_ref(), &layout, request.start, request.span) {
                Ok(samples) => {
                    if !take_span(
                        slot,
                        request.generation,
                        request.at,
                        request.list_frames,
                        &samples,
                    ) {
                        return;
                    }
                }
                Err(e) => {
                    tracing::warn!("TransportDiskIn: {e}");
                    return;
                }
            }
        }
    });
}

/// **Serves a `DiskIn` on the spot**, as [`serve_list`] does a list: runs of
/// its file from where it has read to, until its ring is full or the file is
/// played out (and from the top again, looping).
fn serve_in(id: u32) {
    let Some(files) = files() else {
        return;
    };
    STREAMS.with(|s| {
        let mut s = s.borrow_mut();
        let Some(slot) = s.iter_mut().find(|slot| slot.id == id) else {
            return;
        };
        let looping = slot.looping;
        let (End::Feeding(producer), Some(read)) = (&mut slot.end, slot.offline.as_mut()) else {
            return;
        };
        let channels = read.layout.channels.max(1);
        loop {
            if read.next >= read.layout.frames {
                if !looping || read.layout.frames == 0 {
                    return;
                }
                read.next = 0;
            }
            let room = producer.slots() / channels;
            let n = room
                .min(LIST_SPAN)
                .min((read.layout.frames - read.next) as usize);
            if n == 0 {
                return;
            }
            match read_frames(files.as_ref(), &read.layout, read.next, n) {
                Ok(samples) => {
                    push_frames(producer, &samples);
                    read.next += n as u64;
                }
                Err(e) => {
                    tracing::warn!("DiskIn: {e}");
                    return;
                }
            }
        }
    });
}

/// Points a list stream at `frame` of `list` (or of the list it holds), for
/// a new refill: what the host renders next starts there.
fn refill_slot(
    id: u32,
    list: Option<Arc<Buffer>>,
    frame: u64,
    looping: bool,
    wrap: Option<ListWrap>,
) {
    STREAMS.with(|s| {
        if let Some(slot) = s.borrow_mut().iter_mut().find(|slot| slot.id == id)
            && let Some(state) = slot.list.as_mut()
        {
            if list.is_some() {
                state.list = list;
            }
            state.next = Some(frame);
            state.generation = state.generation.wrapping_add(1);
            state.looping = looping;
            state.wrap = wrap;
        }
    });
}

/// Pushes interleaved frames into a `DiskIn` stream, returning how many
/// samples were taken. Fewer than offered means the ring filled: the rest is
/// the host's to keep and offer again.
pub fn push(id: u32, samples: &[f32]) -> usize {
    STREAMS.with(|s| {
        let mut s = s.borrow_mut();
        let Some(slot) = s.iter_mut().find(|slot| slot.id == id) else {
            return 0;
        };
        let End::Feeding(producer) = &mut slot.end else {
            return 0;
        };
        let mut wrote = 0;
        for v in samples {
            if producer.push(*v).is_err() {
                break;
            }
            wrote += 1;
        }
        wrote
    })
}

/// Pulls what a `DiskOut` stream has produced into `out`, returning how many
/// samples were taken.
pub fn pull(id: u32, out: &mut [f32]) -> usize {
    STREAMS.with(|s| {
        let mut s = s.borrow_mut();
        let Some(slot) = s.iter_mut().find(|slot| slot.id == id) else {
            return 0;
        };
        let End::Draining(consumer) = &mut slot.end else {
            return 0;
        };
        let mut read = 0;
        for cell in out.iter_mut() {
            match consumer.pop() {
                Ok(v) => {
                    *cell = v;
                    read += 1;
                }
                Err(_) => break,
            }
        }
        read
    })
}

// ---- DiskIn ----

/// Streams a file into the graph. Input 0: channel selector. A path the host
/// never declared is inert (silent), as an unopenable file is natively.
pub struct DiskIn {
    active: Option<Active>,
}

struct Active {
    id: u32,
    consumer: Option<Consumer<f32>>,
    producer: Option<Producer<f32>>,
    channels: usize,
    /// Served on the spot each block, in an offline render ([`PageFiles`]).
    serve: bool,
}

impl DiskIn {
    /// The load table is taken and unused: the reader here is the host, not a
    /// thread of the engine, and `wasm32` has no clock to bracket with anyway.
    pub fn open(config: &UGenConfig, ctx: &BuildCtx) -> Self {
        let Some(path) = config.path.clone() else {
            tracing::warn!("DiskIn has no path; it will be silent");
            return Self { active: None };
        };
        // An offline render whose host reads files on the spot: the file's
        // shape is read now, as natively, and the stream is served as it
        // plays.
        let offline = (ctx.offline && files().is_some())
            .then(|| match layout_through_files(&path) {
                Ok(layout) => Some(OfflineRead {
                    next: config.start.min(layout.frames),
                    layout,
                }),
                Err(e) => {
                    tracing::warn!("DiskIn: {e}");
                    None
                }
            })
            .flatten();
        let channels = offline.as_ref().map_or(0, |r| r.layout.channels);
        let serve = offline.is_some();
        // The channel count is not knowable here (see the module docs): the
        // stream starts shapeless and the host fills it in. The ring is sized
        // in samples rather than frames for the same reason -- there is no
        // frame yet -- and the host pushes whole frames, which is what keeps it
        // aligned.
        let (producer, consumer) = RingBuffer::new(RING_SAMPLES);
        let id = fresh_id();
        register(Slot {
            id,
            direction: Direction::In,
            path,
            channels,
            looping: config.looping,
            start: config.start,
            format: String::new(),
            end: End::Feeding(producer),
            list: None,
            offline,
        });
        Self {
            active: Some(Active {
                id,
                consumer: Some(consumer),
                producer: None,
                channels,
                serve,
            }),
        }
    }
}

impl UGen for DiskIn {
    fn reset(&mut self) {
        // A stream is its I/O thread's and cannot be restarted from here: a
        // `Voices` node refuses a def that holds one.
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let Some(a) = self.active.as_mut() else {
            output.fill(0.0);
            return;
        };
        if a.serve {
            serve_in(a.id);
        }
        let Some(consumer) = a.consumer.as_mut() else {
            output.fill(0.0);
            return;
        };
        // Ask once per block, not per sample: the answer only changes when the
        // host first learns the file's shape.
        if a.channels == 0 {
            a.channels = channels_of(a.id);
        }
        let chans = a.channels;
        if chans == 0 {
            // Nobody has looked at the file yet.
            output.fill(0.0);
            return;
        }
        let channel = (inputs[0][0].max(0.0) as usize).min(chans - 1);
        for s in output.iter_mut() {
            // A whole frame or nothing: a partial pop would misalign the ring
            // for good. Nothing means silence, which is what an underrun is.
            if consumer.slots() >= chans {
                let mut sample = 0.0;
                for c in 0..chans {
                    let v = consumer.pop().unwrap_or(0.0);
                    if c == channel {
                        sample = v;
                    }
                }
                *s = sample;
            } else {
                *s = 0.0;
            }
        }
    }
}

impl Drop for DiskIn {
    fn drop(&mut self) {
        if let Some(a) = self.active.as_ref() {
            release(a.id);
        }
    }
}

// ---- DiskOut ----

/// Streams its input to a mono WAV file. Input 0: the signal, passed through
/// so the UGen can sit mid-chain.
pub struct DiskOut {
    active: Option<Active>,
}

impl DiskOut {
    /// As [`DiskIn::open`]: the writer is the host.
    pub fn open(config: &UGenConfig, ctx: &BuildCtx) -> Self {
        let Some(path) = config.path.clone() else {
            tracing::warn!("DiskOut has no path; it will discard its input");
            return Self { active: None };
        };
        let format = config.format.clone().unwrap_or_else(|| "int16".into());
        let (producer, consumer) = RingBuffer::new(RING_SAMPLES);
        let id = fresh_id();
        register(Slot {
            id,
            direction: Direction::Out,
            path,
            channels: 1,
            looping: false,
            start: 0,
            format,
            end: End::Draining(consumer),
            list: None,
            offline: None,
        });
        Self {
            active: Some(Active {
                id,
                consumer: None,
                producer: Some(producer),
                channels: 1,
                // An offline render's host drains it between runs, and the
                // last run's samples are still in the ring when the synth
                // goes: it is told the synth is gone rather than forgotten.
                serve: ctx.offline && files().is_some(),
            }),
        }
    }
}

impl UGen for DiskOut {
    fn reset(&mut self) {
        // A stream is its I/O thread's and cannot be restarted from here: a
        // `Voices` node refuses a def that holds one.
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let signal = inputs[0];
        // Heard regardless of whether anything is recording it.
        for (i, s) in output.iter_mut().enumerate() {
            *s = at(signal, i);
        }
        let Some(a) = self.active.as_mut() else {
            return;
        };
        let Some(producer) = a.producer.as_mut() else {
            return;
        };
        for i in 0..output.len() {
            // A full ring drops the sample, as it does natively: the recording
            // has a hole rather than the audio having a stall.
            if producer.push(at(signal, i)).is_err() {
                break;
            }
        }
    }
}

impl Drop for DiskOut {
    fn drop(&mut self) {
        let Some(a) = self.active.as_ref() else {
            return;
        };
        if !a.serve {
            release(a.id);
            return;
        }
        // Offline the host drains what is left and releases it (`looping`
        // says the synth is gone, as a `PeaksOut`'s does).
        let id = a.id;
        STREAMS.with(|s| {
            if let Some(slot) = s.borrow_mut().iter_mut().find(|slot| slot.id == id) {
                slot.looping = true;
            }
        });
    }
}

// ---- TransportDiskIn ----

/// Plays a segment list at the transport's position, the page's half of
/// `dsp::stream`. Inputs as natively: 0 `bufnum`, 1 `chan`, 2 `offset`,
/// 3 `start`, 4 `loop`.
///
/// **One thread, so no epochs.** Natively a refill is a request to another
/// thread, and the frames already in the ring have to be told apart from the
/// ones after it. Here the host renders between blocks on this same thread:
/// a refill throws the ring away and points the registry at the new frame in
/// one go, and a span still travelling for the old one is refused when it
/// arrives (its generation is old). The silence a refill costs is the host's
/// round trip to the page's files, and it is not counted as running out of
/// lead; a ring that runs dry afterwards is.
///
/// **A list replaced while it plays is not that silence**, as natively: the
/// next frames of the old list are kept aside ([`KEPT`] samples) before the
/// ring is thrown away, they play while the new list is read, and the two
/// fade over [`CROSS`](crate::dsp::stream::CROSS) frames each once the new
/// list's frames have come.
///
/// **A transport's loop is read straight through, and a cue fills the ring
/// where the transport stands**, as natively: the host is asked for the
/// loop's start after its end, and [`UGen::cued`] answers whether the ring
/// holds the standing frame and [`CUE_LEAD`] past it.
pub struct TransportDiskIn {
    id: u32,
    consumer: Consumer<f32>,
    meters: Arc<Meters>,
    sent: usize,
    looping: bool,
    /// The transport's loop in the list's frames, as natively.
    wrap: Option<ListWrap>,
    /// Where the transport stands over the list while it is stopped, as
    /// natively: what a cue fills the ring at.
    standing: Option<u64>,
    head: Option<u64>,
    /// Since the last refill nothing has arrived: its silence is the refill's.
    fresh: bool,
    channels: usize,
    frames: u64,
    /// Served on the spot, in an offline render whose host reads files
    /// ([`PageFiles`]): a refill lands inside the call, as natively.
    serve: bool,
    /// The old list's next frames, kept when a replacement threw the ring
    /// away while it played: allocated once, with the reader.
    kept: Vec<f32>,
    /// What of `kept` plays out: its channels, the next sample and the end
    /// -- `None` when nothing is playing out.
    outgoing: Option<Kept>,
    /// Frames of fade-in left for the list that replaced it.
    fade_in: usize,
    /// What the list it replaced plays over the fade-in, this reader's
    /// channel of it, as natively: the two are mixed. Silence where the
    /// replaced list ran out.
    faded: [f32; crate::dsp::stream::CROSS],
    /// Where the transport stands in the block after the last one it
    /// played, as natively: only a list replaced while it plays on fades.
    continues: Option<u64>,
    /// The loop in force, in the list's frames, and the list frame at the
    /// transport's frame 0, as the last block read them, as natively.
    applied: Option<ListWrap>,
    delta: f64,
}

/// **What of `kept` plays out**, as natively: its channels, the next sample
/// and the end, whether the new reading is spliced on at its end (a loop
/// set, moved or cleared) rather than crossfaded in, and the loop it was
/// read through.
#[derive(Clone, Copy)]
struct Kept {
    channels: usize,
    next: usize,
    end: usize,
    /// Where the new reading is spliced on, as a sample of the kept frames.
    at: usize,
    splice: bool,
    wrap: Option<ListWrap>,
    /// The list frame the next kept sample is, as natively: the kept frames
    /// go where the transport plays something else before the splice.
    frame: u64,
}

/// **How many samples of the old list a replacement keeps** to play while
/// the new one is read: the native reader's count, 85 ms of stereo at
/// 48 kHz, which a page's round trip to its files is well inside.
pub const KEPT: usize = crate::dsp::stream::KEPT;

impl TransportDiskIn {
    pub fn open(ctx: &BuildCtx) -> Self {
        let meters = &ctx.meters;
        let (producer, consumer) = RingBuffer::new(crate::dsp::stream::RING_SAMPLES);
        let id = fresh_id();
        register(Slot {
            id,
            direction: Direction::List,
            path: String::new(),
            channels: 0,
            looping: false,
            start: 0,
            format: String::new(),
            end: End::Feeding(producer),
            list: Some(ListState {
                list: None,
                next: None,
                generation: 0,
                looping: false,
                wrap: None,
            }),
            offline: None,
        });
        Self {
            id,
            consumer,
            meters: Arc::clone(meters),
            sent: 0,
            looping: false,
            wrap: None,
            standing: None,
            head: None,
            fresh: true,
            channels: 1,
            frames: 0,
            serve: ctx.offline && files().is_some(),
            kept: vec![0.0; KEPT],
            outgoing: None,
            fade_in: 0,
            faded: [0.0; crate::dsp::stream::CROSS],
            continues: None,
            applied: None,
            delta: 0.0,
        }
    }

    /// **Keeps the old list's next frames aside** before a replacement throws
    /// the ring away, to play while the new one is read: no more than
    /// `agree`, the frames the new reading reads alike where they are
    /// bounded.
    fn keep_outgoing(&mut self, frame: u64, agree: Option<u64>) {
        // from the frame the transport plays now
        if let Some(head) = self.head
            && head < frame
        {
            let have = (self.consumer.slots() / self.channels) as u64;
            let n = (frame - head).min(have);
            if let Ok(chunk) = self.consumer.read_chunk(n as usize * self.channels) {
                chunk.commit_all();
            }
            self.head = Some(self.advance(head, n));
        }
        let channels = self.channels;
        // all it holds of the old reading, which plays on past where the two
        // part if the loop is not in place by then, as natively
        let n = self.consumer.slots().min(KEPT / channels * channels);
        let at = agree.map_or(n, |f| (f as usize * channels).min(n));
        if n < channels {
            return;
        }
        if let Ok(chunk) = self.consumer.read_chunk(n) {
            let (a, b) = chunk.as_slices();
            self.kept[..a.len()].copy_from_slice(a);
            self.kept[a.len()..a.len() + b.len()].copy_from_slice(b);
        }
        self.outgoing = Some(Kept {
            channels,
            next: 0,
            end: n,
            at,
            splice: false,
            wrap: self.wrap,
            frame,
        });
    }

    /// The next sample of the list a replacement took over from: it plays
    /// while the new one is read, and once the new one has come it is
    /// crossfaded in over the old one's next frames.
    fn take_outgoing(&mut self, frame: u64, channel: usize) -> f32 {
        use crate::dsp::stream::CROSS;
        let Some(Kept {
            channels,
            mut next,
            end,
            at,
            splice,
            wrap,
            frame: kept_frame,
        }) = self.outgoing
        else {
            return 0.0;
        };
        // the kept frames are the take only where the transport plays them,
        // as natively: apart before the splice, they go
        if kept_frame != frame && next + channels <= at {
            self.outgoing = None;
            self.faded = [0.0; CROSS];
            self.fade_in = CROSS;
            return 0.0;
        }
        let step = |f: u64| ListWrap::advance(wrap, f, 1);
        if splice {
            // at the splice the new reading goes on, as natively: in from
            // silence where it is not in yet -- or, where the loop it reads is
            // not in place yet, the kept frames play on and the old reading
            // is read again from their end
            if next + channels > at {
                if self.applied != self.wrap && self.applied == wrap && next + channels <= end {
                    if let Some(kept) = self.outgoing.as_mut() {
                        kept.at = end;
                    }
                    self.wrap = wrap;
                    let left = ((end - next) / channels) as u64;
                    self.refill(None, ListWrap::advance(wrap, frame, left));
                    return self.take_outgoing(frame, channel);
                }
                self.outgoing = None;
                let seamless = self.head == Some(frame) && self.consumer.slots() >= self.channels;
                self.faded = [0.0; CROSS];
                self.fade_in = if seamless { 0 } else { CROSS };
                return 0.0;
            }
            let value = self.kept[next + channel.min(channels - 1)];
            if let Some(kept) = self.outgoing.as_mut() {
                kept.next += channels;
                kept.frame = step(kept.frame);
            }
            return value;
        }
        // The new list has landed once it holds the frame the transport is at
        // and the fade's length past it: what played meanwhile is skipped.
        let behind = self.head.map_or(0, |h| frame.saturating_sub(h)) as usize;
        // the old list's frames left, which the new one may wait on, and the
        // lead it lands with, as natively
        let left = (end - next) / channels;
        let holds = self.consumer.slots() / self.channels;
        let lead = crate::dsp::stream::LAND_LEAD;
        let landed = holds >= behind + CROSS + lead || (holds >= behind + CROSS && left <= CROSS);
        if landed {
            let pick = channel.min(channels - 1);
            self.faded = [0.0; CROSS];
            for slot in self.faded.iter_mut() {
                if next + channels > end {
                    break;
                }
                *slot = self.kept[next + pick];
                next += channels;
            }
            self.outgoing = None;
            self.fade_in = CROSS;
            return 0.0;
        }
        if next + channels > end {
            // the new list did not come while the old one played: a silence,
            // and the new one fades in from it, as natively
            self.outgoing = None;
            self.faded = [0.0; CROSS];
            self.fade_in = CROSS;
            return 0.0;
        }
        let value = self.kept[next + channel.min(channels - 1)];
        if let Some(kept) = self.outgoing.as_mut() {
            kept.next = next + channels;
            kept.frame = step(kept.frame);
        }
        value
    }

    fn refill(&mut self, list: Option<Arc<Buffer>>, frame: u64) {
        let stale = self.consumer.slots();
        if let Ok(chunk) = self.consumer.read_chunk(stale) {
            chunk.commit_all();
        }
        refill_slot(self.id, list, frame, self.looping, self.wrap);
        self.head = Some(frame);
        self.fresh = true;
    }

    fn advance(&self, frame: u64, by: u64) -> u64 {
        if self.looping && self.frames > 0 {
            (frame + by) % self.frames
        } else {
            ListWrap::advance(self.wrap, frame, by)
        }
    }

    /// Whether the ring holds frame `frame` and the lead a play wants past
    /// it, as natively.
    fn holds(&self, frame: u64) -> bool {
        let ahead = if self.looping || ListWrap::before(self.wrap, frame).is_some() {
            CUE_LEAD as u64
        } else {
            self.frames.saturating_sub(frame).min(CUE_LEAD as u64)
        };
        let ring = (crate::dsp::stream::RING_SAMPLES / self.channels) as u64;
        self.outgoing.is_none()
            && self.head == Some(frame)
            && (self.consumer.slots() / self.channels) as u64 >= ahead.min(ring)
    }

    /// **Frame `frame` of the list, read the moment it is needed**, in an
    /// offline render whose host reads files on the spot: what would be a
    /// wait natively is a read here, so a frame is never played as silence.
    fn take_served(&mut self, frame: u64, channel: usize) -> f32 {
        let ring = (crate::dsp::stream::RING_SAMPLES / self.channels) as u64;
        if let Some(head) = self.head
            && head < frame
            && frame - head <= ring
        {
            let have = (self.consumer.slots() / self.channels) as u64;
            let n = (frame - head).min(have);
            if let Ok(chunk) = self.consumer.read_chunk(n as usize * self.channels) {
                chunk.commit_all();
            }
            self.head = Some(self.advance(head, n));
        }
        if self.head != Some(frame) {
            self.refill(None, frame);
        }
        if self.consumer.slots() < self.channels {
            serve_list(self.id);
        }
        if self.head != Some(frame) || self.consumer.slots() < self.channels {
            return 0.0;
        }
        let mut value = 0.0;
        for c in 0..self.channels {
            let v = self.consumer.pop().unwrap_or(0.0);
            if c == channel {
                value = v;
            }
        }
        self.head = Some(self.advance(frame, 1));
        value
    }

    fn take(&mut self, frame: u64, channel: usize, starved: &mut u64) -> f32 {
        if self.serve {
            return self.take_served(frame, channel);
        }
        if self.outgoing.is_some() {
            let value = self.take_outgoing(frame, channel);
            if self.outgoing.is_some() {
                return value;
            }
        }
        let Some(head) = self.head else {
            self.refill(None, frame);
            return 0.0;
        };
        if head != frame {
            let ring = (crate::dsp::stream::RING_SAMPLES / self.channels) as u64;
            if head < frame && frame - head <= ring {
                let have = (self.consumer.slots() / self.channels) as u64;
                let n = (frame - head).min(have);
                if let Ok(chunk) = self.consumer.read_chunk(n as usize * self.channels) {
                    chunk.commit_all();
                }
                self.head = Some(self.advance(head, n));
            } else {
                self.refill(None, frame);
                return 0.0;
            }
        }
        if self.head != Some(frame) || self.consumer.slots() < self.channels {
            if !self.fresh {
                *starved += 1;
            }
            return 0.0;
        }
        self.fresh = false;
        let mut value = 0.0;
        for c in 0..self.channels {
            let v = self.consumer.pop().unwrap_or(0.0);
            if c == channel {
                value = v;
            }
        }
        self.head = Some(self.advance(frame, 1));
        if self.fade_in > 0 {
            let cross = crate::dsp::stream::CROSS;
            let k = cross - self.fade_in;
            let t = k as f32 / cross as f32;
            value = value * t + self.faded[k] * (1.0 - t);
            self.fade_in -= 1;
        }
        value
    }
}

impl UGen for TransportDiskIn {
    fn reset(&mut self) {
        // A stream is the host's and cannot be restarted from here: a
        // `Voices` node refuses a def that holds one.
    }

    fn cued(&self) -> Option<bool> {
        Some(self.standing.is_none_or(|f| self.holds(f)))
    }

    fn looped(&self, transport: &crate::dsp::TransportCtx) -> Option<bool> {
        // as natively: the reading follows the loop asked for and holds the
        // lead it goes on with, before a splice it waits on is reached
        if self.looping {
            return Some(true);
        }
        let asked = ListWrap::of(
            transport.pending_looping.unwrap_or(transport.looping),
            self.delta,
            self.frames,
        );
        // as natively: frames kept from a change before read it already
        if asked != self.wrap {
            return Some(self.kept_reads(asked) >= crate::dsp::stream::LAND_LEAD as u64);
        }
        // the kept frames before a splice count, as natively
        let kept = match &self.outgoing {
            Some(o) if o.splice && o.next > o.at => return Some(false),
            Some(o) if o.splice => ((o.at - o.next) / o.channels) as u64,
            _ => 0,
        };
        let ring = match self.head {
            Some(_) => (self.consumer.slots() / self.channels) as u64,
            None => 0,
        };
        let lead = crate::dsp::stream::LAND_LEAD as u64;
        let ahead = match (self.wrap, self.head) {
            (None, Some(head)) => self.frames.saturating_sub(head).min(lead),
            _ => lead,
        };
        Some(ring + kept >= ahead.max(1))
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let next = ctx.transport.position + output.len() as u64;
        let rolling = ctx.transport.rolling;
        self.play(ctx, inputs, output);
        self.continues = rolling.then_some(next);
    }
}

impl TransportDiskIn {
    /// **How many of the frames kept from a change read the loop `asked`**:
    /// the kept frames of a loop change (spliced, the same list read another
    /// way) from where they stand to where their reading and that one part,
    /// or to their end. A change of the list keeps frames of another take,
    /// and none of them count.
    fn kept_reads(&self, asked: Option<ListWrap>) -> u64 {
        let Some(o) = self.outgoing.as_ref().filter(|o| o.splice) else {
            return 0;
        };
        let left = ((o.end - o.next) / o.channels) as u64;
        ListWrap::agree(o.wrap, asked, o.frame).map_or(left, |n| n.min(left))
    }

    /// **What the loop in force plays from `frame`, kept**, as natively
    /// (`stream::TransportDiskIn::keep_in_force`): the kept frames to their
    /// splice and what the ring holds of the loop in force after it.
    fn keep_in_force(&mut self, frame: u64) {
        let Some(o) = self.outgoing.as_ref() else {
            return;
        };
        let channels = o.channels;
        let (from, to) = (o.next, o.at);
        let spliced = ListWrap::advance(self.wrap, frame, ((to - from) / channels) as u64);
        if !o.splice
            || from > to
            || o.frame != frame
            || channels != self.channels
            || self.applied != self.wrap
            || self.head != Some(spliced)
        {
            return;
        }
        self.kept.copy_within(from..to, 0);
        let prefix = to - from;
        let room = (KEPT / channels * channels).saturating_sub(prefix);
        let n = self.consumer.slots().min(room) / channels * channels;
        if n > 0
            && let Ok(chunk) = self.consumer.read_chunk(n)
        {
            let (a, b) = chunk.as_slices();
            self.kept[prefix..prefix + a.len()].copy_from_slice(a);
            self.kept[prefix + a.len()..prefix + a.len() + b.len()].copy_from_slice(b);
            chunk.commit_all();
            self.head = Some(self.advance(spliced, (n / channels) as u64));
        }
        self.outgoing = Some(Kept {
            channels,
            next: 0,
            end: prefix + n,
            at: prefix + n,
            splice: true,
            wrap: self.wrap,
            frame,
        });
    }

    /// One block of [`UGen::process`].
    fn play(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let index = inputs[0][0].max(0.0) as usize;
        let Some(buf) = ctx.buffers.get(index).and_then(|b| b.as_ref()) else {
            self.standing = None;
            output.fill(0.0);
            return;
        };
        let frames = buf.frames() as u64;
        let looping = inputs[4][0] != 0.0;
        let base = ctx.transport.position as f64;
        let rolling = ctx.transport.rolling;
        let delta = at(inputs[3], 0) as f64 - at(inputs[2], 0) as f64;
        // a loop asked for is read before it is in place, as natively
        let wrap = match looping {
            true => None,
            false => ListWrap::of(
                ctx.transport
                    .pending_looping
                    .unwrap_or(ctx.transport.looping),
                delta,
                frames,
            ),
        };
        self.applied = match looping {
            true => None,
            false => ListWrap::of(ctx.transport.looping, delta, frames),
        };
        self.delta = delta;
        let place = |i: usize| -> (Option<u64>, Option<u64>) {
            if frames == 0 {
                return (None, None);
            }
            let played = if rolling { i as f64 } else { 0.0 };
            let raw = base + played - at(inputs[2], i) as f64 + at(inputs[3], i) as f64;
            if looping {
                let f = (raw.rem_euclid(frames as f64) as u64).min(frames - 1);
                (Some(f), Some(f))
            } else if raw < 0.0 {
                (None, Some(0))
            } else if raw >= frames as f64 {
                (None, None)
            } else {
                (Some(raw as u64), Some(raw as u64))
            }
        };
        self.standing = if rolling { None } else { place(0).1 };
        let address = Arc::as_ptr(buf) as usize;
        if address != self.sent || looping != self.looping || wrap != self.wrap {
            // **Only the loop changed**: the same list read another way, which
            // is spliced on where the kept frames end, as natively.
            let rewrapped = address == self.sent && looping == self.looping;
            let frame0 = place(0).0.unwrap_or(0);
            let mut from = None;
            // A list that was playing plays on, out of what is kept of it,
            // while the new one is read.
            if rolling
                && self.continues == Some(ctx.transport.position)
                && !self.serve
                && !self.fresh
                && self.outgoing.is_none()
            {
                // the old reading is right only as far as the new one reads
                // the same frames, as natively
                let agree = ListWrap::agree(self.wrap, wrap, frame0);
                self.keep_outgoing(frame0, agree);
                if rewrapped && let Some(kept) = self.outgoing.as_mut() {
                    kept.splice = true;
                    let frames = (kept.at / kept.channels) as u64;
                    from = Some(ListWrap::advance(wrap, frame0, frames));
                }
            } else if self.outgoing.is_some() {
                // a change before the last one came, as natively: what the
                // loop in force plays is kept first where the kept frames
                // read another loop than this one
                if rewrapped && self.kept_reads(wrap) < crate::dsp::stream::LAND_LEAD as u64 {
                    self.keep_in_force(frame0);
                }
                let Some(kept) = self.outgoing.as_mut() else {
                    unreachable!("kept just above")
                };
                if rewrapped && kept.splice {
                    let mut left = (kept.end - kept.next) / kept.channels;
                    if let Some(agree) = ListWrap::agree(kept.wrap, wrap, frame0) {
                        left = left.min(agree as usize);
                    }
                    kept.at = kept.next + left * kept.channels;
                    from = Some(ListWrap::advance(wrap, frame0, left as u64));
                } else {
                    kept.splice = false;
                }
            }
            self.looping = looping;
            self.wrap = wrap;
            self.sent = address;
            self.channels = buf.channels().max(1);
            self.frames = frames;
            let at = from.unwrap_or_else(|| place(0).1.unwrap_or(0));
            self.refill(Some(Arc::clone(buf)), at);
            if !self.serve && self.outgoing.is_none() {
                output.fill(0.0);
                return;
            }
        }
        let channel = (inputs[1][0].max(0.0) as usize).min(self.channels - 1);
        if !rolling {
            self.outgoing = None;
            self.fade_in = 0;
            output.fill(0.0);
            if let Some(f) = self.standing
                && self.head != Some(f)
            {
                self.refill(None, f);
            }
            return;
        }
        let mut starved = 0u64;
        for (i, s) in output.iter_mut().enumerate() {
            *s = match place(i) {
                (Some(f), _) => self.take(f, channel, &mut starved),
                (None, Some(ready)) => {
                    if self.head != Some(ready) {
                        self.refill(None, ready);
                    }
                    0.0
                }
                (None, None) => 0.0,
            };
        }
        if starved > 0 {
            self.meters.starve(starved);
        }
    }
}

impl Drop for TransportDiskIn {
    fn drop(&mut self) {
        release(self.id);
    }
}

// ---- PeaksOut ----

/// Writes the peaks file of its inputs, the page's half of `dsp::peaks_out`:
/// the buckets are folded here exactly as natively, and the host drains them
/// and writes the file through the same core once the synth is freed.
pub struct PeaksOut {
    active: Option<PeaksActive>,
}

struct PeaksActive {
    id: u32,
    producer: Producer<f32>,
    acc: Box<[(f32, f32, f64)]>,
    channels: usize,
    filled: usize,
    frames: u64,
    lost: bool,
}

/// The most channels one `PeaksOut` summarizes, as natively.
const PEAKS_CHANNELS: usize = 64;
const PEAKS_EMPTY: (f32, f32, f64) = (f32::INFINITY, f32::NEG_INFINITY, 0.0);

impl PeaksOut {
    pub fn open(config: &UGenConfig, _ctx: &BuildCtx) -> Self {
        let Some(path) = config.path.clone() else {
            tracing::warn!("PeaksOut has no path; it will write nothing");
            return Self { active: None };
        };
        let (producer, consumer) = RingBuffer::new(RING_SAMPLES);
        let id = fresh_id();
        register(Slot {
            id,
            direction: Direction::Peaks,
            path,
            channels: 0,
            looping: false,
            start: 0,
            format: String::new(),
            end: End::Draining(consumer),
            list: None,
            offline: None,
        });
        Self {
            active: Some(PeaksActive {
                id,
                producer,
                acc: vec![PEAKS_EMPTY; PEAKS_CHANNELS].into_boxed_slice(),
                channels: 0,
                filled: 0,
                frames: 0,
                lost: false,
            }),
        }
    }
}

impl PeaksActive {
    fn push(&mut self) {
        let n = self.channels * 3;
        let filled = self.filled.max(1) as f64;
        match self.producer.write_chunk_uninit(n) {
            Ok(chunk) => {
                chunk.fill_from_iter(
                    self.acc[..self.channels]
                        .iter()
                        .flat_map(|&(lo, hi, sq)| [lo, hi, (sq / filled) as f32]),
                );
            }
            Err(_) => self.lost = true,
        }
        self.acc[..self.channels].fill(PEAKS_EMPTY);
        self.filled = 0;
    }

    /// Tells the registry what the host needs to write the file.
    fn publish(&self, ended: bool) {
        let (id, channels, frames, lost) = (self.id, self.channels, self.frames, self.lost);
        STREAMS.with(|s| {
            if let Some(slot) = s.borrow_mut().iter_mut().find(|slot| slot.id == id) {
                slot.channels = channels;
                slot.start = frames;
                slot.looping = ended;
                if lost {
                    slot.format = "lost".into();
                }
            }
        });
    }
}

impl UGen for PeaksOut {
    fn reset(&mut self) {
        // A file is the host's and cannot be restarted from here: a `Voices`
        // node refuses a def that holds one.
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let first = inputs.first().copied().unwrap_or(&[]);
        for (i, s) in output.iter_mut().enumerate() {
            *s = at(first, i);
        }
        let Some(a) = self.active.as_mut() else {
            return;
        };
        if a.channels == 0 {
            a.channels = inputs.len().clamp(1, PEAKS_CHANNELS);
        }
        for i in 0..output.len() {
            for (c, cell) in a.acc[..a.channels].iter_mut().enumerate() {
                let x = inputs.get(c).map_or(0.0, |input| at(input, i));
                cell.0 = cell.0.min(x);
                cell.1 = cell.1.max(x);
                cell.2 += f64::from(x) * f64::from(x);
            }
            a.filled += 1;
            if a.filled == crate::dsp::summary::SUMMARY_BUCKET {
                a.push();
            }
        }
        a.frames += output.len() as u64;
        a.publish(false);
    }
}

impl Drop for PeaksOut {
    fn drop(&mut self) {
        // The stream stays registered until the host has drained it and
        // written the file (`release`); it is only told the synth is gone.
        if let Some(a) = self.active.as_mut() {
            if a.filled > 0 && a.channels > 0 {
                a.push();
            }
            a.publish(true);
        }
    }
}
