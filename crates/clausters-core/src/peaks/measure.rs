//! **The peaks of a signal, measured as its samples go by**: the base level
//! of a summary -- the minimum, the maximum and the mean square of every
//! [`SUMMARY_BUCKET`] frames -- and the finest columns of its spectral
//! summary ([`crate::spectra`]), folded block by block and written beside
//! the samples as the CLPK file a view is drawn from and the spectral file
//! beside it.
//!
//! It is what an application measures a take with: a file opened without
//! peaks read in blocks ([`file`]), and the samples a render writes folded
//! in the render's own thread. The engine measures nothing; this is the one
//! fold, the same natively and in a page.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::peaks::MultiPyramid;
use crate::spectra::{Accumulator, Spectra};

/// The frames one bucket of the base level summarizes.
pub const SUMMARY_BUCKET: usize = 256;

/// Frames read at a time when a file is measured.
const BLOCK: usize = 1 << 14;

/// The empty bucket a fold starts from.
const EMPTY: (f32, f32, f64) = (f32::INFINITY, f32::NEG_INFINITY, 0.0);

/// **One signal's peaks, measured so far.**
pub struct Measure {
    channels: usize,
    /// Per channel: the minimum, the maximum and the sum of squares of the
    /// bucket being filled.
    acc: Vec<(f32, f32, f64)>,
    /// Frames in the bucket being filled.
    filled: usize,
    frames: u64,
    /// The base level so far: three numbers per channel per bucket.
    stats: Vec<f32>,
    spectra: Accumulator,
    /// The spectral columns finished so far.
    columns: Vec<u8>,
}

impl Measure {
    /// A measure of a signal of `channels` channels, nothing pushed yet.
    pub fn new(channels: usize) -> Self {
        let channels = channels.max(1);
        let mut spectra = Accumulator::new(channels);
        spectra.set_channels(channels);
        Self {
            channels,
            acc: vec![EMPTY; channels],
            filled: 0,
            frames: 0,
            stats: Vec::new(),
            spectra,
            columns: Vec::new(),
        }
    }

    /// The channels it measures.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Frames pushed so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// **Folds `interleaved` frames in**; a trailing partial frame is not one.
    pub fn push(&mut self, interleaved: &[f32]) {
        let channels = self.channels;
        for frame in interleaved.chunks_exact(channels) {
            let Self {
                spectra, columns, ..
            } = self;
            spectra.push(|c| frame[c], &mut |column| {
                columns.extend_from_slice(column)
            });
            for (cell, &x) in self.acc.iter_mut().zip(frame) {
                cell.0 = cell.0.min(x);
                cell.1 = cell.1.max(x);
                cell.2 += f64::from(x) * f64::from(x);
            }
            self.filled += 1;
            if self.filled == SUMMARY_BUCKET {
                self.close_bucket();
            }
            self.frames += 1;
        }
    }

    /// Keeps the bucket being filled, as `filled` frames' worth.
    fn close_bucket(&mut self) {
        let filled = self.filled.max(1) as f64;
        for &(lo, hi, sq) in &self.acc {
            self.stats.extend([lo, hi, (sq / filled) as f32]);
        }
        self.acc.fill(EMPTY);
        self.filled = 0;
    }

    /// **The summary of everything pushed**: the pyramid and the spectral
    /// summary, or `None` for a signal of no frames.
    pub fn finish(mut self) -> Option<(MultiPyramid, Option<Spectra>)> {
        if self.frames == 0 {
            return None;
        }
        if self.filled > 0 {
            self.close_bucket();
        }
        let Self {
            mut spectra,
            mut columns,
            ..
        } = self;
        spectra.finish(&mut |column| columns.extend_from_slice(column));
        let frames = self.frames as usize;
        let mut pyramid = MultiPyramid::empty(frames, self.channels, SUMMARY_BUCKET);
        if !pyramid.write_buckets(0, SUMMARY_BUCKET, &self.stats) {
            return None;
        }
        let spectral = Spectra::from_columns(frames, self.channels, &columns);
        Some((pyramid, spectral))
    }

