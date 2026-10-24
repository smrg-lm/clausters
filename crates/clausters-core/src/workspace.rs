//! **An editor's working folder**: where its renders are written, on disk,
//! and gone when the session that made it is.
//!
//! An editor writes a file for every edit that makes samples (a reverse, a
//! gain, a stroke), and a take of a day reversed writes tens of gigabytes. The
//! folders were made in the system's temporary directory, which on many
//! machines is a memory filesystem: the renders filled the memory, and a
//! session that did not end through its window's close left them there for
//! good, holding gigabytes of memory after every process had gone.
//!
//! So a folder lives under the user's cache ([`root`]), on disk, and it is
//! held: the session keeps a lock on a file beside it, which the operating
//! system lets go of when the process ends however it ends, on every
//! platform the project builds for. Dropping a
//! [`Workspace`] removes its folder, and making one first removes every folder
//! whose lock nobody holds ([`sweep`]): what a session killed outright left
//! behind goes the next time any editor opens.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Env var naming the folder the working folders are made in.
pub const CACHE_DIR_ENV: &str = "CLAUSTERS_CACHE_DIR";

/// The extension of the file beside a working folder its session holds
/// locked. Beside it and not inside, since a folder with a file open in it
/// cannot be removed on every platform.
const LOCK: &str = "lock";

/// **How old an unheld lock must be before a sweep takes its folder.** A
/// session creates its lock file and locks it a moment later, and a sweep in
/// between must not read that moment as a dead session.
const SETTLED: Duration = Duration::from_secs(60);

/// **Where working folders are made**: `$CLAUSTERS_CACHE_DIR` where it is
/// set, else the platform's cache folder -- `%LOCALAPPDATA%` on Windows,
/// `~/Library/Caches` on macOS, `$XDG_CACHE_HOME` or `~/.cache` elsewhere --
/// under `clausters/work`, and the system's temporary directory only where
/// none of those can be found.
pub fn root() -> PathBuf {
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
    if let Some(dir) = var(CACHE_DIR_ENV) {
        return PathBuf::from(dir);
    }
    let cache = if cfg!(windows) {
        var("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|home| PathBuf::from(home).join("Library/Caches"))
    } else {
        var("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| var("HOME").map(|home| PathBuf::from(home).join(".cache")))
    };
    match cache {
        Some(cache) => cache.join("clausters").join("work"),
        None => std::env::temp_dir().join("clausters-work"),
    }
}

/// **A working folder, held**: its folder is removed when this is dropped,
/// and by the next [`sweep`] when its process ended without dropping it.
#[derive(Debug)]
pub struct Workspace {
    path: PathBuf,
    /// The file that says the folder is in use, locked as long as this is.
    lock: Option<File>,
}

impl Workspace {
    /// **A new working folder named after `name`** under [`root`], after
    /// removing the folders no session holds any longer.
    pub fn create(name: &str) -> io::Result<Workspace> {
        Workspace::create_in(&root(), name)
    }

    /// [`Workspace::create`] under `root`.
    pub fn create_in(root: &Path, name: &str) -> io::Result<Workspace> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        fs::create_dir_all(root)?;
        sweep(root);
        let pid = std::process::id();
        loop {
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = root.join(format!("{name}-{pid}-{n}"));
            // the lock first, so the folder is never there unheld
            let lock = match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path.with_extension(LOCK))
            {
                Ok(lock) => lock,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            };
            lock.try_lock().map_err(io::Error::other)?;
            match fs::create_dir(&path) {
                Ok(()) => {
                    return Ok(Workspace {
                        path,
                        lock: Some(lock),
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    drop(lock);
                    let _ = fs::remove_file(path.with_extension(LOCK));
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// The folder.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        // nothing to report to: what could not go is swept later
        let _ = fs::remove_dir_all(&self.path);
        // closed before it is removed, which some platforms require
        drop(self.lock.take());
        let _ = fs::remove_file(self.path.with_extension(LOCK));
    }
}

/// **Removes every working folder under `root` no session holds**: one whose
/// lock file is there, settled, and can be locked, its process having ended,
/// which lets go of a lock however it ends. A folder with no lock file beside
/// it is not a working folder and is left alone. Answers how many went.
pub fn sweep(root: &Path) -> usize {
    let Ok(entries) = fs::read_dir(root) else {
        return 0;
    };
    let mut gone = 0;
    for entry in entries.flatten() {
        let lock_path = entry.path();
        if lock_path.extension().is_none_or(|e| e != LOCK) {
            continue;
        }
        let settled = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|at| at.elapsed().ok())
            .is_some_and(|age| age >= SETTLED);
        if !settled {
            continue;
        }
        let Ok(lock) = OpenOptions::new().write(true).open(&lock_path) else {
            continue;
        };
        if lock.try_lock().is_err() {
            continue;
        }
        let folder = lock_path.with_extension("");
        let removed = !folder.exists() || fs::remove_dir_all(&folder).is_ok();
        drop(lock);
        if removed && fs::remove_file(&lock_path).is_ok() {
            gone += 1;
        }
    }
    gone
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("clausters-ws-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_workspace_goes_when_it_is_dropped() {
        let root = scratch("drop");
        let ws = Workspace::create_in(&root, "audio").unwrap();
        let path = ws.path().to_path_buf();
        fs::write(path.join("take.w64"), b"samples").unwrap();
        assert!(path.is_dir());
        drop(ws);
        assert!(!path.exists());
        assert!(!path.with_extension(LOCK).exists(), "its lock goes too");
        let _ = fs::remove_dir_all(&root);
    }

    /// A lock file left by a session that ended, as old as a sweep needs.
    fn left_behind(root: &Path, name: &str) -> PathBuf {
        let dead = root.join(name);
        fs::create_dir_all(&dead).unwrap();
        fs::write(dead.join("reversed.w64"), b"samples").unwrap();
        let lock = File::create(dead.with_extension(LOCK)).unwrap();
        let old = std::time::SystemTime::now() - SETTLED * 2;
        lock.set_modified(old).unwrap();
        dead
    }

    #[test]
    fn a_sweep_takes_what_no_session_holds_and_leaves_what_one_does() {
        let root = scratch("sweep");
        let held = Workspace::create_in(&root, "audio").unwrap();
        let dead = left_behind(&root, "audio-1-0");
        // a lock just made is a session starting, not one that ended
        let young = root.join("audio-2-0");
        fs::create_dir_all(&young).unwrap();
        File::create(young.with_extension(LOCK)).unwrap();
        // and something that is not a working folder
        let other = root.join("notes");
        fs::create_dir_all(&other).unwrap();
        assert_eq!(sweep(&root), 1);
        assert!(!dead.exists(), "the dead session's folder went");
        assert!(!dead.with_extension(LOCK).exists(), "and its lock");
        assert!(held.path().is_dir(), "the live one stayed");
        assert!(young.is_dir(), "a lock too young to judge stayed");
        assert!(other.is_dir(), "a folder with no lock is not a workspace");
        // a new workspace sweeps on its way in
        let dead = left_behind(&root, "audio-3-0");
        let second = Workspace::create_in(&root, "audio").unwrap();
        assert!(!dead.exists());
        assert_ne!(second.path(), held.path());
        drop((held, second));
        let _ = fs::remove_dir_all(&root);
    }
}
