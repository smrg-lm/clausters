//! **The spectral summary of a file**: what its spectrogram is drawn from
//! when a view shows more of it than its samples are worth reading for, kept
//! beside its peaks.
//!
//! A peaks file keeps a signal's extremes at levels that halve; this keeps
//! its spectrum the same way. The analysis is one transform ([`WINDOW`]
//! frames, Hann, every [`HOP`]: the time-frequency view's defaults, the
//! columns [`crate::stft::column_into`] makes), and what is kept is not those
//! columns: a column a frame of the transform would take as many bytes as the
//! samples it describes. The finest level keeps one column for every
//! [`BASE`] of them, each bin the **largest** it was over them, and every
//! level above halves that, as the time pyramid of a stored transform does
//! ([`crate::stft::Stft::build_pyramid`]) and for the reason a peak pyramid
//! keeps the minimum and the maximum: a pixel that stands for seconds shows
//! what happened in them, a click included, rather than whichever window a
//! sampler landed on. The bins are kept whole (a column is the frequency axis
//! at every scale) and a byte apiece, as the texture they are drawn into
//! stores them.
//!
//! So a summary costs an eighth of a byte a frame and a channel, levels
//! included: a take of an hour in stereo keeps 43 MB of it, beside 32 MB of
//! peaks and 1.4 GB of float samples. Finer than its first level a view is
//! drawn from the samples, over what it shows.
//!
//! **Where it is made is where the peaks are.** [`Accumulator`] folds the
//! columns as the samples go by, with nothing allocated per frame (the
//! transform is on the stack), so the UGen that writes a render's peaks
//! writes this beside them in the same pass ([`beside_peaks`]). The file is
//! read a block of columns at a time ([`Head`]), by whoever draws.

use crate::bytes;
use crate::stft::{analysis_window, column_into, quantize};

const MAGIC: &[u8; 4] = b"CLSP";
const VERSION: u32 = 1;

/// The analysis window, in frames.
pub const WINDOW: usize = 1024;

/// Frames between two windows: half a window.
pub const HOP: usize = WINDOW / 2;

/// Windows the finest level keeps one column for.
pub const BASE: usize = 16;

/// Bins of a column: half the window.
pub const BINS: usize = WINDOW / 2;

/// **Frames a column of the finest level stands for**: [`BASE`] hops, 8192.
pub const COLUMN_FRAMES: usize = HOP * BASE;

/// **The summary beside the peaks file at `peaks`**: its extension replaced
/// by `.spectra`, or `.spectra` added to a name with none. The writer of the
/// peaks and their reader both name it so.
pub fn beside_peaks(peaks: &str) -> String {
    let name_at = peaks.rfind('/').map_or(0, |s| s + 1);
    match peaks.rfind('.') {
        Some(dot) if dot > name_at => format!("{}.spectra", &peaks[..dot]),
        _ => format!("{peaks}.spectra"),
    }
}

/// How many windows a signal of `frames` frames is analyzed with: one every
/// [`HOP`] that starts inside it, the last ones reaching past its end into
/// silence, and one for a signal shorter than a hop.
pub fn windows(frames: usize) -> usize {
    frames.div_ceil(HOP).max(1)
}

/// The columns of each level of the summary of `frames` frames, finest first,
/// down to one.
pub fn level_lens(frames: usize) -> Vec<usize> {
    let mut lens = vec![windows(frames).div_ceil(BASE)];
    while *lens.last().expect("one level") > 1 {
        lens.push(lens.last().expect("one level").div_ceil(2));
    }
    lens
}

