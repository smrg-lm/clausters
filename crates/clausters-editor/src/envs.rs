//! **Python virtual environments in the application's data folder**
//! (`~/.local/share/<identifier>/venvs` on Linux), the ones the webview
//! application keeps: created, installed into, chosen and deleted.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread;

use serde::Serialize;
use serde_json::json;

use crate::app::Msg;
use crate::config;
use crate::session::{self, Session};

/// Only one create/install operation at a time.
static BUSY: AtomicBool = AtomicBool::new(false);

/// **The one operation running**: taken before it starts, given back when it
/// is dropped, however the operation ends -- returned, failed or panicked.
struct Busy;

impl Busy {
    fn take() -> Option<Busy> {
        (!BUSY.swap(true, Ordering::SeqCst)).then_some(Busy)
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::SeqCst);
    }
}

#[derive(Serialize)]
pub struct EnvInfo {
    pub name: String,
    pub version: String,
    pub active: bool,
    /// The `pip install` arguments it was created with (empty if nothing was installed).
    pub source: String,
}

/// File, inside each environment, holding its install source.
const SOURCE_FILE: &str = "clausters-editor-source.txt";

/// The environment's source; those created before it was stored use the global one from the config.
fn source_of(dir: &Path) -> String {
    std::fs::read_to_string(dir.join(SOURCE_FILE))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| config::get().install_source)
}

fn envs_dir() -> PathBuf {
    crate::paths::data_dir().join("venvs")
}

fn env_dir(name: &str) -> Result<PathBuf, String> {
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
        && !name.starts_with('.');
    if !valid {
        return Err("Invalid environment name: only letters, digits, '.', '_' or '-'".into());
    }
    Ok(envs_dir().join(name))
}

pub fn python_in(dir: &Path) -> PathBuf {
    if cfg!(windows) {
        dir.join("Scripts").join("python.exe")
    } else {
        dir.join("bin").join("python")
    }
}

/// The active environment's interpreter, if it exists.
pub fn active_python() -> Option<(String, PathBuf)> {
    let name = config::get().active_env?;
    let python = python_in(&env_dir(&name).ok()?);
    python.exists().then_some((name, python))
}

fn version_of(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("pyvenv.cfg"))
        .ok()
        .and_then(|cfg| {
            cfg.lines()
                .filter_map(|l| l.split_once('='))
                .find(|(k, _)| matches!(k.trim(), "version" | "version_info"))
                .map(|(_, v)| v.trim().to_string())
        })
        .unwrap_or_default()
}

fn log(tx: &Sender<Msg>, text: impl Into<String>) {
    let _ = tx.send(Msg::Py(json!({ "type": "info", "text": text.into() })));
}

/// Runs a command, forwarding its output to the post window.
fn run(tx: &Sender<Msg>, program: &Path, args: &[String]) -> Result<(), String> {
    log(
        tx,
        format!("$ {} {}\n", program.display(), shell_words::join(args)),
    );
    let mut child = Command::new(program)
        .args(args)
        .env("PYTHONUNBUFFERED", "1")
        .env("PIP_DISABLE_PIP_VERSION_CHECK", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not run {}: {e}", program.display()))?;
    let forward = |stream: Box<dyn std::io::Read + Send>, kind: &'static str| {
        let tx = tx.clone();
        thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                let _ = tx.send(Msg::Py(json!({ "type": kind, "text": line + "\n" })));
            }
        })
    };
    let out = forward(Box::new(child.stdout.take().expect("piped")), "sys");
    let err = forward(Box::new(child.stderr.take().expect("piped")), "sys");
    let status = child.wait().map_err(|e| e.to_string())?;
    let _ = (out.join(), err.join());
    if status.success() {
        Ok(())
    } else {
        Err(format!("{} failed ({status})", program.display()))
    }
}

