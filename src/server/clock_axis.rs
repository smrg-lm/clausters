//! The sample axes, as types.
//!
//! The server keeps two sample **clocks**: the **device** clock, which never
//! stops, and the **transport** clock, which advances only while the transport
//! rolls. Both are plain sample counts, so nothing but a type stops one being
//! read as the other -- and reading one as the other does not fail, it plays
//! audio in the wrong place.
//!
//! So the axis is the type. The two do not add to, compare with or convert
//! into each other except through [`DeviceSample::to_transport`] and
//! [`TransportSample::to_device`], which both take the frozen total explicitly.
//!
//! Beside them is a third quantity that is **not a clock**: the
//! [`PiecePosition`], where the transport is in the piece. A clock counts
//! what has happened and only goes forward; a position says where you are and
//! moves wherever a locate puts it. Keeping them apart is what lets a
//! scheduler stay on an axis that cannot jump while a playhead sits on one
//! that can -- and it is a type here for the same reason the other two are,
//! since both are sample counts and confusing them is silent.

use std::ops::Range;

/// A position on the **device** clock: samples processed since boot, never
/// pausing. This is what `/clock_query`, the taps, the bus streams and the
/// meters speak.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct DeviceSample(u64);

/// A position on the **transport** clock: samples elapsed under the transport,
/// frozen while it is stopped.
///
/// It is monotonic, which is what the transport scheduler queue needs — "due"
/// only means anything on an axis that cannot jump. It is therefore *not*
/// where the piece is: a locate leaves this untouched. That is
/// [`PiecePosition`].
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct TransportSample(u64);

/// Where the transport is **in the piece**: a sample index of it.
///
/// Not a clock. It advances with the transport clock while rolling, holds
/// while stopped, jumps wherever `/transport_locate` puts it and wraps at a
/// loop's end. A playhead and a buffer reader following the transport want
/// this; a scheduled bundle wants [`TransportSample`].
///
/// Non-negative, like the clocks: locating before the start of the piece
/// clamps to 0.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct PiecePosition(u64);

/// What ties the piece's position to the transport clock: the position the
/// transport was last located to, and the transport sample it was located at.
///
/// Every read is `position + (now - since)`, so **a locate is one store of
/// this pair** and the position costs the audio thread no per-sample work at
/// all — which is the whole reason the position is anchored rather than
/// accumulated. A loop wrap is the same store: the engine cuts its block at
/// the wrap and re-anchors there, so within any one slice the position
/// advances by exactly one per sample and a reader following it can simply
/// ramp.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub struct PositionAnchor {
    position: PiecePosition,
    since: TransportSample,
}

impl DeviceSample {
    pub const fn new(samples: u64) -> Self {
        Self(samples)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    /// Onto the transport axis. `frozen_total` is the sum of every sample the
    /// transport has spent stopped. Saturating: see the test for why.
    pub const fn to_transport(self, frozen_total: u64) -> TransportSample {
        TransportSample(self.0.saturating_sub(frozen_total))
    }

    pub const fn saturating_add(self, samples: u64) -> Self {
        Self(self.0.saturating_add(samples))
    }

    /// The gap to an earlier position on the **same** axis, in samples.
    pub const fn saturating_sub_axis(self, earlier: Self) -> u64 {
        self.0.saturating_sub(earlier.0)
    }
}

impl TransportSample {
    pub const fn new(samples: u64) -> Self {
        Self(samples)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    /// Back onto the device axis.
    pub const fn to_device(self, frozen_total: u64) -> DeviceSample {
        DeviceSample(self.0.saturating_add(frozen_total))
    }

    pub const fn saturating_add(self, samples: u64) -> Self {
        Self(self.0.saturating_add(samples))
    }

    /// The gap to an earlier position on the **same** axis, in samples.
    pub const fn saturating_sub_axis(self, earlier: Self) -> u64 {
        self.0.saturating_sub(earlier.0)
    }
}

impl PiecePosition {
    pub const fn new(samples: u64) -> Self {
        Self(samples)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub const fn saturating_add(self, samples: u64) -> Self {
        Self(self.0.saturating_add(samples))
    }

    /// The gap to an earlier position on the **same** axis, in samples.
    pub const fn saturating_sub_axis(self, earlier: Self) -> u64 {
        self.0.saturating_sub(earlier.0)
    }
}

impl PositionAnchor {
    /// The anchor a locate to `position` at transport sample `now` produces.
    pub const fn located(position: PiecePosition, now: TransportSample) -> Self {
        Self {
            position,
            since: now,
        }
    }

    /// Where the piece is at transport sample `now`.
    ///
    /// Saturating on both halves: `now` before the anchor cannot happen (the
    /// transport clock does not run backwards), and saturating there means a
    /// wrong call reads as the anchor rather than as a position near `u64::MAX`
    /// — which is the same choice [`DeviceSample::to_transport`] makes.
    pub const fn at(self, now: TransportSample) -> PiecePosition {
        self.position
            .saturating_add(now.saturating_sub_axis(self.since))
    }

