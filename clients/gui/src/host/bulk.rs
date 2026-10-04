//! The native bulk loader: resolves a waveform/spectrogram/plot's local
//! resource by mapping it read-only.
//!
//! This is the native fill of the [`BulkLoader`] seam -- the
//! bulk-data principle made concrete on the desktop: a multi-megabyte buffer
//! named by `path`/`cache` is `mmap`-ed once (through [`super::mapfile`]) and
//! read zero-copy, never re-encoded over OSC. The browser cannot map files, so
//! the same seam is filled by fetching the resource over the network; both
//! return the same platform-agnostic [`WaveformData`]/samples so the GPU views
//! are built identically on either platform. Multichannel is kept end to end:
//! a `path` de-interleaves every channel, a `cache` is the single multichannel
//! [`MultiPyramid`] resource (version-1 mono caches still parse).

use crate::host::diag;
use std::path::Path;
use std::sync::Arc;

use super::BulkLoader;
use crate::peaks::MultiPyramid;
use crate::waveform::WaveformData;

/// The native memory-mapping bulk loader. Unit struct: it holds no state, the
/// resources it resolves are named per call.
pub struct MmapLoader;

impl BulkLoader for MmapLoader {
    fn waveform(
        &self,
        cache: Option<&Path>,
        path: Option<&Path>,
        channels: usize,
        base_bucket: usize,
    ) -> Option<WaveformData> {
        mapped_waveform(cache, path, channels, base_bucket)
    }

    fn plot_samples(&self, path: &Path, channels: usize) -> Option<Arc<[f32]>> {
        map_plot_samples(path, channels)
    }

    fn raw_channels(&self, path: &Path, channels: usize) -> Option<Vec<Vec<f32>>> {
        map_raw_channels(path, channels)
    }

    fn file_bytes(&self, path: &Path) -> Option<Vec<u8>> {
        map_file_bytes(path)
    }
}

/// Loads waveform data from a mapped local resource. `cache` is a prebuilt
/// peak-pyramid file (mono v1 or multichannel v2) mapped and used directly
/// (raw samples never loaded); `path` is a file of raw little-endian `f32`
/// mapped and de-interleaved into all `channels`, whose per-channel pyramids
/// are built once and cached as a sibling `<path>.<base_bucket>.peaks` so a
/// re-open skips the rebuild ([`sibling_summary`] says when one may be
/// reused). Unix-only; returns `None` (with a warning) on a non-Unix host or
/// an I/O/format error.
///
/// The samples are **copied** out of the mapping, unlike a server buffer's
/// region, which a view reads where it lies. A `path` is the client's file,
/// and nothing stops it from being rewritten in place: a mapping held across
/// a truncation faults on the next read, which would take the whole host down
/// in a draw. The server's regions are safe to hold because a take that
/// changes shape gets a new file (the generation is in its name).
#[cfg(unix)]
fn mapped_waveform(
    cache: Option<&Path>,
    path: Option<&Path>,
    channels: usize,
    base_bucket: usize,
) -> Option<WaveformData> {
    use super::mapfile::MappedFile;

    if let Some(cache) = cache {
        let map = MappedFile::open(cache)
            .map_err(|e| diag::warn!("waveform cache {}: {e}", cache.display()))
            .ok()?;
        let multi = MultiPyramid::from_bytes(map.bytes()).or_else(|| {
            diag::warn!("waveform cache {}: malformed peak pyramid", cache.display());
            None
        })?;
        diag::info!(
            "waveform: mapped peak cache {} ({} samples x {} channel(s), no raw data, no OSC)",
            cache.display(),
            multi.frames(),
            multi.num_channels()
        );
        return Some(WaveformData::with_multi_pyramid(multi));
    }

    let path = path?;
    let map = MappedFile::open(path)
        .map_err(|e| diag::warn!("waveform path {}: {e}", path.display()))
        .ok()?;
    let split: Vec<Arc<[f32]>> = map
        .channels_f32(channels)
        .into_iter()
        .map(Into::into)
        .collect();
    let frames = split.first().map_or(0, |c| c.len());
    let sibling = path.with_extension(format!("{base_bucket}.peaks"));
    let data = match sibling_summary(path, &sibling, frames, split.len(), base_bucket) {
        Some(m) => WaveformData::from_parts(split.into_iter().zip(m.into_channels()).collect()),
        None => {
            let flat: Vec<f32> = {
                // Rebuild from the interleaved bytes so the sibling cache is
                // written through the one core builder every client shares.
                let mut flat = vec![0.0f32; frames * split.len()];
                for (ch, samples) in split.iter().enumerate() {
                    for (f, &s) in samples.iter().enumerate() {
                        flat[f * split.len() + ch] = s;
                    }
                }
                flat
            };
            let multi = MultiPyramid::build_interleaved(&flat, split.len(), base_bucket);
            write_sibling(&sibling, &multi);
            WaveformData::from_parts(split.into_iter().zip(multi.into_channels()).collect())
        }
    };
    diag::info!(
        "waveform: mapped {} samples x {} channel(s) from {} (no OSC, no re-send)",
        data.total_samples(),
        data.num_channels(),
        path.display()
    );
    Some(data)
}

