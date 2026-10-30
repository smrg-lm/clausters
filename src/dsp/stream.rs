//! **A fed buffer played at the transport's position**: `TransportDiskIn`,
//! the reader of a stream its owner feeds (`dsp::fed`).
//!
//! An application that holds its engine reads its takes itself -- a list of
//! parts over files, rendered by its own reader -- and hands the frames to
//! the engine through a fed buffer: a ring the owner fills and this reader
//! drains, racing each other lock-free. The engine knows no file, list or
//! part; what it knows is where the transport is, and so this reader
//! **follows the transport**:
//!
//! - The audio thread works out which frame of the stream each sample plays,
//!   as `TransportBufRd` does (`offset` is the transport frame at which frame
//!   `start` of the stream plays). While those frames are the ones at the
//!   ring's head, it pops them.
//! - When they are not -- a locate, a transport standing somewhere new -- it
//!   **asks its owner for a refill** from that frame and plays silence until
//!   it arrives. It never plays what the ring held before: that was another
//!   place in the stream.
//! - **New contents while it plays are not a silence** -- a take edited while
//!   it plays, say. The ring still holds the old reading ahead of the
//!   playhead, so that plays on while the new one is read; once the refill
//!   lands, the old fades out and the new fades in over [`CROSS`] frames
//!   each, from the frame the transport has reached.
//! - **A transport's loop is read straight through**: the owner is told the
//!   loop's span (as [`ListWrap`] frames of the stream), renders from its end
//!   on at its start, and the head wraps with the position, so a wrap is in
//!   the ring before it is played.
//! - **A cue fills the ring where the transport stands**: the engine runs the
//!   reader one standing block at a time, and [`UGen::cued`] says whether it
//!   holds that frame and [`CUE_LEAD`] past it, so the play after the cue
//!   starts on frames already read.
//!
//! A buffer that is not fed plays silence: nothing else is a stream.
//!
//! # Which frames are in the ring
//!
//! A refill is an **epoch**. The audio thread sends a request carrying the
//! frame and a new epoch number and stops reading; the owner, on taking it,
//! publishes the epoch together with how many samples it had pushed so far --
//! the **boundary** -- and pushes the new frames after it. The audio thread,
//! seeing its epoch published, throws away everything before the boundary in
//! one commit (no per-sample work) and knows the ring's head is the frame it
//! asked for. One atomic carries both numbers, so the two are read together.
//!
//! # What running out of lead sounds like
//!
//! If the ring is empty where the audio thread needs a frame, that sample is
//! **silence, counted**: `Meters::starve` adds it to a total the server
//! reports (`/server_status`), so a session that outruns its owner says so in
//! a number and never as a click from a stale frame. The frames that went
//! missing are skipped when they arrive, so the stream stays where the
//! transport is.
//!
//! The audio thread neither allocates nor waits: it pops, commits and sends a
//! request through a fixed ring, and wakes the owner's thread.
//!
//! Mono per UGen, like every buffer reader here: `chan` picks one channel of
//! the stream, and a stereo clip is two readers with the same inputs.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rtrb::{Consumer, Producer, RingBuffer};

use crate::dsp::fed::{BOUNDARY, Fed, FedEnds, FeedRequest, FeedShared};
use crate::dsp::{CUE_LEAD, ListWrap, ProcessCtx, UGen, at};
use crate::server::meters::Meters;

/// The ring, in samples: the fed stream's ([`crate::dsp::fed::FED_RING_SAMPLES`]),
/// about 2.7 s of mono at 48 kHz, 1.4 s of stereo -- the lead the reader keeps
/// over the transport.
pub const RING_SAMPLES: usize = crate::dsp::fed::FED_RING_SAMPLES;

/// How many frames a replaced list fades out over, and the new one in
/// ([`crate::dsp::LIST_CROSS`], the page's reader's too).
pub const CROSS: usize = crate::dsp::LIST_CROSS;

/// How many samples of the old reading a replacement keeps.
pub const KEPT: usize = crate::dsp::LIST_KEPT;

/// How far ahead a replacing reading holds before it takes over.
pub const LAND_LEAD: usize = crate::dsp::LIST_LAND_LEAD;

