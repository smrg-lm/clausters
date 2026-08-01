//! Bitcode cache layer over the libfaust factory (the "A" layer of def
//! persistence — see [`crate::server::defstore`]).
//!
//! A compiled factory is opaque LLVM JIT state and cannot be serialized, but
//! libfaust can write/read its **bitcode** (target-independent LLVM IR,
//! re-JIT'd to the host on read). Persisting that lets a restart skip Faust's
//! front-end (parse, type inference, IR generation) and only pay the LLVM
//! back-end. The cache is **non-authoritative**: any failure here (missing
//! file, version mismatch, corrupt bitcode) returns an error so the caller
//! falls back to a full compile from the stored source.

use std::ffi::{CStr, CString, c_char};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::faust::compiler::{CompilePayload, ffi_lock, host_target};
use crate::faust::factory::FaustFactory;
use crate::faust::ffi;
use crate::server::defstore::{atomic_write, read_json_files, sanitize_name};

/// The linked libfaust version, e.g. `"2.85.5"`. Stored in each cache record
/// and compared on load: a different libfaust/LLVM may emit incompatible
/// bitcode, so a mismatch invalidates the cached `.bc`.
pub fn faust_version() -> String {
    // SAFETY: returns a static C string owned by libfaust; not freed.
    let ptr = unsafe { ffi::getCLibFaustVersion() };
    if ptr.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

/// Writes `factory`'s bitcode to `path` atomically (temp file + rename), so a
/// crash mid-write never leaves a torn `.bc`. Best-effort: returns whether it
/// succeeded; the caller treats failure as "no cache written".
pub fn write_bitcode(factory: &FaustFactory, path: &Path) -> bool {
    let tmp = path.with_extension("bc.tmp");
    let Ok(tmp_c) = CString::new(tmp.as_os_str().as_encoded_bytes()) else {
        return false;
    };
    let ok = {
        let _guard = ffi_lock();
        // SAFETY: valid factory pointer; libfaust writes the file itself.
        unsafe { ffi::writeCDSPFactoryToBitcodeFile(factory.as_ptr(), tmp_c.as_ptr()) }
    };
    if !ok {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    std::fs::rename(&tmp, path).is_ok()
}

/// Which of the three Faust front-ends a persisted def came in through.
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FaustKind {
    Source,
    Json,
    Signal,
}

/// The transparent, authoritative record of one persisted Faust def. The
/// original `payload` is what gets recompiled on a libfaust upgrade or when
/// the bitcode cache is missing/corrupt; `faust_version` keys cache validity
/// and `payload_sha256` both checksums the record and names the `.bc` file
/// (binding bitcode to the exact source it was built from).
#[derive(Serialize, Deserialize)]
pub struct FaustRecord {
    pub name: String,
    pub kind: FaustKind,
    pub payload: String,
    pub faust_version: String,
    pub payload_sha256: String,
}

impl FaustRecord {
    /// Builds a record from the def name and the payload just compiled,
    /// stamping the current libfaust version and the payload checksum.
    pub fn new(name: &str, payload: &CompilePayload) -> Self {
        let (kind, body) = match payload {
            CompilePayload::Source(s) => (FaustKind::Source, s),
            CompilePayload::Json(s) => (FaustKind::Json, s),
            CompilePayload::Signal(s) => (FaustKind::Signal, s),
        };
        Self {
            name: name.to_string(),
            kind,
            payload: body.clone(),
            faust_version: faust_version(),
            payload_sha256: sha256_hex(body.as_bytes()),
        }
    }

    /// Reconstructs the compile payload to feed the normal front-end on a
    /// cache miss.
    pub fn to_payload(&self) -> CompilePayload {
        match self.kind {
            FaustKind::Source => CompilePayload::Source(self.payload.clone()),
            FaustKind::Json => CompilePayload::Json(self.payload.clone()),
            FaustKind::Signal => CompilePayload::Signal(self.payload.clone()),
        }
    }
}

/// Lowercase hex SHA-256 of `bytes`.
fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn record_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{}.json", sanitize_name(name)))
}

/// The bitcode file is named `<stem>.<sha16>.bc`, so a stale `.bc` left by an
/// interrupted overwrite (different payload → different sha) is never paired
/// with a fresher record.
fn bitcode_path(dir: &Path, name: &str, sha: &str) -> PathBuf {
    dir.join(format!("{}.{}.bc", sanitize_name(name), &sha[..16]))
}

/// Removes every `<stem>.*.bc` for a def, so each persist leaves exactly one
/// bitcode file matching the current payload.
fn clear_bitcode(dir: &Path, name: &str) {
    let stem = sanitize_name(name);
    let prefix = format!("{stem}.");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let fname = entry.file_name();
        let fname = fname.to_string_lossy();
        if fname.starts_with(&prefix) && fname.ends_with(".bc") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Persists a freshly compiled def: writes the bitcode (best-effort — a
/// failure just disables the speed cache for this def) and then the
/// authoritative JSON record. Both atomic.
pub fn persist(factory: &FaustFactory, name: &str, payload: &CompilePayload, dir: &Path) {
    let record = FaustRecord::new(name, payload);
    clear_bitcode(dir, name);
    write_bitcode(factory, &bitcode_path(dir, name, &record.payload_sha256));
    match serde_json::to_vec_pretty(&record) {
        Ok(bytes) => {
            if let Err(e) = atomic_write(&record_path(dir, name), &bytes) {
                tracing::warn!("faust cache: could not write record for {name}: {e}");
            }
        }
        Err(e) => tracing::warn!("faust cache: could not serialize record for {name}: {e}"),
    }
}

/// Tries to restore a factory straight from the bitcode cache, skipping the
/// Faust front-end. `Err` (version mismatch, missing/corrupt `.bc`) tells the
/// caller to recompile from `record.payload`.
pub fn try_restore(record: &FaustRecord, dir: &Path) -> Result<FaustFactory, String> {
    if record.faust_version != faust_version() {
        return Err(format!(
            "libfaust version changed ({} → {})",
            record.faust_version,
            faust_version()
        ));
    }
    read_bitcode(&bitcode_path(dir, &record.name, &record.payload_sha256))
}

/// Reads every persisted Faust record in `dir` (skipping unparseable ones),
/// for the startup reload.
pub fn load_records(dir: &Path) -> Vec<FaustRecord> {
    read_json_files(dir)
        .into_iter()
        .filter_map(|(_, bytes)| serde_json::from_slice(&bytes).ok())
        .collect()
}

/// Removes a def's record and its bitcode (for `/def_free`).
pub fn remove(dir: &Path, name: &str) {
    let _ = std::fs::remove_file(record_path(dir, name));
    clear_bitcode(dir, name);
}

/// Re-creates a factory from a bitcode file, re-JIT'd to the host. `Err` on
/// any failure (missing/corrupt/incompatible), so callers fall back to a
/// fresh compile.
pub fn read_bitcode(path: &Path) -> Result<FaustFactory, String> {
    let path_c = CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| "NUL byte in cache path".to_string())?;
    let target = host_target();
    let mut error_msg = [0 as c_char; ffi::ERROR_MSG_SIZE];

    let _guard = ffi_lock();
    // SAFETY: paths are valid C strings; error_msg is a 4096-byte buffer as
    // libfaust expects.
    let ptr = unsafe {
        ffi::readCDSPFactoryFromBitcodeFile(
            path_c.as_ptr(),
            target.as_ptr(),
            error_msg.as_mut_ptr(),
            -1,
        )
    };
    match unsafe { FaustFactory::from_raw(ptr) } {
        Some(factory) => Ok(factory),
        None => {
            let msg = unsafe { CStr::from_ptr(error_msg.as_ptr()) };
            let msg = msg.to_string_lossy();
            let msg = msg.trim();
            Err(if msg.is_empty() {
                "bitcode read failed".to_string()
            } else {
                msg.to_string()
            })
        }
    }
}
