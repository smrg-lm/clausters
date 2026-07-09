//! Min/max peak pyramid: resolution-matched peak analysis for navigable views.
//!
//! A waveform view is never drawn sample-by-sample and never processes millions
//! of samples per frame. Instead a min/max *peak* pyramid is computed once:
//! level 0 summarizes every `base_bucket` samples into a `(min, max)` pair, and
//! each higher level halves the resolution. At draw time the level whose bucket
//! size matches the current `samples_per_px` is selected, so each rendered pixel
//! column reads only ~one bucket — work proportional to the window width, not to
//! the buffer length.
//!
//! This is **general Clausters client functionality**, not real-time audio
//! processing: any client with a waveform view wants it, so it lives once in the
//! shared core (and is reachable from non-Rust clients through the FFI) rather
//! than re-implemented per client. The server itself never needs it.
//!
//! Computing peaks for a long buffer is the expensive part, so the result is a
//! cache: it lives in memory and can be serialized to a file (the way audio
//! editors keep an overview/peak file beside the audio) and read back —
//! `to_bytes`/`from_bytes` and `write_cache`/`read_cache`. The layout (see
//! [`crate::bytes`]) is a flat sequence of `f32` arrays, so a build can
//! memory-map it instead of reading it into RAM — the local shared-resource
//! path the GUI host uses to render a multi-megabyte buffer with no per-frame or
//! over-the-wire re-send. The format is machine-local (native float byte order).

use std::fs;
use std::io;
use std::path::Path;

use crate::bytes;

const MAGIC: &[u8; 4] = b"CLPK";
/// Version 1 is the mono layout ([`Pyramid::to_bytes`]); version 2 prefixes a
/// channel count and carries one level sequence per channel
/// ([`MultiPyramid::to_bytes`]). Readers accept both (v1 parses as one channel).
const VERSION: u32 = 1;
const VERSION_MULTI: u32 = 2;

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

    /// The bucket size (source samples per entry) of `level`, if it exists.
    /// A renderer cross-fading between adjacent levels uses it to weight the
    /// blend by where `samples_per_px` sits between the two buckets.
    pub fn level_bucket(&self, level: usize) -> Option<usize> {
        self.levels.get(level).map(|l| l.bucket)
    }

    /// Pick the finest level whose bucket does not exceed `samples_per_px`, so
    /// each pixel column aggregates ~one bucket (no gaps, minimal work). When
    /// zoomed in finer than level 0, level 0 is returned and the caller should
    /// read raw samples instead (see the waveform view's `column`).
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

    /// Write the cache to `path` (e.g. a file beside the audio).
    pub fn write_cache(&self, path: impl AsRef<Path>) -> io::Result<()> {
        fs::write(path, self.to_bytes())
    }

    /// Read a cache from `path`. Returns `Ok(None)` if the file is malformed
    /// (e.g. an older format), so the caller can recompute.
    pub fn read_cache(path: impl AsRef<Path>) -> io::Result<Option<Self>> {
        Ok(Self::from_bytes(&fs::read(path)?))
    }
}

/// The exact [`Pyramid::to_bytes`] length for a pyramid of `total_samples`
/// samples at `base_bucket`, computed **without** building it. A client (e.g.
/// over the FFI) sizes its output buffer with this before calling `build`. It
/// mirrors the level structure `build` produces (level 0 of `ceil(n /
/// base_bucket)` buckets, each higher level halving until length 1), so the
/// `cache_size_matches_to_bytes_len` test pins the two together.
pub fn cache_size(total_samples: usize, base_bucket: usize) -> usize {
    assert!(base_bucket >= 1);
    // Header: MAGIC(4) + VERSION(4) + base_bucket(8) + total_samples(8) + n_levels(8).
    let mut size = 4 + 4 + 8 + 8 + 8;
    let mut level_len = total_samples.div_ceil(base_bucket);
    loop {
        // Per level: bucket(8) + len(8) + min(4*len) + max(4*len).
        size += 8 + 8 + 8 * level_len;
        if level_len <= 1 {
            break;
        }
        level_len = level_len.div_ceil(2);
    }
    size
}