/// **The summary beside `path`, when it still describes it** -- `None` sends
/// the caller back to the samples.
///
/// The shape is checked (frames, channels, the bucket), and so is the age: a
/// summary is reused only when it was written **after** the samples were.
/// The shape alone cannot tell a file that was rewritten with new samples of
/// the same length, which is exactly what an edit that does not move a frame
/// produces, and it drew the old picture over the new audio. The comparison is
/// strict, so a file system whose clock is too coarse to order the two writes
/// rebuilds the summary rather than trusting it.
#[cfg(unix)]
fn sibling_summary(
    path: &Path,
    sibling: &Path,
    frames: usize,
    channels: usize,
    base_bucket: usize,
) -> Option<MultiPyramid> {
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    if modified(sibling)? <= modified(path)? {
        return None;
    }
    MultiPyramid::read_cache(sibling)
        .ok()
        .flatten()
        .filter(|m| {
            m.frames() == frames && m.base_bucket() == base_bucket && m.num_channels() == channels
        })
}

/// Writes the summary beside the samples **whole or not at all**: into a
/// file of its own first, then renamed over the old one. Another host opening
/// the same `path` meanwhile reads either summary complete, never a half
/// written one that happens to parse. A directory that cannot be written to
/// leaves no summary, which only costs the next open a rebuild.
#[cfg(unix)]
fn write_sibling(sibling: &Path, multi: &MultiPyramid) {
    let mut part = sibling.as_os_str().to_owned();
    part.push(format!(".{}.part", std::process::id()));
    let part = std::path::PathBuf::from(part);
    if std::fs::write(&part, multi.to_bytes()).is_err() || std::fs::rename(&part, sibling).is_err()
    {
        let _ = std::fs::remove_file(&part);
    }
}

#[cfg(not(unix))]
fn mapped_waveform(
    _cache: Option<&Path>,
    _path: Option<&Path>,
    _channels: usize,
    _base_bucket: usize,
) -> Option<WaveformData> {
    diag::warn!("waveform path/cache (mapped local resource) is only supported on Unix");
    None
}

/// Reads `path` as raw little-endian `f32`, **kept interleaved** (the plot
/// draws every channel; a trailing partial frame is dropped) -- the same
/// read-only `mmap` the waveform bulk path uses. Unix-only; returns `None`
/// (with a warning) elsewhere or on an I/O error.
#[cfg(unix)]
fn map_plot_samples(path: &Path, channels: usize) -> Option<Arc<[f32]>> {
    use super::mapfile::MappedFile;
    let map = MappedFile::open(path)
        .map_err(|e| diag::warn!("plot path {}: {e}", path.display()))
        .ok()?;
    let mut floats: Vec<f32> = clausters_core::osc::blob_samples(map.bytes()).collect();
    let channels = channels.max(1);
    floats.truncate(floats.len() / channels * channels);
    let samples: Arc<[f32]> = floats.into();
    diag::info!(
        "plot: mapped {} samples from {} (no OSC)",
        samples.len(),
        path.display()
    );
    Some(samples)
}

#[cfg(not(unix))]
fn map_plot_samples(_path: &Path, _channels: usize) -> Option<Arc<[f32]>> {
    diag::warn!("plot path (mapped local resource) is only supported on Unix");
    None
}

