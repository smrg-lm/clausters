//! The short-time Fourier transform a spectrogram is drawn from: the analysis,
//! the time pyramid over it and the cache it is kept in.
//!
//! One step above [`crate::fft`], [`crate::window`] and [`crate::spectrum`]:
//! a whole signal windowed and transformed every `hop` samples into
//! `n_frames` x `n_bins` magnitudes, normalized over a fixed decibel range
//! and stored the way a peak pyramid stores a signal's extremes. It is a
//! **cache**: computed once, kept in memory, and written to or read from a
//! flat buffer or a file.
//!
//! It is here, and was in the GUI host, for the reason the peak pyramid is
//! here: more than one process needs the same columns. The host draws them;
//! a client that analyzes a take itself has to write the file the host reads
//! ([`Stft::to_bytes`]), and a column analyzed twice by two implementations
//! is two pictures of one sound. What stays with whoever draws is the
//! display: the frequency scale, the decibel window, the colors, and how
//! much of a transform is on a card at once.

use crate::{bytes, fft};

const MAGIC: &[u8; 4] = b"CLSG";
const VERSION: u32 = 2;
/// Reference dB range the stored magnitudes are normalized over: the floor a
/// spectrum curve is clamped to ([`crate::spectrum::REF_FLOOR`]), so a
/// spectrogram column and a spectrum of the same audio agree. The *display*
/// dB window (which controls contrast) is whoever draws it's, within this
/// range, so it can change live without recomputing the transform.
pub const REF_FLOOR: f32 = crate::spectrum::REF_FLOOR;

/// The most magnitudes a stored transform keeps: 64 MiB of them. A transform
/// used to be as wide as one texture, so five minutes at 48 kHz, asked for at
/// a hop of 512, came out at a hop of 1758; they come out at 512 now, and
/// only past some six minutes does the hop rise. What bounds a transform is
/// memory; whoever draws it shows a window of it.
pub const MAX_STORED: usize = 1 << 24;

/// The frames a transform is never held under, however many bins each has:
/// what one magnitude texture is wide, so a window of many bins is still as
/// fine in time as it always was.
pub const MIN_FRAMES: usize = 8192;

/// The most frames a stored transform of `window_size` keeps: [`MAX_STORED`]
/// magnitudes' worth, and never fewer than [`MIN_FRAMES`].
pub fn max_frames(window_size: usize) -> usize {
    (MAX_STORED / (window_size / 2).max(1)).max(MIN_FRAMES)
}

/// The hop to analyze `total_samples` with: the requested `hop`, raised just
/// enough that the STFT yields at most [`max_frames`] frames. A very long
/// file thus trades time resolution for the memory its transform takes --
/// not, any more, for the width of a texture.
pub fn hop_capped(total_samples: usize, window_size: usize, hop: usize) -> usize {
    let needed = total_samples
        .saturating_sub(window_size)
        .div_ceil(max_frames(window_size).saturating_sub(1).max(1))
        .max(1);
    hop.max(needed)
}

/// **How many bytes the cache of [`Stft::analyze`] takes** for
/// `total_samples` at `window_size` and `hop` -- what a caller that fills a
/// buffer sizes it with. `0` where the analysis has no answer.
pub fn cache_size(total_samples: usize, window_size: usize, hop: usize) -> usize {
    if !fft::supports(window_size) || hop == 0 {
        return 0;
    }
    let hop = hop_capped(total_samples, window_size, hop);
    let frames = if total_samples < window_size {
        1
    } else {
        1 + (total_samples - window_size) / hop
    };
    HEADER + 4 * frames * (window_size / 2)
}

/// The cache's header: the tag, the version, five lengths and the rate.
const HEADER: usize = 4 + 4 + 5 * 8 + 4;