/// A peak pyramid per channel of a multichannel buffer, sharing one
/// `base_bucket` and one per-channel length. This is **one cache resource**
/// (a single file/byte buffer, `to_bytes` version 2) rather than per-channel
/// sibling files, so a multichannel waveform view names exactly one `cache`
/// prop and the channels can never drift apart. `from_bytes` also accepts the
/// version-1 mono layout (as one channel), so existing caches keep working.
pub struct MultiPyramid {
    channels: Vec<Pyramid>,
}

impl MultiPyramid {
    /// Builds one pyramid per channel from `samples` holding `channels`
    /// interleaved channels (`channels >= 1`; a trailing partial frame is
    /// ignored). The de-interleave lives here — core-side — so every client
    /// builds the identical multichannel cache from the same flat buffer.
    pub fn build_interleaved(samples: &[f32], channels: usize, base_bucket: usize) -> Self {
        let channels = channels.max(1);
        let frames = samples.len() / channels;
        let pyramids = (0..channels)
            .map(|ch| {
                let one: Vec<f32> = (0..frames).map(|f| samples[f * channels + ch]).collect();
                Pyramid::build(&one, base_bucket)
            })
            .collect();
        Self { channels: pyramids }
    }

    /// Wraps already-built per-channel pyramids (they must share `base_bucket`
    /// and length; `build_interleaved` guarantees it).
    pub fn from_channels(channels: Vec<Pyramid>) -> Self {
        assert!(!channels.is_empty());
        Self { channels }
    }

    pub fn num_channels(&self) -> usize {
        self.channels.len()
    }

    /// Channel `ch`'s pyramid.
    pub fn channel(&self, ch: usize) -> Option<&Pyramid> {
        self.channels.get(ch)
    }

    /// Consumes the cache into its per-channel pyramids.
    pub fn into_channels(self) -> Vec<Pyramid> {
        self.channels
    }

    /// Samples per channel (the length a view of this cache spans).
    pub fn frames(&self) -> usize {
        self.channels[0].total_samples()
    }

    pub fn base_bucket(&self) -> usize {
        self.channels[0].base_bucket()
    }

    /// Serialize to the version-2 flat byte layout (see [`crate::bytes`]).
    pub fn to_bytes(&self) -> Vec<u8> {
        let first = &self.channels[0];
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        bytes::push_u32(&mut out, VERSION_MULTI);
        bytes::push_u64(&mut out, first.base_bucket);
        bytes::push_u64(&mut out, first.total_samples);
        bytes::push_u64(&mut out, self.channels.len());
        bytes::push_u64(&mut out, first.levels.len());
        for ch in &self.channels {
            for lvl in &ch.levels {
                bytes::push_u64(&mut out, lvl.bucket);
                bytes::push_u64(&mut out, lvl.min.len());
                bytes::push_f32s(&mut out, &lvl.min);
                bytes::push_f32s(&mut out, &lvl.max);
            }
        }
        out
    }

    /// Parse a version-2 buffer, or a version-1 (mono) one as a single channel.
    /// `None` if malformed.
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        let mut r = bytes::Reader::new(data);
        r.tag(MAGIC)?;
        let version = r.u32()?;
        if version == VERSION {
            return Pyramid::from_bytes(data).map(|p| Self { channels: vec![p] });
        }
        if version != VERSION_MULTI {
            return None;
        }
        let base_bucket = r.usize()?;
        let total_samples = r.usize()?;
        let n_channels = r.usize()?.max(1);
        let n_levels = r.usize()?;
        let mut channels = Vec::with_capacity(n_channels);
        for _ in 0..n_channels {
            let mut levels = Vec::with_capacity(n_levels);
            for _ in 0..n_levels {
                let bucket = r.usize()?;
                let len = r.usize()?;
                let min = r.f32_vec(len)?;
                let max = r.f32_vec(len)?;
                levels.push(Level { bucket, min, max });
            }
            channels.push(Pyramid {
                base_bucket,
                total_samples,
                levels,
            });
        }
        Some(Self { channels })
    }

    /// Write the cache to `path` (one file for all channels).
    pub fn write_cache(&self, path: impl AsRef<Path>) -> io::Result<()> {
        fs::write(path, self.to_bytes())
    }

    /// Read a cache from `path`. `Ok(None)` if malformed, so the caller can
    /// recompute.
    pub fn read_cache(path: impl AsRef<Path>) -> io::Result<Option<Self>> {
        Ok(Self::from_bytes(&fs::read(path)?))
    }
}