/// **The reading a replacement took over from**, its next frames kept aside
/// (`TransportDiskIn::kept`): it plays on from there until the new reading
/// lands, which is then crossfaded in over it.
struct Outgoing {
    /// Its channels, which the kept frames are interleaved by.
    channels: usize,
    /// The next sample of it to play, and the end of what was kept.
    next: usize,
    end: usize,
    /// **Where the new reading is spliced on**, as a sample of the kept
    /// frames: where the two readings part, or the end of what was kept.
    at: usize,
    /// **Whether the new reading goes on from where this one ends**, rather
    /// than replacing it: a loop set, moved or cleared reads what the kept
    /// frames read up to where the two part, so the new reading is asked for
    /// from there and spliced on, with no fade. A change the engine has not
    /// put in place by then is not: the kept frames play on past it, and the
    /// old reading is read again from their end.
    splice: bool,
    /// The loop the kept frames were read through.
    wrap: Option<ListWrap>,
    /// **The list frame the next kept sample is**: what the transport has to
    /// be playing for the kept frames to be the take. A loop the engine put
    /// in place for a reader that was not ready leaves the two apart, and the
    /// kept frames go.
    frame: u64,
}

/// Plays a fed buffer, following the transport. Inputs: 0 `bufnum` (the fed
/// buffer), 1 `chan`, 2 `offset` (the transport frame at which frame `start`
/// plays), 3 `start`, 4 `loop`. Outside the stream, while the transport is
/// stopped, and over a buffer that is not fed, it is silent.
pub struct TransportDiskIn {
    /// The ring it reads: the fed stream's, while one plays, and a placeholder
    /// of its own otherwise, which the two are swapped with.
    consumer: Consumer<f32>,
    meters: Arc<Meters>,
    /// The buffer last asked to be read, as an address: the pool holds that
    /// `Arc` while it plays, so the address is the buffer's.
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
    /// In an offline render: wait for a refill and for each frame, rather
    /// than play silence.
    offline: bool,
    /// The list a replacement while playing took over from, playing out.
    outgoing: Option<Outgoing>,
    /// **The old reading's next frames**, kept when a replacement threw the
    /// ring away while it played: allocated once, with the reader. The ring
    /// then holds one reading only, so a second change before the first one
    /// lands never plays what the first one had begun to read.
    kept: Vec<f32>,
    /// Frames of fade-in left for the list that replaced it.
    fade_in: usize,
    /// **The reader fell short of a transport playing on**: the old
    /// reading's kept frames ran out before the new one landed (played to
    /// their end or to the splice), or the transport went a ring past what
    /// was read. The silence until the refill lands is the ring out of lead
    /// and is counted as starved -- unlike the silence a locate, a play from
    /// a stop or a transport gone elsewhere lands on.
    short: bool,
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
    /// Whether the block being played goes on from the one before
    /// (`continues`), noted once a block: a refill this reader falls behind
    /// into then is a shortage, and one a locate asks for is not.
    playing_on: bool,
    /// The loop in force, in the list's frames, as the last block read it:
    /// what decides at a splice whether the new reading or the old one goes
    /// on. A loop asked for and not yet in place is not it.
    applied: Option<ListWrap>,
    /// The list frame its offset and start put at the transport's frame 0,
    /// as the last block read it: what a loop asked for is read through.
    delta: f64,
    /// The ring was at half or less after the last block: whoever fills it
    /// was woken as it fell there, and is not woken again until it has
    /// filled it past half.
    low: bool,
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
    /// This reader's own ring and its measure, until it plays no fed stream
    /// again, in the box the fed ring came in: the two are swapped,
    /// never moved out of it, so nothing is freed on the audio thread.
    own: Box<Consumer<f32>>,
    own_popped: u64,
}

impl TransportDiskIn {
    /// A reader with nothing to play yet: its ring is a placeholder, swapped
    /// for a fed stream's when it plays one.
    pub fn open(ctx: &crate::dsp::registry::BuildCtx) -> Self {
        let meters = &ctx.meters;
        let (_, consumer) = RingBuffer::<f32>::new(1);
        Self {
            consumer,
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
            kept: vec![0.0; KEPT],
            fade_in: 0,
            short: false,
            faded: [0.0; CROSS],
            fed: None,
            retired: None,
            continues: None,
            playing_on: false,
            applied: None,
            delta: 0.0,
            low: true,
        }
    }

