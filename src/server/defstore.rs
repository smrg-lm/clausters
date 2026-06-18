//! On-disk persistence of compiled defs, reloaded when the server starts.
//!
//! Two kinds of def live in separate subdirectories of one data directory:
//!
//! - `synthdefs/<name>.json` — the `SynthDefSpec` JSON of a `/d_recv` UGen
//!   graph, stored verbatim. Reloading just re-parses and recompiles it
//!   (cheap); there is no compiled artifact to cache.
//! - `faustdefs/<name>.json` — a [`crate::faust::cache::FaustRecord`] holding
//!   the original Faust source/JSON and metadata, plus a sibling
//!   `faustdefs/<name>.<sha>.bc` bitcode cache (the "A" layer, see
//!   `faust::cache`). The JSON record is always the source of truth; the
//!   bitcode is a non-authoritative speed cache.
//!
//! The original definition (the JSON) is the transparent source of truth in
//! both cases: it is what gets recompiled on a libfaust upgrade or a corrupt
//! cache. Writes are atomic (temp file + rename) so an interrupted startup
//! never leaves a half-written record.

use std::io;
use std::path::{Path, PathBuf};

/// Env var overriding the data directory (highest priority after an explicit
/// CLI path).
const DATA_DIR_ENV: &str = "CLAUSTERS_DATA_DIR";

/// Resolves the data directory: an explicit `--data-dir` wins, then
/// `$CLAUSTERS_DATA_DIR`, then `$XDG_DATA_HOME/clausters`, then
/// `$HOME/.local/share/clausters`. `None` only if no home can be found and
/// nothing was given — persistence is then disabled.
pub fn resolve_data_dir(cli_override: Option<&str>) -> Option<PathBuf> {
    if let Some(path) = cli_override {
        return Some(PathBuf::from(path));
    }
    if let Ok(path) = std::env::var(DATA_DIR_ENV)
        && !path.is_empty()
    {
        return Some(PathBuf::from(path));
    }
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME")
        && !xdg.is_empty()
    {
        return Some(PathBuf::from(xdg).join("clausters"));
    }
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .map(|home| PathBuf::from(home).join(".local/share/clausters"))
}

/// The two on-disk def directories, created on open.
pub struct DefStore {
    synthdefs_dir: PathBuf,
    faustdefs_dir: PathBuf,
}

impl DefStore {
    /// Opens (creating if needed) `<data_dir>/synthdefs` and
    /// `<data_dir>/faustdefs`.
    pub fn open(data_dir: &Path) -> io::Result<Self> {
        let synthdefs_dir = data_dir.join("synthdefs");
        let faustdefs_dir = data_dir.join("faustdefs");
        std::fs::create_dir_all(&synthdefs_dir)?;
        std::fs::create_dir_all(&faustdefs_dir)?;
        Ok(Self {
            synthdefs_dir,
            faustdefs_dir,
        })
    }

    /// The Faust def directory, where `faust::cache` reads/writes records and
    /// bitcode.
    pub fn faustdefs_dir(&self) -> &Path {
        &self.faustdefs_dir
    }

    fn synthdef_path(&self, name: &str) -> PathBuf {
        self.synthdefs_dir
            .join(format!("{}.json", sanitize_name(name)))
    }

    /// Stores a `/d_recv` SynthDef's spec JSON verbatim. Best-effort: errors
    /// are returned so the caller can log them, never fatal.
    pub fn save_synthdef(&self, name: &str, spec_json: &[u8]) -> io::Result<()> {
        atomic_write(&self.synthdef_path(name), spec_json)
    }

    /// Removes a persisted SynthDef (no error if absent).
    pub fn remove_synthdef(&self, name: &str) {
        let _ = std::fs::remove_file(self.synthdef_path(name));
    }

    /// Reads every persisted SynthDef spec (raw JSON bytes), to be fed back
    /// through the normal `/d_recv` path on startup. Unreadable entries are
    /// skipped.
    pub fn load_synthdef_specs(&self) -> Vec<Vec<u8>> {
        read_json_files(&self.synthdefs_dir)
            .into_iter()
            .map(|(_, bytes)| bytes)
            .collect()
    }
}

/// Maps an arbitrary def name to a safe file stem by percent-encoding any
/// character outside `[A-Za-z0-9._-]`. Lossy is fine: the real name is stored
/// inside the record/spec; this only has to avoid collisions and illegal
/// path characters.
pub fn sanitize_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Writes `bytes` to `path` atomically: a sibling `*.tmp` then a rename, so a
/// crash mid-write cannot leave a torn file at `path`.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Reads all `*.json` files in `dir` as `(path, bytes)`, skipping unreadable
/// ones. Shared by the SynthDef loader and the Faust record loader.
pub fn read_json_files(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json")
            && let Ok(bytes) = std::fs::read(&path)
        {
            out.push((path, bytes));
        }
    }
    out
}
