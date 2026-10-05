//! The native [`FontSource`] -- a typeface read from a file.
//!
//! The host draws with the face it carries unless one is named (`--font
//! <path>`, or `[gui] font` in the config): this is the named one. It used to
//! look through the usual system places when nothing was named, which made
//! the same window a different picture on every machine; the default is the
//! crate's own now, the same in a native window and in a page.
//!
//! The read is the mmap the rest of the bulk path uses, on the platforms that
//! have it: the file is mapped, parsed once into the rasterizer's own tables and
//! unmapped -- the bytes are never held.

use crate::host::diag;
use std::path::{Path, PathBuf};

use super::FontSource;

/// A typeface file on this machine.
pub struct FontFile {
    path: PathBuf,
}

impl FontFile {
    /// The face at `path`, whatever it is -- the `--font` answer, so a path that
    /// turns out unreadable warns at load time rather than being skipped here.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The file this face is read from.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl FontSource for FontFile {
    fn face(&self) -> Option<Vec<u8>> {
        // The mapped read where the platform has one (the same `mmap` the bulk
        // path uses), an ordinary read where it does not.
        #[cfg(unix)]
        let read = super::mapfile::MappedFile::open(&self.path).map(|m| m.bytes().to_vec());
        #[cfg(not(unix))]
        let read = std::fs::read(&self.path);
        match read {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                diag::warn!("cannot read the font {}: {e}", self.path.display());
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_is_no_face() {
        assert!(FontFile::at("/nonexistent/face.ttf").face().is_none());
    }
}
