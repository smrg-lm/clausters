//! **Where the editor keeps what it keeps**: the folders of the webview
//! application, by the same identifier, so the two read one configuration,
//! one set of environments and one documentation folder while they coexist.

use std::path::PathBuf;

/// The application's identifier, its folders' name.
pub const IDENTIFIER: &str = "io.github.smrg-lm.clausters.editor";

fn under(base: Option<PathBuf>) -> PathBuf {
    base.unwrap_or_else(std::env::temp_dir).join(IDENTIFIER)
}

/// The configuration (`config.json`, the preferences).
pub fn config_dir() -> PathBuf {
    under(dirs::config_dir())
}

/// The environments (`venvs/`).
pub fn data_dir() -> PathBuf {
    under(dirs::data_dir())
}

/// What is written again at every start (the session's driver).
pub fn cache_dir() -> PathBuf {
    under(dirs::cache_dir())
}
