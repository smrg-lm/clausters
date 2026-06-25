//! Min/max peak pyramid: resolution-matched peak analysis for navigable views.
//!
//! The waveform is never drawn sample-by-sample and never processes millions of
//! samples per frame. Instead a min/max *peak* pyramid is computed once: level 0
//! summarizes every `base_bucket` samples into a `(min, max)` pair, and each
//! higher level halves the resolution. At draw time the level whose bucket size
//! matches the current `samples_per_px` is selected, so each rendered pixel
//! column reads only ~one bucket - work proportional to the window width, not to
//! the buffer length.
//!
//! Computing peaks for a long file is the expensive part, so the result is a
//! cache: it lives in memory and can be serialized to a temp/cache file (the way
//! audio editors keep an overview/peak file beside the audio) and read back -
//! `to_bytes`/`from_bytes` and `write_cache`/`read_cache`. The layout is a flat
//! sequence of `f32` arrays, so a production build can memory-map it instead of
//! reading it into RAM. The format is machine-local (native float byte order).

use std::fs;
use std::io;
use std::path::Path;

use crate::bytes;

const MAGIC: &[u8; 4] = b"CLPK";
const VERSION: u32 = 1;

/// Min/max over a slice, or `None` if empty.
pub fn min_max(samples: &[f32]) -> Option<(f32, f32)> {
    let (&first, rest) = samples.split_first()?;
    let mut lo = first;
    let mut hi = first;
    for &s in rest {
        lo = lo.min(s);
        hi = hi.max(s);
    }
    Some((lo, hi))
}

/// One resolution level: `min[i]`/`max[i]` summarize `bucket` source samples.
struct Level {
    bucket: usize,
    min: Vec<f32>,
    max: Vec<f32>,
}

/// A min/max pyramid over a mono buffer. Total storage is ~2x the level-0 size,
/// i.e. a small constant fraction of the source (e.g. ~0.8% at `base_bucket`
/// 256), so it is cheap to keep resident or cache to disk.
pub struct Pyramid {
    base_bucket: usize,
    total_samples: usize,
    levels: Vec<Level>,
}

impl Pyramid {
    /// Build the pyramid from mono `samples`. `base_bucket` is the level-0
    /// bucket size (e.g. 256). Smaller buckets give finer detail before the
    /// renderer must fall back to raw samples, at the cost of more storage.
    pub fn build(samples: &[f32], base_bucket: usize) -> Self {
        assert!(base_bucket >= 1);
        let total_samples = samples.len();

        let n0 = total_samples.div_ceil(base_bucket);
        let mut min0 = vec![0.0f32; n0];
        let mut max0 = vec![0.0f32; n0];
        for (b, chunk) in samples.chunks(base_bucket).enumerate() {
            let (lo, hi) = min_max(chunk).unwrap_or((0.0, 0.0));
            min0[b] = lo;
            max0[b] = hi;
        }
        let mut levels = vec![Level {
            bucket: base_bucket,
            min: min0,
            max: max0,
        }];

        while levels.last().unwrap().min.len() > 1 {
            let prev = levels.last().unwrap();
            let n = prev.min.len().div_ceil(2);
            let mut min = vec![0.0f32; n];
            let mut max = vec![0.0f32; n];
            for i in 0..n {
                let a = 2 * i;
                let b = (2 * i + 1).min(prev.min.len() - 1);
                min[i] = prev.min[a].min(prev.min[b]);
                max[i] = prev.max[a].max(prev.max[b]);
            }
            levels.push(Level {
                bucket: prev.bucket * 2,
                min,
                max,
            });
        }

        Self {
            base_bucket,
            total_samples,
            levels,
        }
    }

    pub fn base_bucket(&self) -> usize {
        self.base_bucket
    }

    pub fn total_samples(&self) -> usize {
        self.total_samples
    }

    pub fn num_levels(&self) -> usize {
        self.levels.len()
    }

    /// Pick the finest level whose bucket does not exceed `samples_per_px`, so
    /// each pixel column aggregates ~one bucket (no gaps, minimal work). When
    /// zoomed in finer than level 0, level 0 is returned and the caller should
    /// read raw samples instead (see `waveform::WaveformData::column`).
    pub fn level_for(&self, samples_per_px: f64) -> usize {
        let mut chosen = 0;
        for (i, lvl) in self.levels.iter().enumerate() {
            if (lvl.bucket as f64) <= samples_per_px {
                chosen = i;
            } else {
                break;
            }
        }
        chosen
    }

    /// Min/max of the buckets overlapping `[s0, s1)` at `level`, or `None` if
    /// the level is empty.
    pub fn column(&self, level: usize, s0: f64, s1: f64) -> Option<(f32, f32)> {
        let lvl = self.levels.get(level)?;
        if lvl.min.is_empty() {
            return None;
        }
        let last = lvl.min.len() - 1;
        let b0 = ((s0 / lvl.bucket as f64).floor().max(0.0) as usize).min(last);
        let b1 = ((s1 / lvl.bucket as f64).ceil() as usize)
            .saturating_sub(1)
            .clamp(b0, last);
        let lo = lvl.min[b0..=b1]
            .iter()
            .copied()
            .fold(f32::INFINITY, f32::min);
        let hi = lvl.max[b0..=b1]
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        Some((lo, hi))
    }

