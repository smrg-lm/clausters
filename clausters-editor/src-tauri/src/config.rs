//! The app's persistent configuration (JSON in the config directory): the
//! active environment, the pip install source and the documentation folder.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    /// Name of the virtual environment the session uses; `None` = the system Python.
    pub active_env: Option<String>,
    /// The global install source of earlier versions; each environment now keeps
    /// its own (envs.rs). Only used for environments created before that change.
    pub install_source: String,
    /// Markdown documentation folder; `None` = the one bundled with the app.
    pub docs_dir: Option<String>,
    /// Code run at the start of every session (e.g. default imports).
    pub startup_code: String,
}

pub struct ConfigState(pub Mutex<Config>);

fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?
        .join("config.json"))
}

pub fn load(app: &AppHandle) -> Config {
    config_path(app)
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(app: &AppHandle, config: &Config) -> Result<(), String> {
    let path = config_path(app)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

/// A copy of the current configuration.
pub fn get(app: &AppHandle) -> Config {
    app.state::<ConfigState>().0.lock().unwrap().clone()
}

/// Changes the configuration and saves it to disk.
pub fn update(app: &AppHandle, change: impl FnOnce(&mut Config)) -> Result<Config, String> {
    let state = app.state::<ConfigState>();
    let mut config = state.0.lock().unwrap();
    change(&mut config);
    save(app, &config)?;
    Ok(config.clone())
}

#[tauri::command]
pub fn get_config(state: State<ConfigState>) -> Config {
    state.0.lock().unwrap().clone()
}

#[tauri::command]
pub fn set_docs_dir(app: AppHandle, dir: Option<String>) -> Result<Config, String> {
    if let Some(d) = &dir {
        if !std::path::Path::new(d).is_dir() {
            return Err(format!("Not a folder: {d}"));
        }
    }
    update(&app, |c| c.docs_dir = dir)
}

#[tauri::command]
pub fn set_startup_code(app: AppHandle, code: String) -> Result<Config, String> {
    update(&app, |c| c.startup_code = code)
}