/// Reads `path` as raw little-endian `f32` de-interleaved into all `channels`
/// (the spectrogram's row source). Unix-only, like the rest of the mmap path.
#[cfg(unix)]
fn map_raw_channels(path: &Path, channels: usize) -> Option<Vec<Vec<f32>>> {
    use super::mapfile::MappedFile;
    let map = MappedFile::open(path)
        .map_err(|e| diag::warn!("spectrogram path {}: {e}", path.display()))
        .ok()?;
    Some(map.channels_f32(channels))
}

#[cfg(not(unix))]
fn map_raw_channels(_path: &Path, _channels: usize) -> Option<Vec<Vec<f32>>> {
    diag::warn!("spectrogram path (mapped local resource) is only supported on Unix");
    None
}

/// Reads a local resource's raw bytes (a prebuilt STFT cache). Unix-only.
#[cfg(unix)]
fn map_file_bytes(path: &Path) -> Option<Vec<u8>> {
    use super::mapfile::MappedFile;
    let map = MappedFile::open(path)
        .map_err(|e| diag::warn!("cache {}: {e}", path.display()))
        .ok()?;
    Some(map.bytes().to_vec())
}

#[cfg(not(unix))]
fn map_file_bytes(_path: &Path) -> Option<Vec<u8>> {
    diag::warn!("cache (mapped local resource) is only supported on Unix");
    None
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    /// A scratch directory of its own per test, so two tests never share a
    /// sibling.
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("clausters_bulk_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_samples(path: &Path, samples: &[f32]) {
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        std::fs::write(path, bytes).unwrap();
    }

    fn touch(path: &Path, at: SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(at)
            .unwrap();
    }

    /// The loudest sample the summary reports over the whole take.
    fn summary_peak(data: &WaveformData) -> f32 {
        let pyramid = data.pyramid();
        let level = pyramid.num_levels() - 1;
        let (_, hi) = pyramid
            .column(level, 0.0, pyramid.total_samples() as f64)
            .unwrap();
        hi
    }

    #[test]
    fn a_fresh_sibling_is_reused_and_a_stale_one_rebuilt() {
        let dir = scratch("sibling");
        let path = dir.join("take.f32");
        let sibling = path.with_extension("4.peaks");
        let quiet = vec![0.25f32; 64];
        let loud = vec![0.75f32; 64];

        // First open: no summary yet, so one is built and written beside it.
        write_samples(&path, &quiet);
        let data = mapped_waveform(None, Some(&path), 1, 4).unwrap();
        assert_eq!(summary_peak(&data), 0.25);
        assert!(sibling.exists());
        assert!(!dir.join("take.4.peaks.part").exists());

        // A summary written after the samples is the one read back: here a
        // planted one of the same shape, so reading it shows.
        std::fs::write(
            &sibling,
            MultiPyramid::build_interleaved(&loud, 1, 4).to_bytes(),
        )
        .unwrap();
        let now = SystemTime::now();
        touch(&path, now - Duration::from_secs(10));
        touch(&sibling, now);
        let data = mapped_waveform(None, Some(&path), 1, 4).unwrap();
        assert_eq!(summary_peak(&data), 0.75);

        // The samples rewritten after it, same length: the shape still
        // matches and the summary is not trusted.
        write_samples(&path, &quiet);
        touch(&path, now + Duration::from_secs(10));
        let data = mapped_waveform(None, Some(&path), 1, 4).unwrap();
        assert_eq!(summary_peak(&data), 0.25);
        assert_eq!(
            summary_peak(&WaveformData::with_multi_pyramid(
                MultiPyramid::read_cache(&sibling).unwrap().unwrap()
            )),
            0.25,
            "the rebuilt summary replaced the stale one"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_sibling_of_another_shape_is_not_reused() {
        let dir = scratch("shape");
        let path = dir.join("take.f32");
        let sibling = path.with_extension("4.peaks");
        write_samples(&path, &[0.5f32; 64]);
        // Newer, but a summary of a different length.
        std::fs::write(
            &sibling,
            MultiPyramid::build_interleaved(&[0.9f32; 32], 1, 4).to_bytes(),
        )
        .unwrap();
        let now = SystemTime::now();
        touch(&path, now - Duration::from_secs(10));
        touch(&sibling, now);
        let data = mapped_waveform(None, Some(&path), 1, 4).unwrap();
        assert_eq!(summary_peak(&data), 0.5);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
