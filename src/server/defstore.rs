//! On-disk persistence of compiled defs, reloaded when the server starts.
//!
//! All persisted definitions live under a `defs/` subdirectory of the data
//! directory, one subdirectory per kind, so the data directory itself stays
//! free for other persistent aspects (`midi.json`, `boot.json`, and whatever
//! comes later):
//!
//! - `defs/synthdefs/<name>.json` — the `SynthDefSpec` JSON of a `/d_recv`
//!   UGen graph, stored verbatim. Reloading just re-parses and recompiles it
//!   (cheap); there is no compiled artifact to cache.
//! - `defs/faustdefs/<name>.json` — a `crate::faust::cache::FaustRecord`
//!   holding the original Faust source/JSON and metadata, plus a sibling
//!   `defs/faustdefs/<name>.<sha>.bc` bitcode cache (the "A" layer, see
//!   `faust::cache`). The JSON record is always the source of truth; the
//!   bitcode is a non-authoritative speed cache.
//! - `defs/graphdefs/<name>.json` — the `/d_graph` GraphDef spec, verbatim.
//!
//! The original definition (the JSON) is the transparent source of truth in
//! both cases: it is what gets recompiled on a libfaust upgrade or a corrupt
//! cache. Writes are atomic (temp file + rename) so an interrupted startup
//! never leaves a half-written record.
//!
//! **A name identifies one def, across all three kinds.** Sending a def frees
//! the name in the other two — last one wins — so a stale record of another
//! kind can never shadow what a client just sent (see
//! [`DefStore::remove_other_kinds`]).
//!
//! **Ephemeral defs are not persisted.** A name starting with [`TMP_PREFIX`]
//! marks a def the client built to hold an expression that has no name of its
//! own (`clausters.defs.as_def`), and those must not accumulate in a store
//! that outlives the process: see [`is_ephemeral`].

use std::io;
use std::path::{Path, PathBuf};

/// Name prefix marking a def as **ephemeral**: built to carry an expression
/// rather than named by anyone, so it has no business outliving the process
/// that sent it. See [`is_ephemeral`].
pub const TMP_PREFIX: &str = "tmp_";

/// Whether `name` marks an ephemeral def — one the server keeps in memory and
/// never writes to the persistent store.
///
/// The convention is the name itself rather than a wire flag, so it needs no
/// per-command argument and reads as what it is in a log line or a `/d_query`
/// listing. The cost is that a *user* def named `tmp_...` is ephemeral too;
/// that is the documented meaning of the prefix, not an accident.
pub fn is_ephemeral(name: &str) -> bool {
    name.starts_with(TMP_PREFIX)
}

/// Where an ephemeral def's unavoidable artifacts go: a subdirectory of the
/// OS temp directory, never the data directory. Only the Faust pair lands
/// here — the record and its bitcode — since a `/d_recv` or `/d_graph` has no
/// compiled artifact to keep; a replayed expression then still skips the
/// recompile while the persistent store stays clean, and the OS reclaims the
/// directory on its own schedule.
pub fn ephemeral_dir() -> PathBuf {
    std::env::temp_dir().join("clausters-tmpdefs")
}

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

/// Which kind of def a name currently holds — the argument to
/// [`DefStore::remove_other_kinds`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefKind {
    Synth,
    Faust,
    Graph,
}

/// The on-disk def directories and config files, created on open.
pub struct DefStore {
    synthdefs_dir: PathBuf,
    faustdefs_dir: PathBuf,
    graphdefs_dir: PathBuf,
    /// `<data_dir>/midi.json` — persisted MIDI bindings (M19).
    bindings_path: PathBuf,
    /// `<data_dir>/boot.json` — the boot preset of standalone graphs (M19).
    boot_path: PathBuf,
}