    /// Where the refill last taken is published: the owner's word, or nothing
    /// while no fed stream plays.
    fn ack(&self) -> Option<&AtomicU64> {
        self.fed.as_ref().map(|link| &link.shared.ack)
    }

    /// Wakes the owner that renders into the ring.
    fn wake(&self) {
        if let Some(link) = &self.fed {
            link.shared.wake();
        }
    }

    /// **Plays the stream `fed` feeds, or nothing**: the rings change over, and what the old ones held is nothing to the new.
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
        self.short = false;
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

    /// Asks the owner for a refill from `frame`. Fails, and changes nothing,
    /// when the request ring is full (the next block asks again) or no fed
    /// stream plays.
    fn refill(&mut self, frame: u64) -> bool {
        let epoch = self.epoch.wrapping_add(1);
        // The owner knows what it renders: the request names where.
        let pushed = self.fed.as_mut().is_some_and(|link| {
            link.requests
                .push(FeedRequest {
                    frame,
                    epoch,
                    wrap: self.wrap,
                })
                .is_ok()
        });
        if !pushed {
            return false;
        }
        self.epoch = epoch;
        self.pending = Some(frame);
        self.head = None;
        self.short = false;
        self.wake();
        if self.offline {
            self.collect();
        }
        true
    }

    /// Lands the refill in flight once the owner has taken it: what it
    /// pushed before is thrown away in one commit, and the head is the frame
    /// that was asked for.
    fn collect(&mut self) {
        let Some(frame) = self.pending else {
            return;
        };
        let Some(ack) = self.ack() else {
            return;
        };
        if self.offline {
            let epoch = self.epoch;
            wait_for(|| (ack.load(Ordering::Acquire) >> 48) as u16 == epoch);
        }
        let ack = ack.load(Ordering::Acquire);
        if (ack >> 48) as u16 != self.epoch {
            return;
        }
        let stale = (ack & BOUNDARY).saturating_sub(self.popped) as usize;
        // Everything before the boundary was pushed before it was published,
        // so it is all in the ring.
        let stale = stale.min(self.consumer.slots());
        if let Ok(chunk) = self.consumer.read_chunk(stale) {
            chunk.commit_all();
        }
        self.popped += stale as u64;
        self.pending = None;
        self.short = false;
        self.head = Some(frame);
        // The owner may be parked on a ring that was full of what was just
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

    /// **The next sample of the reading a replacement took over from**: it
    /// plays on from what was kept while the new one is read, and once the
    /// new one has landed -- its refill in, holding the frame the transport is
    /// at, the fade's length and [`LAND_LEAD`] past it, or what it can when
    /// the kept frames are running out -- the new one is crossfaded in over
    /// it. What played meanwhile is skipped in the new one.
    fn take_outgoing(&mut self, frame: u64, channel: usize) -> f32 {
        let behind = self.head.map_or(0, |h| frame.saturating_sub(h)) as usize;
        let Some(outgoing) = self.outgoing.as_ref() else {
            return 0.0;
        };
        // The kept frames are the take only where the transport plays them:
        // apart before the splice (where the two readings part, as they
        // should), they go, and the reading comes in from silence.
        if outgoing.frame != frame && outgoing.next + outgoing.channels <= outgoing.at {
            self.outgoing = None;
            self.faded = [0.0; CROSS];
            self.fade_in = CROSS;
            return 0.0;
        }
        let channels = outgoing.channels;
        // the old reading's frames left, which the new one may wait on
        let left = (outgoing.end - outgoing.next) / channels;
        let holds = self.consumer.slots() / self.channels;
        let landed = self.pending.is_none()
            && self.head.is_some()
            && (holds >= behind + CROSS + LAND_LEAD || (holds >= behind + CROSS && left <= CROSS));
        let Some(outgoing) = self.outgoing.as_mut() else {
            return 0.0;
        };
        if outgoing.splice {
            // **At the splice**: the new reading begins here, and goes on as
            // one reading with the kept frames -- or, where it is not in yet,
            // comes in from silence once it is. Where the loop it reads is
            // not in place yet, the kept frames are still the take: they play
            // on, and the old reading is read again from their end.
            if outgoing.next + channels > outgoing.at {
                let (end, wrap) = (outgoing.end, outgoing.wrap);
                if self.applied != self.wrap
                    && self.applied == wrap
                    && outgoing.next + channels <= end
                {
                    outgoing.at = end;
                    let left = ((end - outgoing.next) / channels) as u64;
                    self.discard();
                    self.wrap = wrap;
                    self.refill(ListWrap::advance(wrap, frame, left));
                    return self.take_outgoing(frame, channel);
                }
                self.outgoing = None;
                // played to the splice: the new reading is owed from here
                self.short = self.pending.is_some();
                let seamless = self.pending.is_none()
                    && self.head == Some(frame)
                    && self.consumer.slots() >= self.channels;
                self.faded = [0.0; CROSS];
                self.fade_in = if seamless { 0 } else { CROSS };
                return 0.0;
            }
        } else if landed || outgoing.next + channels > outgoing.end {
            self.end_outgoing(channel);
            // played to their end: the new reading is owed from here
            self.short = self.pending.is_some();
            return 0.0;
        }
        let value = self.kept[outgoing.next + channel.min(channels - 1)];
        outgoing.next += channels;
        outgoing.frame = if self.looping && self.frames > 0 {
            (outgoing.frame + 1) % self.frames
        } else {
            ListWrap::advance(outgoing.wrap, outgoing.frame, 1)
        };
        value
    }

    /// **The outgoing reading is done**: the frames of it the fade-in plays
    /// over are taken from what was kept (`channel`'s, up to the fade's
    /// length, silence past what is left), and the reading that replaced it
    /// is crossfaded in from here -- in from silence where the kept frames
    /// ran out before it landed.
    fn end_outgoing(&mut self, channel: usize) {
        self.faded = [0.0; CROSS];
        if let Some(Outgoing {
            channels,
            mut next,
            end,
            ..
        }) = self.outgoing.take()
        {
            let pick = channel.min(channels - 1);
            for slot in self.faded.iter_mut() {
                if next + channels > end {
                    break;
                }
                *slot = self.kept[next + pick];
                next += channels;
            }
        }
        self.fade_in = CROSS;
    }

    /// **Keeps the reading in hand aside and throws the ring away**: its next
    /// frames from `frame` (the one the transport plays now), as many as
    /// [`KEPT`] holds and no more than `agree` (the frames the new reading
    /// reads alike, where they are bounded), to play while the new reading is
    /// read into an empty ring. What the I/O thread pushes of the old reading
    /// after this is before the new one's boundary, and goes when it lands.
    fn keep_outgoing(&mut self, frame: u64, agree: Option<u64>) {
        if let Some(head) = self.head
            && head < frame
        {
            self.skip(frame - head);
        }
        let channels = self.channels.max(1);
        // all it holds of the old reading, which plays on past where the two
        // part if the loop is not in place by then
        let n = self.consumer.slots().min(KEPT / channels * channels);
        let at = agree.map_or(n, |f| (f as usize * channels).min(n));
        if n >= channels
            && let Ok(chunk) = self.consumer.read_chunk(n)
        {
            let (a, b) = chunk.as_slices();
            self.kept[..a.len()].copy_from_slice(a);
            self.kept[a.len()..a.len() + b.len()].copy_from_slice(b);
            chunk.commit_all();
            self.popped += n as u64;
            self.outgoing = Some(Outgoing {
                channels,
                next: 0,
                end: n,
                at,
                splice: false,
                wrap: self.wrap,
                frame,
            });
        }
        self.discard();
    }

    /// Throws away everything the ring holds.
    fn discard(&mut self) {
        let n = self.consumer.slots();
        if let Ok(chunk) = self.consumer.read_chunk(n) {
            chunk.commit_all();
        }
        self.popped += n as u64;
    }

    /// The sample of `channel` at list frame `frame`, or silence -- counted
    /// in `starved` when the frame should have been there and was not.
    fn take(&mut self, frame: u64, channel: usize, starved: &mut u64) -> f32 {
        if self.outgoing.is_some() {
            let value = self.take_outgoing(frame, channel);
            if self.outgoing.is_some() {
                return value;
            }
        }
        if self.pending.is_some() {
            if self.short {
                *starved += 1;
            }
            return 0.0;
        }
        let Some(head) = self.head else {
            // Offline the refill is waited for and lands at once.
            if !self.refill(frame) || !self.offline {
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
                let refilled = self.refill(frame);
                // **Fallen a ring behind a transport playing on**: the reader
                // did not keep up, and the silence until it does is a
                // shortage. A locate lands here too, and is not one.
                if self.playing_on && head < frame && !self.offline {
                    self.short = true;
                    *starved += 1;
                }
                if !refilled || !self.offline || self.pending.is_some() {
                    return 0.0;
                }
                return self.take(frame, channel, starved);
            }
        }
        if self.offline && self.head == Some(frame) && self.consumer.slots() < self.channels {
            self.wake();
            let (consumer, channels) = (&self.consumer, self.channels);
            wait_for(|| consumer.slots() >= channels);
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
        // A stream is its owner's and cannot be restarted from here: a
        // `Voices` node refuses a def that holds one.
    }

    fn cued(&self) -> Option<bool> {
        // Over a buffer that is not fed it stands nowhere, and is ready.
        Some(self.standing.is_none_or(|f| self.holds(f)))
    }

    fn looped(&self, transport: &crate::dsp::TransportCtx) -> Option<bool> {
        // A reader with no fed stream reads no loop ahead, and holds no loop
        // change up.
        self.fed.as_ref()?;
        // a list that loops on its own reads no transport loop
        if self.looping {
            return Some(true);
        }
        let asked = ListWrap::of(
            transport.pending_looping.unwrap_or(transport.looping),
            self.delta,
            self.frames,
        );
        // the reading follows the loop asked for -- or the frames kept from
        // a change before it read it already: a second switch turned on the
        // first one's heels, just before the loop's end, puts back the loop
        // those frames were read through, and they are ready the moment it is
        // asked, before this reader has played a block of it
        if asked != self.wrap {
            return Some(self.kept_reads(asked) >= LAND_LEAD as u64);
        }
        // **What it holds of that reading**: the ring once its refill has
        // landed, and, spliced on ahead and before the splice is reached,
        // the kept frames up to it, which read the loop asked for as far as
        // there and play first. A second change turned while the first one's
        // kept frames still play is ready on them alone: waiting on the ring
        // kept the loop in force in place past where the two part, and the
        // transport went its way while the reader read the one asked.
        let kept = match &self.outgoing {
            Some(o) if o.splice && o.next > o.at => return Some(false),
            Some(o) if o.splice => ((o.at - o.next) / o.channels) as u64,
            _ => 0,
        };
        let ring = match (self.pending, self.head) {
            (None, Some(_)) => (self.consumer.slots() / self.channels) as u64,
            _ => 0,
        };
        // as much as the lead a replacing reading lands with -- or what is
        // left of the list -- so it is not starved once it goes on
        let ahead = match (self.wrap, self.head) {
            (None, Some(head)) => self.frames.saturating_sub(head).min(LAND_LEAD as u64),
            _ => LAND_LEAD as u64,
        };
        Some(ring + kept >= ahead.max(1))
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let next = ctx.transport.position + output.len() as u64;
        let rolling = ctx.transport.rolling;
        self.play(ctx, inputs, output);
        self.continues = rolling.then_some(next);
        // **The ring fallen to half wakes whoever fills it**, once per fall:
        // the reader parks with a full ring rather than looking every few
        // milliseconds, and half a ring is over half a second of lead.
        let low = self.consumer.slots() <= self.consumer.buffer().capacity() / 2;
        if low && !self.low {
            self.wake();
        }
        self.low = low;
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

    /// **What the loop in force plays from `frame`, kept**: on a loop change
    /// turned while the kept frames of the change before it still play, and
    /// that one put in place, what plays from here is the kept frames to
    /// their splice and what the ring has read of the loop in force after
    /// it. Those become the kept frames, read through the loop in force, so
    /// the new change is spliced on where that loop and the new one part --
    /// and, where it is not in place by then, they play on past it and the
    /// loop in force is read again from their end. Kept frames of another
    /// loop than the one in force, read on past where the two part, were
    /// the take only until the transport went the other way there.
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
            || self.pending.is_some()
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
            self.popped += n as u64;
            self.head = Some(self.advance(spliced, (n / channels) as u64));
        }
        self.outgoing = Some(Outgoing {
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
        self.playing_on = rolling && self.continues == Some(ctx.transport.position);
        // The transport's loop, read straight through: what its wrap plays
        // next is in the ring before the position gets there. A list that
        // loops on its own wraps at its own end instead.
        let delta = at(inputs[3], 0) as f64 - at(inputs[2], 0) as f64;
        // **A loop asked for is read before it is in place**: the reading
        // follows it, and the engine puts it in place once this reader holds
        // it (`UGen::looped`). The loop in force decides at a splice.
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
        // took first is silent here, and so is any other buffer.
        if !self.follow_source(buf.fed()) || self.fed.is_none() {
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
            // The reading in hand is kept aside and the ring emptied, so the
            // ring holds one reading at a time; one already kept (a change
            // before the last one landed) plays on, and what the ring holds of
            // the reading that did not land goes.
            // **Only the loop changed**: the same list, read another way. What
            // is kept of the old reading is the take up to where the two
            // readings part, so the new one is read from there and spliced on.
            let rewrapped = address == self.sent && looping == self.looping && generation.is_none();
            let mut from = None;
            if playing {
                let frame = place(0).0.unwrap_or(0);
                // the old reading is right only as far as the new one reads
                // the same frames
                let agree = ListWrap::agree(self.wrap, wrap, frame);
                self.keep_outgoing(frame, agree);
                if rewrapped && let Some(outgoing) = self.outgoing.as_mut() {
                    outgoing.splice = true;
                    let kept = (outgoing.at / outgoing.channels) as u64;
                    from = Some(ListWrap::advance(wrap, frame, kept));
                }
            } else if !self.offline && self.outgoing.is_some() {
                // A change before the last one landed: the kept frames play
                // on, and what the ring holds of the reading that did not land
                // goes. Spliced, they are the take as far as they agree with
                // this reading too, which goes on from there -- unless they
                // read another loop than this one and the one before was put
                // in place: then what plays is that one's, kept first.
                let frame = place(0).0.unwrap_or(0);
                if rewrapped && self.kept_reads(wrap) < LAND_LEAD as u64 {
                    self.keep_in_force(frame);
                }
                self.discard();
                if let Some(outgoing) = self.outgoing.as_mut() {
                    if rewrapped && outgoing.splice {
                        let channels = outgoing.channels;
                        let mut left = (outgoing.end - outgoing.next) / channels;
                        if let Some(agree) = ListWrap::agree(outgoing.wrap, wrap, frame) {
                            left = left.min(agree as usize);
                        }
                        outgoing.at = outgoing.next + left * channels;
                        from = Some(ListWrap::advance(wrap, frame, left as u64));
                    } else {
                        outgoing.splice = false;
                    }
                }
            }
            let previous_wrap = (self.looping, self.wrap);
            self.looping = looping;
            self.wrap = wrap;
            let frame = from.unwrap_or_else(|| place(0).1.unwrap_or(0));
            // The shape first: offline the refill lands inside the call.
            let (channels, previous) = (buf.channels().max(1), (self.channels, self.frames));
            self.channels = channels;
            self.frames = frames;
            if self.refill(frame) {
                self.sent = address;
                if let (Some(link), Some(g)) = (self.fed.as_mut(), generation) {
                    link.generation = g;
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
                self.refill(f);
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
                        self.refill(ready);
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

/// **Waits for `ready`**, in an offline render, where the owner is a thread
/// beside the render. A page whose engine has no threads has no owner to wait
/// for, and looks once.
fn wait_for(ready: impl FnMut() -> bool) -> bool {
    #[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
    return crate::dsp::io_thread::wait_for(ready);
    #[cfg(all(target_arch = "wasm32", not(target_feature = "atomics")))]
    {
        let mut ready = ready;
        ready()
    }
}