/// **Folds the summary's finest columns as the samples go by.** Built for a
/// number of channels at most, with every buffer it needs, so a channel count
/// set later and every frame pushed allocate nothing: it runs where a render's
/// samples are made, on the audio thread.
pub struct Accumulator {
    max_channels: usize,
    channels: usize,
    /// The last [`WINDOW`] frames of each channel, a ring.
    ring: Box<[f32]>,
    at: usize,
    /// Frames pushed, and frames through the ring, the silence `finish`
    /// reads past the end included.
    frames: u64,
    through: usize,
    /// Windows analyzed, and windows folded into the column being made.
    analyzed: usize,
    folded: usize,
    /// The column being made, each channel's bins in turn: the largest each
    /// bin was over the windows folded.
    column: Box<[u8]>,
    hann: Box<[f32]>,
    gain: f32,
    frame: Box<[f32]>,
    windowed: Box<[f32]>,
    spectrum: Box<[f32]>,
    magnitudes: Box<[f32]>,
}

impl Accumulator {
    /// An accumulator for up to `max_channels` channels.
    pub fn new(max_channels: usize) -> Self {
        let max_channels = max_channels.max(1);
        let (hann, gain) = analysis_window(WINDOW);
        Self {
            max_channels,
            channels: 0,
            ring: vec![0.0; max_channels * WINDOW].into_boxed_slice(),
            at: 0,
            frames: 0,
            through: 0,
            analyzed: 0,
            folded: 0,
            column: vec![0; max_channels * BINS].into_boxed_slice(),
            hann: hann.into_boxed_slice(),
            gain,
            frame: vec![0.0; WINDOW].into_boxed_slice(),
            windowed: vec![0.0; WINDOW].into_boxed_slice(),
            spectrum: vec![0.0; BINS].into_boxed_slice(),
            magnitudes: vec![0.0; BINS].into_boxed_slice(),
        }
    }

    /// The channels it folds, set before the first frame: at most the
    /// number it was built for.
    pub fn set_channels(&mut self, channels: usize) {
        if self.frames == 0 {
            self.channels = channels.clamp(1, self.max_channels);
        }
    }

    /// The channels it folds.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Frames pushed so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// **One frame**, its channel `c` read from `sample(c)`; a finished
    /// column is handed to `done`, each channel's [`BINS`] in turn.
    pub fn push(&mut self, sample: impl Fn(usize) -> f32, done: &mut impl FnMut(&[u8])) {
        if self.channels == 0 {
            self.channels = 1;
        }
        self.frames += 1;
        self.step(sample, done);
    }

    /// **The columns of the frames pushed that are not handed over yet**:
    /// the windows that start inside the signal and run past its end, read
    /// over silence, and the column they fold into. A signal of no frames
    /// has none.
    pub fn finish(&mut self, done: &mut impl FnMut(&[u8])) {
        let total = windows(self.frames as usize);
        if self.frames == 0 {
            return;
        }
        while self.analyzed < total {
            self.step(|_| 0.0, done);
        }
        if self.folded > 0 {
            self.hand(done);
        }
    }

    /// Writes a frame into the ring, and analyzes the window it ends: window
    /// `j` starts at frame `j * HOP` and ends a window later.
    fn step(&mut self, sample: impl Fn(usize) -> f32, done: &mut impl FnMut(&[u8])) {
        for c in 0..self.channels {
            self.ring[c * WINDOW + self.at] = sample(c);
        }
        self.at = (self.at + 1) % WINDOW;
        self.through += 1;
        if self.through >= WINDOW && (self.through - WINDOW).is_multiple_of(HOP) {
            self.window_at_ring(done);
        }
    }

    /// Analyzes the window the ring holds and folds it into the column.
    fn window_at_ring(&mut self, done: &mut impl FnMut(&[u8])) {
        for c in 0..self.channels {
            let ring = &self.ring[c * WINDOW..(c + 1) * WINDOW];
            for (i, x) in self.frame.iter_mut().enumerate() {
                *x = ring[(self.at + i) % WINDOW];
            }
            column_into(
                &self.frame,
                &self.hann,
                self.gain,
                &mut self.windowed,
                &mut self.spectrum,
                &mut self.magnitudes,
            );
            let out = &mut self.column[c * BINS..(c + 1) * BINS];
            for (o, m) in out.iter_mut().zip(self.magnitudes.iter()) {
                *o = (*o).max(quantize(*m));
            }
        }
        self.analyzed += 1;
        self.folded += 1;
        if self.folded == BASE {
            self.hand(done);
        }
    }