impl DefStore {
    /// Opens (creating if needed) the def subdirectories under `<data_dir>/defs`
    /// (`synthdefs`, `faustdefs`, `graphdefs`). The data directory itself holds
    /// the other persistent files (`midi.json`, `boot.json`).
    pub fn open(data_dir: &Path) -> io::Result<Self> {
        let defs_dir = data_dir.join("defs");
        let synthdefs_dir = defs_dir.join("synthdefs");
        let faustdefs_dir = defs_dir.join("faustdefs");
        let graphdefs_dir = defs_dir.join("graphdefs");
        std::fs::create_dir_all(&synthdefs_dir)?;
        std::fs::create_dir_all(&faustdefs_dir)?;
        std::fs::create_dir_all(&graphdefs_dir)?;
        Ok(Self {
            synthdefs_dir,
            faustdefs_dir,
            graphdefs_dir,
            bindings_path: data_dir.join("midi.json"),
            boot_path: data_dir.join("boot.json"),
        })
    }

    /// The Faust def directory, where `faust::cache` reads/writes records and
    /// bitcode.
    pub fn faustdefs_dir(&self) -> &Path {
        &self.faustdefs_dir
    }

    /// The SynthDef spec directory (`defs/synthdefs`).
    pub fn synthdefs_dir(&self) -> &Path {
        &self.synthdefs_dir
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

    fn graphdef_path(&self, name: &str) -> PathBuf {
        self.graphdefs_dir
            .join(format!("{}.json", sanitize_name(name)))
    }

    /// Stores a `/d_graph` GraphDef's spec JSON verbatim (M18). Like a
    /// SynthDef, the JSON is the transparent source of truth; there is no
    /// compiled artifact (a GraphDef only references other defs).
    pub fn save_graphdef(&self, name: &str, spec_json: &[u8]) -> io::Result<()> {
        atomic_write(&self.graphdef_path(name), spec_json)
    }

    /// Removes a persisted GraphDef (no error if absent).
    pub fn remove_graphdef(&self, name: &str) {
        let _ = std::fs::remove_file(self.graphdef_path(name));
    }

    /// Frees `name` in every def kind **except** `keep`, on disk.
    ///
    /// A name identifies one def, so a def arriving under a name another kind
    /// holds replaces it rather than sitting beside it. Without this the two
    /// records both survive a restart and the reload order decides which one
    /// answers — which is how a stale mono SynthDef came to shadow a stereo
    /// FaustDef of the same name and report the wrong bus usage.
    pub fn remove_other_kinds(&self, name: &str, keep: DefKind) {
        if keep != DefKind::Synth {
            self.remove_synthdef(name);
        }
        if keep != DefKind::Graph {
            self.remove_graphdef(name);
        }
        #[cfg(feature = "faust")]
        if keep != DefKind::Faust {
            crate::faust::cache::remove(&self.faustdefs_dir, name);
        }
    }

    /// Reads every persisted GraphDef spec (raw JSON bytes) for the startup
    /// reload, fed back through the normal `/d_graph` path.
    pub fn load_graphdef_specs(&self) -> Vec<Vec<u8>> {
        read_json_files(&self.graphdefs_dir)
            .into_iter()
            .map(|(_, bytes)| bytes)
            .collect()
    }

    /// Writes the MIDI bindings to `midi.json` (M19). Best-effort: the caller
    /// logs an error, never fatal.
    pub fn save_bindings(&self, bindings: &[crate::midi::PersistedBinding]) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(bindings).map_err(io::Error::other)?;
        atomic_write(&self.bindings_path, &json)
    }

    /// Reads the persisted MIDI bindings (empty if absent or unreadable).
    pub fn load_bindings(&self) -> Vec<crate::midi::PersistedBinding> {
        std::fs::read(&self.bindings_path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Reads the boot preset of standalone graphs (empty if absent). The file
    /// is authored by the user / a client; the server only reads it.
    pub fn load_boot(&self) -> Vec<crate::osc::graphdef::BootInstance> {
        std::fs::read(&self.boot_path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
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