/// The analysis window and its coherent gain: a Hann window, and the sum that
/// normalizes a full-scale sine to about 0 dB. Computed once per transform, not
/// once per column.
pub fn analysis_window(window_size: usize) -> (Vec<f32>, f32) {
    let hann: Vec<f32> = (0..window_size)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / window_size as f32).cos())
        .collect();
    let gain = hann.iter().sum::<f32>() * 0.5;
    (hann, gain)
}

/// **One column of a spectrogram**: `frame` windowed, transformed, and mapped
/// to the normalized 0..1 magnitudes the texture stores.
///
/// It is a free function rather than a method because two paths produce
/// columns and they must produce the *same* ones: the stored transform
/// ([`Stft::compute`], analyzing a whole buffer at once) and the rolling one a
/// retained live view keeps (`host::waterfall`, analyzing a column at a time as
/// the samples arrive). A retained waterfall and an offline spectrogram of the
/// same audio are then the same picture, which is the only reason the renderer,
/// the frequency ruler and the cursor readout can stay one implementation.
///
/// `windowed` and `spectrum` are scratch the caller owns, so a rolling
/// analysis allocates nothing per column.
pub fn column_into(
    frame: &[f32],
    hann: &[f32],
    win_gain: f32,
    windowed: &mut [f32],
    spectrum: &mut [f32],
    out: &mut [f32],
) {
    for (i, w) in windowed.iter_mut().enumerate() {
        *w = frame.get(i).copied().unwrap_or(0.0) * hann[i];
    }
    // The forward FFT lives once in the shared core (`clausters_core::fft`).
    fft::rfft_magnitudes_into(windowed, spectrum);
    for (o, m) in out.iter_mut().zip(spectrum.iter()) {
        let db = 20.0 * (m / win_gain + 1e-9).log10();
        *o = ((db - REF_FLOOR) / -REF_FLOOR).clamp(0.0, 1.0);
    }
}

/// A short-time Fourier transform: `n_frames` x `n_bins` normalized magnitudes
/// in `[0, 1]` (dB mapped from `[DB_FLOOR, 0]`), row-major by frame. Frame `f`
/// is centred on samples starting at `f * hop`.
///
/// A transform is **stored** or **rolling**. A stored one is analyzed once and
/// its columns are exactly the ones it holds. A rolling one ([`Stft::rolling`])
/// is a fixed-capacity ring a live view pushes into, one column per hop, the
/// oldest falling off the front -- the same magnitudes in the same order, read
/// through [`Stft::column`] instead of straight off `mags`.
pub struct Stft {
    total_samples: usize,
    n_frames: usize,
    n_bins: usize,
    hop: usize,
    window_size: usize,
    sample_rate: f32,
    mags: Vec<f32>,
    /// Ring capacity in columns, or `0` for a stored transform. A rolling
    /// transform's `mags` is always `capacity * n_bins` long, however few
    /// columns have landed.
    capacity: usize,
    /// Ring index of the oldest retained column (always `0` when stored).
    head: usize,
    /// **The time pyramid** of a stored transform, built for the view that
    /// draws it ([`Stft::build_pyramid`]): level `l` (index `l - 1`) holds a
    /// column for every `2^l` of the transform's own, each bin the **largest**
    /// of the columns it stands for, quantized as the texture stores them.
    ///
    /// The same reduction a peak pyramid makes of a signal, for the same
    /// reason: zoomed out, a pixel column stands for many frames, and what it
    /// has to show is what happened in them -- a click included -- rather than
    /// whichever two of them a sampler landed between.
    levels: Vec<Vec<u8>>,
}

/// A magnitude as a texture stores it: one byte of the normalized range.
pub fn quantize(m: f32) -> u8 {
    (m.clamp(0.0, 1.0) * 255.0).round() as u8
}

