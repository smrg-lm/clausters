//! The three axes a session measures time on, as **types that do not mix**.
//!
//! A session carries three of them at once and they are not interchangeable:
//!
//! - [`Beat`] — the **musical** axis. Where a region sits in the piece, where a
//!   tempo change happens, what a bar line is. Moved by a tempo map, not by a
//!   sample rate.
//! - [`TimelineFrame`] — the **timeline's own** sample frames: what the
//!   transport counts, what a view scrolls over, what a render writes. Related
//!   to [`Beat`] only through the tempo map, and to nothing else.
//! - [`ContentFrame`] — a frame **inside one source**. Where a region's window
//!   opens into the samples it plays. It is a coordinate in somebody else's
//!   recording, and the only reason it looks like a timeline frame is that both
//!   are counted in samples.
//!
//! # Why these are types and not comments
//!
//! They were one type — `f64`, with a doc comment saying which axis a
//! particular one was on — and the comment is not read by anything. It has
//! already cost a defect: a threshold computed on the wrong axis turned every
//! clip move into a trim, because a number of timeline samples was compared
//! against a number of beats and both are `f64`. A newtype makes that a
//! compile error, and it is the cheapest thing in this milestone.
//!
//! Zrythm's 2026 arrangement overhaul reached the same shape from the same
//! problem, minting `ContentTick`/`TimelineTick` beside its `Position`
//! primitive. The names here are ours; the reason is theirs as well as ours.
//!
//! # What they deliberately do not have
//!
//! **No conversions between axes.** Not `From`, not `Into`, not a method. A
//! beat becomes a frame only through a tempo map and a sample rate, and both
//! belong to whoever holds them — the client, or the host. An implicit
//! conversion here would be this crate guessing a tempo, which is the one thing
//! it refuses to do everywhere else ([`crate::Body`]'s opaque leaf, the
//! `secs_to_beats` a caller has to supply). What they do have is the arithmetic
//! that stays on one axis: adding two lengths, subtracting two positions,
//! scaling by a plain number.
//!
//! **No unit inside the name of a length.** A position and a length are the
//! same type on each axis, as they are in every DAW's format, because the
//! difference is what a field *means* rather than what it holds — `position`
//! and `length` on a region say it, and a type that said it too would double
//! every operator below for nothing.

use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

use serde::{Deserialize, Serialize};

