//! **A time on an axis**, as a type: an `i64` of NTP ticks.
//!
//! A tick is 2^-32 seconds, the fraction of an OSC timetag: 233 ps, and 68
//! years either way of the axis' origin. Times in the host and the
//! applications were bare `f64`, `usize` and `u64`, and three conversions to
//! 32 bits slipped through in one day, each found only by eye on a take long
//! enough to need the bits. A type makes the mistake unwritable: a time
//! becomes a float only as a difference from an origin ([`Time::px_from`]), a
//! frame only at a rate ([`Time::frame`]), and a timetag only from the
//! timetag of its origin ([`Time::timetag_from`]).
//!
//! Three relations follow from the unit, and they are why it was chosen
//! (`docs/decisions.md`, "Time is NTP ticks on every axis"):
//!
//! - **A time on an axis and a timetag are one arithmetic.** The timetag of
//!   the origin plus a time is a timetag, exactly; a time in `f64` seconds
//!   keeps half a microsecond at the NTP era's four billion seconds.
//! - **Takes at two rates share one axis**, since a tick is the same instant
//!   whatever the rate of what is drawn there.
//! - **Sums and differences are exact**, so a view of the end of a take of
//!   days is drawn as precisely as one of its start.
//!
//! A sample index is not a time: it stays a whole frame, and frame `f` at
//! rate `r` covers the ticks from [`Time::of_frame`]`(f, r)` up to the next
//! frame's. A wall-clock instant is not one either: it stays a timetag, and a
//! time is measured from one.

use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};

/// Ticks in a second: the fraction of an OSC timetag.
pub const TICKS_PER_SECOND: i64 = 1 << 32;

/// A time on an axis, in ticks of 2^-32 s from the axis' origin: see the
/// module. Sums saturate at the range rather than wrap.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Time(i64);

impl Time {
    /// The axis' origin.
    pub const ZERO: Time = Time(0);
    /// The latest time an axis holds, 68 years after its origin.
    pub const MAX: Time = Time(i64::MAX);
    /// The earliest, 68 years before it.
    pub const MIN: Time = Time(i64::MIN);

    /// The time `ticks` ticks from the origin.
    pub const fn from_ticks(ticks: i64) -> Time {
        Time(ticks)
    }

    /// Its ticks from the origin: the form it crosses a wire as.
    pub const fn ticks(self) -> i64 {
        self.0
    }

    /// The time `seconds` from the origin, to the nearest tick; saturating
    /// past the range, and the origin for a NaN.
    pub fn from_seconds(seconds: f64) -> Time {
        // `as` saturates, and takes a NaN to 0
        Time((seconds * TICKS_PER_SECOND as f64).round() as i64)
    }

    /// Its seconds from the origin. Exact to the tick within about 24 days
    /// of it, where an `f64` stops holding every tick, and to a few ticks
    /// past that.
    pub fn seconds(self) -> f64 {
        self.0 as f64 / TICKS_PER_SECOND as f64
    }

    /// **Where frame `frame` begins at `rate`** frames a second: the first
    /// tick at or after the frame's start, so that [`Time::frame`] reads the
    /// frame back whatever the rate. Exact in integers at an integral rate.
    pub fn of_frame(frame: i64, rate: f64) -> Time {
        match integral(rate) {
            // ceil(frame * 2^32 / rate)
            Some(rate) => Time(saturate(div_ceil(
                i128::from(frame) << 32,
                i128::from(rate),
            ))),
            None => Time((frame as f64 * TICKS_PER_SECOND as f64 / rate).ceil() as i64),
        }
    }

    /// **The frame it falls in at `rate`**: the last frame that begins at or
    /// before it.
    pub fn frame(self, rate: f64) -> i64 {
        match integral(rate) {
            // floor(ticks * rate / 2^32): an arithmetic shift floors
            Some(rate) => saturate((i128::from(self.0) * i128::from(rate)) >> 32),
            None => (self.0 as f64 * rate / TICKS_PER_SECOND as f64).floor() as i64,
        }
    }

    /// **The frame nearest it at `rate`**: what a selection snaps to.
    pub fn nearest_frame(self, rate: f64) -> i64 {
        match integral(rate) {
            Some(rate) => saturate((i128::from(self.0) * i128::from(rate) + (1 << 31)) >> 32),
            None => (self.0 as f64 * rate / TICKS_PER_SECOND as f64).round() as i64,
        }
    }

    /// **Its position in frames at `rate`, with the fraction**: where a frame
    /// boundary falls between two pixels. Only a difference from an origin
    /// is small enough to keep every fraction, so a caller drawing passes one.
    pub fn frames(self, rate: f64) -> f64 {
        self.0 as f64 * rate / TICKS_PER_SECOND as f64
    }

    /// **The pixel it is drawn at**, `ticks_per_px` ticks to a pixel from
    /// `origin`, the view's left edge. The difference is taken in integers
    /// and only it becomes a float, which is what keeps the pixel of the end
    /// of a take of days as exact as that of its start.
    pub fn px_from(self, origin: Time, ticks_per_px: f64) -> f64 {
        (self - origin).0 as f64 / ticks_per_px
    }

    /// **The time `px` pixels from `origin`**, `ticks_per_px` ticks to a
    /// pixel: [`Time::px_from`] read back, to the nearest tick.
    pub fn at_px(origin: Time, px: f64, ticks_per_px: f64) -> Time {
        origin + Time((px * ticks_per_px).round() as i64)
    }