impl Stft {
    /// Compute the STFT of mono `samples`. `window_size` must be a power of two;
    /// `hop` is the frame advance (e.g. `window_size / 2`); `sample_rate` is used
    /// for the frequency axis.
    pub fn compute(samples: &[f32], window_size: usize, hop: usize, sample_rate: f32) -> Self {
        assert!(
            fft::supports(window_size) && hop >= 1,
            "window_size must be a supported FFT size {:?}",
            fft::SUPPORTED_SIZES
        );
        let total_samples = samples.len();
        let n_bins = window_size / 2;
        let n_frames = if total_samples < window_size {
            1
        } else {
            1 + (total_samples - window_size) / hop
        };

        let (hann, win_gain) = analysis_window(window_size);
        let mut mags = vec![0.0f32; n_frames * n_bins];
        let mut windowed = vec![0.0f32; window_size];
        let mut spectrum = vec![0.0f32; n_bins]; // n_bins == window_size / 2
        for f in 0..n_frames {
            let start = f * hop;
            let frame: Vec<f32> = (0..window_size)
                .map(|i| samples.get(start + i).copied().unwrap_or(0.0))
                .collect();
            column_into(
                &frame,
                &hann,
                win_gain,
                &mut windowed,
                &mut spectrum,
                &mut mags[f * n_bins..(f + 1) * n_bins],
            );
        }

        Self {
            total_samples,
            n_frames,
            n_bins,
            hop,
            window_size,
            sample_rate,
            mags,
            capacity: 0,
            head: 0,
            levels: Vec::new(),
        }
    }

    /// **The transform of a take as every end makes it**: [`compute`] at the
    /// hop asked for, raised only where the take is longer than a transform
    /// keeps ([`hop_capped`]). `None` for a window the FFT has no size for,
    /// or a hop of nothing.
    ///
    /// The host analyzing samples it mapped and a client writing a cache for
    /// it call this, so the file one writes is the transform the other
    /// would have computed.
    ///
    /// [`compute`]: Stft::compute
    pub fn analyze(
        samples: &[f32],
        window_size: usize,
        hop: usize,
        sample_rate: f32,
    ) -> Option<Self> {
        (fft::supports(window_size) && hop >= 1).then(|| {
            let hop = hop_capped(samples.len(), window_size, hop);
            Self::compute(samples, window_size, hop, sample_rate)
        })
    }

    /// An empty **rolling** transform: a ring of `capacity` columns a retained
    /// live view pushes into ([`push_column`]), the oldest falling off the front.
    ///
    /// What needs it is that a live picture is not analyzed at a moment: a
    /// retained view adds one column per hop, and recomputing the whole
    /// transform each tick would redo hundreds of FFTs to learn what one of them
    /// says -- and, worse, re-upload the whole texture to show it. The ring is
    /// what makes both costs follow the *hop* instead of the span: a landing
    /// column is one FFT and one texel write.
    ///
    /// `capacity` is the caller's to bound (a host by what its texture holds).
    ///
    /// [`push_column`]: Stft::push_column
    pub fn rolling(
        capacity: usize,
        n_bins: usize,
        hop: usize,
        window_size: usize,
        sample_rate: f32,
    ) -> Self {
        let capacity = capacity.max(1);
        Stft {
            total_samples: window_size,
            n_frames: 0,
            n_bins,
            hop: hop.max(1),
            window_size,
            sample_rate,
            mags: vec![0.0; capacity * n_bins],
            capacity,
            head: 0,
            levels: Vec::new(),
        }
    }

