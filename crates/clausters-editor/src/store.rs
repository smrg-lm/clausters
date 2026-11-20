//! **Where the editor keeps its configuration and its preferences**, by a
//! file's name: the configuration folder on the desktop, the page's local
//! storage in a page. Both answer at once, so what is kept is read and
//! written on the application's turn.

/// The text kept under `name`, if any.
pub fn read(name: &str) -> Option<String> {
    imp::read(name)
}

/// Keeps `text` under `name`.
pub fn write(name: &str, text: &str) -> Result<(), String> {
    imp::write(name, text)
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    pub fn read(name: &str) -> Option<String> {
        std::fs::read_to_string(crate::native::paths::config_dir().join(name)).ok()
    }

    pub fn write(name: &str, text: &str) -> Result<(), String> {
        let path = crate::native::paths::config_dir().join(name);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(path, text).map_err(|e| e.to_string())
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    /// What a page's keys start with, so the editor's are its own.
    const PREFIX: &str = "clausters-editor:";

    fn storage() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok().flatten()
    }

    pub fn read(name: &str) -> Option<String> {
        storage()?
            .get_item(&format!("{PREFIX}{name}"))
            .ok()
            .flatten()
    }

    pub fn write(name: &str, text: &str) -> Result<(), String> {
        let storage = storage().ok_or("The page keeps no local storage.")?;
        storage
            .set_item(&format!("{PREFIX}{name}"), text)
            .map_err(|_| "The page's local storage is full or refused.".to_string())
    }
}