    /// The transport sample at which the piece reaches `position`, or `None`
    /// when it already has — what the engine asks in order to cut its block at
    /// a loop's end.
    pub const fn reaching(
        self,
        position: PiecePosition,
        now: TransportSample,
    ) -> Option<TransportSample> {
        let ahead = position.saturating_sub_axis(self.at(now));
        if ahead == 0 {
            None
        } else {
            Some(now.saturating_add(ahead))
        }
    }

    /// Re-anchored at `now` without moving the piece: the same position, tied
    /// to a fresh transport sample. What a loop wrap and a resume both do.
    pub const fn wrapped_to(self, position: PiecePosition, now: TransportSample) -> Self {
        Self::located(position, now)
    }
}

/// A loop over the piece: the span the position wraps inside while looping is
/// on. Half-open — the end sample is the first one *not* played, so a loop of
/// `0..n` over an `n`-sample take plays every sample exactly once and joins
/// its own start with no repeated frame.
pub type Loop = Range<PiecePosition>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_to_transport_subtracts_frozen_time() {
        let d = DeviceSample::new(48_000);
        assert_eq!(d.to_transport(1_000), TransportSample::new(47_000));
    }

    #[test]
    fn transport_to_device_adds_frozen_time() {
        let t = TransportSample::new(47_000);
        assert_eq!(t.to_device(1_000), DeviceSample::new(48_000));
    }

    #[test]
    fn conversion_round_trips() {
        let d = DeviceSample::new(123_456);
        assert_eq!(d.to_transport(7_890).to_device(7_890), d);
    }

    #[test]
    fn frozen_longer_than_elapsed_saturates_at_zero() {
        // Cannot happen (frozen_total is always <= now), but a wrapping
        // subtraction here would produce a bundle ~584,000 years in the
        // future, which is the worst possible failure mode.
        let d = DeviceSample::new(100);
        assert_eq!(d.to_transport(500), TransportSample::new(0));
    }

    #[test]
    fn ordering_is_by_value() {
        assert!(DeviceSample::new(1) < DeviceSample::new(2));
        assert!(TransportSample::new(9) > TransportSample::new(8));
    }

    /// The anchor's whole job: the position advances one per transport sample.
    #[test]
    fn the_position_advances_with_the_transport_clock() {
        let a = PositionAnchor::located(PiecePosition::new(1_000), TransportSample::new(500));
        assert_eq!(a.at(TransportSample::new(500)), PiecePosition::new(1_000));
        assert_eq!(a.at(TransportSample::new(564)), PiecePosition::new(1_064));
    }

    /// A locate moves the piece and leaves the clock alone -- the distinction
    /// the two types exist for. Both anchors are read at the same transport
    /// sample and give different positions.
    #[test]
    fn a_locate_moves_the_position_and_not_the_clock() {
        let now = TransportSample::new(48_000);
        let before = PositionAnchor::located(PiecePosition::new(0), TransportSample::new(0));
        assert_eq!(before.at(now), PiecePosition::new(48_000));
        let after = PositionAnchor::located(PiecePosition::new(10), now);
        assert_eq!(after.at(now), PiecePosition::new(10));
    }

    /// What the engine asks in order to cut a block at a loop's end.
    #[test]
    fn reaching_reports_the_transport_sample_a_position_arrives_at() {
        let a = PositionAnchor::located(PiecePosition::new(100), TransportSample::new(0));
        let now = TransportSample::new(10);
        assert_eq!(
            a.reaching(PiecePosition::new(150), now),
            Some(TransportSample::new(50)),
            "the piece is at 110, so 40 samples to play and 40 of the clock"
        );
        assert_eq!(
            a.reaching(PiecePosition::new(110), now),
            None,
            "already there: a loop end at the current position is not ahead"
        );
        assert_eq!(a.reaching(PiecePosition::new(0), now), None, "behind");
    }

    /// A wrap re-anchors without the position drifting: the sample after the
    /// last one of the loop is the loop's first, not the one after it.
    #[test]
    fn a_wrap_re_anchors_on_the_loop_start() {
        let a = PositionAnchor::located(PiecePosition::new(0), TransportSample::new(0));
        let end = a
            .reaching(PiecePosition::new(64), TransportSample::new(0))
            .expect("ahead");
        let wrapped = a.wrapped_to(PiecePosition::new(0), end);
        assert_eq!(wrapped.at(end), PiecePosition::new(0));
        assert_eq!(wrapped.at(end.saturating_add(1)), PiecePosition::new(1));
    }

    /// A read before the anchor cannot happen; if it did it must not read as a
    /// position near `u64::MAX`, for the reason the frozen-total test gives.
    #[test]
    fn a_read_before_the_anchor_saturates_at_the_anchor() {
        let a = PositionAnchor::located(PiecePosition::new(7), TransportSample::new(500));
        assert_eq!(a.at(TransportSample::new(100)), PiecePosition::new(7));
    }
}
