//! The persistent Python process that runs the code sent from the editor.
//!
//! Rust only carries messages: it writes JSON requests to the stdin of the
//! driver (`python/driver.py`) and forwards each line of its stdout to the
//! frontend as a `py-msg` event. Output on stderr (the driver's own errors,
//! output from C or from subprocesses) arrives as `sys` messages.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use std::thread;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};

const DRIVER: &str = include_str!("../python/driver.py");

struct Proc {
    child: Child,
    stdin: ChildStdin,
    generation: u64,
}

#[derive(Default)]
pub struct Session {
    proc: Mutex<Option<Proc>>,
    generation: AtomicU64,
    /// Current PID, outside the mutex: interrupting must not wait on a blocked write.
    pid: AtomicU32,
}

/// The session's interpreter and environment name: `CLAUSTERS_EDITOR_PYTHON`, else the active
/// environment, else the system Python.
fn python_exe(app: &AppHandle) -> (String, Option<String>) {
    if let Ok(exe) = std::env::var("CLAUSTERS_EDITOR_PYTHON") {
        return (exe, None);
    }
    if let Some((name, python)) = crate::envs::active_python(app) {
        return (python.to_string_lossy().into_owned(), Some(name));
    }
    (
        if cfg!(windows) { "python" } else { "python3" }.into(),
        None,
    )
}

impl Session {
    fn spawn(&self, app: &AppHandle) -> Result<Proc, String> {
        let dir = app.path().app_cache_dir().map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let driver = dir.join("driver.py");
        std::fs::write(&driver, DRIVER).map_err(|e| e.to_string())?;

        let (exe, env) = python_exe(app);
        let mut child = Command::new(&exe)
            .arg("-u")
            .arg(&driver)
            .env("PYTHONIOENCODING", "utf-8")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Could not start '{exe}': {e}"))?;

        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.pid.store(child.id(), Ordering::SeqCst);
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        let handle = app.clone();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let msg = serde_json::from_str::<Value>(&line)
                    .unwrap_or_else(|_| json!({ "type": "sys", "text": line + "\n" }));
                let _ = handle.emit("py-msg", msg);
            }
            // stdout closed: the process ended (or was restarted).
            let session = handle.state::<Session>();
            let mut guard = session.proc.lock().unwrap();
            let code = match guard.as_mut() {
                Some(p) if p.generation == generation => {
                    let code = p.child.wait().ok().and_then(|s| s.code());
                    *guard = None;
                    session.pid.store(0, Ordering::SeqCst);
                    code
                }
                _ => None,
            };
            let _ = handle.emit("py-exit", json!({ "generation": generation, "code": code }));
        });

        let handle = app.clone();
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let _ = handle.emit("py-msg", json!({ "type": "sys", "text": line + "\n" }));
            }
        });

        let _ = app.emit("py-start", json!({ "generation": generation, "env": env }));
        Ok(Proc {
            child,
            stdin,
            generation,
        })
    }

    fn send(&self, app: &AppHandle, request: Value) -> Result<(), String> {
        let mut line = request.to_string();
        line.push('\n');
        let mut guard = self.proc.lock().unwrap();
        if guard.is_none() {
            *guard = Some(self.spawn(app)?);
        }
        let ok = guard
            .as_mut()
            .unwrap()
            .stdin
            .write_all(line.as_bytes())
            .is_ok();
        if !ok {
            // Broken pipe: the process died. It is restarted and the write retried once.
            *guard = Some(self.spawn(app)?);
            guard
                .as_mut()
                .unwrap()
                .stdin
                .write_all(line.as_bytes())
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn kill(&self) {
        self.pid.store(0, Ordering::SeqCst);
        if let Some(mut p) = self.proc.lock().unwrap().take() {
            let _ = p.child.kill();
            let _ = p.child.wait();
        }
    }
}

#[tauri::command]
pub fn py_eval(
    app: AppHandle,
    session: State<Session>,
    id: u64,
    code: String,
    file: Option<String>,
    line: u32,
) -> Result<(), String> {
    session.send(
        &app,
        json!({ "id": id, "op": "eval", "code": code, "file": file, "line": line }),
    )
}

#[tauri::command]
pub fn py_help(
    app: AppHandle,
    session: State<Session>,
    id: u64,
    expr: String,
) -> Result<(), String> {
    session.send(&app, json!({ "id": id, "op": "help", "expr": expr }))
}

/// Signature and docstring of a name in the session (for the editor's tooltips).
/// The driver answers it even while code is running.
#[tauri::command]
pub fn py_inspect(
    app: AppHandle,
    session: State<Session>,
    id: u64,
    expr: String,
) -> Result<(), String> {
    session.send(&app, json!({ "id": id, "op": "inspect", "expr": expr }))
}

/// Ends the current session and starts a new one (with whichever interpreter applies now).
pub fn restart(app: &AppHandle) -> Result<(), String> {
    let session = app.state::<Session>();
    session.kill();
    let proc = session.spawn(app)?;
    *session.proc.lock().unwrap() = Some(proc);
    Ok(())
}

#[tauri::command]
pub fn py_restart(app: AppHandle) -> Result<(), String> {
    restart(&app)
}

#[tauri::command]
pub fn py_interrupt(session: State<Session>) -> Result<(), String> {
    let pid = session.pid.load(Ordering::SeqCst);
    if pid == 0 {
        return Ok(());
    }
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGINT) };
        Ok(())
    }
    #[cfg(not(unix))]
    {
        Err("Interrupting is not supported on this system; restart the session.".into())
    }
}
