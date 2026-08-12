//! The **rolling analysis** a retained live time-frequency view draws.
//!
//! A stored spectrogram is analyzed once: the samples are all there, the
//! transform is computed, the texture is uploaded. A live one has no such
//! moment — the samples arrive a tick at a time, and the picture is the last
//! `retention` seconds of them. Recomputing the whole transform each tick would
//! redo hundreds of FFTs to learn what the newest one or two say, so the
//! **columns are what is kept**: each whole hop that becomes available is
//! analyzed once, pushed on the back, and the oldest falls off the front.
//!
//! The state is keyed by widget rather than by bus, because two views of one
//! bus may analyze it differently (a different `fft_size`, a different `hop`) —
//! the history they read is shared, the transform of it is not.
//!
//! What this module deliberately does *not* do is own the picture. It analyzes,
//! and hands the columns that landed to whoever holds the transform — the GPU
//! view, whose [`Stft`] is the *same type* the stored path produces (a ring
//! rather than a whole analysis, which the renderer, the frequency ruler and
//! the cursor readout never have to know). A retained waterfall stays a
//! spectrogram in every respect but where its samples came from.
//!
//! [`Stft`]: crate::spectrogram::Stft

use crate::spectrogram::{MAX_ROLLING_FRAMES, analysis_window, column_into};

/// One retaining time-frequency view's rolling transform.
#[derive(Clone, Debug)]
pub struct Waterfall {
    /// The columns analyzed since the renderer last took them, frame-major.
    /// Retention is *not* kept here: the ring the picture rolls through belongs
    /// to the transform the GPU view owns, so a column is analyzed once, handed
    /// over once, and written into one texel.
    pending: Vec<f32>,
    n_bins: usize,
    window_size: usize,
    hop: usize,
    sample_rate: f32,
    /// The stream position of the first sample of the **next** column to
    /// analyze. The analysis is anchored to the bus's own stream rather than to
    /// the history's start, so a column covers the same samples however much
    /// history happens to be retained around it.
    next: Option<u64>,
    /// How many columns the retained span asks for — carried here because this
    /// is where the span is read off the tree, and handed to the view, which
    /// sizes its ring by it.
    capacity: usize,
    /// Whether the picture moved since the last time the renderer asked: a
    /// column landed, or the span changed under it.
    dirty: bool,
    /// The analysis window and its gain, plus the per-column scratch — held so
    /// a landing column allocates nothing.
    hann: Vec<f32>,
    gain: f32,
    windowed: Vec<f32>,
    spectrum: Vec<f32>,
}

impl Waterfall {
    /// A view analyzing at `window_size`/`hop`, retaining `capacity` columns.
    pub fn new(window_size: usize, hop: usize, sample_rate: f32, capacity: usize) -> Waterfall {
        let (hann, gain) = analysis_window(window_size);
        Waterfall {
            pending: Vec::new(),
            n_bins: window_size / 2,
            window_size,
            hop: hop.max(1),
            sample_rate,
            next: None,
            capacity: capacity.clamp(1, MAX_ROLLING_FRAMES),
            dirty: false,
            hann,
            gain,
            windowed: vec![0.0; window_size],
            spectrum: vec![0.0; window_size / 2],
        }
    }

    /// Whether the analysis parameters still match — a `/gui_set` of any of
    /// them restarts the roll rather than splicing two transforms, since the
    /// columns of one are not the columns of the other.
    pub fn matches(&self, window_size: usize, hop: usize, sample_rate: f32) -> bool {
        self.window_size == window_size && self.hop == hop.max(1) && self.sample_rate == sample_rate
    }

    /// Sets the retained span in columns (a live `/gui_set retention`), marking
    /// the picture moved so the view resizes its ring on the next pass.
    pub fn set_capacity(&mut self, capacity: usize) {
        let capacity = capacity.clamp(1, MAX_ROLLING_FRAMES);
        if capacity != self.capacity {
            self.capacity = capacity;
            self.dirty = true;
        }
    }

    /// The retained span in columns, as the view should size its ring.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// The analysis geometry a view is built against: window size, hop and
    /// sample rate.
    pub fn geometry(&self) -> (usize, usize, f32) {
        (self.window_size, self.hop, self.sample_rate)
    }

    /// Analyzes every whole window that `history` now holds and this view has
    /// not seen, where `end` is the stream position just past its newest
    /// sample. Returns how many columns landed.
    ///
    /// The history is a moving window of the bus, so a column can be *missed*
    /// rather than merely late: when the retained span no longer reaches back
    /// to where the next column would start, the analysis skips forward to the
    /// oldest sample still held. That is a real discontinuity in the picture
    /// and it is the honest one — the samples it would have covered are gone.
    pub fn advance(&mut self, history: &[f32], end: u64) -> usize {
        if history.len() < self.window_size {
            return 0;
        }
        let start = end - history.len() as u64; // position of `history[0]`
        let mut next = match self.next {
            Some(next) if next >= start => next,
            _ => start,
        };
        let mut landed = 0;
        while next + self.window_size as u64 <= end {
            let at = (next - start) as usize;
            self.push(&history[at..at + self.window_size]);
            next += self.hop as u64;
            landed += 1;
        }
        self.next = Some(next);
        if landed > 0 {
            self.dirty = true;
        }
        landed
    }

