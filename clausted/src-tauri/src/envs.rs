//! Python virtual environments in the app's data directory
//! (~/.local/share/<identifier>/venvs on Linux).

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

use crate::{config, session};

/// Only one create/install operation at a time.
static BUSY: AtomicBool = AtomicBool::new(false);

#[derive(Serialize)]
pub struct EnvInfo {
    name: String,
    version: String,
    active: bool,
    /// The `pip install` arguments it was created with (empty if nothing was installed).
    source: String,
}

/// File, inside each environment, holding its install source.
const SOURCE_FILE: &str = "clausted-source.txt";

/// The environment's source; those created before it was stored use the global one from the config.
fn source_of(app: &AppHandle, dir: &Path) -> String {
    std::fs::read_to_string(dir.join(SOURCE_FILE))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| config::get(app).install_source)
}

fn envs_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("venvs"))
}

fn env_dir(app: &AppHandle, name: &str) -> Result<PathBuf, String> {
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
        && !name.starts_with('.');
    if !valid {
        return Err("Invalid environment name: only letters, digits, '.', '_' or '-'".into());
    }
    Ok(envs_dir(app)?.join(name))
}

pub fn python_in(dir: &Path) -> PathBuf {
    if cfg!(windows) {
        dir.join("Scripts").join("python.exe")
    } else {
        dir.join("bin").join("python")
    }
}

/// The active environment's interpreter, if it exists.
pub fn active_python(app: &AppHandle) -> Option<(String, PathBuf)> {
    let name = config::get(app).active_env?;
    let python = python_in(&env_dir(app, &name).ok()?);
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

fn log(app: &AppHandle, text: impl Into<String>) {
    let _ = app.emit("py-msg", json!({ "type": "info", "text": text.into() }));
}

/// Runs a command, forwarding its output to the post window.
fn run(app: &AppHandle, program: &Path, args: &[String]) -> Result<(), String> {
    log(
        app,
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
        let app = app.clone();
        thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                let _ = app.emit("py-msg", json!({ "type": kind, "text": line + "\n" }));
            }
        })
    };
    let out = forward(Box::new(child.stdout.take().unwrap()), "sys");
    let err = forward(Box::new(child.stderr.take().unwrap()), "sys");
    let status = child.wait().map_err(|e| e.to_string())?;
    let _ = (out.join(), err.join());
    if status.success() {
        Ok(())
    } else {
        Err(format!("{} failed ({status})", program.display()))
    }
}

fn pip_install(app: &AppHandle, dir: &Path, source: &str) -> Result<(), String> {
    if source.is_empty() {
        log(
            app,
            "No install source configured: no package is installed.\n",
        );
        return Ok(());
    }
    let mut args: Vec<String> = ["-m", "pip", "install"].map(String::from).to_vec();
    args.extend(shell_words::split(source).map_err(|e| e.to_string())?);
    run(app, &python_in(dir), &args)
}

/// Runs `task` on a separate thread, marking the operation as busy.
async fn exclusive<T: Send + 'static>(
    task: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    if BUSY.swap(true, Ordering::SeqCst) {
        return Err("An environment operation is already in progress".into());
    }
    let result = tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|e| e.to_string());
    BUSY.store(false, Ordering::SeqCst);
    result?
}

#[tauri::command]
pub fn list_envs(app: AppHandle) -> Result<Vec<EnvInfo>, String> {
    let active = config::get(&app).active_env;
    let dir = envs_dir(&app)?;
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
                        source: source_of(&app, &e.path()),
                        name,
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    envs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(envs)
}

/// Creates the environment, installs `source` (`pip install` arguments) and makes it the active one.
#[tauri::command]
pub async fn create_env(app: AppHandle, name: String, source: String) -> Result<(), String> {
    let dir = env_dir(&app, &name)?;
    if dir.exists() {
        return Err(format!("An environment named '{name}' already exists"));
    }
    let source = source.trim().to_string();
    shell_words::split(&source).map_err(|e| format!("Invalid install source: {e}"))?;
    exclusive(move || {
        log(&app, format!("Creating the environment '{name}'...\n"));
        let base = PathBuf::from(if cfg!(windows) { "python" } else { "python3" });
        let created = run(
            &app,
            &base,
            &["-m".into(), "venv".into(), dir.to_string_lossy().into()],
        )
        .and_then(|_| std::fs::write(dir.join(SOURCE_FILE), &source).map_err(|e| e.to_string()))
        .and_then(|_| pip_install(&app, &dir, &source));
        if let Err(e) = created {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }
        config::update(&app, |c| c.active_env = Some(name.clone()))?;
        log(&app, format!("Environment '{name}' ready.\n"));
        session::restart(&app)
    })
    .await
}

/// Runs `pip install` again with the environment's source (to update the package).
#[tauri::command]
pub async fn install_in_env(app: AppHandle, name: String) -> Result<(), String> {
    let dir = env_dir(&app, &name)?;
    exclusive(move || {
        let source = source_of(&app, &dir);
        pip_install(&app, &dir, &source)?;
        log(&app, "Installation finished.\n");
        // The active environment is restarted so that the session sees the updated package.
        if config::get(&app).active_env.as_deref() == Some(name.as_str()) {
            session::restart(&app)?;
        }
        Ok(())
    })
    .await
}

#[tauri::command]
pub fn delete_env(app: AppHandle, name: String) -> Result<(), String> {
    if BUSY.load(Ordering::SeqCst) {
        return Err("An environment operation is in progress; wait for it to finish".into());
    }
    let dir = env_dir(&app, &name)?;
    let was_active = config::get(&app).active_env.as_deref() == Some(name.as_str());
    if was_active {
        config::update(&app, |c| c.active_env = None)?;
        session::restart(&app)?;
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("Could not delete {}: {e}", dir.display()))
}

/// Chooses the session's environment (`None` = the system Python) and restarts the session.
#[tauri::command]
pub fn use_env(app: AppHandle, name: Option<String>) -> Result<(), String> {
    if let Some(n) = &name {
        if !python_in(&env_dir(&app, n)?).exists() {
            return Err(format!("The environment '{n}' does not exist"));
        }
    }
    config::update(&app, |c| c.active_env = name)?;
    session::restart(&app)
}
