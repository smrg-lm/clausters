//! Timing math for a `TempoClock` and a beat-ordered event queue.
//!
//! This is the value-and-time core a client `TempoClock` is built on (the
//! coroutine driver that resumes `yield`s stays in the host language). Two
//! pieces:
//!
//! - the free conversions a clock reaches the server's sample clock through
//!   ([`secs_to_samples`]/[`samples_to_secs`]) and the grid it reads a
//!   position off ([`quant_delay`], [`bar`], [`beat_in_bar`]);
//! - [`Scheduler`] — a min-heap keyed by beat time with stable insertion
//!   order, the structure a clock pops due events from.
//!
//! **Beats↔seconds is not here.** It is [`crate::tempomap::TempoMap`], and
//! there is one of it: an affine clock is a one-segment map, so a struct
//! holding `(tempo, base_beats, base_seconds)` beside it would be a second
//! implementation of the same relation.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

/// Seconds → sample count at `sample_rate`, rounded to the nearest sample
/// (ties to even, matching the builtins' `rint`).
#[inline]
pub fn secs_to_samples(secs: f64, sample_rate: f64) -> i64 {
    (secs * sample_rate).round_ties_even() as i64
}

/// Sample count → seconds at `sample_rate`.
#[inline]
pub fn samples_to_secs(samples: i64, sample_rate: f64) -> f64 {
    samples as f64 / sample_rate
}

/// Beats to wait so a routine starts on the next `quant` boundary of a grid
/// currently at `pos` beats (`quant <= 0` → now). A position already on the
/// boundary waits 0 — the shared quantization rule every client applies.
#[inline]
pub fn quant_delay(pos: f64, quant: f64) -> f64 {
    if quant <= 0.0 {
        return 0.0;
    }
    ((pos / quant).ceil() * quant - pos).max(0.0)
}

/// The bar index a beat position falls in, on a grid of `quant` beats per bar
/// (0-based: beats `[0, quant)` are bar 0). `quant <= 0` means no bar grid —
/// everything is bar 0. The complement of [`quant_delay`] for *reading* a
/// position off the grid (a display, a transport readout) rather than
/// scheduling onto it.
#[inline]
pub fn bar(beats: f64, quant: f64) -> f64 {
    if quant <= 0.0 {
        return 0.0;
    }
    (beats / quant).floor()
}

/// The beat within its bar for a beat position on a grid of `quant` beats per
/// bar (0-based: `[0, quant)`). `quant <= 0` means no bar grid — the position
/// itself is returned.
#[inline]
pub fn beat_in_bar(beats: f64, quant: f64) -> f64 {
    if quant <= 0.0 {
        return beats;
    }
    beats - bar(beats, quant) * quant
}

/// One queued event: a beat time and a flat `u64` payload id (the client maps
/// the id back to its routine — only flat data crosses the boundary).
#[derive(Clone, Copy, Debug)]
struct Entry {
    time: f64,
    seq: u64, // insertion order: stable tie-break for equal times
    id: u64,
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Entry {}
impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse so `BinaryHeap` (a max-heap) yields the *earliest* time, and
        // the lowest seq among equal times, first.
        other
            .time
            .total_cmp(&self.time)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}

/// A beat-ordered min-queue of event ids. Stable for equal times.
#[derive(Default)]
pub struct Scheduler {
    heap: BinaryHeap<Entry>,
    next_seq: u64,
}

impl Scheduler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues `id` to fire at beat `time`.
    pub fn push(&mut self, time: f64, id: u64) {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.heap.push(Entry { time, seq, id });
    }

    /// Beat time of the earliest queued event, if any.
    pub fn peek_time(&self) -> Option<f64> {
        self.heap.peek().map(|e| e.time)
    }

    /// Pops the earliest event whose time is `<= now`, else `None`.
    pub fn pop_due(&mut self, now: f64) -> Option<(f64, u64)> {
        match self.heap.peek() {
            Some(e) if e.time <= now => {
                let e = self.heap.pop().unwrap();
                Some((e.time, e.id))
            }
            _ => None,
        }
    }

    /// Removes every queued entry with `id` (a routine may be queued more than
    /// once); returns how many were dropped. The stable order of the remaining
    /// entries is preserved (their `seq` values do not change).
    pub fn remove(&mut self, id: u64) -> usize {
        let before = self.heap.len();
        self.heap.retain(|e| e.id != id);
        before - self.heap.len()
    }

    pub fn len(&self) -> usize {
        self.heap.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    pub fn clear(&mut self) {
        self.heap.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The beats<->seconds round trip and the pinned instant are the tempo
    // map's tests (`tempomap::tests`), where the one implementation of that
    // relation lives.

    #[test]
    fn sample_conversion() {
        assert_eq!(secs_to_samples(1.0, 48_000.0), 48_000);
        assert!((samples_to_secs(24_000, 48_000.0) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn quant_delay_snaps_to_the_next_boundary() {
        assert_eq!(quant_delay(4.0, 4.0), 0.0); // on the boundary -> now
        assert!((quant_delay(3.5, 4.0) - 0.5).abs() < 1e-12);
        assert!((quant_delay(5.0, 2.0) - 1.0).abs() < 1e-12);
        assert_eq!(quant_delay(3.5, 0.0), 0.0); // no quant -> now
    }

    #[test]
    fn bar_and_beat_in_bar_read_the_grid() {
        // 4 beats per bar: beat 9.5 is bar 2, beat 1.5 within it (0-based).
        assert_eq!(bar(9.5, 4.0), 2.0);
        assert!((beat_in_bar(9.5, 4.0) - 1.5).abs() < 1e-12);
        // On a boundary the beat-in-bar is exactly 0.
        assert_eq!(bar(8.0, 4.0), 2.0);
        assert_eq!(beat_in_bar(8.0, 4.0), 0.0);
        // Negative positions (before the origin) keep the floor convention.
        assert_eq!(bar(-0.5, 4.0), -1.0);
        assert!((beat_in_bar(-0.5, 4.0) - 3.5).abs() < 1e-12);
        // No quant -> no bar grid.
        assert_eq!(bar(9.5, 0.0), 0.0);
        assert_eq!(beat_in_bar(9.5, 0.0), 9.5);
    }

    #[test]
    fn scheduler_remove_drops_all_entries_of_an_id() {
        let mut s = Scheduler::new();
        s.push(1.0, 7);
        s.push(2.0, 8);
        s.push(3.0, 7);
        assert_eq!(s.remove(7), 2);
        assert_eq!(s.remove(7), 0);
        assert_eq!(s.pop_due(10.0), Some((2.0, 8)));
        assert!(s.is_empty());
    }

    #[test]
    fn scheduler_pops_in_time_then_insertion_order() {
        let mut s = Scheduler::new();
        s.push(2.0, 20);
        s.push(1.0, 10);
        s.push(1.0, 11); // same time as id 10, inserted later
        assert_eq!(s.pop_due(0.5), None);
        assert_eq!(s.pop_due(1.0), Some((1.0, 10)));
        assert_eq!(s.pop_due(1.0), Some((1.0, 11)));
        assert_eq!(s.pop_due(1.0), None); // 2.0 not due yet
        assert_eq!(s.pop_due(2.0), Some((2.0, 20)));
        assert!(s.is_empty());
    }
}
