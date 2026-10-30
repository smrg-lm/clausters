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

use rtrb::{Consumer, Producer, RingBuffer};

use crate::dsp::registry::{BuildCtx, UGenConfig};
use crate::dsp::{ProcessCtx, UGen, at};

/// Ring capacity in samples, as natively (~1.4 s of mono at 48 kHz).
const RING_SAMPLES: usize = 1 << 16;

/// Which way a stream runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    /// A file being read into the graph (`DiskIn`).
    In,
    /// The graph being written to a file (`DiskOut`).
    Out,
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
/// need to: a `DiskIn` its next run. What is
/// rendered is then what a native render renders. The output (`DiskOut`)
/// needs nothing of it: the render drains it between two runs, as the host
/// does live.
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

/// Forgets a stream: what the host calls once it has drained one whose
/// synth is gone.
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
            });
        }
        out
    })
}

/// The frames an offline `DiskIn` reads in one run: 0.4 s at 48 kHz.
const READ_SPAN: usize = 19_200;

fn push_frames(producer: &mut Producer<f32>, frames: &[f32]) {
    if let Ok(chunk) = producer.write_chunk_uninit(frames.len()) {
        chunk.fill_from_iter(frames.iter().copied());
    }
}

/// **Serves a `DiskIn` on the spot**: runs of its file from where it has read to, until its ring is full or the file is
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
                .min(READ_SPAN)
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
