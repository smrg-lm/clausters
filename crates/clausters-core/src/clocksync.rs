//! Sample-clock tracking model: `sample(t) = a + b·t` by least squares.
//!
//! The value/time half of locking a client clock to a server's sample counter
//! over a network transport (the Python client's `UdpSampleClock`, the future
//! TS client alike): anchors are `(local monotonic time, counter)` pairs from
//! `/clock` round trips, fitted over a sliding window (JACK-DLL / Ableton-Link
//! in spirit). The transport — sockets, threads, round-trip midpoints — stays
//! in the host language; this module owns only the model, so every client
//! predicts the same sample from the same anchors.

/// Least-squares line `sample(t) = a + b·t` over a sliding anchor window.
///
/// With fewer than two anchors the slope falls back to the nominal rate
/// (anchored at the latest pair); from two on it is the fitted slope.
#[derive(Clone, Debug)]
pub struct SampleClockModel {
    rate: f64,
    window: usize,
    anchors: Vec<(f64, i64)>,
    a: f64,
    b: f64,
}

impl SampleClockModel {
    /// A model at `nominal_rate` keeping at most `window` anchors (min 1).
    pub fn new(nominal_rate: f64, window: usize) -> Self {
        Self {
            rate: nominal_rate,
            window: window.max(1),
            anchors: Vec::new(),
            a: 0.0,
            b: nominal_rate,
        }
    }

    /// Adds an anchor pair and refits. A finite positive `rate` updates the
    /// nominal rate (the `/clock` reply carries it); pass a non-positive value
    /// to keep the current one.
    pub fn add_anchor(&mut self, t_local: f64, sample: i64, rate: f64) {
        if rate > 0.0 && rate.is_finite() {
            self.rate = rate;
        }
        self.anchors.push((t_local, sample));
        if self.anchors.len() > self.window {
            let drop = self.anchors.len() - self.window;
            self.anchors.drain(..drop);
        }
        self.fit();
    }

    fn fit(&mut self) {
        let n = self.anchors.len();
        let (t_ref, s_ref) = *self.anchors.last().expect("fit after push");
        if n < 2 {
            self.b = self.rate;
            self.a = s_ref as f64 - self.rate * t_ref;
            return;
        }
        let inv = 1.0 / n as f64;
        let t_mean = self.anchors.iter().map(|(t, _)| t).sum::<f64>() * inv;
        let s_mean = self.anchors.iter().map(|(_, s)| *s as f64).sum::<f64>() * inv;
        let var: f64 = self.anchors.iter().map(|(t, _)| (t - t_mean).powi(2)).sum();
        let cov: f64 = self
            .anchors
            .iter()
            .map(|(t, s)| (t - t_mean) * (*s as f64 - s_mean))
            .sum();
        self.b = if var > 0.0 { cov / var } else { self.rate };
        self.a = s_mean - self.b * t_mean;
    }

    /// The predicted counter at local time `t_local` (nearest sample).
    pub fn sample_at(&self, t_local: f64) -> i64 {
        (self.a + self.b * t_local).round_ties_even() as i64
    }

    /// Inverse: the local time the counter reaches `sample`.
    pub fn local_time_of(&self, sample: i64) -> f64 {
        (sample as f64 - self.a) / self.b
    }

    /// Fitted-slope deviation from the nominal rate, in parts per million.
    pub fn drift_ppm(&self) -> f64 {
        (self.b / self.rate - 1.0) * 1e6
    }

    /// Local-time span covered by the current anchor window (0 below two).
    pub fn span(&self) -> f64 {
        match (self.anchors.first(), self.anchors.last()) {
            (Some((t0, _)), Some((t1, _))) if self.anchors.len() >= 2 => t1 - t0,
            _ => 0.0,
        }
    }

    /// The nominal (or last reported) sample rate.
    pub fn rate(&self) -> f64 {
        self.rate
    }

    /// Fitted intercept `a` (samples at local time 0).
    pub fn intercept(&self) -> f64 {
        self.a
    }

    /// Fitted slope `b` (samples per local second).
    pub fn slope(&self) -> f64 {
        self.b
    }

    /// Number of anchors currently in the window.
    pub fn len(&self) -> usize {
        self.anchors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.anchors.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovers_a_clean_line() {
        let mut m = SampleClockModel::new(48_000.0, 64);
        for i in 0..6 {
            let t = i as f64 * 0.05;
            m.add_anchor(t, (1000.0 + 48_000.0 * t).round() as i64, 48_000.0);
        }
        assert!((m.sample_at(1.0) - (1000 + 48_000)).abs() <= 1);
        assert!(m.drift_ppm().abs() < 1.0);
        assert!((m.local_time_of(1000 + 24_000) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn measures_crystal_drift() {
        let mut m = SampleClockModel::new(48_000.0, 64);
        for i in 0..10 {
            let t = i as f64 * 0.1;
            m.add_anchor(t, (1000.0 + 48_010.0 * t).round() as i64, 48_000.0);
        }
        assert!((m.drift_ppm() - 208.3).abs() < 2.0);
        assert!((m.span() - 0.9).abs() < 1e-6);
    }

    #[test]
    fn single_anchor_falls_back_to_nominal_rate() {
        let mut m = SampleClockModel::new(48_000.0, 64);
        m.add_anchor(2.0, 96_000, 48_000.0);
        assert_eq!(m.slope(), 48_000.0);
        assert_eq!(m.sample_at(3.0), 96_000 + 48_000);
    }

    #[test]
    fn window_slides() {
        let mut m = SampleClockModel::new(48_000.0, 4);
        for i in 0..10 {
            m.add_anchor(i as f64, i * 48_000, 48_000.0);
        }
        assert_eq!(m.len(), 4);
        assert!((m.span() - 3.0).abs() < 1e-9);
    }
}
