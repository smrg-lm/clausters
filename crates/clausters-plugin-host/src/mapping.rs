//! Linux mapping ownership. Creation, initialization, prefault and cleanup are non-RT.

use clausters_core::plugin_shm::{Config, View};
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;

pub(crate) struct Mapping {
    base: NonNull<u8>,
    len: usize,
    generation: u64,
    path: PathBuf,
    owner: bool,
    _file: File,
}
// SAFETY: immutable mapping ownership, payload accesses follow plugin_shm's SPSC protocol.
unsafe impl Send for Mapping {}
// SAFETY: only atomics/control are shared; one endpoint owns each mutable payload.
unsafe impl Sync for Mapping {}
impl Mapping {
    pub(crate) fn create(config: Config) -> Result<Self, String> {
        let len = config.bytes().ok_or("invalid plugin capacities")?;
        let path = std::env::temp_dir().join(format!(
            "clausters-plugin-{}-{}.shm",
            std::process::id(),
            config.generation
        ));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| e.to_string())?;
        if let Err(e) = file.set_len(len as u64) {
            let _ = std::fs::remove_file(&path);
            return Err(e.to_string());
        }
        let mapping = Self::map(file, path, len, config.generation, true)?;
        // SAFETY: fresh, private mapping with checked exact length, mmap alignment and lifetime.
        unsafe { View::initialize(mapping.base.as_ptr(), len, config) }.map_err(str::to_owned)?;
        Ok(mapping)
    }
    #[cfg(any(feature = "clap", feature = "vst3"))]
    pub(crate) fn open(path: &Path, generation: u64) -> Result<Self, String> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        let len = file.metadata().map_err(|e| e.to_string())?.len();
        if len == 0 || len > 2 * 1024 * 1024 {
            return Err("invalid plugin mapping length".into());
        }
        let mapping = Self::map(file, path.into(), len as usize, generation, false)?;
        mapping.view()?;
        Ok(mapping)
    }
    fn map(
        file: File,
        path: PathBuf,
        len: usize,
        generation: u64,
        owner: bool,
    ) -> Result<Self, String> {
        // SAFETY: valid open file, checked bounded length; MAP_SHARED keeps both peers on one plane.
        let p = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if p == libc::MAP_FAILED {
            if owner {
                let _ = std::fs::remove_file(&path);
            }
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(Self {
            base: NonNull::new(p.cast()).ok_or("null plugin mapping")?,
            len,
            generation,
            path,
            owner,
            _file: file,
        })
    }
    pub(crate) fn view(&self) -> Result<View<'_>, String> {
        // SAFETY: Mapping owns the initialized segment; callers create exactly one
        // producer/consumer, while other views only access immutable fields/atomics.
        unsafe { View::attach(self.base.as_ptr(), self.len, self.generation) }
            .map_err(str::to_owned)
    }
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: owner is dropped only after the local endpoint and helper are finished.
        unsafe {
            libc::munmap(self.base.as_ptr().cast(), self.len);
        }
        if self.owner {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
