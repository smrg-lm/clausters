//! **The application's persistent configuration** (`config.json`, kept by
//! [`store`](crate::store)): the active environment, the pip install source,
//! the documentation folder and the startup code -- the file the webview
//! application keeps, read and written as it does.

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

const NAME: &str = "config.json";

/// **The startup code a page's session starts with** until the preferences
/// say otherwise: the page's engine and host, ready to use. A desktop's
/// session starts with none, since its Python may not have the client.
#[cfg(target_arch = "wasm32")]
const STARTUP: &str = "import { Session } from \"clausters\";\n\
    const session = (await Session.live()).activate();\n\
    const host = await session.gui();\n";
#[cfg(not(target_arch = "wasm32"))]
const STARTUP: &str = "";

fn load() -> Config {
    crate::store::read(NAME)
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| Config {
            startup_code: STARTUP.into(),
            ..Config::default()
        })
}

fn save(config: &Config) -> Result<(), String> {
    let json = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    crate::store::write(NAME, &json)
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
