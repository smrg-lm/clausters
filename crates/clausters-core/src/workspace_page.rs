//! **An editor's working folder in a page**: the native module's surface
//! (`workspace.rs`), over the origin's storage.
//!
//! Natively a folder is held by a file lock beside it, which the operating
//! system lets go of however the process ends. A page has no file lock; it
//! has Web Locks, which the browser lets go of however the tab goes. So a
//! folder here is held by one, named after the folder, and a sweep removes
//! every folder no tab holds or waits for. Both are calls of the file layer
//! (`crate::files`), made from the application's thread, and the page's file
//! worker holds the locks for as long as the tab lives.

use std::io;
use std::path::{Path, PathBuf};

/// **Where working folders are made in a page**: under the origin's
/// storage, as the web client's own folders are.
pub fn root() -> PathBuf {
    PathBuf::from("clausters/work")
}

/// **A working folder, held**: its folder is removed when this is dropped,
/// and by the next [`sweep`] when its tab went without dropping it.
#[derive(Debug)]
pub struct Workspace {
    path: PathBuf,
}

impl Workspace {
    /// **A new working folder named after `name`** under [`root`], after
    /// removing the folders no tab holds any longer.
    pub fn create(name: &str) -> io::Result<Workspace> {
        Workspace::create_in(&root(), name)
    }

    /// [`Workspace::create`] under `root`.
    pub fn create_in(root: &Path, name: &str) -> io::Result<Workspace> {
        crate::files::create_dir_all(root)?;
        sweep(root);
        let path = crate::files::hold_new(root, name)?;
        Ok(Workspace {
            path: PathBuf::from(path),
        })
    }

    /// The folder.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        // nothing to report to: what could not go is swept later, once the
        // tab is gone and its lock with it
        if crate::files::remove_dir_all(&self.path).is_ok() || !crate::files::exists(&self.path) {
            let _ = crate::files::release(&self.path);
        }
    }
}

/// **Removes every working folder under `root` no tab holds**; answers how
/// many went.
pub fn sweep(root: &Path) -> usize {
    crate::files::sweep(root).unwrap_or(0)
}
