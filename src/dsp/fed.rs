//! **A stream fed by its owner**: frames a program that embeds the engine
//! reads and hands over, positioned by the transport, with the engine knowing
//! nothing of where they came from.
//!
//! An editing application holds its engine and its takes: the take's
//! structure, its files and its reading are the application's, and the
//! engine plays what it is handed. So a reader that follows the transport
//! (`TransportDiskIn`) has two sources. A segment list in the pool is read by
//! a thread the reader opens (`dsp::stream`); a **fed** buffer is read by its
//! owner, through the two rings made here. The reader keeps all of its
//! real-time half -- which frame each sample plays, the epochs a refill is
//! told apart by, a loop read straight through, the fade into a list that
//! replaced the one playing, whether a cue holds its frame -- and the owner
//! keeps the other: it takes the latest request, renders the frames from
//! there into the ring, and publishes the refill's epoch once its first
//! frames are in.
//!
//! The protocol is `dsp::stream`'s, unchanged: a request carries a frame and
//! an epoch (and the loop to read straight through), the owner publishes the
//! epoch with how many samples it had pushed before the refill's first frame,
//! and the reader throws away everything before that boundary in one commit.
//!
//! **The rings change hands once.** The reader's halves (the samples'
//! consumer, the requests' producer) are held here and taken by the first
//! reader that plays the buffer, with an atomic swap: no lock and no
//! allocation on the audio thread. A buffer is fed to one reader, so a second
//! reader over it finds nothing to take and is silent. An edit that changes
//! what the owner renders bumps the buffer's **generation**, which the reader
//! treats as a list replaced while it plays.

use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use rtrb::{Consumer, Producer, RingBuffer};

use crate::dsp::ListWrap;

/// The samples a fed ring holds, as a list reader's own ring does.
pub const FED_RING_SAMPLES: usize = 1 << 17;

/// How many requests may be in flight; the owner takes only the latest.
const REQUESTS: usize = 8;

/// The low 48 bits of the published word: the boundary in samples.
pub const BOUNDARY: u64 = (1 << 48) - 1;

/// **A refill the reader asks for**: render from `frame`, under `epoch`, and
/// read the loop `wrap` straight through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeedRequest {
    /// The frame of the stream to render from.
    pub frame: u64,
    /// The refill's epoch, published back once its first frames are in.
    pub epoch: u16,
    /// The transport's loop in the stream's frames, if one crosses it.
    pub wrap: Option<ListWrap>,
}

/// What the two halves share besides the rings.
#[derive(Debug, Default)]
pub struct FeedShared {
    /// The epoch taken (high 16 bits) and the samples pushed before its first
    /// frame (low 48), as `dsp::stream` publishes it.
    pub ack: AtomicU64,
    /// Bumped by the owner when what it renders changed: the reader fades
    /// from what its ring holds into a refill of the new contents.
    pub generation: AtomicU64,
    /// The owner's reading thread, which the reader wakes after a request.
    waker: OnceLock<std::thread::Thread>,
}

impl FeedShared {
    /// Wakes the owner's reading thread, when one has said it is the one.
    pub fn wake(&self) {
        if let Some(thread) = self.waker.get() {
            thread.unpark();
        }
    }
}

/// **The reader's end of a fed stream**: held in the buffer, and taken once
/// by the reader that plays it.
pub struct Fed {
    consumer: AtomicPtr<Consumer<f32>>,
    requests: AtomicPtr<Producer<FeedRequest>>,
    shared: Arc<FeedShared>,
}

impl std::fmt::Debug for Fed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fed").field("shared", &self.shared).finish()
    }
}

/// The halves a reader takes from a [`Fed`], in the boxes they were made in:
/// a reader swaps their contents rather than moving them out, so taking them
/// frees nothing on the audio thread.
pub struct FedEnds {
    /// The samples, interleaved by the stream's channels.
    pub consumer: Box<Consumer<f32>>,
    /// Where refills are asked for.
    pub requests: Box<Producer<FeedRequest>>,
}

impl Fed {
    /// **Takes the reader's halves**, if no reader took them first: an atomic
    /// swap each, so it is legal on the audio thread.
    pub fn take(&self) -> Option<FedEnds> {
        let consumer = self.consumer.swap(std::ptr::null_mut(), Ordering::AcqRel);
        let requests = self.requests.swap(std::ptr::null_mut(), Ordering::AcqRel);
        if consumer.is_null() || requests.is_null() {
            // Half taken cannot happen (both are taken together); put back
            // whatever was there so nothing leaks.
            if !consumer.is_null() {
                self.consumer.store(consumer, Ordering::Release);
            }
            if !requests.is_null() {
                self.requests.store(requests, Ordering::Release);
            }
            return None;
        }
        // SAFETY: both pointers came from `Box::into_raw` in `feed` and were
        // swapped out exactly once, so this is their one owner now.
        Some(FedEnds {
            consumer: unsafe { Box::from_raw(consumer) },
            requests: unsafe { Box::from_raw(requests) },
        })
    }