    /// Serialize to a flat byte buffer (the on-disk/cache layout).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        bytes::push_u32(&mut out, VERSION);
        bytes::push_u64(&mut out, self.base_bucket);
        bytes::push_u64(&mut out, self.total_samples);
        bytes::push_u64(&mut out, self.levels.len());
        for lvl in &self.levels {
            bytes::push_u64(&mut out, lvl.bucket);
            bytes::push_u64(&mut out, lvl.min.len());
            bytes::push_f32s(&mut out, &lvl.min);
            bytes::push_f32s(&mut out, &lvl.max);
        }
        out
    }

    /// Parse a buffer produced by `to_bytes`, or `None` if malformed.
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        let mut r = bytes::Reader::new(data);
        r.tag(MAGIC)?;
        if r.u32()? != VERSION {
            return None;
        }
        let base_bucket = r.usize()?;
        let total_samples = r.usize()?;
        let n_levels = r.usize()?;
        let mut levels = Vec::with_capacity(n_levels);
        for _ in 0..n_levels {
            let bucket = r.usize()?;
            let len = r.usize()?;
            let min = r.f32_vec(len)?;
            let max = r.f32_vec(len)?;
            levels.push(Level { bucket, min, max });
        }
        Some(Self {
            base_bucket,
            total_samples,
            levels,
        })
    }

    /// Write the cache to `path` (e.g. a temp file beside the audio).
    pub fn write_cache(&self, path: impl AsRef<Path>) -> io::Result<()> {
        fs::write(path, self.to_bytes())
    }

    /// Read a cache from `path`. Returns `Ok(None)` if the file is malformed
    /// (e.g. an older format), so the caller can recompute.
    pub fn read_cache(path: impl AsRef<Path>) -> io::Result<Option<Self>> {
        Ok(Self::from_bytes(&fs::read(path)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(n: usize) -> Vec<f32> {
        (0..n).map(|i| (i as f32 * 0.01).sin()).collect()
    }

    #[test]
    fn level0_matches_bruteforce() {
        let s = ramp(1000);
        let p = Pyramid::build(&s, 4);
        for (b, chunk) in s.chunks(4).enumerate() {
            let (lo, hi) = min_max(chunk).unwrap();
            let (clo, chi) = p.column(0, (b * 4) as f64, (b * 4 + 4) as f64).unwrap();
            assert_eq!((lo, hi), (clo, chi));
        }
    }

    #[test]
    fn top_level_is_global_min_max() {
        let s = ramp(1000);
        let p = Pyramid::build(&s, 4);
        let top = p.num_levels() - 1;
        let (lo, hi) = p.column(top, 0.0, s.len() as f64).unwrap();
        let (glo, ghi) = min_max(&s).unwrap();
        assert!((lo - glo).abs() < 1e-6 && (hi - ghi).abs() < 1e-6);
    }

    #[test]
    fn level_for_tracks_resolution() {
        let s = ramp(100_000);
        let p = Pyramid::build(&s, 256);
        // Zoomed in (few samples/px) -> finest level.
        assert_eq!(p.level_for(10.0), 0);
        // Zoomed out -> a coarser level whose bucket fits in samples_per_px.
        let coarse = p.level_for(5000.0);
        assert!(coarse > 0);
    }

    #[test]
    fn cache_round_trip() {
        let s = ramp(5000);
        let p = Pyramid::build(&s, 64);
        let bytes = p.to_bytes();
        let q = Pyramid::from_bytes(&bytes).expect("parse");
        assert_eq!(p.base_bucket(), q.base_bucket());
        assert_eq!(p.total_samples(), q.total_samples());
        assert_eq!(p.num_levels(), q.num_levels());
        let top = p.num_levels() - 1;
        assert_eq!(
            p.column(top, 0.0, s.len() as f64),
            q.column(top, 0.0, s.len() as f64)
        );
    }

    #[test]
    fn cache_file_round_trip() {
        let s = ramp(3000);
        let p = Pyramid::build(&s, 32);
        let path = std::env::temp_dir().join(format!("clausters_peaks_{}.bin", std::process::id()));
        p.write_cache(&path).expect("write");
        let q = Pyramid::read_cache(&path).expect("read").expect("parse");
        let _ = std::fs::remove_file(&path);
        let top = p.num_levels() - 1;
        assert_eq!(
            p.column(top, 0.0, s.len() as f64),
            q.column(top, 0.0, s.len() as f64)
        );
    }

    #[test]
    fn from_bytes_rejects_garbage() {
        assert!(Pyramid::from_bytes(b"not a pyramid").is_none());
        assert!(Pyramid::from_bytes(&[]).is_none());
    }
}
