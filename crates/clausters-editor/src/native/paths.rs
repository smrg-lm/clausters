//! **Where the editor keeps what it keeps**: the platform's ordinary folders
//! under the program's name -- `$XDG_CONFIG_HOME/clausters-editor`,
//! `$XDG_DATA_HOME/clausters-editor` and `$XDG_CACHE_HOME/clausters-editor`
//! on Linux, and what the `dirs` crate names elsewhere.
//!
//! The webview application kept them under a reverse-domain identifier its
//! framework asked for; the first start after it moves them here
//! ([`migrate`]).

use std::path::{Path, PathBuf};

/// The folders' name: the program's.
pub const NAME: &str = "clausters-editor";

/// The webview application's name for the same folders.
const OLD: &str = "io.github.smrg-lm.clausters.editor";

/// What a moved environment holds that marks it to be built again
/// (`envs::rebuild_moved`).
pub const REBUILD: &str = "clausters-editor-rebuild";

fn base(base: Option<PathBuf>) -> PathBuf {
    base.unwrap_or_else(std::env::temp_dir)
}

/// The configuration (`config.json`, the preferences).
pub fn config_dir() -> PathBuf {
    base(dirs::config_dir()).join(NAME)
}

/// The environments (`venvs/`).
pub fn data_dir() -> PathBuf {
    base(dirs::data_dir()).join(NAME)
}

/// What is written again at every start (the session's driver).
pub fn cache_dir() -> PathBuf {
    base(dirs::cache_dir()).join(NAME)
}

/// **The webview application's folders, moved under the program's name**:
/// each one whose new folder does not exist yet. A virtual environment
/// names its own path in its scripts, so one moved with the data is marked
/// to be built again where it is now ([`REBUILD`]). What cannot be moved is
/// left where it is and said on the standard error.
pub fn migrate() {
    for base in [dirs::config_dir(), dirs::data_dir(), dirs::cache_dir()]
        .into_iter()
        .flatten()
    {
        let (old, new) = (base.join(OLD), base.join(NAME));
        if !old.is_dir() || new.exists() {
            continue;
        }
        match std::fs::rename(&old, &new) {
            Ok(()) => mark_moved_envs(&new.join("venvs")),
            Err(e) => eprintln!(
                "clausters-editor: {} was not moved to {}: {e}",
                old.display(),
                new.display()
            ),
        }
    }
}

/// Marks each environment in `venvs` to be built again.
fn mark_moved_envs(venvs: &Path) {
    let Ok(entries) = std::fs::read_dir(venvs) else {
        return;
    };
    for env in entries.flatten().filter(|e| e.path().is_dir()) {
        let _ = std::fs::write(env.path().join(REBUILD), "");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A moved environment is marked; a folder that is not one is left.
    #[test]
    fn a_moved_environment_is_marked() {
        let dir =
            std::env::temp_dir().join(format!("clausters-editor-paths-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("venvs/a")).unwrap();
        std::fs::write(dir.join("venvs/notes.txt"), "").unwrap();
        mark_moved_envs(&dir.join("venvs"));
        assert!(dir.join("venvs/a").join(REBUILD).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
