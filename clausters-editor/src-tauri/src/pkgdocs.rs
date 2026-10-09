//! The books the packages of the session's environment carry for the editor.
//!
//! A package registers one with an entry point in the `clausters_editor.docs` group,
//! named after the package: a callable that returns a folder of `.md` files, or
//! `None`. The clausters client registers its book this way. The session's
//! interpreter is asked, since the packages are the ones it imports; the answer is
//! kept until the session starts again, which is when an install or another
//! environment can change it.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;

use tauri::AppHandle;

/// Lists the group's entry points and prints `{name: folder}` as JSON.
const SCRIPT: &str = r#"
import json
from importlib.metadata import entry_points
eps = entry_points()
eps = eps.select(group="clausters_editor.docs") if hasattr(eps, "select") else eps.get("clausters_editor.docs", [])
books = {}
for ep in eps:
    try:
        folder = ep.load()()
    except Exception:
        continue
    if folder:
        books[ep.name] = str(folder)
print(json.dumps(books))
"#;

/// A package's name and its book's folder.
pub type Book = (String, PathBuf);

/// The interpreter asked last and its answer.
static CACHE: Mutex<Option<(String, Vec<Book>)>> = Mutex::new(None);

/// The books of the session's environment, by package name, sorted.
pub fn books(app: &AppHandle) -> Vec<Book> {
    let (exe, _) = crate::session::python_exe(app);
    let mut cache = CACHE.lock().unwrap();
    if let Some((asked, books)) = cache.as_ref() {
        if *asked == exe {
            return books.clone();
        }
    }
    let books = ask(&exe);
    *cache = Some((exe, books.clone()));
    books
}

/// Forgets the answer, so that the next listing asks again.
pub fn forget() {
    *CACHE.lock().unwrap() = None;
}

fn ask(exe: &str) -> Vec<Book> {
    // Run from the temporary folder, so that no file in the editor's working
    // directory shadows a module the script imports.
    let Ok(out) = Command::new(exe)
        .arg("-c")
        .arg(SCRIPT)
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    serde_json::from_slice::<BTreeMap<String, String>>(&out.stdout)
        .map(|books| {
            books
                .into_iter()
                .map(|(name, dir)| (name, PathBuf::from(dir)))
                .collect()
        })
        .unwrap_or_default()
}