    /// Builds the time pyramid of a stored transform (see the field): levels
    /// halving the frame count until one fits a screen several times over.
    /// Nothing for a rolling transform, whose ring is its whole picture, and
    /// nothing a second time.
    pub fn build_pyramid(&mut self) {
        if self.capacity > 0 || !self.levels.is_empty() || self.n_bins == 0 {
            return;
        }
        let bins = self.n_bins;
        let mut frames = self.n_frames;
        // Down to a level a narrow view still has more columns of than
        // pixels; below that a coarser one would never be asked for.
        while frames > 256 {
            let next = frames.div_ceil(2);
            let mut level = vec![0u8; next * bins];
            let below = self.levels.last();
            for (f, out) in level.chunks_exact_mut(bins).enumerate() {
                for k in [2 * f, 2 * f + 1] {
                    if k >= frames {
                        continue;
                    }
                    match below {
                        Some(texels) => {
                            for (o, m) in out.iter_mut().zip(&texels[k * bins..(k + 1) * bins]) {
                                *o = (*o).max(*m);
                            }
                        }
                        None => {
                            for (o, m) in out.iter_mut().zip(self.column(k)) {
                                *o = (*o).max(quantize(*m));
                            }
                        }
                    }
                }
            }
            frames = next;
            self.levels.push(level);
        }
    }

    /// How many levels the pyramid has above the transform's own columns.
    pub fn levels(&self) -> usize {
        self.levels.len()
    }

    /// How many columns level `level` has (`0` = the transform's own).
    pub fn level_frames(&self, level: usize) -> usize {
        match level.checked_sub(1).and_then(|l| self.levels.get(l)) {
            Some(texels) => texels.len() / self.n_bins.max(1),
            None => self.n_frames,
        }
    }

    /// Column `frame` of level `level`, quantized, into `out` (`n_bins` long).
    pub fn level_column(&self, level: usize, frame: usize, out: &mut [u8]) {
        match level.checked_sub(1).and_then(|l| self.levels.get(l)) {
            Some(texels) => {
                let at = frame * self.n_bins;
                out.copy_from_slice(&texels[at..at + self.n_bins]);
            }
            None => {
                for (o, m) in out.iter_mut().zip(self.column(frame)) {
                    *o = quantize(*m);
                }
            }
        }
    }

    /// Nyquist frequency in Hz (the top of the frequency axis).
    pub fn nyquist(&self) -> f32 {
        self.sample_rate * 0.5
    }