    /// **Writes the summary to `peaks`**, and the spectral summary beside it
    /// ([`crate::spectra::beside_peaks`]). A signal of no frames writes
    /// nothing.
    pub fn write(self, peaks: &str) -> Result<(), String> {
        let Some((pyramid, spectral)) = self.finish() else {
            return Ok(());
        };
        pyramid
            .write_cache(peaks)
            .map_err(|e| format!("{peaks}: {e}"))?;
        if let Some(spectral) = spectral {
            let beside = crate::spectra::beside_peaks(peaks);
            spectral
                .write(&beside)
                .map_err(|e| format!("{beside}: {e}"))?;
        }
        Ok(())
    }
}

/// **Measures the PCM file at `path` and writes its peaks to `peaks`**,
/// reading it in blocks while `live` holds and telling `progress` the frames
/// done and the frames it comes to. Answers how many frames it measured.
pub fn file(
    path: &str,
    peaks: &str,
    live: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<u64, String> {
    let layout = crate::pcm::Layout::open(path)?;
    let mut reader = crate::pcm::Reader::open(&layout)?;
    let mut measure = Measure::new(layout.channels);
    let mut block = Vec::new();
    let mut at = 0;
    while at < layout.frames {
        if !live.load(Ordering::Acquire) {
            return Err("called off".into());
        }
        let read = reader
            .read(at, BLOCK, &mut block)
            .map_err(|e| format!("{path}: {e}"))?;
        if read == 0 {
            break;
        }
        measure.push(&block);
        at += read as u64;
        progress(at, layout.frames);
    }
    measure.write(peaks)?;
    Ok(at)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A measure folds what a pyramid built at once holds**: the same
    /// base level whatever blocks the samples came in, a short last bucket
    /// included.
    #[test]
    fn a_measure_is_the_pyramid_of_its_samples_whatever_the_blocks() {
        let frames = 3 * SUMMARY_BUCKET + 17;
        let samples: Vec<f32> = (0..frames * 2)
            .map(|i| ((i as f32) * 0.013).sin() * if i % 2 == 0 { 1.0 } else { 0.5 })
            .collect();
        let whole = MultiPyramid::build_interleaved(&samples, 2, SUMMARY_BUCKET);
        for block in [1, 7, 256, 1000, frames] {
            let mut measure = Measure::new(2);
            for chunk in samples.chunks(block * 2) {
                measure.push(chunk);
            }
            assert_eq!(measure.frames(), frames as u64);
            let (pyramid, spectral) = measure.finish().expect("a summary");
            assert_eq!(pyramid.to_bytes(), whole.to_bytes(), "blocks of {block}");
            assert!(spectral.is_some(), "and its spectral summary");
        }
        assert!(Measure::new(1).finish().is_none(), "no frames, no summary");
    }

    /// **A file measured in blocks is its peaks**, written where it is told
    /// and the spectral summary beside them.
    #[test]
    fn a_file_is_measured_in_blocks_and_written_beside() {
        let dir = std::env::temp_dir().join(format!("clausters-measure-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tone.wav").to_string_lossy().into_owned();
        let peaks = dir.join("tone.peaks").to_string_lossy().into_owned();
        let frames = 40_000;
        let samples: Vec<f32> = (0..frames).map(|i| (i as f32 * 0.05).sin()).collect();
        let mut w = crate::pcm::Writer::create(&path, 1, 48_000.0, "float").unwrap();
        w.write(&samples).unwrap();
        w.finalize().unwrap();
        let mut told = 0;
        let measured = file(&path, &peaks, &AtomicBool::new(true), |done, _| told = done).unwrap();
        assert_eq!((measured, told), (frames as u64, frames as u64));
        let read = MultiPyramid::read_cache(&peaks)
            .unwrap()
            .expect("the peaks");
        let whole = MultiPyramid::build_interleaved(&samples, 1, SUMMARY_BUCKET);
        assert_eq!(read.to_bytes(), whole.to_bytes());
        assert!(std::path::Path::new(&crate::spectra::beside_peaks(&peaks)).exists());
        assert!(
            file(&path, &peaks, &AtomicBool::new(false), |_, _| {}).is_err(),
            "called off"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