    fn hand(&mut self, done: &mut impl FnMut(&[u8])) {
        let n = self.channels * BINS;
        done(&self.column[..n]);
        self.column[..n].fill(0);
        self.folded = 0;
    }
}

/// **A summary in memory**: its shape, and each channel's levels.
#[derive(Clone, Debug, PartialEq)]
pub struct Spectra {
    pub head: Head,
    /// Per channel, per level, the columns one after another.
    levels: Vec<Vec<Vec<u8>>>,
}

impl Spectra {
    /// **The summary of `frames` frames of `channels` channels** from its
    /// finest columns as [`Accumulator`] hands them over, one after another,
    /// each channel's bins in turn. `None` when they are not the columns
    /// such a signal has.
    pub fn from_columns(frames: usize, channels: usize, columns: &[u8]) -> Option<Self> {
        let channels = channels.max(1);
        let lens = level_lens(frames);
        if columns.len() != lens[0] * channels * BINS {
            return None;
        }
        let levels = (0..channels)
            .map(|c| {
                let mut levels = vec![
                    columns
                        .chunks_exact(channels * BINS)
                        .flat_map(|col| &col[c * BINS..(c + 1) * BINS])
                        .copied()
                        .collect::<Vec<u8>>(),
                ];
                for &n in &lens[1..] {
                    let below = levels.last().expect("the finest level");
                    let mut level = vec![0u8; n * BINS];
                    for (k, out) in level.as_chunks_mut::<BINS>().0.iter_mut().enumerate() {
                        for j in [2 * k, 2 * k + 1] {
                            if let Some(col) = below.get(j * BINS..(j + 1) * BINS) {
                                for (o, m) in out.iter_mut().zip(col) {
                                    *o = (*o).max(*m);
                                }
                            }
                        }
                    }
                    levels.push(level);
                }
                levels
            })
            .collect();
        Some(Self {
            head: Head::of(frames, channels),
            levels,
        })
    }

    /// Column `k` of level `level` of channel `channel`.
    pub fn column(&self, channel: usize, level: usize, k: usize) -> &[u8] {
        &self.levels[channel][level][k * BINS..(k + 1) * BINS]
    }

    /// The file's bytes: the head, then each channel's levels, finest first.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.head.size() as usize);
        self.head.write(&mut out);
        for channel in &self.levels {
            for level in channel {
                out.extend_from_slice(level);
            }
        }
        out
    }

    /// Reads a whole summary back from its bytes.
    pub fn from_bytes(data: &[u8]) -> Result<Self, String> {
        let head = Head::parse(data)?;
        if data.len() as u64 != head.size() {
            return Err("a spectral summary of another size than its head says".into());
        }
        let levels = (0..head.channels)
            .map(|c| {
                (0..head.lens.len())
                    .map(|l| {
                        let at = head.offset(c, l, 0) as usize;
                        data[at..at + head.lens[l] * BINS].to_vec()
                    })
                    .collect()
            })
            .collect();
        Ok(Self { head, levels })
    }

    /// Writes the file at `path`.
    pub fn write(&self, path: impl AsRef<std::path::Path>) -> std::io::Result<()> {
        crate::files::write(path, self.to_bytes())
    }
}

/// **The head of a summary file**: what a reader reads first to find any
/// column in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Head {
    /// The frames summarized.
    pub frames: usize,
    pub channels: usize,
    /// Columns at each level, finest first.
    pub lens: Vec<usize>,
}

impl Head {
    /// The bytes the head is.
    pub const BYTES: usize = 4 + 4 + 6 * 8;

