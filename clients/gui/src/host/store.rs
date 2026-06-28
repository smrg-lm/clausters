//! On-disk persistence of GuiDefs — the GUI counterpart of the server's def store.
//!
//! A GuiDef persists the same way a `SynthDef`/`GraphDef` does on the server
//! (`src/server/defstore.rs`): JSON under a `defs/` subdirectory of the data
//! directory, keyed by name, the JSON being the transparent source of truth. The
//! GUI keeps its own small store rather than linking the server's — the same
//! independence the rest of the gui crate holds — and resolves the **same** data
//! directory, so a bundle's SynthDefs (`defs/synthdefs`), GraphDefs
//! (`defs/graphdefs`) and GuiDefs (`defs/guidefs`) sit side by side and the
//! standalone host (G10) can read them all.
//!
//! A GuiDef is identified on the wire by an integer id (`/gui_def <id> …`), so a
//! saved record carries both the id and the verbatim tree JSON: `{ "id": <i32>,
//! "gui": <tree> }`. Loading replays it as a `/gui_def`.

use std::io;
use std::path::{Path, PathBuf};

use clausters_core::osc::{OscMessage, OscType};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Env var overriding the data directory (matches the server's `defstore`).
const DATA_DIR_ENV: &str = "CLAUSTERS_DATA_DIR";

/// Resolves the data directory the same way the server does: an explicit
/// override wins, then `$CLAUSTERS_DATA_DIR`, then `$XDG_DATA_HOME/clausters`,
/// then `$HOME/.local/share/clausters`. `None` when nothing is set and no home
/// can be found — persistence is then disabled.
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

/// A persisted GuiDef record: the id it was defined with plus its tree JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedGuiDef {
    pub id: i32,
    pub gui: Value,
}

/// The GuiDef store under `<data_dir>/defs/guidefs`, with read access to the
/// sibling SynthDef/GraphDef spec directories the standalone boot needs.
pub struct GuiStore {
    guidefs_dir: PathBuf,
    synthdefs_dir: PathBuf,
    graphdefs_dir: PathBuf,
}

impl GuiStore {
    /// Opens (creating the `guidefs` directory) the store under `<data_dir>/defs`.
    pub fn open(data_dir: &Path) -> io::Result<Self> {
        let defs = data_dir.join("defs");
        let guidefs_dir = defs.join("guidefs");
        std::fs::create_dir_all(&guidefs_dir)?;
        Ok(Self {
            guidefs_dir,
            synthdefs_dir: defs.join("synthdefs"),
            graphdefs_dir: defs.join("graphdefs"),
        })
    }

    fn guidef_path(&self, name: &str) -> PathBuf {
        self.guidefs_dir
            .join(format!("{}.json", sanitize_name(name)))
    }

    /// Persists GuiDef `id` (its verbatim tree JSON) under `name`.
    pub fn save(&self, name: &str, id: i32, tree_json: &[u8]) -> io::Result<()> {
        let gui: Value = serde_json::from_slice(tree_json).map_err(io::Error::other)?;
        let record = SavedGuiDef { id, gui };
        let bytes = serde_json::to_vec_pretty(&record).map_err(io::Error::other)?;
        atomic_write(&self.guidef_path(name), &bytes)
    }

    /// Loads the GuiDef saved under `name`: its id and tree JSON (ready to replay
    /// as a `/gui_def`).
    pub fn load(&self, name: &str) -> io::Result<(i32, Vec<u8>)> {
        let bytes = std::fs::read(self.guidef_path(name))?;
        let record: SavedGuiDef = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        let tree = serde_json::to_vec(&record.gui).map_err(io::Error::other)?;
        Ok((record.id, tree))
    }

    /// The names of every persisted GuiDef (file stems, sorted).
    pub fn list(&self) -> Vec<String> {
        let mut names: Vec<String> = read_json_files(&self.guidefs_dir)
            .into_iter()
            .filter_map(|(path, _)| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .map(str::to_string)
            })
            .collect();
        names.sort();
        names
    }

    /// Every persisted SynthDef spec (raw `/d_recv` JSON), for the standalone
    /// boot to replay into the embedded server.
    pub fn synthdef_specs(&self) -> Vec<Vec<u8>> {
        read_json_files(&self.synthdefs_dir)
            .into_iter()
            .map(|(_, b)| b)
            .collect()
    }

    /// Every persisted GraphDef spec (raw `/d_graph` JSON).
    pub fn graphdef_specs(&self) -> Vec<Vec<u8>> {
        read_json_files(&self.graphdefs_dir)
            .into_iter()
            .map(|(_, b)| b)
            .collect()
    }
}

