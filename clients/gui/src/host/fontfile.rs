//! The native [`FontSource`] — a typeface read from a file.
//!
//! The crate embeds **no** outline face. A font is hundreds of kilobytes with a
//! license of its own, and a native machine already has faces installed — so
//! the `font-atlas` build points at one (`--font <path>`, or `[gui] font` in
//! the config) and, with nothing named, looks through the usual system places.
//! Finding none is not a failure: the embedded bitmap face draws, as it always
//! did.
//!
//! The read is the mmap the rest of the bulk path uses, on the platforms that
//! have it: the file is mapped, parsed once into the rasterizer's own tables and
//! unmapped — the bytes are never held.

use std::path::{Path, PathBuf};

use super::FontSource;

/// The places a face is looked for when the command line names none, in order.
/// A monospaced face first: it is the closest thing to what the bitmap draws,
/// so a host that gains a typeface does not also change how everything reads.
const SYSTEM_FACES: &[&str] = &[
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoSansMono-Regular.ttf",
    "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
    "/System/Library/Fonts/Menlo.ttc",
    "/System/Library/Fonts/Monaco.ttf",
    "C:/Windows/Fonts/consola.ttf",
];

/// A typeface file on this machine.
pub struct FontFile {
    path: PathBuf,
}

impl FontFile {
    /// The face at `path`, whatever it is — the `--font` answer, so a path that
    /// turns out unreadable warns at load time rather than being skipped here.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The first of the system faces that exists, or `None` on a machine with
    /// none of them (which is why this answers an `Option` and the bitmap face
    /// stays the floor).
    pub fn system() -> Option<Self> {
        SYSTEM_FACES
            .iter()
            .map(Path::new)
            .find(|p| p.is_file())
            .map(Self::at)
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
                tracing::warn!("cannot read the font {}: {e}", self.path.display());
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

    /// Whatever the system offers, it is a file that reads — the search must
    /// never answer a path it cannot open.
    #[test]
    fn the_system_face_reads_when_there_is_one() {
        if let Some(face) = FontFile::system() {
            assert!(face.path().is_file());
            assert!(face.face().is_some_and(|b| !b.is_empty()));
        }
    }
}