fn pip_install(tx: &Sender<Msg>, dir: &Path, source: &str) -> Result<(), String> {
    let mut args: Vec<String> = ["-m", "pip", "install"].map(String::from).to_vec();
    args.extend(shell_words::split(source).map_err(|e| e.to_string())?);
    // Jedi, beside whatever the source installs: the session's completions,
    // signatures and hover help come from it (driver.py).
    args.push("jedi".into());
    run(tx, &python_in(dir), &args)
}

/// Runs `task` on a thread of its own, marking the operation as busy, and
/// tells the application how it ended (`Msg::EnvsDone`).
fn exclusive(
    tx: &Sender<Msg>,
    task: impl FnOnce() -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    let busy = Busy::take().ok_or("An environment operation is already in progress")?;
    let tx = tx.clone();
    thread::spawn(move || {
        let _busy = busy;
        let result = task();
        let _ = tx.send(Msg::EnvsDone(result));
    });
    Ok(())
}

pub fn list_envs() -> Vec<EnvInfo> {
    let active = config::get().active_env;
    let dir = envs_dir();
    let mut envs: Vec<EnvInfo> = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| python_in(&e.path()).exists())
                .map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    EnvInfo {
                        version: version_of(&e.path()),
                        active: active.as_deref() == Some(name.as_str()),
                        source: source_of(&e.path()),
                        name,
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    envs.sort_by(|a, b| a.name.cmp(&b.name));
    envs
}

/// Creates the environment, installs `source` (`pip install` arguments) and makes it the active one.
pub fn create_env(
    tx: &Sender<Msg>,
    session: &Session,
    name: String,
    source: String,
) -> Result<(), String> {
    let dir = env_dir(&name)?;
    if dir.exists() {
        return Err(format!("An environment named '{name}' already exists"));
    }
    let source = source.trim().to_string();
    shell_words::split(&source).map_err(|e| format!("Invalid install source: {e}"))?;
    let (log_tx, session) = (tx.clone(), session.clone());
    exclusive(tx, move || {
        let tx = log_tx;
        log(&tx, format!("Creating the environment '{name}'...\n"));
        let base = PathBuf::from(session::default_python());
        let created = run(
            &tx,
            &base,
            &["-m".into(), "venv".into(), dir.to_string_lossy().into()],
        )
        .and_then(|_| std::fs::write(dir.join(SOURCE_FILE), &source).map_err(|e| e.to_string()))
        .and_then(|_| pip_install(&tx, &dir, &source));
        if let Err(e) = created {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }
        config::update(|c| c.active_env = Some(name.clone()))?;
        log(&tx, format!("Environment '{name}' ready.\n"));
        session.restart()
    })
}

/// Runs `pip install` again with the environment's source (to update the package).
pub fn install_in_env(tx: &Sender<Msg>, session: &Session, name: String) -> Result<(), String> {
    let dir = env_dir(&name)?;
    let (log_tx, session) = (tx.clone(), session.clone());
    exclusive(tx, move || {
        let source = source_of(&dir);
        pip_install(&log_tx, &dir, &source)?;
        log(&log_tx, "Installation finished.\n");
        // The active environment is restarted so that the session sees the updated package.
        if config::get().active_env.as_deref() == Some(name.as_str()) {
            session.restart()?;
        }
        Ok(())
    })
}

pub fn delete_env(session: &Session, name: String) -> Result<(), String> {
    if BUSY.load(Ordering::SeqCst) {
        return Err("An environment operation is in progress; wait for it to finish".into());
    }
    let dir = env_dir(&name)?;
    let was_active = config::get().active_env.as_deref() == Some(name.as_str());
    if was_active {
        config::update(|c| c.active_env = None)?;
        session.restart()?;
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("Could not delete {}: {e}", dir.display()))
}

/// Chooses the session's environment (`None` = the system Python) and restarts the session.
pub fn use_env(session: &Session, name: Option<String>) -> Result<(), String> {
    if let Some(n) = &name
        && !python_in(&env_dir(n)?).exists()
    {
        return Err(format!("The environment '{n}' does not exist"));
    }
    config::update(|c| c.active_env = name)?;
    session.restart()
}
