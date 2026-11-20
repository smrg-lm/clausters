//! **The sizes windows were last left at**, by the key each one names.
//!
//! A window opens at the size its def declares, and a redefinition keeps
//! the size a person gave it; this is what survives the window. The size is
//! the host's by what it is about -- the geometry of a window on a screen,
//! which no application sees -- and keyed by what the window is, which only
//! the application can name: a window's `remember` prop. An application
//! names its kind (the multitrack, the audio editor), a script whatever key
//! it likes.
//!
//! A size is kept in **logical** pixels, so a window moved to a display of
//! another density opens at the same extent there. The memory is a small
//! JSON object, `{"key": [w, h]}`, read when the host starts and written
//! when a window that changed it closes and when the host ends. Where the
//! file lives is the shell's business ([`Sizes::at`]): a host with no file
//! (headless, under a test) remembers for as long as it runs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The sizes windows were last left at, keyed by their `remember` prop.
#[derive(Debug, Default)]
pub struct Sizes {
    sizes: BTreeMap<String, (u32, u32)>,
    /// Where the memory is kept, or `None` for a host that keeps none.
    path: Option<PathBuf>,
    /// A size moved since the file was last written.
    changed: bool,
}

/// The file the native host keeps window sizes in: `windows.json` beside
/// the user configuration (`$XDG_CONFIG_HOME/clausters/`), so the key table
/// and the geometry of the windows are in one place.
#[cfg(not(target_arch = "wasm32"))]
pub fn default_path() -> Option<PathBuf> {
    let config = clausters_core::config::user_config_path()?;
    Some(config.parent()?.join("windows.json"))
}

impl Sizes {
    /// The memory kept at `path`, read now: a missing or unreadable file is
    /// a memory with nothing in it yet.
    pub fn at(path: PathBuf) -> Sizes {
        let sizes = std::fs::read_to_string(&path)
            .ok()
            .map(|text| parse(&text))
            .unwrap_or_default();
        Sizes {
            sizes,
            path: Some(path),
            changed: false,
        }
    }

    /// The size the window keyed `key` was last left at, logical.
    pub fn get(&self, key: &str) -> Option<(u32, u32)> {
        self.sizes.get(key).copied()
    }

    /// The window keyed `key` is now `size` logical pixels.
    pub fn set(&mut self, key: &str, size: (u32, u32)) {
        let size = (size.0.max(1), size.1.max(1));
        if self.sizes.get(key) != Some(&size) {
            self.sizes.insert(key.to_string(), size);
            self.changed = true;
        }
    }

    /// Writes the memory if a size moved since it was last written. A
    /// failure is logged and the sizes stay in memory, to be written again.
    pub fn save(&mut self) {
        if !self.changed {
            return;
        }
        let Some(path) = &self.path else {
            self.changed = false;
            return;
        };
        match write(path, &self.sizes) {
            Ok(()) => self.changed = false,
            Err(e) => tracing::warn!("window sizes: cannot write {}: {e}", path.display()),
        }
    }
}

fn parse(text: &str) -> BTreeMap<String, (u32, u32)> {
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str(text) else {
        return BTreeMap::new();
    };
    let dim = |v: &serde_json::Value| v.as_u64().and_then(|n| u32::try_from(n).ok());
    map.into_iter()
        .filter_map(|(key, v)| {
            let pair = v.as_array()?;
            Some((key, (dim(pair.first()?)?, dim(pair.get(1)?)?)))
        })
        .collect()
}

fn write(path: &Path, sizes: &BTreeMap<String, (u32, u32)>) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let map: serde_json::Map<String, serde_json::Value> = sizes
        .iter()
        .map(|(k, &(w, h))| (k.clone(), serde_json::json!([w, h])))
        .collect();
    let text = serde_json::to_string_pretty(&serde_json::Value::Object(map))?;
    // written beside and moved over, so a crash never leaves half a file
    let partial = path.with_extension("json.partial");
    std::fs::write(&partial, text)?;
    std::fs::rename(&partial, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_survives_the_file() {
        let dir = std::env::temp_dir().join(format!("clausters-sizes-{}", std::process::id()));
        let path = dir.join("windows.json");
        let mut sizes = Sizes::at(path.clone());
        assert_eq!(sizes.get("multitrack"), None);
        sizes.set("multitrack", (900, 540));
        sizes.save();
        let again = Sizes::at(path.clone());
        assert_eq!(again.get("multitrack"), Some((900, 540)));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn a_broken_entry_is_skipped() {
        let read = parse(r#"{"a":[10,20],"b":"x","c":[1],"d":[-1,4]}"#);
        assert_eq!(read.len(), 1);
        assert_eq!(read.get("a"), Some(&(10, 20)));
    }
}