/// The filesystem store is the native fill of the [`DefStore`](super::DefStore)
/// seam: the protocol dispatch persists named GuiDefs and serves `/gui_load`
/// through this trait, so it never names the filesystem directly (a browser host
/// simply runs without a store).
impl super::DefStore for GuiStore {
    fn save(&self, name: &str, id: i32, tree_json: &[u8]) -> io::Result<()> {
        GuiStore::save(self, name, id, tree_json)
    }

    fn load(&self, name: &str) -> io::Result<(i32, Vec<u8>)> {
        GuiStore::load(self, name)
    }
}

/// The `boot` messages declared at a GuiDef's root: a list of `[addr, args…]`
/// the standalone host sends to the server right after the defs load, to bring
/// the instrument up (e.g. `["/s_new", "drone", 1000, 0, 0]`). The int/float
/// distinction is preserved (a JSON integer is an OSC `Int`, so node ids stay
/// integers). Empty when the GuiDef declares no `boot`.
pub fn boot_messages(tree_json: &[u8]) -> Vec<OscMessage> {
    let Ok(Value::Object(root)) = serde_json::from_slice::<Value>(tree_json) else {
        return Vec::new();
    };
    let Some(Value::Array(list)) = root.get("boot") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in list {
        if let Value::Array(items) = entry
            && let Some(Value::String(addr)) = items.first()
        {
            let args = items[1..].iter().filter_map(value_to_osc).collect();
            out.push(OscMessage {
                addr: addr.clone(),
                args,
            });
        }
    }
    out
}

/// One JSON value as an OSC primitive, keeping integers and floats apart.
fn value_to_osc(v: &Value) -> Option<OscType> {
    match v {
        Value::Number(n) if n.is_i64() || n.is_u64() => Some(OscType::Int(n.as_i64()? as i32)),
        Value::Number(n) => Some(OscType::Float(n.as_f64()? as f32)),
        Value::String(s) => Some(OscType::String(s.clone())),
        Value::Bool(b) => Some(OscType::Int(*b as i32)),
        _ => None,
    }
}

/// Maps an arbitrary def name to a safe file stem (percent-encoding anything
/// outside `[A-Za-z0-9._-]`), matching the server's `defstore::sanitize_name`.
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

/// Writes `bytes` to `path` atomically (temp file + rename), so a crash mid-write
/// cannot leave a torn file.
fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Reads all `*.json` files in `dir` as `(path, bytes)`, skipping unreadable ones
/// and an absent directory.
fn read_json_files(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("clausters_gui_store_{}", std::process::id()));
        p.push(format!("{:?}", std::time::Instant::now()));
        p
    }

    #[test]
    fn save_then_load_round_trips_id_and_tree() {
        let dir = temp_dir();
        let store = GuiStore::open(&dir).unwrap();
        let tree =
            br#"{"type":"window","name":"inst","children":[{"id":10,"type":"knob","value":0.5}]}"#;
        store.save("inst", 7, tree).unwrap();

        let (id, json) = store.load("inst").unwrap();
        assert_eq!(id, 7);
        // The reloaded tree parses back to the same structure (int/float kept).
        let v: Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(v["type"], "window");
        assert_eq!(v["children"][0]["value"], 0.5);
        assert_eq!(store.list(), vec!["inst".to_string()]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn loading_a_missing_name_is_an_error() {
        let dir = temp_dir();
        let store = GuiStore::open(&dir).unwrap();
        assert!(store.load("nope").is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn boot_messages_parse_with_the_int_float_distinction() {
        let json = br#"{"type":"window","boot":[
            ["/s_new","drone",1000,0,0],
            ["/n_set",1000,"amp",0.2]
        ]}"#;
        let msgs = boot_messages(json);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].addr, "/s_new");
        // Node ids stay integers; the amp is a float.
        assert_eq!(msgs[0].args[1], OscType::Int(1000));
        assert_eq!(msgs[1].args[2], OscType::Float(0.2));
        // A GuiDef without `boot` yields nothing.
        assert!(boot_messages(br#"{"type":"window"}"#).is_empty());
    }

    #[test]
    fn sanitize_keeps_safe_chars_and_escapes_the_rest() {
        assert_eq!(sanitize_name("My Inst/2"), "My%20Inst%2F2");
        assert_eq!(sanitize_name("a.b-c_1"), "a.b-c_1");
    }
}
