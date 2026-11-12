//! Where the device's sample axis sits on the wall clock.
//!
//! A wall-clocked client stamps its bundles with NTP timetags, and the server
//! has to turn each one into a sample on the device clock. The counter it
//! reads for that is published once per block, so reading it together with
//! the wall clock *now* pairs two different instants: the gap is how far into
//! the current audio callback the packet happened to arrive, anywhere from
//! nothing to a whole device buffer, and it differs for every bundle. Placing
//! a timetag against that pair throws away exactly the relative timing the
//! timetags carry.
//!
//! The pair that means something is taken where the counter moves: the audio
//! callback knows which frame it is about to hand the device and can read the
//! wall clock at that moment. The **epoch** is the line through those stamps
//! -- the Unix instant sample 0 falls on -- so a timetag `T` lands on sample
//! `(T - epoch) * rate` no matter when its packet was handled.
//!
//! A stamp is late by however long the callback thread took to wake, never
//! early by design, so the line is tracked by its lower edge ([`EpochFilter`])
//! and published to the network thread through one atomic ([`DeviceEpoch`]).
//! Only a backend with a device callback publishes one; a host that drives the
//! engine itself (headless, NRT, wasm) leaves it unknown, and its server keeps
//! the counter-plus-delta placement, which is exact there because its time
//! *is* the counter.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// How fast the estimate may rise without a stamp pulling it up, in seconds
/// per second: 1000 ppm. The line has to follow a sound card whose crystal
/// runs slower than the system clock, and consumer hardware stays well inside
/// a few hundred ppm; between two callbacks this lets the estimate rise by a
/// few tens of microseconds, which is what keeps it from sticking to one
/// early stamp forever.
const LEAK: f64 = 1e-3;

/// A stamp later than the estimate by more than this many callback periods is
/// not wake-up jitter but a discontinuity -- an xrun that paused the counter,
/// a stream that restarted, a wall clock stepped forward -- and the estimate
/// restarts from it instead of creeping up to it.
const RESEED_PERIODS: f64 = 3.0;

/// The lower edge of `stamp - frame / rate` over the callbacks: the audio
/// thread's half, plain arithmetic with no allocation.
#[derive(Clone, Copy, Debug, Default)]
pub struct EpochFilter {
    /// `(epoch, the stamp it was last updated at)`, once there is one.
    state: Option<(f64, f64)>,
}

impl EpochFilter {
    pub const fn new() -> Self {
        Self { state: None }
    }

    /// Takes one callback's stamp: frame `frame` is being handed to the device
    /// at Unix time `stamp`, and a callback lasts `period` seconds. Returns the
    /// epoch estimate after it.
    pub fn observe(&mut self, stamp: f64, frame: u64, rate: f64, period: f64) -> f64 {
        let epoch = stamp - frame as f64 / rate;
        let next = match self.state {
            None => epoch,
            Some((previous, at)) => {
                let risen = previous + LEAK * (stamp - at).max(0.0);
                if epoch < risen || epoch - risen > RESEED_PERIODS * period {
                    epoch
                } else {
                    risen
                }
            }
        };
        self.state = Some((next, stamp));
        next
    }

    /// The current estimate, if any stamp has been taken.
    pub fn epoch(&self) -> Option<f64> {
        self.state.map(|(epoch, _)| epoch)
    }
}

/// The estimate as the network thread reads it: an `f64` in one atomic, unknown
/// (`NaN`) until a device callback publishes one. Cloning shares it.
#[derive(Clone, Debug)]
pub struct DeviceEpoch(Arc<AtomicU64>);

impl Default for DeviceEpoch {
    fn default() -> Self {
        Self(Arc::new(AtomicU64::new(f64::NAN.to_bits())))
    }
}

impl DeviceEpoch {
    /// Publishes an estimate (audio thread: one store).
    pub fn publish(&self, epoch: f64) {
        self.0.store(epoch.to_bits(), Ordering::Relaxed);
    }

    /// The Unix instant device sample 0 falls on, or `None` when nothing
    /// publishes one.
    pub fn get(&self) -> Option<f64> {
        let epoch = f64::from_bits(self.0.load(Ordering::Relaxed));
        epoch.is_finite().then_some(epoch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 48_000.0;
    const FRAMES: u64 = 1024;
    const PERIOD: f64 = FRAMES as f64 / RATE;
    const TRUE_EPOCH: f64 = 1_800_000_000.0;

    /// Wake-up delays in seconds, deterministic and irregular (0 to ~3 ms).
    fn jitter(i: u64) -> f64 {
        ((i * 7919) % 31) as f64 * 1e-4
    }

    #[test]
    fn the_estimate_sits_on_the_lower_edge_of_the_stamps() {
        let mut filter = EpochFilter::new();
        let mut epoch = 0.0;
        for i in 0..500 {
            let frame = i * FRAMES;
            let stamp = TRUE_EPOCH + frame as f64 / RATE + jitter(i);
            epoch = filter.observe(stamp, frame, RATE, PERIOD);
        }
        let error = epoch - TRUE_EPOCH;
        assert!((0.0..2e-4).contains(&error), "error {error}");
    }

    #[test]
    fn a_card_slower_than_the_wall_clock_is_followed() {
        // 200 ppm slow: each frame takes a little longer in wall time.
        let wall_per_frame = (1.0 + 200e-6) / RATE;
        let mut filter = EpochFilter::new();
        let mut last = (0.0, 0.0);
        for i in 0..5_000 {
            let frame = i * FRAMES;
            let stamp = TRUE_EPOCH + frame as f64 * wall_per_frame + jitter(i);
            let epoch = filter.observe(stamp, frame, RATE, PERIOD);
            last = (epoch, stamp - jitter(i) - frame as f64 / RATE);
        }
        let error = last.0 - last.1;
        assert!(error.abs() < 5e-4, "error {error}");
    }

    #[test]
    fn an_xrun_restarts_the_estimate_instead_of_creeping_to_it() {
        let mut filter = EpochFilter::new();
        for i in 0..100 {
            let frame = i * FRAMES;
            filter.observe(TRUE_EPOCH + frame as f64 / RATE, frame, RATE, PERIOD);
        }
        // The counter paused for 0.2 s: the same frames now fall later.
        let gap = 0.2;
        let frame = 100 * FRAMES;
        let epoch = filter.observe(TRUE_EPOCH + gap + frame as f64 / RATE, frame, RATE, PERIOD);
        assert!((epoch - (TRUE_EPOCH + gap)).abs() < 1e-9);
    }

    #[test]
    fn nothing_is_published_until_a_callback_publishes_it() {
        let shared = DeviceEpoch::default();
        assert_eq!(shared.get(), None);
        shared.clone().publish(TRUE_EPOCH);
        assert_eq!(shared.get(), Some(TRUE_EPOCH));
    }
}