    /// **The time from `origin` to `timetag`**, both the 64 bits of an OSC
    /// timetag (`seconds << 32 | fraction`): exact, as the two are one unit.
    pub fn since(timetag: u64, origin: u64) -> Time {
        Time(timetag.wrapping_sub(origin) as i64)
    }

    /// **The timetag it is at**, its axis' origin at timetag `origin`:
    /// [`Time::since`] read back.
    pub fn timetag_from(self, origin: u64) -> u64 {
        origin.wrapping_add(self.0 as u64)
    }

    /// It as a fraction of `whole` (`0` for an empty whole): how far along a
    /// span a time is.
    pub fn ratio(self, whole: Time) -> f64 {
        if whole.0 == 0 {
            0.0
        } else {
            self.0 as f64 / whole.0 as f64
        }
    }

    /// It times `factor`, to the nearest tick: a span zoomed.
    pub fn scaled(self, factor: f64) -> Time {
        Time((self.0 as f64 * factor).round() as i64)
    }

    /// How long it is from the origin either way.
    pub const fn abs(self) -> Time {
        Time(self.0.saturating_abs())
    }
}

impl Add for Time {
    type Output = Time;
    fn add(self, other: Time) -> Time {
        Time(self.0.saturating_add(other.0))
    }
}

impl Sub for Time {
    type Output = Time;
    fn sub(self, other: Time) -> Time {
        Time(self.0.saturating_sub(other.0))
    }
}

impl Neg for Time {
    type Output = Time;
    fn neg(self) -> Time {
        Time(self.0.saturating_neg())
    }
}

impl AddAssign for Time {
    fn add_assign(&mut self, other: Time) {
        *self = *self + other;
    }
}

impl SubAssign for Time {
    fn sub_assign(&mut self, other: Time) {
        *self = *self - other;
    }
}

/// A rate that is a whole number of frames a second, which every rate a
/// device runs at is: the conversions are then exact in integers.
fn integral(rate: f64) -> Option<i64> {
    (rate > 0.0 && rate.fract() == 0.0 && rate <= f64::from(u32::MAX)).then_some(rate as i64)
}

fn div_ceil(n: i128, d: i128) -> i128 {
    let q = n.div_euclid(d);
    if n.rem_euclid(d) == 0 { q } else { q + 1 }
}

fn saturate(v: i128) -> i64 {
    v.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_reads_back_at_every_rate() {
        for rate in [44_100.0, 48_000.0, 96_000.0, 192_000.0, 22_050.5] {
            for frame in [0i64, 1, 2, 47_999, 48_000, 4_147_200_000, -1, -48_001] {
                let t = Time::of_frame(frame, rate);
                assert_eq!(t.frame(rate), frame, "frame {frame} at {rate}");
                assert_eq!(t.nearest_frame(rate), frame, "frame {frame} at {rate}");
                // the tick before it is the frame before
                assert_eq!(
                    (t - Time::from_ticks(1)).frame(rate),
                    frame - 1,
                    "frame {frame} at {rate}"
                );
            }
        }
    }

    #[test]
    fn a_second_is_its_frames() {
        let second = Time::from_seconds(1.0);
        assert_eq!(second.ticks(), TICKS_PER_SECOND);
        assert_eq!(Time::of_frame(48_000, 48_000.0), second);
        assert_eq!(second.frame(44_100.0), 44_100);
        assert_eq!(second.seconds(), 1.0);
        assert_eq!(Time::from_seconds(f64::NAN), Time::ZERO);
        assert_eq!(Time::from_seconds(1e30), Time::MAX);
    }

    /// The pixel of the last frame of a day is as exact as that of the
    /// first: the difference from the origin is taken in integers.
    #[test]
    fn a_pixel_is_a_difference_from_the_origin() {
        let rate = 48_000.0;
        let day = 24 * 3600 * 48_000i64;
        let origin = Time::of_frame(day - 100, rate);
        // ten pixels a frame
        let per_px = TICKS_PER_SECOND as f64 / rate / 10.0;
        for k in 0..100 {
            let px = Time::of_frame(day - 100 + k, rate).px_from(origin, per_px);
            assert!((px - 10.0 * k as f64).abs() < 1e-3, "frame {k}: {px}");
        }
        let back = Time::at_px(origin, 250.0, per_px);
        assert_eq!(back.nearest_frame(rate), day - 100 + 25);
    }

    #[test]
    fn a_time_from_a_timetag_is_exact() {
        // a timetag of the NTP era and one 3.5 s and one tick later
        let origin = (3_990_000_000u64 << 32) | 123_456_789;
        let later = origin + (3u64 << 32) + (1u64 << 31) + 1;
        let t = Time::since(later, origin);
        assert_eq!(t.ticks(), 3 * TICKS_PER_SECOND + (1 << 31) + 1);
        assert_eq!(t.timetag_from(origin), later);
        assert_eq!(Time::since(origin, later), -t);
    }

    #[test]
    fn sums_saturate_and_spans_scale() {
        assert_eq!(Time::MAX + Time::from_ticks(1), Time::MAX);
        assert_eq!(Time::MIN - Time::from_ticks(1), Time::MIN);
        let span = Time::from_seconds(10.0);
        assert_eq!(span.scaled(0.5), Time::from_seconds(5.0));
        assert_eq!(Time::from_seconds(2.5).ratio(span), 0.25);
        assert_eq!(Time::from_seconds(1.0).ratio(Time::ZERO), 0.0);
    }
}
