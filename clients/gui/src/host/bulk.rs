//! The native bulk loader: resolves a waveform/spectrogram/plot's local
//! resource by mapping it read-only.
//!
//! This is the native fill of the [`BulkLoader`](super::BulkLoader) seam — the
//! bulk-data principle made concrete on the desktop: a multi-megabyte buffer
//! named by `path`/`cache` is `mmap`-ed once (through [`super::mapfile`]) and
//! read zero-copy, never re-encoded over OSC. The browser cannot map files, so
//! the same seam is filled by fetching the resource over the network; both
//! return the same platform-agnostic [`WaveformData`]/samples so the GPU views
//! are built identically on either platform. Multichannel is kept end to end:
//! a `path` de-interleaves every channel, a `cache` is the single multichannel
//! [`MultiPyramid`] resource (version-1 mono caches still parse).

use std::path::Path;
use std::sync::Arc;

use tracing::{info, warn};

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
/// re-open skips the rebuild. Unix-only; returns `None` (with a warning) on a
/// non-Unix host or an I/O/format error.
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
            .map_err(|e| warn!("waveform cache {}: {e}", cache.display()))
            .ok()?;
        let multi = MultiPyramid::from_bytes(map.bytes()).or_else(|| {
            warn!("waveform cache {}: malformed peak pyramid", cache.display());
            None
        })?;
        info!(
            "waveform: mapped peak cache {} ({} samples x {} channel(s), no raw data, no OSC)",
            cache.display(),
            multi.frames(),
            multi.num_channels()
        );
        return Some(WaveformData::with_multi_pyramid(multi));
    }

    let path = path?;
    let map = MappedFile::open(path)
        .map_err(|e| warn!("waveform path {}: {e}", path.display()))
        .ok()?;
    let split: Vec<Arc<[f32]>> = map
        .channels_f32(channels)
        .into_iter()
        .map(Into::into)
        .collect();
    let frames = split.first().map_or(0, |c| c.len());
    // Reuse a sibling cache keyed by base_bucket if it matches, else build it.
    let sibling = path.with_extension(format!("{base_bucket}.peaks"));
    let data = match MultiPyramid::read_cache(&sibling) {
        Ok(Some(m))
            if m.frames() == frames
                && m.base_bucket() == base_bucket
                && m.num_channels() == split.len() =>
        {
            WaveformData::from_parts(split.into_iter().zip(m.into_channels()).collect())
        }
        _ => {
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
            let _ = multi.write_cache(&sibling);
            WaveformData::from_parts(split.into_iter().zip(multi.into_channels()).collect())
        }
    };
    info!(
        "waveform: mapped {} samples x {} channel(s) from {} (no OSC, no re-send)",
        data.total_samples(),
        data.num_channels(),
        path.display()
    );
    Some(data)
}

#[cfg(not(unix))]
fn mapped_waveform(
    _cache: Option<&Path>,
    _path: Option<&Path>,
    _channels: usize,
    _base_bucket: usize,
) -> Option<WaveformData> {
    warn!("waveform path/cache (mapped local resource) is only supported on Unix");
    None
}

/// Reads `path` as raw little-endian `f32`, **kept interleaved** (the plot
/// draws every channel; a trailing partial frame is dropped) — the same
/// read-only `mmap` the waveform bulk path uses. Unix-only; returns `None`
/// (with a warning) elsewhere or on an I/O error.
#[cfg(unix)]
fn map_plot_samples(path: &Path, channels: usize) -> Option<Arc<[f32]>> {
    use super::mapfile::MappedFile;
    let map = MappedFile::open(path)
        .map_err(|e| warn!("plot path {}: {e}", path.display()))
        .ok()?;
    let mut floats: Vec<f32> = map
        .bytes()
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let channels = channels.max(1);
    floats.truncate(floats.len() / channels * channels);
    let samples: Arc<[f32]> = floats.into();
    info!(
        "plot: mapped {} samples from {} (no OSC)",
        samples.len(),
        path.display()
    );
    Some(samples)
}

#[cfg(not(unix))]
fn map_plot_samples(_path: &Path, _channels: usize) -> Option<Arc<[f32]>> {
    warn!("plot path (mapped local resource) is only supported on Unix");
    None
}

/// Reads `path` as raw little-endian `f32` de-interleaved into all `channels`
/// (the spectrogram's lane source). Unix-only, like the rest of the mmap path.
#[cfg(unix)]
fn map_raw_channels(path: &Path, channels: usize) -> Option<Vec<Vec<f32>>> {
    use super::mapfile::MappedFile;
    let map = MappedFile::open(path)
        .map_err(|e| warn!("spectrogram path {}: {e}", path.display()))
        .ok()?;
    Some(map.channels_f32(channels))
}

#[cfg(not(unix))]
fn map_raw_channels(_path: &Path, _channels: usize) -> Option<Vec<Vec<f32>>> {
    warn!("spectrogram path (mapped local resource) is only supported on Unix");
    None
}

/// Reads a local resource's raw bytes (a prebuilt STFT cache). Unix-only.
#[cfg(unix)]
fn map_file_bytes(path: &Path) -> Option<Vec<u8>> {
    use super::mapfile::MappedFile;
    let map = MappedFile::open(path)
        .map_err(|e| warn!("cache {}: {e}", path.display()))
        .ok()?;
    Some(map.bytes().to_vec())
}

#[cfg(not(unix))]
fn map_file_bytes(_path: &Path) -> Option<Vec<u8>> {
    warn!("cache (mapped local resource) is only supported on Unix");
    None
}