    /// The head of the summary of `frames` frames of `channels` channels.
    pub fn of(frames: usize, channels: usize) -> Self {
        Self {
            frames,
            channels: channels.max(1),
            lens: level_lens(frames),
        }
    }

    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(MAGIC);
        bytes::push_u32(out, VERSION);
        for v in [
            WINDOW,
            HOP,
            BASE,
            self.frames,
            self.channels,
            self.lens.len(),
        ] {
            bytes::push_u64(out, v);
        }
    }

    /// **Reads a head**, refusing one of another analysis than this build
    /// draws with: a summary is a picture of one transform.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < Self::BYTES {
            return Err("not a spectral summary".into());
        }
        let mut r = bytes::Reader::new(bytes);
        r.tag(MAGIC).ok_or("not a spectral summary")?;
        let version = r.u32().ok_or("not a spectral summary")?;
        if version != VERSION {
            return Err(format!(
                "a spectral summary of version {version}, not {VERSION}"
            ));
        }
        let mut field = || r.usize().ok_or("a spectral summary's head is cut short");
        let (window, hop, base) = (field()?, field()?, field()?);
        let (frames, channels, levels) = (field()?, field()?, field()?);
        if (window, hop, base) != (WINDOW, HOP, BASE) {
            return Err(format!(
                "a spectral summary of a window of {window} every {hop} in columns of {base}, \
                 not {WINDOW} every {HOP} in columns of {BASE}"
            ));
        }
        let head = Self::of(frames, channels);
        if channels == 0 || head.lens.len() != levels {
            return Err("a spectral summary whose head does not add up".into());
        }
        Ok(head)
    }

    /// The bytes the file has.
    pub fn size(&self) -> u64 {
        (Self::BYTES + self.channels * self.channel_bytes()) as u64
    }

    /// Where column `k` of level `level` of channel `channel` begins.
    pub fn offset(&self, channel: usize, level: usize, k: usize) -> u64 {
        let before: usize = self.lens[..level].iter().sum();
        (Self::BYTES + channel * self.channel_bytes() + (before + k) * BINS) as u64
    }

    fn channel_bytes(&self) -> usize {
        self.lens.iter().sum::<usize>() * BINS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stft::Stft;
    use std::f32::consts::PI;

    fn signal(frames: usize, channels: usize) -> Vec<f32> {
        (0..frames * channels)
            .map(|i| {
                let (f, c) = (i / channels, i % channels);
                // a tone per channel, and a click in the second
                let tone =
                    0.5 * (2.0 * PI * (500.0 + 2500.0 * c as f32) * f as f32 / 48_000.0).sin();
                if c == 1 && f == 20_000 { 1.0 } else { tone }
            })
            .collect()
    }

    fn summarize(samples: &[f32], channels: usize) -> Spectra {
        let mut acc = Accumulator::new(8);
        acc.set_channels(channels);
        let mut columns = Vec::new();
        let mut done = |col: &[u8]| columns.extend_from_slice(col);
        for frame in samples.chunks_exact(channels) {
            acc.push(|c| frame[c], &mut done);
        }
        acc.finish(&mut done);
        Spectra::from_columns(samples.len() / channels, channels, &columns).expect("columns")
    }

    /// **A column of the finest level is the largest of the windows it
    /// stands for**, each window the transform's own column at its hop, the
    /// last ones read past the end over silence.
    #[test]
    fn a_column_keeps_the_largest_of_the_transforms_windows() {
        for frames in [100, HOP, WINDOW, 3 * COLUMN_FRAMES + 777] {
            let samples = signal(frames, 2);
            let spectra = summarize(&samples, 2);
            assert_eq!(spectra.head.lens[0], windows(frames).div_ceil(BASE));
            for c in 0..2 {
                let mono: Vec<f32> = samples.iter().skip(c).step_by(2).copied().collect();
                // the transform's windows, padded as the summary pads them
                let mut padded = mono.clone();
                padded.resize(windows(frames) * HOP + WINDOW, 0.0);
                let stft = Stft::compute(&padded, WINDOW, HOP, 48_000.0);
                for k in 0..spectra.head.lens[0] {
                    let mut want = [0u8; BINS];
                    for j in k * BASE..((k + 1) * BASE).min(windows(frames)) {
                        for (w, m) in want.iter_mut().zip(stft.column(j)) {
                            *w = (*w).max(quantize(*m));
                        }
                    }
                    assert_eq!(
                        spectra.column(c, 0, k),
                        &want[..],
                        "{frames}: ch {c} col {k}"
                    );
                }
            }
        }
    }

    /// **Each level halves the one below, keeping each bin's largest**, so a
    /// click is still in the one column of the coarsest.
    #[test]
    fn a_level_keeps_the_largest_of_the_two_below_and_a_click_reaches_the_top() {
        let frames = 9 * COLUMN_FRAMES;
        let spectra = summarize(&signal(frames, 2), 2);
        let lens = &spectra.head.lens;
        assert_eq!(lens, &vec![9, 5, 3, 2, 1]);
        for l in 1..lens.len() {
            for k in 0..lens[l] {
                for b in 0..BINS {
                    let below = |j: usize| {
                        if j < lens[l - 1] {
                            spectra.column(1, l - 1, j)[b]
                        } else {
                            0
                        }
                    };
                    assert_eq!(
                        spectra.column(1, l, k)[b],
                        below(2 * k).max(below(2 * k + 1))
                    );
                }
            }
        }
        // The click is broadband: high bins the tone leaves dark are lit in
        // the column over it, and so all the way up.
        let high = |col: &[u8]| col[BINS - 40];
        assert!(high(spectra.column(1, 0, 2)) > high(spectra.column(1, 0, 5)) + 40);
        assert!(high(spectra.column(1, lens.len() - 1, 0)) >= high(spectra.column(1, 0, 2)));
        assert!(high(spectra.column(0, lens.len() - 1, 0)) < high(spectra.column(1, 0, 2)));
    }

    /// **The file is what the reader finds**: its bytes round-trip, the head
    /// says where every column is, and one of another analysis is refused.
    #[test]
    fn the_file_round_trips_and_its_head_finds_every_column() {
        let frames = 5 * COLUMN_FRAMES + 3;
        let spectra = summarize(&signal(frames, 2), 2);
        let bytes = spectra.to_bytes();
        assert_eq!(bytes.len() as u64, spectra.head.size());
        assert_eq!(Spectra::from_bytes(&bytes).unwrap(), spectra);
        let head = Head::parse(&bytes[..Head::BYTES]).unwrap();
        for c in 0..2 {
            for (l, &n) in head.lens.iter().enumerate() {
                for k in 0..n {
                    let at = head.offset(c, l, k) as usize;
                    assert_eq!(&bytes[at..at + BINS], spectra.column(c, l, k));
                }
            }
        }
        let mut other = bytes.clone();
        other[9] = 0; // the window, 1024 little-endian
        assert!(Head::parse(&other).is_err());
        assert!(Head::parse(&bytes[..10]).is_err());
    }

    #[test]
    fn the_summary_is_named_beside_the_peaks() {
        assert_eq!(beside_peaks("/w/take-1a.peaks"), "/w/take-1a.spectra");
        assert_eq!(beside_peaks("/w.d/r1"), "/w.d/r1.spectra");
        assert_eq!(beside_peaks("r1.peaks"), "r1.spectra");
    }

    /// Pushing frames allocates nothing once built: it runs on the audio
    /// thread. (The engine's own test over the UGen guards it there.)
    #[test]
    fn a_channel_count_is_bounded_by_what_it_was_built_for() {
        let mut acc = Accumulator::new(2);
        acc.set_channels(9);
        assert_eq!(acc.channels(), 2);
    }
}
