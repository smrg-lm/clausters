//! **A segment list played from disk**: `TransportDiskIn`, the reader of a
//! list whose parts name files.
//!
//! A part of a segment list names a pool buffer or a span of a PCM file, with
//! the same fades, rate and channel map either way (`dsp::stitch`); only where
//! its samples are differs. A list of pool buffers is read at random by
//! `BufRd` and its family, because its samples are in memory. A list with a
//! file part cannot be: an hour of take is not in the pool, and reading it
//! where the audio thread asks would be I/O on the audio thread. So it is
//! **streamed**, by `DiskIn`'s design generalized -- one I/O thread per
//! reader, racing the audio thread through a lock-free ring -- and the
//! generalization is that the stream **follows the transport** instead of
//! running from a file's top:
//!
//! - The audio thread works out which frame of the list each sample plays, as
//!   `TransportBufRd` does (`offset` is the transport frame at which frame
//!   `start` of the list plays). While those frames are the ones at the
//!   ring's head, it pops them.
//! - When they are not -- a locate, a transport standing somewhere new -- it
//!   **asks for a refill** from that frame and plays silence until it
//!   arrives. It never plays what the ring held before: that was another
//!   place in the list.
//! - **A list replaced while it plays is not a silence** -- a multitrack's
//!   clip moved while it plays, say. The ring still holds the old list ahead
//!   of the playhead, so that plays on while the new one is read; once the
//!   refill lands, the old fades out and the new fades in over [`CROSS`]
//!   frames each, from the frame the transport has reached.
//! - **A transport's loop is read straight through**: the reader is told the
//!   loop's span (`TransportCtx::looping`, as [`ListWrap`] frames of the
//!   list), the I/O thread renders from its end on at its start, and the
//!   head wraps with the position, so a wrap is in the ring before it is
//!   played. A loop set, moved or cleared is the list read another way, a
//!   replacement like the one above.
//! - **A cue fills the ring where the transport stands** (`/transport_cue`):
//!   the engine runs the reader one standing block at a time, and
//!   [`UGen::cued`] says whether it holds that frame and [`CUE_LEAD`] past
//!   it, so the play after the cue starts on frames already read.
//! - The I/O thread renders frames of the list ahead of the transport,
//!   part by part, through [`PartSpan::read_with`](clausters_core::segments::PartSpan::read_with) --
//!   the read a pool buffer's part makes, over the span it just read from the
//!   file. A seam is therefore nothing to the audio thread: the next part's
//!   frames are rendered into the ring before it gets there, and the next
//!   part's file is opened [`SEAM_LEAD`] frames ahead of its seam.
//!
//! # Which frames are in the ring
//!
//! A refill is an **epoch**. The audio thread sends a request carrying the
//! frame and a new epoch number and stops reading; the I/O thread, on taking
//! it, publishes the epoch together with how many samples it had pushed so
//! far -- the **boundary** -- and pushes the new frames after it. The audio
//! thread, seeing its epoch published, throws away everything before the
//! boundary in one commit (no per-sample work) and knows the ring's head is
//! the frame it asked for. One atomic carries both numbers, so the two are
//! read together.
//!
//! # What running out of lead sounds like
//!
//! If the ring is empty where the audio thread needs a frame, that sample is
//! **silence, counted**: `Meters::starve` adds it to a total the server
//! reports (`/server_status`), so a session that outruns the disk says so in
//! a number and never as a click from a stale frame. The frames that went
//! missing are skipped when they arrive, so the list stays where the
//! transport is.
//!
//! The audio thread neither allocates nor waits: it pops, commits and sends a
//! request through a fixed ring, and wakes the I/O thread with
//! [`std::thread::Thread::unpark`] -- a futex wake, the way a real-time
//! thread signals a disk thread. The list reaches the I/O thread as an `Arc`
//! cloned on the audio thread (a count incremented) and dropped over there.
//!
//! Mono per UGen, like every buffer reader here: `chan` picks one channel of
//! the list, and a stereo clip is two readers with the same inputs.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;

use rtrb::{Consumer, Producer, RingBuffer};

use crate::dsp::buffer::Buffer;
use crate::dsp::fed::{Fed, FedEnds, FeedRequest, FeedShared, Feeder};
use crate::dsp::pcm::{Layout, Reader};
use crate::dsp::{CUE_LEAD, ListWrap, ProcessCtx, UGen, at};
use crate::server::meters::{Meters, Role};

/// The ring, in samples: about 2.7 s of mono at 48 kHz, 1.4 s of stereo.
///
/// It is the lead the reader keeps over the transport, and it only has to
/// cover a slow read: the next part's file is opened ahead of its seam, so a
/// seam costs the ring nothing it does not cost in the middle of a part.
pub const RING_SAMPLES: usize = 1 << 17;