/// The exact [`MultiPyramid::to_bytes`] length for `frames` samples per channel
/// across `channels` channels at `base_bucket`, computed without building —
/// the multichannel sibling of [`cache_size`], pinned to `to_bytes` by the
/// `multi_cache_size_matches_to_bytes_len` test.
pub fn multi_cache_size(frames: usize, channels: usize, base_bucket: usize) -> usize {
    assert!(base_bucket >= 1);
    let channels = channels.max(1);
    // Header: MAGIC(4) + VERSION(4) + base_bucket(8) + frames(8) + channels(8)
    // + n_levels(8).
    let mut size = 4 + 4 + 8 + 8 + 8 + 8;
    let mut level_len = frames.div_ceil(base_bucket);
    loop {
        // Per level per channel: bucket(8) + len(8) + min(4*len) + max(4*len).
        size += channels * (8 + 8 + 8 * level_len);
        if level_len <= 1 {
            break;
        }
        level_len = level_len.div_ceil(2);
    }
    size
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
        let raw = p.to_bytes();
        let q = Pyramid::from_bytes(&raw).expect("parse");
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

    #[test]
    fn multi_build_matches_per_channel_builds() {
        // Interleaved stereo whose channels are a ramp and its negation: each
        // channel's pyramid must equal the one built from the channel alone.
        let frames = 3000;
        let (l, r): (Vec<f32>, Vec<f32>) =
            (ramp(frames), ramp(frames).iter().map(|x| -x).collect());
        let inter: Vec<f32> = l.iter().zip(&r).flat_map(|(&a, &b)| [a, b]).collect();
        let multi = MultiPyramid::build_interleaved(&inter, 2, 64);
        assert_eq!(multi.num_channels(), 2);
        assert_eq!(multi.frames(), frames);
        for (ch, mono) in [(0, &l), (1, &r)] {
            let alone = Pyramid::build(mono, 64);
            let got = multi.channel(ch).unwrap();
            for level in 0..alone.num_levels() {
                assert_eq!(
                    got.column(level, 0.0, frames as f64),
                    alone.column(level, 0.0, frames as f64),
                    "channel {ch} level {level}"
                );
            }
        }
    }

    #[test]
    fn multi_cache_round_trip_and_v1_compatibility() {
        let inter: Vec<f32> = ramp(4000);
        let multi = MultiPyramid::build_interleaved(&inter, 2, 32);
        let back = MultiPyramid::from_bytes(&multi.to_bytes()).expect("parse v2");
        assert_eq!(back.num_channels(), 2);
        assert_eq!(back.frames(), multi.frames());
        assert_eq!(back.base_bucket(), 32);
        let top = back.channel(1).unwrap().num_levels() - 1;
        assert_eq!(
            back.channel(1).unwrap().column(top, 0.0, 2000.0),
            multi.channel(1).unwrap().column(top, 0.0, 2000.0)
        );
        // A v1 (mono) cache parses as one channel.
        let mono = Pyramid::build(&ramp(1000), 16);
        let as_multi = MultiPyramid::from_bytes(&mono.to_bytes()).expect("parse v1");
        assert_eq!(as_multi.num_channels(), 1);
        assert_eq!(as_multi.frames(), 1000);
        // And garbage is still rejected.
        assert!(MultiPyramid::from_bytes(b"junk").is_none());
    }

    #[test]
    fn multi_cache_size_matches_to_bytes_len() {
        for &(frames, channels, base) in &[(0, 1, 256), (1000, 2, 64), (5000, 4, 256), (77, 3, 8)] {
            let inter = ramp(frames * channels);
            let built = MultiPyramid::build_interleaved(&inter, channels, base)
                .to_bytes()
                .len();
            assert_eq!(
                multi_cache_size(frames, channels, base),
                built,
                "frames={frames} channels={channels} base={base}"
            );
        }
    }

    #[test]
    fn cache_size_matches_to_bytes_len() {
        // The size predicted without building must equal the built cache length,
        // across small/large and exact/ragged bucket counts (and the empty case).
        for &(n, base) in &[(0, 256), (1, 256), (1000, 4), (5000, 64), (100_000, 256)] {
            let built = Pyramid::build(&ramp(n), base).to_bytes().len();
            assert_eq!(cache_size(n, base), built, "n={n} base={base}");
        }
    }
}
