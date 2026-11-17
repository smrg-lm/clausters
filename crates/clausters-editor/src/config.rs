//! **The application's persistent configuration** (`config.json` in the
//! configuration folder): the active environment, the pip install source,
//! the documentation folder and the startup code -- the file the webview
//! application keeps, read and written as it does.

use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    /// Name of the virtual environment the session uses; `None` = the system Python.
    pub active_env: Option<String>,
    /// The global install source of earlier versions; each environment now keeps
    /// its own (envs.rs). Only used for environments created before that change.
    pub install_source: String,
    /// Markdown documentation folder; `None` = the one that comes with the editor.
    pub docs_dir: Option<String>,
    /// Code run at the start of every session (e.g. default imports).
    pub startup_code: String,
}

static CONFIG: Mutex<Option<Config>> = Mutex::new(None);

fn path() -> PathBuf {
    crate::paths::config_dir().join("config.json")
}

fn load() -> Config {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(config: &Config) -> Result<(), String> {
    let path = path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

/// A copy of the current configuration.
pub fn get() -> Config {
    CONFIG
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(load)
        .clone()
}

/// Changes the configuration and saves it to disk.
pub fn update(change: impl FnOnce(&mut Config)) -> Result<Config, String> {
    let mut guard = CONFIG.lock().unwrap_or_else(PoisonError::into_inner);
    let config = guard.get_or_insert_with(load);
    change(config);
    save(config)?;
    Ok(config.clone())
}
