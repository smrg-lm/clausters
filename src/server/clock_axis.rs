//! The two sample axes, as two types.
//!
//! The server keeps two sample counters: the **device** clock, which never
//! stops, and the **transport** clock, which advances only while the transport
//! rolls. Both are plain sample counts, so nothing but a type stops one being
//! read as the other -- and reading one as the other does not fail, it plays
//! audio in the wrong place.
//!
//! So the axis is the type. The two do not add to, compare with or convert
//! into each other except through [`DeviceSample::to_transport`] and
//! [`TransportSample::to_device`], which both take the frozen total explicitly.

/// A position on the **device** clock: samples processed since boot, never
/// pausing. This is what `/clock_query`, the taps, the bus streams and the
/// meters speak.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct DeviceSample(u64);

/// A position on the **transport** clock: samples elapsed under the transport,
/// frozen while it is stopped. This is the time of the piece.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct TransportSample(u64);

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
}