/// Mints one axis: a transparent newtype with the arithmetic that stays on it.
macro_rules! axis {
    ($(#[$meta:meta])* $name:ident($inner:ty)) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub $inner);

        impl $name {
            /// The origin of this axis.
            pub const ZERO: Self = Self(0 as $inner);

            /// The number, for a caller that is leaving the axis on purpose —
            /// writing a message, drawing a pixel, calling a converter that
            /// takes plain numbers.
            pub fn get(self) -> $inner {
                self.0
            }

            /// The larger of two.
            pub fn max(self, other: Self) -> Self {
                if other.0 > self.0 { other } else { self }
            }

            /// The smaller of two.
            pub fn min(self, other: Self) -> Self {
                if other.0 < self.0 { other } else { self }
            }

            /// This position or length, never before the origin.
            pub fn clamp_positive(self) -> Self {
                self.max(Self::ZERO)
            }
        }

        impl Add for $name {
            type Output = Self;
            fn add(self, rhs: Self) -> Self { Self(self.0 + rhs.0) }
        }

        impl Sub for $name {
            type Output = Self;
            fn sub(self, rhs: Self) -> Self { Self(self.0 - rhs.0) }
        }

        impl AddAssign for $name {
            fn add_assign(&mut self, rhs: Self) { self.0 += rhs.0; }
        }

        impl SubAssign for $name {
            fn sub_assign(&mut self, rhs: Self) { self.0 -= rhs.0; }
        }

        impl Neg for $name {
            type Output = Self;
            fn neg(self) -> Self { Self(-self.0) }
        }

        /// Scaling by a plain number stays on the axis: half a length is a
        /// length, and a playrate applied to a window is still frames of that
        /// source.
        impl Mul<f64> for $name {
            type Output = Self;
            fn mul(self, k: f64) -> Self { Self((self.0 as f64 * k) as $inner) }
        }

        /// Dividing one of these by another leaves the axis, which is the
        /// point: a ratio is a plain number and has no axis to be on.
        impl Div for $name {
            type Output = f64;
            fn div(self, rhs: Self) -> f64 { self.0 as f64 / rhs.0 as f64 }
        }
    };
}

axis! {
    /// A position or a length on the **musical** axis.
    ///
    /// One beat, not one bar and not one tick: the meter says how beats make
    /// bars, and a resolution finer than a beat is a fraction of one. What
    /// moves a beat to any other axis is the tempo map, and this type has no
    /// opinion about it.
    Beat(f64)
}

axis! {
    /// A position or a length in the **timeline's own** sample frames.
    ///
    /// What the transport counts and a view scrolls over. Signed, because a
    /// difference of two positions is one of these and may run backwards; a
    /// position that must not is clamped where it is read, not by the type.
    TimelineFrame(i64)
}

axis! {
    /// A frame **inside one source** — where a region's window opens into the
    /// samples it plays.
    ///
    /// Never a timeline position, however much it looks like one. Two regions
    /// over one source at different points in the piece hold the same
    /// [`ContentFrame`] and different [`TimelineFrame`]s, which is the whole of
    /// what non-destructive editing is, and the reason these are two types.
    ContentFrame(i64)
}

axis! {
    /// A position or a length **inside a source made of events** — the beats of
    /// a node this document holds, rather than of the piece.
    ///
    /// The [`ContentFrame`] of material that has no frames. A window onto a
    /// timeline of notes opens at one of these, and it moves with that
    /// content's own tempo rather than with the session's.
    ContentBeat(f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_axis_adds_and_subtracts() {
        assert_eq!(Beat(2.0) + Beat(1.5), Beat(3.5));
        assert_eq!(TimelineFrame(480) - TimelineFrame(80), TimelineFrame(400));
        let mut at = ContentFrame(0);
        at += ContentFrame(256);
        assert_eq!(at, ContentFrame(256));
    }

    #[test]
    fn scaling_stays_on_the_axis_and_a_ratio_leaves_it() {
        assert_eq!(Beat(4.0) * 0.5, Beat(2.0));
        // A playrate over a window: still frames of that source.
        assert_eq!(ContentFrame(1000) * 2.0, ContentFrame(2000));
        // ...and what a ratio of two lengths is, is a number.
        assert_eq!(TimelineFrame(960) / TimelineFrame(480), 2.0);
    }

    #[test]
    fn each_axis_serializes_as_the_bare_number() {
        assert_eq!(serde_json::to_string(&Beat(1.5)).unwrap(), "1.5");
        assert_eq!(
            serde_json::to_string(&TimelineFrame(48_000)).unwrap(),
            "48000"
        );
        assert_eq!(
            serde_json::from_str::<ContentFrame>("256").unwrap(),
            ContentFrame(256)
        );
    }

    #[test]
    fn a_position_and_a_length_are_ordered_within_their_axis() {
        assert!(Beat(1.0) < Beat(2.0));
        assert_eq!(Beat(1.0).max(Beat(2.0)), Beat(2.0));
        assert_eq!(TimelineFrame(-5).clamp_positive(), TimelineFrame::ZERO);
    }

    /// The whole point, and the only thing a test can say about it: the axes do
    /// not mix. `Beat(1.0) + TimelineFrame(1)` does not compile, and neither
    /// does comparing them, so what is checked here is that the *values* are
    /// distinct types rather than one alias with two names.
    #[test]
    fn the_axes_are_different_types() {
        fn takes_a_beat(_: Beat) {}
        fn takes_a_frame(_: TimelineFrame) {}
        takes_a_beat(Beat(1.0));
        takes_a_frame(TimelineFrame(1));
        // Same number, same representation, and neither function accepts the
        // other's argument -- which is what the trim defect needed.
        assert_eq!(Beat(1.0).get(), 1.0);
        assert_eq!(TimelineFrame(1).get(), 1);
    }
}
