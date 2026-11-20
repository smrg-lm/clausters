//! **The desktop's platform**: the session a Python process, the files the
//! disk's, the dialogs and the light or dark preference the desktop
//! portal's, the fonts the system's, and the environments folders of the
//! editor's data. Each answer that takes time runs on a thread of its own
//! and reaches the application as a message.

pub mod desktop;
pub mod envs;
pub mod fonts;
pub mod paths;
pub mod pkgdocs;
pub mod session;

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::Duration;

use serde_json::Value;

use crate::app::Msg;
use crate::platform::{self, EnvInfo, Language, Offers, Platform, Purpose, Read};
use session::Session;

/// The desktop's platform, telling the application on `tx`.
pub struct Native {
    tx: Sender<Msg>,
    session: Session,
}

impl Native {
    pub fn new(tx: Sender<Msg>) -> Self {
        Self {
            session: Session::new(tx.clone()),
            tx,
        }
    }

    /// `run` on a thread of its own, its answer told to the application.
    fn off_turn(&self, run: impl FnOnce() -> Msg + Send + 'static) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(run());
        });
    }
}

impl Platform for Native {
    fn language(&self) -> &'static Language {
        &platform::PYTHON
    }

    fn offers(&self) -> Offers {
        Offers {
            envs: true,
            docs_folder: true,
            fonts: true,
            quit: true,
            dialogs: true,
        }
    }

    fn start_session(&self) -> Result<(), String> {
        self.session.start()
    }

    fn send(&self, request: &Value) -> Result<(), String> {
        self.session.send(request)
    }

    /// The driver answers questions about code too, between evaluations.
    fn ask(&self, request: &Value) -> Result<(), String> {
        self.session.send(request)
    }

    fn interrupt(&self) -> Result<(), String> {
        self.session.interrupt()
    }

    fn restart(&self) -> Result<(), String> {
        self.session.restart()
    }

    fn stop(&self) {
        self.session.kill();
    }

    /// Read at once: a file a tab opens is small, and the answer still
    /// reaches the application on its next turn, as a page's does.
    fn read(&self, path: PathBuf, why: Read) {
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string());
        let _ = self.tx.send(Msg::Read { path, why, text });
    }

    fn write(&self, path: PathBuf, text: String, code: i32) {
        let done = std::fs::write(&path, &text).map_err(|e| e.to_string());
        let _ = self.tx.send(Msg::Written { path, code, done });
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn absolute(&self, path: &Path) -> PathBuf {
        std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
    }

    fn home(&self) -> PathBuf {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
    }

    fn choose(&self, purpose: Purpose, start: Option<PathBuf>) {
        desktop::choose(&self.tx, purpose, start);
    }

    fn follow_color_scheme(&self) {
        desktop::follow_color_scheme(&self.tx);
    }

    fn read_docs(&self) {
        self.off_turn(|| Msg::Docs(crate::docs::read_all()));
    }

    fn find_fonts(&self) {
        self.off_turn(|| Msg::Fonts(fonts::system_mono()));
    }

    fn font(&self, path: &Path) -> Vec<u8> {
        std::fs::read(path).unwrap_or_default()
    }

    /// In the desktop's browser.
    fn open_url(&self, url: &str) {
        let program = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        if let Err(e) = std::process::Command::new(program).arg(url).spawn() {
            tracing::warn!("cannot open {url}: {e}");
        }
    }

    fn later(&self, ms: u32, msg: Msg) {
        self.off_turn(move || {
            std::thread::sleep(Duration::from_millis(u64::from(ms)));
            msg
        });
    }

    fn default_interpreter(&self) -> String {
        session::default_python()
    }

    fn envs(&self) -> Vec<EnvInfo> {
        envs::list_envs()
    }

    fn use_env(&self, name: Option<String>) -> Result<(), String> {
        envs::use_env(&self.session, name)
    }

    fn install_in_env(&self, name: String) -> Result<(), String> {
        envs::install_in_env(&self.tx, &self.session, name)
    }

    fn delete_env(&self, name: String) -> Result<(), String> {
        envs::delete_env(&self.session, name)
    }

    fn create_env(&self, name: String, source: String) -> Result<(), String> {
        envs::create_env(&self.tx, &self.session, name, source)
    }
}
