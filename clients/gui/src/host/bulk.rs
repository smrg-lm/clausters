//! The native bulk loader: resolves a waveform/plot's local resource by mapping
//! it read-only.
//!
//! This is the native fill of the [`BulkLoader`](super::BulkLoader) seam — the
//! G7 bulk-data principle made concrete on the desktop: a multi-megabyte buffer
//! named by `path`/`cache` is `mmap`-ed once (through [`super::mapfile`]) and
//! read zero-copy, never re-encoded over OSC. The browser cannot map files, so a
//! later milestone fills the same seam by fetching the resource over the network;
//! both return the same platform-agnostic [`WaveformData`]/samples so the GPU
//! views are built identically on either platform.

use std::path::Path;
use std::sync::Arc;

use tracing::{info, warn};

use super::BulkLoader;
use crate::peaks::Pyramid;
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
}

/// Loads waveform data from a mapped local resource. `cache` is a prebuilt
/// peak-pyramid file mapped and used directly (raw samples never loaded); `path`
/// is a file of raw little-endian `f32` mapped and de-interleaved (channel 0 of
/// `channels`), whose pyramid is built once and cached as a sibling
/// `<path>.<base_bucket>.peaks` so a re-open skips the rebuild. Unix-only;
/// returns `None` (with a warning) on a non-Unix host or an I/O/format error.
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
        let pyramid = Pyramid::from_bytes(map.bytes()).or_else(|| {
            warn!("waveform cache {}: malformed peak pyramid", cache.display());
            None
        })?;
        info!(
            "waveform: mapped peak cache {} ({} samples, no raw data, no OSC)",
            cache.display(),
            pyramid.total_samples()
        );
        return Some(WaveformData::with_pyramid(
            Arc::from([] as [f32; 0]),
            pyramid,
        ));
    }

    let path = path?;
    let map = MappedFile::open(path)
        .map_err(|e| warn!("waveform path {}: {e}", path.display()))
        .ok()?;
    let samples: Arc<[f32]> = map.channel0_f32(channels).into();
    // Reuse a sibling cache keyed by base_bucket if it matches, else build it.
    let sibling = path.with_extension(format!("{base_bucket}.peaks"));
    let data = match Pyramid::read_cache(&sibling) {
        Ok(Some(p)) if p.total_samples() == samples.len() && p.base_bucket() == base_bucket => {
            WaveformData::with_pyramid(samples, p)
        }
        _ => {
            let data = WaveformData::new(Arc::clone(&samples), base_bucket);
            let _ = data.pyramid().write_cache(&sibling);
            data
        }
    };
    info!(
        "waveform: mapped {} samples from {} (no OSC, no re-send)",
        data.total_samples(),
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

/// Reads `path` as raw little-endian `f32`, de-interleaving channel 0 of
/// `channels` — the same read-only `mmap` the waveform bulk path uses. Unix-only;
/// returns `None` (with a warning) elsewhere or on an I/O error.
#[cfg(unix)]
fn map_plot_samples(path: &Path, channels: usize) -> Option<Arc<[f32]>> {
    use super::mapfile::MappedFile;
    let map = MappedFile::open(path)
        .map_err(|e| warn!("plot path {}: {e}", path.display()))
        .ok()?;
    let samples: Arc<[f32]> = map.channel0_f32(channels).into();
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
