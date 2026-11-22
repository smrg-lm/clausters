//! **What the editor asks of the program it runs in**: the session, the
//! files, the dialogs, the desktop's look, the documentation, the fonts and
//! the environments -- everything the application (`app`) does not decide
//! itself. The desktop's program answers with a process, the disk and the
//! portal (`native`); a page answers with a worker, its own storage and the
//! browser (`page`). The application is the same in both.
//!
//! Every answer that takes time arrives later as a [`Msg`], on the
//! application's turn, whichever platform gave it; one that does not is a
//! return value.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::app::Msg;

/// **The language the session runs**, and how the editor names it.
pub struct Language {
    /// What the footer and the messages call it.
    pub name: &'static str,
    /// The code element's `language`.
    pub code: &'static str,
    /// A new file's extension, without the dot.
    pub extension: &'static str,
    /// What the session is when no environment is chosen.
    pub system: &'static str,
}

/// The desktop's session: a Python process.
pub const PYTHON: Language = Language {
    name: "Python",
    code: "python",
    extension: "py",
    system: "System Python",
};

/// A page's session: JavaScript in a worker.
pub const JAVASCRIPT: Language = Language {
    name: "JavaScript",
    code: "javascript",
    extension: "mjs",
    system: "The page's worker",
};

/// **What a platform has beside what every one has**: what the menus and
/// the preferences offer only where it exists.
#[derive(Clone, Copy, Debug)]
pub struct Offers {
    /// Virtual environments to create, choose and delete.
    pub envs: bool,
    /// A documentation folder of the user's to choose.
    pub docs_folder: bool,
    /// The system's monospaced families, for the code's font.
    pub fonts: bool,
    /// A quit of the program's own (a page's tab is closed by the browser).
    pub quit: bool,
    /// **Copies between the disk and the storage**, where the storage is
    /// not the disk (a page's): its file dialog uploads and downloads.
    pub transfers: bool,
}

/// What a file dialog was opened for, handed back with what was chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// A file to open in a tab.
    Open,
    /// Where to save the tab of window `win`.
    SaveAs { win: i32 },
    /// The documentation folder.
    DocsFolder,
}

/// One environment, as the environments' dialog lists it.
#[derive(Clone, Debug, Serialize)]
pub struct EnvInfo {
    pub name: String,
    pub version: String,
    pub active: bool,
    /// The `pip install` arguments it was created with (empty if nothing
    /// was installed).
    pub source: String,
}

/// The face the host carries for code, which needs no file.
pub const BUNDLED: &str = "JetBrains Mono";

/// One monospaced family: its regular weight's file and its bold's.
#[derive(Clone, Debug)]
pub struct MonoFamily {
    pub name: String,
    pub regular: PathBuf,
    pub bold: Option<PathBuf>,
}

/// A change the editor's own file dialog makes to the storage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// A folder made, with the ones on its way.
    MakeFolder(PathBuf),
    /// A file or a folder given another path.
    Rename(PathBuf, PathBuf),
    /// A file, or a folder and everything in it, removed.
    Remove(PathBuf),
    /// Files from the disk copied into the folder, as the browser asks for
    /// them.
    Upload(PathBuf),
    /// A file handed to the browser to keep on the disk.
    Download(PathBuf),
}

/// A file read for a tab: why it was read, handed back with what was read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Read {
    /// Opened by a hand: it goes first in the recent files.
    Open,
    /// Opened again at start, as the last run left it.
    Reopen,
}

/// **The editor's platform.** See the module.
pub trait Platform {
    /// The language the session runs.
    fn language(&self) -> &'static Language;
    /// What this platform has that not every one does.
    fn offers(&self) -> Offers;

    // -- the session: its messages arrive as `Msg::Py`, its start and its
    // end as `Msg::PyStart` and `Msg::PyExit`

    /// Starts the session if it is not running.
    fn start_session(&self) -> Result<(), String>;
    /// One request to run (`eval`), the session started first if it is not
    /// running.
    fn send(&self, request: &Value) -> Result<(), String>;
    /// One question about code (`complete`, `signatures`, `inspect`,
    /// `help`), answered as a session's message with the request's id.
    fn ask(&self, request: &Value) -> Result<(), String>;
    /// Asks the running code to stop.
    fn interrupt(&self) -> Result<(), String>;
    /// Ends the session and starts another.
    fn restart(&self) -> Result<(), String>;
    /// Ends the session.
    fn stop(&self);

    // -- files

    /// Reads the file at `path` for a tab: `Msg::Read`.
    fn read(&self, path: PathBuf, why: Read);
    /// Writes `text` to the file at `path`, the save of the code `code`:
    /// `Msg::Written`.
    fn write(&self, path: PathBuf, text: String, code: i32);
    /// Whether there is anything at `path` now.
    fn exists(&self, path: &Path) -> bool;
    /// Whether `path` is a folder now.
    fn is_dir(&self, path: &Path) -> bool;
    /// A path made absolute against where the program runs.
    fn absolute(&self, path: &Path) -> PathBuf {
        path.to_path_buf()
    }
    /// The user's folder, which the recent files are written from (`~`),
    /// empty where there is none.
    fn home(&self) -> PathBuf;
    /// Asks the platform's dialog for the documentation folder, a
    /// preference: `Msg::Chosen`. Only asked where it
    /// [`Offers::docs_folder`]; a file is opened and saved in the editor's
    /// own file dialog.
    fn choose(&self, purpose: Purpose, start: Option<PathBuf>);
    /// Makes a change to the storage, asked by the editor's file dialog:
    /// `Msg::Changed`.
    fn change(&self, change: Change);

    // -- the look and the documentation

    /// Follows the desktop's light or dark preference: what it is now and
    /// every change, as `Msg::SystemDark`.
    fn follow_color_scheme(&self);
    /// Reads every book and every document: `Msg::Docs`.
    fn read_docs(&self);
    /// Finds the system's monospaced families: `Msg::Fonts`.
    fn find_fonts(&self);
    /// The bytes of a face's file, empty when it cannot be read.
    fn font(&self, path: &Path) -> Vec<u8>;
    /// Opens a web address where the platform opens one.
    fn open_url(&self, url: &str);
    /// `msg` on the application's turn after `ms` milliseconds.
    fn later(&self, ms: u32, msg: Msg);

    // -- the environments, where it [`Offers::envs`]

    /// The interpreter used when no environment is chosen.
    fn default_interpreter(&self) -> String {
        String::new()
    }
    /// The environments, in their folder's order.
    fn envs(&self) -> Vec<EnvInfo> {
        Vec::new()
    }
    /// The session uses environment `name`, or the system's with `None`.
    fn use_env(&self, _name: Option<String>) -> Result<(), String> {
        Err(NO_ENVS.into())
    }
    /// Installs the environment's source in it again: `Msg::EnvsDone`.
    fn install_in_env(&self, _name: String) -> Result<(), String> {
        Err(NO_ENVS.into())
    }
    /// Deletes the environment.
    fn delete_env(&self, _name: String) -> Result<(), String> {
        Err(NO_ENVS.into())
    }
    /// Creates the environment and installs `source` in it: `Msg::EnvsDone`.
    fn create_env(&self, _name: String, _source: String) -> Result<(), String> {
        Err(NO_ENVS.into())
    }
}

const NO_ENVS: &str = "This editor keeps no environments.";
