//! Exact rational arithmetic, in lowest terms.
//!
//! This is **duration and position arithmetic**, not a notation detail. A
//! musical length is a fraction of a whole note and a musical position is a
//! fraction from the start; both have to add, subtract, split and compare
//! without drift, and neither is exactly representable in binary floating point
//! (a triplet eighth is `1/12`) nor on any fixed grid of ticks (the same `1/12`
//! is not an integer count of 32nds). So the type lives here beside
//! [`crate::measure`], [`crate::scale`] and [`crate::tempoclock`] rather than
//! inside the module that needed it first — a function that outlives its first
//! caller goes where its subject is.
//!
//! **Ticks are a boundary, not a foundation.** A protocol counts in whatever it
//! counts in — MIDI in its own ticks, OSC in seconds or beats, MEI in `@dur`
//! values and dots — and each conversion happens at that protocol's edge, with
//! [`Ratio::as_ticks`] and [`Ratio::from_ticks`] as the door. Nothing above the
//! edge is expressed in ticks, which is what keeps a tuplet exact all the way
//! to the moment it is written down.
//!
//! The representation is deliberately small and total: an `i64` numerator over
//! a non-zero `i64` denominator, always normalized (the sign on the numerator,
//! the pair coprime, zero as `0/1`). Every constructor normalizes, so two
//! `Ratio`s are equal exactly when they are the same number, and `Ord` and
//! `Hash` follow from that without a custom comparison anywhere else.

use std::cmp::Ordering;
use std::fmt;
use std::ops::{Add, Div, Mul, Neg, Sub};

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};

/// An exact rational number in lowest terms, with a positive denominator.
///
/// Construct with [`Ratio::new`] (or [`Ratio::from`] an integer), and read the
/// parts back with [`Ratio::numer`] / [`Ratio::denom`]. Arithmetic is the usual
/// operators; every result is normalized.
///
/// As JSON a ratio is the two-element array `[numer, denom]` — short enough to
/// read in a payload, and unambiguous in every language a client is written in
/// (an object would invite a float, and a float is what this type exists to
/// avoid).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ratio {
    numer: i64,
    denom: i64,
}

impl Ratio {
    /// Zero.
    pub const ZERO: Ratio = Ratio { numer: 0, denom: 1 };
    /// One — a whole note, when the ratio is a duration.
    pub const ONE: Ratio = Ratio { numer: 1, denom: 1 };

    /// `numer / denom`, normalized. A zero denominator is **not** an error the
    /// caller has to handle: it is a programming mistake in code that computes
    /// a length, so it panics rather than propagating a `Result` through every
    /// arithmetic site.
    ///
    /// # Panics
    /// If `denom` is zero.
    pub fn new(numer: i64, denom: i64) -> Ratio {
        assert!(denom != 0, "a ratio's denominator cannot be zero");
        let sign = if denom < 0 { -1 } else { 1 };
        let g = gcd(numer.unsigned_abs(), denom.unsigned_abs()) as i64;
        Ratio {
            numer: sign * numer / g,
            denom: sign * denom / g,
        }
    }

    /// The numerator, carrying the sign.
    pub fn numer(&self) -> i64 {
        self.numer
    }

    /// The denominator, always positive.
    pub fn denom(&self) -> i64 {
        self.denom
    }

    /// Whether this is zero — the length of nothing, and the position of the
    /// start.
    pub fn is_zero(&self) -> bool {
        self.numer == 0
    }

    /// Whether this is strictly greater than zero, the test a *sounding* length
    /// has to pass.
    pub fn is_positive(&self) -> bool {
        self.numer > 0
    }

    /// The nearest `f64`, for a boundary that genuinely is inexact — seconds,
    /// milliseconds, a pixel. Never used to compare or accumulate: that is what
    /// the exact type is for.
    pub fn to_f64(&self) -> f64 {
        self.numer as f64 / self.denom as f64
    }

    /// This length as an integer count of `per_whole`ths, if it is exactly one.
    /// `None` when it is not — a triplet against a grid of 32nds, which is
    /// precisely the case a tick count cannot express and must not round away.
    pub fn as_ticks(&self, per_whole: i64) -> Option<i64> {
        let scaled = self.numer.checked_mul(per_whole)?;
        (scaled % self.denom == 0).then_some(scaled / self.denom)
    }

    /// A count of `per_whole`ths as an exact ratio — the other side of the same
    /// boundary, and how a v1 payload counted in ticks reaches the model.
    ///
    /// # Panics
    /// If `per_whole` is zero.
    pub fn from_ticks(ticks: i64, per_whole: i64) -> Ratio {
        Ratio::new(ticks, per_whole)
    }

    /// The largest integer not greater than this ratio, and what is left over —
    /// `(floor, self - floor)`, with the remainder always in `[0, 1)`. How a
    /// position on the time axis resolves into a whole count and an offset.
    pub fn floor_rem(&self) -> (i64, Ratio) {
        let floor = self.numer.div_euclid(self.denom);
        (floor, *self - Ratio::from(floor))
    }
}

