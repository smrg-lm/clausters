//! Streaming disk I/O in a page: the same two UGens, fed by the host.
//!
//! The native module ([`crate::dsp::disk`]'s other half) gives each UGen a
//! background thread that opens a file and races the audio thread through a
//! lock-free ring. A page has neither half of that: no `std::thread`, and no
//! filesystem the engine can reach — the private one it does have (OPFS) is a
//! JS API, and a synchronous handle on it exists only in a dedicated Worker.
//!
//! So the ring stays and the thread goes. **The host is the reader.** A stream
//! registers itself when the synth is built, the host asks what each open
//! stream wants ([`poll`]) and fills or drains it ([`push`], [`pull`]); the
//! `process` half is unchanged, and so is what it does when the host is late —
//! an underrun plays silence, an overrun drops samples, exactly as a slow disk
//! would. That is the honest shape of it: without shared memory the two threads
//! cannot share a ring, so the chunks are *moved* across and **how far ahead
//! the host reads is the design**, not a tuning constant.
//!
//! **A file's shape arrives late, and that is fine.** Natively `DiskIn::open`
//! opens the file and learns its channel count on the spot. Here it cannot —
//! reading is asynchronous and belongs to another thread — so a stream is born
//! not knowing, plays silence while it does not, and is told ([`set_shape`])
//! when the host has looked. Nothing declares anything up front, which matters
//! for more than convenience: a declaration would be a call the other client
//! has no counterpart for, and this way the surface a script writes against is
//! the same one in both.
//!
//! The registry is a `thread_local`, and that is not a shortcut: everything
//! that touches it — building a synth, dropping one, and the host's own calls —
//! happens on the one thread a page's engine has.

use std::cell::RefCell;

use rtrb::{Consumer, Producer, RingBuffer};

use crate::dsp::registry::UGenConfig;
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
    /// `DiskIn`: the file's channel count, as declared. `DiskOut`: 1 — it
    /// writes a mono file, as it does natively.
    pub channels: usize,
    /// `DiskIn`: restart from the top at end of stream.
    pub looping: bool,
    /// `DiskOut`: the WAV sample format (`int16` | `int24` | `float`).
    pub format: String,
    /// `DiskIn`: room in the ring, in samples — how much the host may push.
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
    format: String,
    end: End,
}

thread_local! {
    static STREAMS: RefCell<Vec<Slot>> = const { RefCell::new(Vec::new()) };
    static NEXT_ID: RefCell<u32> = const { RefCell::new(0) };
}

/// Tells a stream how many channels its file has, once the host has looked.
///
/// A `DiskIn` reports `channels: 0` until this arrives and plays silence
/// meanwhile; the host sees the zero in [`poll`] and knows the one thing it
/// still owes. `0` here is ignored — a file with no channels is a file that
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

fn release(id: u32) {
    STREAMS.with(|s| s.borrow_mut().retain(|slot| slot.id != id));
}

/// Every open stream and what it wants. The host walks this each turn: it is
/// the whole interface between the graph and whatever is reading files.
///
/// A stream whose synth has been freed is simply not here any more.
pub fn poll() -> Vec<StreamRequest> {
    STREAMS.with(|s| {
        s.borrow()
            .iter()
            .map(|slot| StreamRequest {
                id: slot.id,
                direction: slot.direction,
                path: slot.path.clone(),
                channels: slot.channels,
                looping: slot.looping,
                format: slot.format.clone(),
                samples: match &slot.end {
                    End::Feeding(p) => p.slots(),
                    End::Draining(c) => c.slots(),
                },
            })
            .collect()
    })
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
}

impl DiskIn {
    pub fn open(config: &UGenConfig) -> Self {
        let Some(path) = config.path.clone() else {
            tracing::warn!("DiskIn has no path; it will be silent");
            return Self { active: None };
        };
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
            channels: 0,
            looping: config.looping,
            format: String::new(),
            end: End::Feeding(producer),
        });
        Self {
            active: Some(Active {
                id,
                consumer: Some(consumer),
                producer: None,
                channels: 0,
            }),
        }
    }
}

impl UGen for DiskIn {
    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let Some(a) = self.active.as_mut() else {
            output.fill(0.0);
            return;
        };
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
    pub fn open(config: &UGenConfig) -> Self {
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
            format,
            end: End::Draining(consumer),
        });
        Self {
            active: Some(Active {
                id,
                consumer: None,
                producer: Some(producer),
                channels: 1,
            }),
        }
    }
}

impl UGen for DiskOut {
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
        if let Some(a) = self.active.as_ref() {
            release(a.id);
        }
    }
}