    /// Analyzes one window into a column, through the very function the
    /// stored transform uses — so a retained waterfall and an offline
    /// spectrogram of the same audio are the same picture.
    fn push(&mut self, frame: &[f32]) {
        let at = self.pending.len();
        self.pending.resize(at + self.n_bins, 0.0);
        column_into(
            frame,
            &self.hann,
            self.gain,
            &mut self.windowed,
            &mut self.spectrum,
            &mut self.pending[at..],
        );
        // A tick that fell far behind (a stalled window, a resumed stream) can
        // land more columns than the span retains; only the newest ones would
        // survive the ring, so the older ones never reach the GPU at all.
        let max = self.capacity * self.n_bins;
        if self.pending.len() > max {
            let excess = self.pending.len() - max;
            self.pending.drain(..excess);
        }
    }

    /// The columns analyzed since the last call, frame-major and oldest first,
    /// for the view to push into its ring. Clears the dirty flag.
    pub fn take_pending(&mut self) -> Vec<f32> {
        self.dirty = false;
        std::mem::take(&mut self.pending)
    }

    /// Whether a column landed (or the span moved) since the renderer last
    /// took the transform — what keeps the texture upload to the ticks that
    /// changed something.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// How many columns are waiting for the renderer. A test accessor: what the
    /// renderer wants is the columns themselves.
    #[cfg(test)]
    pub fn pending_columns(&self) -> usize {
        self.pending.len().checked_div(self.n_bins).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A steady tone: every column that lands measures the same thing, and the
    /// bin it lands in is the tone's.
    #[test]
    fn a_column_lands_per_hop_and_measures_the_tone() {
        let sr = 48_000.0f32;
        let ws = 256;
        let hop = 64;
        let mut w = Waterfall::new(ws, hop, sr, 100);
        // A 3 kHz sine: bin = 3000 / (48000 / 256) = 16.
        let n = 1024;
        let history: Vec<f32> = (0..n)
            .map(|i| (std::f32::consts::TAU * 3000.0 * i as f32 / sr).sin())
            .collect();
        let landed = w.advance(&history, n as u64);
        // Windows at 0, hop, 2*hop, ... while a whole one still fits.
        assert_eq!(landed, 1 + (n - ws) / hop);
        assert_eq!(w.pending_columns(), landed);
        let cols = w.take_pending();
        let bins = w.n_bins;
        let col = &cols[..bins];
        let peak = col
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert_eq!(peak, 16, "the tone's bin");
        // ...and it is the same column the stored transform computes.
        let stored = crate::spectrogram::Stft::compute(&history, ws, hop, sr);
        assert_eq!(&stored.magnitudes()[..bins], col);
    }

    /// The roll is anchored to the bus's stream, not to the history's start:
    /// feeding the same samples again as a longer history adds only the
    /// columns that are genuinely new.
    #[test]
    fn advancing_twice_does_not_re_analyze_what_it_saw() {
        let mut w = Waterfall::new(64, 32, 48_000.0, 100);
        let a: Vec<f32> = (0..128).map(|i| i as f32 * 0.001).collect();
        let first = w.advance(&a, 128);
        assert!(first > 0);
        // The same window again: nothing new past the position it reached.
        assert_eq!(w.advance(&a, 128), 0);
        // Sixty-four more samples: two more hops fit.
        let b: Vec<f32> = (0..192).map(|i| i as f32 * 0.001).collect();
        assert_eq!(w.advance(&b, 192), 2);
        assert_eq!(w.pending_columns(), first + 2);
    }

    /// The span caps what a single tick hands over: a backlog longer than the
    /// retained span drops the columns the ring would have discarded anyway,
    /// rather than uploading them to be overwritten. Narrowing it live marks
    /// the picture moved, so the view resizes its ring on the next pass.
    #[test]
    fn the_span_caps_what_a_tick_hands_over() {
        let mut w = Waterfall::new(64, 32, 48_000.0, 4);
        let s: Vec<f32> = (0..1024).map(|i| i as f32 * 0.001).collect();
        w.advance(&s, 1024);
        assert_eq!(w.pending_columns(), 4, "the oldest are not handed over");
        w.take_pending();
        w.set_capacity(2);
        assert_eq!(w.capacity(), 2);
        assert!(w.is_dirty());
    }

    /// A history shorter than one window analyzes nothing rather than
    /// analyzing a partial one — a half-filled window is not a measurement.
    #[test]
    fn a_history_shorter_than_a_window_lands_nothing() {
        let mut w = Waterfall::new(256, 128, 48_000.0, 10);
        assert_eq!(w.advance(&[0.0; 100], 100), 0);
        assert_eq!(w.pending_columns(), 0);
        assert!(!w.is_dirty());
    }
}