/// **How far ahead of a seam the next part's file is opened**, in frames of
/// the list. Opening a file and reading its first span is the one cost a
/// seam adds to a read; done this far ahead, it happens while the ring still
/// holds the frames before the seam.
pub const SEAM_LEAD: usize = 1 << 14;

/// How many frames a replaced list fades out over, and the new one in
/// ([`crate::dsp::LIST_CROSS`], the page's reader's too).
pub const CROSS: usize = crate::dsp::LIST_CROSS;

/// The frames rendered in one turn of the I/O thread.
const CHUNK: usize = 4096;

/// The first turn after a refill renders this few, so the frames the audio
/// thread is waiting for arrive as soon as one short read allows.
const FIRST_CHUNK: usize = 256;

/// How many refills may be in flight. The audio thread asks once per block at
/// most, and the I/O thread takes only the latest.
const REQUESTS: usize = 8;

/// How many files one reader keeps open: a list cut from many takes reopens
/// one only after this many others were read since.
const OPEN_FILES: usize = 8;

/// The low 48 bits of the published word: the boundary in samples.
const BOUNDARY: u64 = (1 << 48) - 1;

/// A refill: start rendering from `frame`, of `list` if it names one and of
/// the list already held otherwise.
struct Request {
    list: Option<Arc<Buffer>>,
    frame: u64,
    epoch: u16,
    looping: bool,
    wrap: Option<ListWrap>,
}

/// **The list a replacement took over from**, still in the ring ahead of the
/// new one: it plays on until the refill lands, and the new one is then
/// crossfaded in over it.
struct Outgoing {
    /// Its channels, which the ring's frames are interleaved by.
    channels: usize,
    /// Its samples left in the ring, once the refill has landed and the
    /// boundary is known; unknown before.
    left: Option<usize>,
}

/// What the two threads share besides the rings.
struct Shared {
    stop: AtomicBool,
    /// The epoch taken (high 16 bits) and the samples pushed before its first
    /// frame (low 48).
    ack: AtomicU64,
}

/// Streams a segment list from disk, following the transport. Inputs:
/// 0 `bufnum` (the list, or any buffer), 1 `chan`, 2 `offset` (the transport
/// frame at which frame `start` plays), 3 `start`, 4 `loop`. Outside the
/// list, and while the transport is stopped, it is silent.
pub struct TransportDiskIn {
    consumer: Consumer<f32>,
    requests: Producer<Request>,
    shared: Arc<Shared>,
    thread: std::thread::Thread,
    handle: Option<JoinHandle<()>>,
    meters: Arc<Meters>,
    /// The list last sent to the I/O thread, as an address: it holds that
    /// `Arc`, so the address cannot be reused while it is the one compared.
    sent: usize,
    looping: bool,
    /// The transport's loop in the list's frames, which the I/O thread reads
    /// straight through.
    wrap: Option<ListWrap>,
    /// Where the transport stands over the list while it is stopped: the
    /// frame a cue fills the ring at. `None` while it rolls, and outside the
    /// list.
    standing: Option<u64>,
    /// The epoch last asked for.
    epoch: u16,
    /// The frame the refill in flight starts at.
    pending: Option<u64>,
    /// The list frame at the ring's head, once a refill has landed.
    head: Option<u64>,
    /// Samples popped or thrown away since the start, the boundary's measure.
    popped: u64,
    /// The channels and frames of the list the current epoch reads.
    channels: usize,
    frames: u64,
    /// In an offline render: wait for a refill and for each frame, as
    /// scsynth's NRT reads a file, rather than play silence.
    offline: bool,
    /// The list a replacement while playing took over from, playing out.
    outgoing: Option<Outgoing>,
    /// Frames of fade-in left for the list that replaced it.
    fade_in: usize,
    /// **What the list it replaced plays over the fade-in**, this reader's
    /// channel of it: the replaced list's next frames, taken out of the ring
    /// as the new one lands, so the two are mixed rather than one faded out
    /// before the other comes in. Silence where the replaced list ran out.
    faded: [f32; CROSS],
    /// **The stream its owner feeds**, while the buffer it plays is one
    /// (`dsp::fed`): its rings are in use, and this reader's own are kept
    /// aside.
    fed: Option<FedLink>,
    /// A fed stream played before, kept so that leaving it frees nothing on
    /// the audio thread.
    retired: Option<FedLink>,
    /// Where the transport stands in the block after the last one this
    /// reader played, while it plays on: only a list replaced while it is
    /// **playing on** fades. A play after a stop, a locate or a thaw at
    /// another place is not that, whatever the ring holds from before; a
    /// reader the transport froze ran no block in between, so continuity of
    /// the position is what tells the two apart.
    continues: Option<u64>,
}