    pub fn total_samples(&self) -> usize {
        self.total_samples
    }
    pub fn n_frames(&self) -> usize {
        self.n_frames
    }
    pub fn n_bins(&self) -> usize {
        self.n_bins
    }
    pub fn hop(&self) -> usize {
        self.hop
    }
    pub fn window_size(&self) -> usize {
        self.window_size
    }
    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }
    /// The ring's capacity in columns, or `0` when the transform is stored.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
    /// Whether this is a ring a live view pushes into rather than a stored
    /// analysis.
    pub fn is_rolling(&self) -> bool {
        self.capacity > 0
    }
    /// The magnitudes as they sit in memory -- frame-major for a stored
    /// transform, and in *ring* order (rotated by `head`) for a rolling one, so
    /// a caller that wants columns in time order asks [`Stft::column`] instead.
    pub fn magnitudes(&self) -> &[f32] {
        &self.mags
    }

    /// Logical column `i` (0 = the oldest retained), whichever texel it lives in.
    pub fn column(&self, i: usize) -> &[f32] {
        let at = self.texel_of(i) * self.n_bins;
        &self.mags[at..at + self.n_bins]
    }

    /// The texture column logical column `i` occupies.
    pub fn texel_of(&self, i: usize) -> usize {
        if self.capacity > 0 {
            (self.head + i) % self.capacity
        } else {
            i
        }
    }

    /// The width of the magnitude texture this transform is drawn from.
    ///
    /// A rolling ring is stored **twice**, back to back, which is what keeps the
    /// visible window one contiguous run of texels however far the write cursor
    /// has wrapped. The alternative is wrapping in the shader, and a linear
    /// sample across the seam blends the newest column into the oldest -- a
    /// visible stripe travelling through the picture. The doubled width is why
    /// a rolling ring is bounded at half a texture by whoever draws it.
    pub fn tex_width(&self) -> usize {
        if self.capacity > 0 {
            self.capacity * 2
        } else {
            self.n_frames.max(1)
        }
    }

    /// Appends one analyzed column, dropping the oldest once the ring is full.
    /// Returns the texel column it landed in; its mirror sits `capacity` texels
    /// to the right. A no-op (returning 0) on a stored transform.
    pub fn push_column(&mut self, col: &[f32]) -> usize {
        if self.capacity == 0 {
            return 0;
        }
        let slot = (self.head + self.n_frames) % self.capacity;
        let at = slot * self.n_bins;
        let n = self.n_bins.min(col.len());
        self.mags[at..at + n].copy_from_slice(&col[..n]);
        self.mags[at + n..at + self.n_bins].fill(0.0);
        if self.n_frames < self.capacity {
            self.n_frames += 1;
        } else {
            self.head = (self.head + 1) % self.capacity;
        }
        self.total_samples = self.n_frames.saturating_sub(1) * self.hop + self.window_size;
        slot
    }

    /// Resizes the ring to `capacity` columns, keeping the newest ones. The ring
    /// comes back unrotated (`head` 0), so the caller reallocates the texture and
    /// re-uploads -- this is the live `retention` change, not a per-tick cost.
    pub fn set_capacity(&mut self, capacity: usize) {
        let capacity = capacity.max(1);
        if self.capacity == 0 || capacity == self.capacity {
            return;
        }
        let keep = self.n_frames.min(capacity);
        let first = self.n_frames - keep;
        let mut mags = vec![0.0f32; capacity * self.n_bins];
        for i in 0..keep {
            let at = i * self.n_bins;
            mags[at..at + self.n_bins].copy_from_slice(self.column(first + i));
        }
        self.mags = mags;
        self.capacity = capacity;
        self.head = 0;
        self.n_frames = keep;
        self.total_samples = keep.saturating_sub(1) * self.hop + self.window_size;
    }

    /// The sample range `[start, start + len)` as a normalized horizontal
    /// `[start, start+len]` across the **whole** transform's texel run
    /// ([`tex_width`](Self::tex_width)). A rolling ring measures from its
    /// `head` over the doubled width, which is the whole difference between
    /// the two forms as far as whoever samples it is concerned -- a stored
    /// transform has `head` 0 and a width of its own frame count, so this is
    /// the plain fraction it always was.
    pub fn time_fraction(&self, start: f64, len: f64) -> (f32, f32) {
        let width = self.tex_width() as f64;
        let at = (self.head as f64 + start / self.hop as f64) / width;
        let len = (len / self.hop as f64) / width;
        (at as f32, len as f32)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        bytes::push_u32(&mut out, VERSION);
        bytes::push_u64(&mut out, self.total_samples);
        bytes::push_u64(&mut out, self.n_frames);
        bytes::push_u64(&mut out, self.n_bins);
        bytes::push_u64(&mut out, self.hop);
        bytes::push_u64(&mut out, self.window_size);
        bytes::push_u32(&mut out, self.sample_rate.to_bits());
        bytes::push_f32s(&mut out, &self.mags);
        out
    }

    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        let mut r = bytes::Reader::new(data);
        r.tag(MAGIC)?;
        if r.u32()? != VERSION {
            return None;
        }
        let total_samples = r.usize()?;
        let n_frames = r.usize()?;
        let n_bins = r.usize()?;
        let hop = r.usize()?;
        let window_size = r.usize()?;
        let sample_rate = f32::from_bits(r.u32()?);
        let mags = r.f32_vec(n_frames.checked_mul(n_bins)?)?;
        Some(Self {
            total_samples,
            n_frames,
            n_bins,
            hop,
            window_size,
            sample_rate,
            mags,
            capacity: 0,
            head: 0,
            levels: Vec::new(),
        })
    }

    pub fn write_cache(&self, path: impl AsRef<std::path::Path>) -> std::io::Result<()> {
        std::fs::write(path, self.to_bytes())
    }

    pub fn read_cache(path: impl AsRef<std::path::Path>) -> std::io::Result<Option<Self>> {
        Ok(Self::from_bytes(&std::fs::read(path)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    // The FFT correctness tests (impulse -> flat, cosine -> single bin) live
    // with the transform in `crate::fft`; here we test the STFT built on it.

    #[test]
    fn stft_locates_sine_frequency() {
        // A 1 kHz sine at 48 kHz, window 1024 -> bin = 1000/48000*1024 ~= 21.
        let sr = 48_000.0f32;
        let freq = 1000.0f32;
        let samples: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * PI * freq * i as f32 / sr).sin())
            .collect();
        let stft = Stft::compute(&samples, 1024, 512, sr);
        let nb = stft.n_bins();
        // Average magnitude per bin across frames; the max should be near bin 21.
        let mut acc = vec![0.0f32; nb];
        for row in stft.magnitudes().chunks_exact(nb) {
            for (acc_b, &m) in acc.iter_mut().zip(row) {
                *acc_b += m;
            }
        }
        let peak = (0..nb)
            .max_by(|&a, &b| acc[a].partial_cmp(&acc[b]).unwrap())
            .unwrap();
        let expected = (freq / sr * 1024.0).round() as usize;
        assert!(
            (peak as i32 - expected as i32).abs() <= 1,
            "peak bin {peak}, expected ~{expected}"
        );
    }

    /// A transform is bounded by the memory it takes and not by a texture: a
    /// file a texture's worth of frames could not hold keeps the hop it was
    /// asked for, and only one past [`MAX_STORED`] magnitudes has it raised.
    #[test]
    fn hop_capped_bounds_the_frame_count() {
        // Five minutes at 48 kHz, window 1024: 28 125 frames at a hop of 512,
        // more than three textures wide -- and kept.
        let five_minutes = 300 * 48_000;
        assert!(five_minutes / 512 > 3 * MIN_FRAMES);
        assert_eq!(hop_capped(five_minutes, 1024, 512), 512);
        // Two hours is past what a transform keeps: the hop rises until the
        // frames fit, and no further.
        let two_hours = 7200 * 48_000;
        let hop = hop_capped(two_hours, 1024, 512);
        let frames = 1 + (two_hours - 1024) / hop;
        assert!(hop > 512 && frames <= max_frames(1024));
        assert!(1 + (two_hours - 1024) / (hop - 1) >= max_frames(1024));
        // A short buffer is left alone, and a window of many bins still gets
        // a texture's worth of frames.
        assert_eq!(hop_capped(48_000, 1024, 512), 512);
        assert_eq!(max_frames(1 << 16), MIN_FRAMES);
    }

    /// **A level of the pyramid keeps what happened in the frames it stands
    /// for.** One loud frame in a quiet transform is in every level above it,
    /// at the column that covers it and at full height -- which is what a
    /// zoomed-out picture has to show of a click.
    #[test]
    fn the_pyramid_keeps_a_frame_s_peak_in_every_level() {
        let (frames, bins) = (2000usize, 8usize);
        let mut stft = Stft::rolling(1, bins, 64, 2 * bins, 48_000.0);
        // A stored transform, written by hand: silence, and frame 1234 full.
        stft.capacity = 0;
        stft.n_frames = frames;
        stft.mags = vec![0.0; frames * bins];
        stft.mags[1234 * bins..1235 * bins].fill(1.0);
        stft.build_pyramid();
        assert!(stft.levels() >= 2);
        let mut col = vec![0u8; bins];
        let mut at = frames;
        for level in 1..=stft.levels() {
            at = at.div_ceil(2);
            assert_eq!(stft.level_frames(level), at, "each level halves the frames");
            let k = 1234 >> level;
            stft.level_column(level, k, &mut col);
            assert!(
                col.iter().all(|q| *q == 255),
                "level {level} keeps the click"
            );
            stft.level_column(level, k + 1, &mut col);
            assert!(col.iter().all(|q| *q == 0), "and only where it was");
        }
        assert!(stft.level_frames(stft.levels()) <= 256);
        stft.build_pyramid();
        assert_eq!(stft.level_frames(1), frames.div_ceil(2), "built once");
    }

    #[test]
    fn cache_round_trip() {
        let samples: Vec<f32> = (0..5000).map(|i| (i as f32 * 0.02).sin()).collect();
        let stft = Stft::compute(&samples, 256, 128, 44_100.0);
        let back = Stft::from_bytes(&stft.to_bytes()).expect("parse");
        assert_eq!(stft.n_frames(), back.n_frames());
        assert_eq!(stft.n_bins(), back.n_bins());
        assert_eq!(stft.total_samples(), back.total_samples());
        assert_eq!(stft.nyquist(), back.nyquist());
        assert_eq!(stft.magnitudes(), back.magnitudes());
    }

    /// **A cache is as long as it was sized, and reads back the transform.**
    /// A caller that fills a buffer asks the size first; the bytes are what
    /// the host reads, and what it reads is what it would have analyzed.
    #[test]
    fn a_cache_is_the_size_it_was_asked_and_the_analysis_it_holds() {
        let samples: Vec<f32> = (0..9000).map(|i| (i as f32 * 0.02).sin()).collect();
        let stft = Stft::analyze(&samples, 512, 128, 44_100.0).unwrap();
        let cache = stft.to_bytes();
        assert_eq!(cache.len(), cache_size(samples.len(), 512, 128));
        let back = Stft::from_bytes(&cache).unwrap();
        assert_eq!(back.magnitudes(), stft.magnitudes());
        assert_eq!((back.hop(), back.window_size()), (128, 512));
        // A window the FFT has no size for is no analysis and no bytes.
        assert!(Stft::analyze(&samples, 300, 128, 44_100.0).is_none());
        assert_eq!(cache_size(samples.len(), 300, 128), 0);
        assert_eq!(cache_size(samples.len(), 512, 0), 0);
        // Shorter than a window: one frame, as `compute` makes.
        assert_eq!(cache_size(100, 512, 128), HEADER + 4 * 256);
    }

    /// A ring column for column `i` of a ramp, so a test can tell them apart.
    fn marked(n_bins: usize, i: usize) -> Vec<f32> {
        vec![i as f32 / 255.0; n_bins]
    }

    /// The ring keeps the newest columns in time order once the cursor has
    /// wrapped, and reports the texel each one landed in - which is what the
    /// texture writes against.
    #[test]
    fn the_ring_wraps_and_keeps_the_newest_in_order() {
        let bins = 4;
        let mut stft = Stft::rolling(3, bins, 32, 64, 48_000.0);
        assert_eq!(stft.n_frames(), 0);
        for i in 0..3 {
            assert_eq!(stft.push_column(&marked(bins, i)), i, "fills in order");
        }
        assert_eq!(stft.n_frames(), 3);
        // Two more: the oldest two fall off and their texels are reused.
        assert_eq!(stft.push_column(&marked(bins, 3)), 0);
        assert_eq!(stft.push_column(&marked(bins, 4)), 1);
        assert_eq!(stft.n_frames(), 3, "the span is a cap");
        let seen: Vec<f32> = (0..3).map(|i| stft.column(i)[0] * 255.0).collect();
        assert_eq!(seen, vec![2.0, 3.0, 4.0], "oldest first, newest last");
        assert_eq!(stft.total_samples(), 2 * 32 + 64);
    }

    /// The ring is stored twice so the visible window is one contiguous run of
    /// texels - which is the invariant `time_fraction` maps against.
    #[test]
    fn a_rolling_window_is_contiguous_in_the_texture() {
        let bins = 4;
        let mut stft = Stft::rolling(4, bins, 32, 64, 48_000.0);
        assert_eq!(stft.tex_width(), 8);
        for i in 0..6 {
            stft.push_column(&marked(bins, i));
        }
        // Head is at texel 2 with four columns retained: 2..6, past the wrap
        // and still one run inside the doubled width.
        let (start, len) = stft.time_fraction(0.0, (stft.n_frames() - 1) as f64 * 32.0);
        assert!((start - 2.0 / 8.0).abs() < 1e-6, "{start}");
        assert!((len - 3.0 / 8.0).abs() < 1e-6, "{len}");
        assert!(start + len <= 1.0, "the window never leaves the texture");
    }

    /// A stored transform is the degenerate ring: no doubling, no offset, and
    /// the same fraction the renderer always uploaded.
    #[test]
    fn a_stored_transform_maps_exactly_as_it_did() {
        let samples: Vec<f32> = (0..5000).map(|i| (i as f32 * 0.02).sin()).collect();
        let stft = Stft::compute(&samples, 256, 128, 48_000.0);
        assert!(!stft.is_rolling());
        assert_eq!(stft.tex_width(), stft.n_frames());
        let frames = stft.n_frames() as f64;
        let (start, len) = stft.time_fraction(1000.0, 2000.0);
        assert_eq!(start, ((1000.0 / 128.0) / frames) as f32);
        assert_eq!(len, ((2000.0 / 128.0) / frames) as f32);
        // ...and its columns are read straight off the magnitudes.
        assert_eq!(stft.column(7), &stft.magnitudes()[7 * 128..8 * 128]);
    }

    /// A live `retention` change resizes the ring around the newest columns
    /// rather than restarting the picture.
    #[test]
    fn resizing_the_ring_keeps_the_newest_columns() {
        let bins = 4;
        let mut stft = Stft::rolling(6, bins, 32, 64, 48_000.0);
        for i in 0..6 {
            stft.push_column(&marked(bins, i));
        }
        stft.set_capacity(3);
        assert_eq!(stft.capacity(), 3);
        assert_eq!(stft.n_frames(), 3);
        let seen: Vec<f32> = (0..3).map(|i| stft.column(i)[0] * 255.0).collect();
        assert_eq!(seen, vec![3.0, 4.0, 5.0]);
        // Growing keeps them too, and the ring goes on filling from there.
        stft.set_capacity(5);
        stft.push_column(&marked(bins, 6));
        let seen: Vec<f32> = (0..stft.n_frames())
            .map(|i| stft.column(i)[0] * 255.0)
            .collect();
        assert_eq!(seen, vec![3.0, 4.0, 5.0, 6.0]);
    }

    /// A rolling transform pushed the columns of a signal is the same picture
    /// the stored analysis of it computes - the property the whole live path
    /// rests on, now that the two hold their magnitudes differently.
    #[test]
    fn a_rolling_transform_is_the_stored_one() {
        let (ws, hop, sr) = (256usize, 64usize, 48_000.0f32);
        let samples: Vec<f32> = (0..2048)
            .map(|i| (2.0 * PI * 3000.0 * i as f32 / sr).sin())
            .collect();
        let stored = Stft::compute(&samples, ws, hop, sr);
        let (hann, gain) = analysis_window(ws);
        let mut rolling = Stft::rolling(stored.n_frames(), ws / 2, hop, ws, sr);
        let mut windowed = vec![0.0; ws];
        let mut spectrum = vec![0.0; ws / 2];
        let mut col = vec![0.0; ws / 2];
        for f in 0..stored.n_frames() {
            let at = f * hop;
            column_into(
                &samples[at..at + ws],
                &hann,
                gain,
                &mut windowed,
                &mut spectrum,
                &mut col,
            );
            rolling.push_column(&col);
        }
        assert_eq!(rolling.n_frames(), stored.n_frames());
        assert_eq!(rolling.total_samples(), stored.total_samples());
        for f in 0..stored.n_frames() {
            assert_eq!(rolling.column(f), stored.column(f), "column {f}");
        }
    }
}