/// Greatest common divisor, iterative so a long voice cannot recurse deeply.
/// `gcd(0, n)` is `n`, which is what normalizing zero needs.
fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a.max(1)
}

impl From<i64> for Ratio {
    fn from(n: i64) -> Ratio {
        Ratio { numer: n, denom: 1 }
    }
}

impl PartialOrd for Ratio {
    fn partial_cmp(&self, other: &Ratio) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Ratio {
    fn cmp(&self, other: &Ratio) -> Ordering {
        // Cross-multiply in i128: both denominators are positive, so the
        // comparison keeps its direction, and the widening is what stops a
        // long score's accumulated position from overflowing the compare.
        let left = self.numer as i128 * other.denom as i128;
        let right = other.numer as i128 * self.denom as i128;
        left.cmp(&right)
    }
}

impl Add for Ratio {
    type Output = Ratio;
    fn add(self, rhs: Ratio) -> Ratio {
        Ratio::new(
            self.numer * rhs.denom + rhs.numer * self.denom,
            self.denom * rhs.denom,
        )
    }
}

impl Sub for Ratio {
    type Output = Ratio;
    fn sub(self, rhs: Ratio) -> Ratio {
        self + (-rhs)
    }
}

impl Mul for Ratio {
    type Output = Ratio;
    fn mul(self, rhs: Ratio) -> Ratio {
        Ratio::new(self.numer * rhs.numer, self.denom * rhs.denom)
    }
}

impl Div for Ratio {
    type Output = Ratio;
    /// # Panics
    /// If `rhs` is zero.
    fn div(self, rhs: Ratio) -> Ratio {
        assert!(!rhs.is_zero(), "a ratio cannot be divided by zero");
        Ratio::new(self.numer * rhs.denom, self.denom * rhs.numer)
    }
}

impl Neg for Ratio {
    type Output = Ratio;
    fn neg(self) -> Ratio {
        Ratio {
            numer: -self.numer,
            denom: self.denom,
        }
    }
}

impl fmt::Display for Ratio {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.denom == 1 {
            write!(f, "{}", self.numer)
        } else {
            write!(f, "{}/{}", self.numer, self.denom)
        }
    }
}

impl Serialize for Ratio {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        [self.numer, self.denom].serialize(s)
    }
}

impl<'de> Deserialize<'de> for Ratio {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Ratio, D::Error> {
        let [numer, denom] = <[i64; 2]>::deserialize(d)?;
        if denom == 0 {
            return Err(de::Error::custom("a ratio's denominator cannot be zero"));
        }
        Ok(Ratio::new(numer, denom))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_on_construction() {
        assert_eq!(Ratio::new(2, 4), Ratio::new(1, 2));
        assert_eq!(Ratio::new(-2, -4), Ratio::new(1, 2));
        // The sign always rides the numerator, so equality is structural.
        let negative = Ratio::new(1, -2);
        assert_eq!(negative.numer(), -1);
        assert_eq!(negative.denom(), 2);
        // Zero normalizes to 0/1 whatever it was written over.
        assert_eq!(Ratio::new(0, 7), Ratio::ZERO);
        assert_eq!(Ratio::ZERO.denom(), 1);
    }

    #[test]
    fn arithmetic_is_exact_where_floats_are_not() {
        // Three triplet eighths are exactly one quarter -- the sum a float
        // gets wrong and a grid of 32nds cannot hold at all.
        let triplet = Ratio::new(1, 12);
        assert_eq!(triplet + triplet + triplet, Ratio::new(1, 4));
        assert_eq!(triplet.as_ticks(32), None);
        assert_eq!(Ratio::new(1, 8).as_ticks(32), Some(4));
        assert_eq!(Ratio::from_ticks(4, 32), Ratio::new(1, 8));
    }

    #[test]
    fn compares_without_overflowing() {
        // Denominators that would overflow i64 when cross-multiplied.
        let a = Ratio::new(1, 3_000_000_000);
        let b = Ratio::new(1, 3_000_000_001);
        assert!(a > b);
        assert!(Ratio::new(-1, 2) < Ratio::ZERO);
    }

    #[test]
    fn floor_rem_splits_a_position() {
        assert_eq!(Ratio::new(7, 2).floor_rem(), (3, Ratio::new(1, 2)));
        assert_eq!(Ratio::new(4, 2).floor_rem(), (2, Ratio::ZERO));
        // A negative position floors downward, so the remainder stays in [0, 1).
        assert_eq!(Ratio::new(-1, 2).floor_rem(), (-1, Ratio::new(1, 2)));
    }

    #[test]
    fn json_is_the_pair() {
        let r = Ratio::new(3, 4);
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, "[3,4]");
        assert_eq!(serde_json::from_str::<Ratio>(&json).unwrap(), r);
        // An unnormalized pair still arrives as the number it names.
        assert_eq!(
            serde_json::from_str::<Ratio>("[2,4]").unwrap(),
            Ratio::new(1, 2)
        );
        assert!(serde_json::from_str::<Ratio>("[1,0]").is_err());
    }
}