/// A fed stream a reader is playing: the requests' producer, what the halves
/// share, and this reader's own ring kept aside meanwhile.
struct FedLink {
    /// The fed stream, as an address: the buffer holds that `Arc`.
    address: usize,
    requests: Box<Producer<FeedRequest>>,
    shared: Arc<FeedShared>,
    /// The owner's generation last played: a new one is new contents.
    generation: u64,
    /// This reader's own ring and its measure, until a list in the pool is
    /// played again, in the box the fed ring came in: the two are swapped,
    /// never moved out of it, so nothing is freed on the audio thread.
    own: Box<Consumer<f32>>,
    own_popped: u64,
}

impl TransportDiskIn {
    /// Starts the reader's I/O thread, which accounts its work to the build's
    /// load table as a `disk-in` thread.
    pub fn open(ctx: &crate::dsp::registry::BuildCtx) -> Self {
        let meters = &ctx.meters;
        let (producer, consumer) = RingBuffer::new(RING_SAMPLES);
        let (requests, inbox) = RingBuffer::new(REQUESTS);
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            ack: AtomicU64::new(0),
        });
        let handle = {
            let shared = Arc::clone(&shared);
            let turn = crate::dsp::disk::Turn::new(Arc::clone(meters), Role::DiskIn);
            std::thread::Builder::new()
                .name("diskin-list".into())
                .spawn(move || fill(inbox, producer, shared, turn))
                .expect("failed to spawn the TransportDiskIn thread")
        };
        Self {
            consumer,
            requests,
            shared,
            thread: handle.thread().clone(),
            handle: Some(handle),
            meters: Arc::clone(meters),
            sent: 0,
            looping: false,
            wrap: None,
            standing: None,
            epoch: 0,
            pending: None,
            head: None,
            popped: 0,
            channels: 1,
            frames: 0,
            offline: ctx.offline,
            outgoing: None,
            fade_in: 0,
            faded: [0.0; CROSS],
            fed: None,
            retired: None,
            continues: None,
        }
    }

    /// Where the refill last taken is published: the owner's word while a
    /// fed stream plays, the I/O thread's otherwise.
    fn ack(&self) -> &AtomicU64 {
        match &self.fed {
            Some(link) => &link.shared.ack,
            None => &self.shared.ack,
        }
    }

    /// Wakes whoever renders into the ring.
    fn wake(&self) {
        match &self.fed {
            Some(link) => link.shared.wake(),
            None => self.thread.unpark(),
        }
    }

    /// **Plays the stream `fed` feeds, or a list of the pool again**: the
    /// rings change over, and what the old ones held is nothing to the new.
    /// A fed stream another reader took first answers `false` and is silent.
    fn follow_source(&mut self, fed: Option<&Arc<Fed>>) -> bool {
        let address = fed.map_or(0, |f| Arc::as_ptr(f) as usize);
        if self.fed.as_ref().map_or(0, |l| l.address) == address {
            return true;
        }
        // Back to this reader's own ring first, whatever comes next. The
        // stream left is kept, not dropped: its rings are freed with the
        // reader, off the audio thread.
        if let Some(mut link) = self.fed.take() {
            std::mem::swap(&mut self.consumer, &mut *link.own);
            self.popped = link.own_popped;
            if self.retired.is_none() {
                self.retired = Some(link);
            }
        }
        self.pending = None;
        self.head = None;
        self.outgoing = None;
        self.fade_in = 0;
        let Some(fed) = fed else {
            return true;
        };
        let Some(FedEnds {
            consumer: mut own,
            requests,
        }) = fed.take()
        else {
            return false;
        };
        std::mem::swap(&mut self.consumer, &mut *own);
        let own_popped = std::mem::replace(&mut self.popped, 0);
        self.fed = Some(FedLink {
            address,
            requests,
            shared: Arc::clone(fed.shared()),
            generation: fed.shared().generation.load(Ordering::Acquire),
            own,
            own_popped,
        });
        // A fresh ring: nothing the old epochs said applies to it.
        self.sent = 0;
        true
    }

    /// Asks for a refill from `frame`, of `list` when it changed. Fails, and
    /// changes nothing, when the request ring is full: the next block asks
    /// again.
    fn refill(&mut self, list: Option<Arc<Buffer>>, frame: u64) -> bool {
        let epoch = self.epoch.wrapping_add(1);
        let pushed = match self.fed.as_mut() {
            // The owner knows what it renders: the request names where.
            Some(link) => link
                .requests
                .push(FeedRequest {
                    frame,
                    epoch,
                    wrap: self.wrap,
                })
                .is_ok(),
            None => self
                .requests
                .push(Request {
                    list,
                    frame,
                    epoch,
                    looping: self.looping,
                    wrap: self.wrap,
                })
                .is_ok(),
        };
        if !pushed {
            return false;
        }
        self.epoch = epoch;
        self.pending = Some(frame);
        self.head = None;
        self.wake();
        if self.offline {
            self.collect();
        }
        true
    }

    /// Lands the refill in flight once the I/O thread has taken it: what it
    /// pushed before is thrown away in one commit, and the head is the frame
    /// that was asked for.
    fn collect(&mut self) {
        let Some(frame) = self.pending else {
            return;
        };
        if self.offline {
            let (ack, epoch) = (self.ack(), self.epoch);
            crate::dsp::disk::wait_for(|| (ack.load(Ordering::Acquire) >> 48) as u16 == epoch);
        }
        let ack = self.ack().load(Ordering::Acquire);
        if (ack >> 48) as u16 != self.epoch {
            return;
        }
        let stale = (ack & BOUNDARY).saturating_sub(self.popped) as usize;
        // Everything before the boundary was pushed before it was published,
        // so it is all in the ring.
        let stale = stale.min(self.consumer.slots());
        if let Some(outgoing) = self.outgoing.as_mut() {
            // The list it replaced fades out of what is left of it first.
            outgoing.left = Some(stale);
        } else {
            if let Ok(chunk) = self.consumer.read_chunk(stale) {
                chunk.commit_all();
            }
            self.popped += stale as u64;
        }
        self.pending = None;
        self.head = Some(frame);
        // The thread may be parked on a ring that was full of what was just
        // thrown away.
        self.wake();
    }

    /// Throws away `frames` frames at the head, as many as are there.
    fn skip(&mut self, frames: u64) -> u64 {
        let have = (self.consumer.slots() / self.channels) as u64;
        let n = frames.min(have);
        let samples = n as usize * self.channels;
        if let Ok(chunk) = self.consumer.read_chunk(samples) {
            chunk.commit_all();
        }
        self.popped += samples as u64;
        if let Some(h) = self.head {
            self.head = Some(self.advance(h, n));
        }
        n
    }

    fn advance(&self, frame: u64, by: u64) -> u64 {
        if self.looping && self.frames > 0 {
            (frame + by) % self.frames
        } else {
            ListWrap::advance(self.wrap, frame, by)
        }
    }

    /// Whether the ring holds frame `frame` and the lead a play wants past
    /// it: [`CUE_LEAD`] frames, or what is left before the list ends.
    fn holds(&self, frame: u64) -> bool {
        let ahead = if self.looping || ListWrap::before(self.wrap, frame).is_some() {
            CUE_LEAD as u64
        } else {
            self.frames.saturating_sub(frame).min(CUE_LEAD as u64)
        };
        let ring = (RING_SAMPLES / self.channels) as u64;
        self.pending.is_none()
            && self.outgoing.is_none()
            && self.head == Some(frame)
            && (self.consumer.slots() / self.channels) as u64 >= ahead.min(ring)
    }

    /// **The next sample of the list a replacement took over from**: it
    /// plays on while the new one is read, and fades out once the new one has
    /// landed; what is left of it then goes, and the new one fades in.
    fn take_outgoing(&mut self, frame: u64, channel: usize) -> f32 {
        // The new list has landed once its refill is in and it holds the
        // frame the transport is at and the fade's length past it: what
        // played meanwhile is skipped.
        let behind = self.head.map_or(0, |h| frame.saturating_sub(h)) as usize;
        let slots = self.consumer.slots();
        let Some(outgoing) = self.outgoing.as_mut() else {
            return 0.0;
        };
        let channels = outgoing.channels;
        let landed = outgoing
            .left
            .is_some_and(|left| slots.saturating_sub(left) / self.channels >= behind + CROSS);
        // Until the boundary is known, never read so near the ring's end that
        // what is read could be the new list's first frames.
        let readable = match outgoing.left {
            Some(left) => left,
            None => self.consumer.slots().saturating_sub(CROSS * channels),
        };
        // Landed, the new list takes over from this frame, crossfaded over
        // what the old one plays next.
        if landed || readable < channels {
            self.end_outgoing(channel);
            return 0.0;
        }
        let mut value = 0.0;
        for c in 0..channels {
            let v = self.consumer.pop().unwrap_or(0.0);
            if c == channel.min(channels - 1) {
                value = v;
            }
        }
        self.popped += channels as u64;
        if let Some(left) = self.outgoing.as_mut().and_then(|o| o.left.as_mut()) {
            *left -= channels;
        }
        value
    }

    /// **The outgoing list is done**: the frames of it the fade-in plays over
    /// are kept (`channel`'s, as many as are left, up to the fade's length),
    /// what is left of it goes, and the list that replaced it is crossfaded
    /// in from here.
    fn end_outgoing(&mut self, channel: usize) {
        self.faded = [0.0; CROSS];
        if let Some(Outgoing {
            channels,
            left: Some(mut left),
        }) = self.outgoing.take()
        {
            let pick = channel.min(channels - 1);
            for slot in self.faded.iter_mut() {
                if left < channels {
                    break;
                }
                for c in 0..channels {
                    let v = self.consumer.pop().unwrap_or(0.0);
                    if c == pick {
                        *slot = v;
                    }
                }
                left -= channels;
                self.popped += channels as u64;
            }
            if let Ok(chunk) = self.consumer.read_chunk(left.min(self.consumer.slots())) {
                chunk.commit_all();
            }
            self.popped += left as u64;
            self.fade_in = CROSS;
        } else {
            // the refill never landed while it played: a silence, as before
            self.fade_in = 0;
        }
    }

    /// The sample of `channel` at list frame `frame`, or silence -- counted
    /// in `starved` when the frame should have been there and was not.
    fn take(&mut self, frame: u64, channel: usize, starved: &mut u64) -> f32 {
        if self.outgoing.is_some() {
            let value = self.take_outgoing(frame, channel);
            if self.outgoing.is_some() || self.pending.is_some() {
                return value;
            }
        }
        if self.pending.is_some() {
            return 0.0;
        }
        let Some(head) = self.head else {
            // Offline the refill is waited for and lands at once.
            if !self.refill(None, frame) || !self.offline {
                return 0.0;
            }
            return self.take(frame, channel, starved);
        };
        if head != frame {
            let ring = (RING_SAMPLES / self.channels) as u64;
            if head < frame && frame - head <= ring {
                // Behind: frames that arrived late, or never. They are
                // skipped so the list stays where the transport is.
                self.skip(frame - head);
            } else {
                if !self.refill(None, frame) || !self.offline || self.pending.is_some() {
                    return 0.0;
                }
                return self.take(frame, channel, starved);
            }
        }
        if self.offline && self.head == Some(frame) && self.consumer.slots() < self.channels {
            let (consumer, channels) = (&self.consumer, self.channels);
            crate::dsp::disk::wait_for(|| consumer.slots() >= channels);
        }
        if self.head != Some(frame) || self.consumer.slots() < self.channels {
            *starved += 1;
            return 0.0;
        }
        let mut value = 0.0;
        for c in 0..self.channels {
            let v = self.consumer.pop().unwrap_or(0.0);
            if c == channel {
                value = v;
            }
        }
        self.popped += self.channels as u64;
        self.head = Some(self.advance(frame, 1));
        if self.fade_in > 0 {
            let k = CROSS - self.fade_in;
            let t = k as f32 / CROSS as f32;
            value = value * t + self.faded[k] * (1.0 - t);
            self.fade_in -= 1;
        }
        value
    }
}