    /// What the two halves share.
    pub fn shared(&self) -> &Arc<FeedShared> {
        &self.shared
    }
}

impl Drop for Fed {
    fn drop(&mut self) {
        // Halves no reader took are freed with the buffer, off the audio
        // thread (a buffer leaves the pool through the garbage FIFO).
        drop(self.take());
    }
}

/// **The owner's end of a fed stream**: the frames go in here, the requests
/// come out, and the refill's epoch is published once its first frames are
/// in.
pub struct Feeder {
    producer: Producer<f32>,
    inbox: Consumer<FeedRequest>,
    shared: Arc<FeedShared>,
    /// Samples pushed since the start, the boundary's measure.
    pushed: u64,
    /// The refill taken and not yet published: its epoch and its boundary.
    due: Option<(u16, u64)>,
}

impl Feeder {
    /// **The latest request**, if any arrived since the last: the ones before
    /// it are stale, as the reader only ever wants where it is now. Its
    /// boundary is fixed here, so [`Self::publish`] reports it once the
    /// refill's first frames are in.
    pub fn latest(&mut self) -> Option<FeedRequest> {
        let mut latest = None;
        while let Ok(request) = self.inbox.pop() {
            latest = Some(request);
        }
        if let Some(request) = latest {
            self.due = Some((request.epoch, self.pushed));
        }
        latest
    }

    /// The room in the ring, in samples.
    pub fn room(&self) -> usize {
        self.producer.slots()
    }

    /// Pushes interleaved samples, as many as fit; answers how many went in.
    pub fn push(&mut self, samples: &[f32]) -> usize {
        let n = samples.len().min(self.producer.slots());
        if let Ok(chunk) = self.producer.write_chunk_uninit(n) {
            chunk.fill_from_iter(samples[..n].iter().copied());
        }
        self.pushed += n as u64;
        n
    }

    /// **Publishes the refill taken last**, so the reader lands it: called
    /// once its first frames are in, or when there is nothing to render.
    pub fn publish(&mut self) {
        if let Some((epoch, boundary)) = self.due.take() {
            self.shared.ack.store(
                ((epoch as u64) << 48) | (boundary & BOUNDARY),
                Ordering::Release,
            );
        }
    }

    /// **What it renders changed**: the reader fades into a refill of the
    /// new contents from where it is.
    pub fn changed(&self) {
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// Says the calling thread is the one the reader wakes after a request.
    /// The first call wins.
    pub fn wake_me(&self) {
        let _ = self.shared.waker.set(std::thread::current());
    }

    /// What the two halves share.
    pub fn shared(&self) -> &Arc<FeedShared> {
        &self.shared
    }
}

/// **A fed stream's two ends**: the reader's, to put in a buffer, and the
/// owner's, to feed it from.
pub fn feed() -> (Fed, Feeder) {
    let (producer, consumer) = RingBuffer::new(FED_RING_SAMPLES);
    let (requests, inbox) = RingBuffer::new(REQUESTS);
    let shared = Arc::new(FeedShared::default());
    let fed = Fed {
        consumer: AtomicPtr::new(Box::into_raw(Box::new(consumer))),
        requests: AtomicPtr::new(Box::into_raw(Box::new(requests))),
        shared: Arc::clone(&shared),
    };
    let feeder = Feeder {
        producer,
        inbox,
        shared,
        pushed: 0,
        due: None,
    };
    (fed, feeder)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reader's halves are taken once, and a second reader finds none.
    #[test]
    fn the_ends_change_hands_once() {
        let (fed, mut feeder) = feed();
        let mut ends = fed.take().expect("the first reader takes them");
        assert!(fed.take().is_none(), "and a second finds none");
        ends.requests
            .push(FeedRequest {
                frame: 5,
                epoch: 1,
                wrap: None,
            })
            .unwrap();
        assert_eq!(feeder.latest().map(|r| r.frame), Some(5));
        assert_eq!(feeder.push(&[1.0, 2.0]), 2);
        feeder.publish();
        assert_eq!(fed.shared().ack.load(Ordering::Acquire) >> 48, 1);
        assert_eq!(ends.consumer.pop(), Ok(1.0));
    }
}