impl UGen for TransportDiskIn {
    fn reset(&mut self) {
        // A stream is its I/O thread's and cannot be restarted from here: a
        // `Voices` node refuses a def that holds one.
    }

    fn cued(&self) -> Option<bool> {
        Some(self.standing.is_none_or(|f| self.holds(f)))
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let next = ctx.transport.position + output.len() as u64;
        let rolling = ctx.transport.rolling;
        self.play(ctx, inputs, output);
        self.continues = rolling.then_some(next);
    }
}

impl TransportDiskIn {
    /// One block of [`UGen::process`].
    fn play(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        self.collect();
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
        // The transport's loop, read straight through: what its wrap plays
        // next is in the ring before the position gets there. A list that
        // loops on its own wraps at its own end instead.
        let delta = at(inputs[3], 0) as f64 - at(inputs[2], 0) as f64;
        let wrap = match looping {
            true => None,
            false => ListWrap::of(ctx.transport.looping, delta, frames),
        };
        // Which list frame sample `i` plays: `None` outside the list, and the
        // frame to stand ready at besides.
        let place = |i: usize| -> (Option<u64>, Option<u64>) {
            if frames == 0 {
                return (None, None);
            }
            let played = if rolling { i as f64 } else { 0.0 };
            let raw = base + played - at(inputs[2], i) as f64 + at(inputs[3], i) as f64;
            if looping {
                let f = raw.rem_euclid(frames as f64) as u64;
                (Some(f.min(frames - 1)), Some(f.min(frames - 1)))
            } else if raw < 0.0 {
                (None, Some(0))
            } else if raw >= frames as f64 {
                (None, None)
            } else {
                let f = raw as u64;
                (Some(f), Some(f))
            }
        };
        self.standing = if rolling { None } else { place(0).1 };
        // A fed buffer is read from its owner's rings; one another reader
        // took first is silent here.
        if !self.follow_source(buf.fed()) {
            self.standing = None;
            output.fill(0.0);
            return;
        }
        // The owner renders something else now: a list replaced in place.
        let generation = self
            .fed
            .as_ref()
            .map(|l| l.shared.generation.load(Ordering::Acquire))
            .filter(|&g| self.fed.as_ref().is_some_and(|l| l.generation != g));
        let address = Arc::as_ptr(buf) as usize;
        if address != self.sent
            || looping != self.looping
            || wrap != self.wrap
            || generation.is_some()
        {
            // A new list, or the same one read another way -- a loop set,
            // moved or cleared is one: what the ring holds is not what will
            // play. A list that was playing plays on until the refill lands
            // and then fades into the new one; one that was not is silence
            // until the refill lands.
            let playing = rolling
                && self.continues == Some(ctx.transport.position)
                && !self.offline
                && self.head.is_some()
                && self.pending.is_none()
                && self.outgoing.is_none();
            let was = self.channels;
            let previous_wrap = (self.looping, self.wrap);
            self.looping = looping;
            self.wrap = wrap;
            let frame = place(0).1.unwrap_or(0);
            // The shape first: offline the refill lands inside the call.
            let (channels, previous) = (buf.channels().max(1), (self.channels, self.frames));
            self.channels = channels;
            self.frames = frames;
            let list = self.fed.is_none().then(|| Arc::clone(buf));
            if self.refill(list, frame) {
                self.sent = address;
                if let (Some(link), Some(g)) = (self.fed.as_mut(), generation) {
                    link.generation = g;
                }
                if playing {
                    self.outgoing = Some(Outgoing {
                        channels: was,
                        left: None,
                    });
                }
            } else {
                (self.channels, self.frames) = previous;
                (self.looping, self.wrap) = previous_wrap;
            }
            if self.outgoing.is_none() && (!self.offline || self.pending.is_some()) {
                output.fill(0.0);
                return;
            }
        }
        let channel = (inputs[1][0].max(0.0) as usize).min(self.channels - 1);
        if !rolling {
            // Standing: silent, and ready at the frame it stands on. A list
            // that was playing out is done with.
            if self.outgoing.is_some() {
                self.end_outgoing(channel);
                self.fade_in = 0;
            }
            output.fill(0.0);
            if let Some(f) = self.standing
                && self.head != Some(f)
                && self.pending != Some(f)
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
                    // Before the list: ready at its first frame.
                    if self.head != Some(ready) && self.pending != Some(ready) {
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
        self.shared.stop.store(true, Ordering::Release);
        self.thread.unpark();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// The files one reader has open, the most recently read first.
struct Files {
    open: Vec<Reader>,
}

impl Files {
    fn reader(&mut self, layout: &Layout) -> Option<&mut Reader> {
        let found = self.open.iter().position(|r| r.layout() == layout);
        let reader = match found {
            Some(i) => self.open.remove(i),
            None => match Reader::open(layout) {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("TransportDiskIn: {e}; the part is silent");
                    return None;
                }
            },
        };
        self.open.insert(0, reader);
        self.open.truncate(OPEN_FILES);
        self.open.first_mut()
    }
}

/// **The two ends a list's renderer works between**: where requests come
/// from and frames go, and where a refill is published. A reader's own I/O
/// thread has its rings; a fed stream's owner has a [`Feeder`] and the list
/// it is told to render.
trait Side {
    /// Whether the renderer should stop.
    fn stopped(&self) -> bool;
    /// The latest request, with the list it names when it names one.
    fn latest(&mut self) -> Option<(FeedRequest, bool, Option<Arc<Buffer>>)>;
    /// A list the owner put in place since the last turn.
    fn new_list(&mut self) -> Option<Arc<Buffer>> {
        None
    }
    /// Room for samples.
    fn room(&self) -> usize;
    /// Pushes interleaved samples.
    fn push(&mut self, samples: &[f32]);
    /// Publishes `epoch` with its boundary.
    fn publish(&self, epoch: u16, boundary: u64);
}

/// A reader's own I/O thread's ends.
struct Own {
    inbox: Consumer<Request>,
    producer: Producer<f32>,
    shared: Arc<Shared>,
}

impl Side for Own {
    fn stopped(&self) -> bool {
        self.shared.stop.load(Ordering::Acquire)
    }

    fn latest(&mut self) -> Option<(FeedRequest, bool, Option<Arc<Buffer>>)> {
        let mut latest: Option<Request> = None;
        let mut list = None;
        while let Ok(request) = self.inbox.pop() {
            if request.list.is_some() {
                list = request.list.clone();
            }
            latest = Some(request);
        }
        latest.map(|r| {
            (
                FeedRequest {
                    frame: r.frame,
                    epoch: r.epoch,
                    wrap: r.wrap,
                },
                r.looping,
                list,
            )
        })
    }

    fn room(&self) -> usize {
        self.producer.slots()
    }

    fn push(&mut self, samples: &[f32]) {
        if let Ok(chunk) = self.producer.write_chunk_uninit(samples.len()) {
            chunk.fill_from_iter(samples.iter().copied());
        }
    }

    fn publish(&self, epoch: u16, boundary: u64) {
        self.shared.ack.store(
            ((epoch as u64) << 48) | (boundary & BOUNDARY),
            Ordering::Release,
        );
    }
}

/// The I/O thread: takes the latest refill, renders the list from there into
/// the ring, and opens each next part's file ahead of its seam.
fn fill(
    inbox: Consumer<Request>,
    producer: Producer<f32>,
    shared: Arc<Shared>,
    turn: crate::dsp::disk::Turn,
) {
    render_list(
        Own {
            inbox,
            producer,
            shared,
        },
        turn,
    );
}

/// **A list rendered into a ring, following a reader's requests**: the loop
/// of a reader's own I/O thread, and of a fed stream's owner.
fn render_list(mut side: impl Side, mut turn: crate::dsp::disk::Turn) {
    let mut list: Option<Arc<Buffer>> = None;
    let mut next: Option<u64> = None;
    let mut looping = false;
    let mut wrap = None;
    let mut first = false;
    // The refill taken and not yet published: its epoch and its boundary.
    let mut due: Option<(u16, u64)> = None;
    let mut pushed: u64 = 0;
    let mut files = Files { open: Vec::new() };
    let mut frames = Vec::new();
    let mut span = Vec::new();
    loop {
        if side.stopped() {
            return;
        }
        if let Some(placed) = side.new_list() {
            list = Some(placed);
        }
        if let Some((request, loops, named)) = side.latest() {
            if named.is_some() {
                list = named;
            }
            next = Some(request.frame);
            looping = loops;
            wrap = request.wrap;
            first = true;
            due = Some((request.epoch, pushed));
        }
        // **A refill is published once its first frames are in**, so the
        // audio thread that lands it finds them there and a locate is not
        // counted as running out of lead. A ring still full of what the
        // refill makes stale is the exception: publishing is what lets the
        // audio thread throw that away.
        let publish = |due: &mut Option<(u16, u64)>, side: &dyn Side| {
            if let Some((epoch, boundary)) = due.take() {
                side.publish(epoch, boundary);
            }
        };
        let (Some(l), Some(at)) = (list.as_ref(), next) else {
            publish(&mut due, &side);
            turn.park();
            continue;
        };
        let channels = l.channels().max(1);
        let total = l.frames() as u64;
        if at >= total {
            if looping && total > 0 {
                next = Some(0);
            } else {
                publish(&mut due, &side);
                turn.park();
            }
            continue;
        }
        let want = if first { FIRST_CHUNK } else { CHUNK };
        let room = side.room() / channels;
        // A read stops at the loop's end, and the next one starts at its
        // start: the ring holds the loop straight through.
        let left = ListWrap::before(wrap, at).unwrap_or(total - at) as usize;
        let n = want.min(room).min(left);
        if n == 0 || (!first && room < want && left > room) {
            publish(&mut due, &side);
            turn.park();
            continue;
        }
        turn.worked();
        render(l, at, n, &mut frames, &mut span, &mut files);
        side.push(&frames);
        pushed += frames.len() as u64;
        let after = ListWrap::advance(wrap, at, n as u64);
        next = Some(after);
        first = false;
        publish(&mut due, &side);
        open_ahead(l, after, wrap, &mut files);
    }
}

/// **A fed stream's owner rendering a list**: the [`Feeder`]'s end, the list
/// put in place by [`FedList::set`], and the stop.
struct FedSide {
    feeder: Feeder,
    placed: Arc<std::sync::Mutex<Option<Arc<Buffer>>>>,
    stop: Arc<AtomicBool>,
}

impl Side for FedSide {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    fn latest(&mut self) -> Option<(FeedRequest, bool, Option<Arc<Buffer>>)> {
        self.feeder.latest().map(|r| (r, false, None))
    }

    fn new_list(&mut self) -> Option<Arc<Buffer>> {
        self.placed.lock().ok()?.take()
    }

    fn room(&self) -> usize {
        self.feeder.room()
    }

    fn push(&mut self, samples: &[f32]) {
        self.feeder.push(samples);
    }

    fn publish(&self, epoch: u16, boundary: u64) {
        self.feeder.shared().ack.store(
            ((epoch as u64) << 48) | (boundary & BOUNDARY),
            Ordering::Release,
        );
    }
}

/// **A thread of the program that owns a fed stream, rendering a list into
/// it** as the reader asks: what an application that holds its engine runs
/// beside it for each take it plays. [`FedList::set`] puts the list it
/// renders in place and tells the reader the contents changed; dropping it
/// stops the thread.
pub struct FedList {
    placed: Arc<std::sync::Mutex<Option<Arc<Buffer>>>>,
    stop: Arc<AtomicBool>,
    shared: Arc<FeedShared>,
    thread: std::thread::Thread,
    handle: Option<JoinHandle<()>>,
}

impl FedList {
    /// Starts the thread over `feeder`, accounting its work to `meters` as a
    /// `disk-in` thread.
    pub fn start(feeder: Feeder, meters: Arc<Meters>) -> FedList {
        let placed = Arc::new(std::sync::Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::clone(feeder.shared());
        let side = FedSide {
            feeder,
            placed: Arc::clone(&placed),
            stop: Arc::clone(&stop),
        };
        let handle = std::thread::Builder::new()
            .name("fed-list".into())
            .spawn(move || {
                side.feeder.wake_me();
                render_list(side, crate::dsp::disk::Turn::new(meters, Role::DiskIn));
            })
            .expect("failed to spawn a fed list's thread");
        FedList {
            placed,
            stop,
            shared,
            thread: handle.thread().clone(),
            handle: Some(handle),
        }
    }

    /// **Renders `list` from now on**: the reader fades from what its ring
    /// holds into a refill of it, from where it is.
    pub fn set(&self, list: Arc<Buffer>) {
        if let Ok(mut placed) = self.placed.lock() {
            *placed = Some(list);
        }
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        self.thread.unpark();
    }
}

impl Drop for FedList {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.thread.unpark();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Renders `n` frames of `list` from `at` into `out`, interleaved.
fn render(
    list: &Buffer,
    at: u64,
    n: usize,
    out: &mut Vec<f32>,
    span: &mut Vec<f32>,
    files: &mut Files,
) {
    let channels = list.channels().max(1);
    out.clear();
    out.resize(n * channels, 0.0);
    let at = at as usize;
    if let Some(stitch) = list.stitch() {
        let mut k = 0;
        while k < n {
            let Some((_, part)) = stitch.part_at(at + k) else {
                break;
            };
            let inner = at + k - part.start();
            let len = (part.end() - (at + k)).min(n - k);
            let dst = &mut out[k * channels..(k + len) * channels];
            if let Some(layout) = part.src.file() {
                let (first, end) = part.source_span(inner, inner + len);
                let read = files
                    .reader(layout)
                    .map(|r| r.read(first as u64, end - first, span));
                if !matches!(read, Some(Ok(_))) {
                    span.clear();
                }
                let width = layout.channels;
                let span = &*span;
                for (j, frame) in dst.chunks_mut(channels).enumerate() {
                    for (c, cell) in frame.iter_mut().enumerate() {
                        *cell = part.read_with(inner + j, c, |f, sc| {
                            f.checked_sub(first)
                                .and_then(|f| span.get(f * width + sc))
                                .copied()
                                .unwrap_or(0.0)
                        });
                    }
                }
            } else {
                for (j, frame) in dst.chunks_mut(channels).enumerate() {
                    for (c, cell) in frame.iter_mut().enumerate() {
                        *cell = part.read_with(inner + j, c, |f, sc| part.src.sample(f, sc));
                    }
                }
            }
            k += len;
        }
    } else if let Some(layout) = list.file() {
        if let Some(reader) = files.reader(layout)
            && reader.read(at as u64, n, span).is_ok()
        {
            out[..span.len()].copy_from_slice(span);
        }
    } else {
        for (j, frame) in out.chunks_mut(channels).enumerate() {
            for (c, cell) in frame.iter_mut().enumerate() {
                *cell = list.sample(at + j, c);
            }
        }
    }
}

/// Opens the next part's file when the frames rendered are within
/// [`SEAM_LEAD`] of its seam -- or, before a loop's end, the file its start
/// reads.
fn open_ahead(list: &Buffer, at: u64, wrap: Option<ListWrap>, files: &mut Files) {
    let Some(stitch) = list.stitch() else {
        return;
    };
    if let Some(w) = wrap
        && ListWrap::before(wrap, at).is_some_and(|left| left <= SEAM_LEAD as u64)
        && let Some(layout) = stitch
            .part_at(w.from as usize)
            .and_then(|(_, p)| p.src.file())
    {
        files.reader(layout);
    }
    let Some((i, part)) = stitch.part_at(at as usize) else {
        return;
    };
    if part.end() - at as usize > SEAM_LEAD {
        return;
    }
    if let Some(layout) = stitch.parts().get(i + 1).and_then(|p| p.src.file()) {
        files.reader(layout);
    }
}
