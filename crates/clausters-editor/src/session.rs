//! **The persistent Python process that runs the code sent from the editor.**
//!
//! The driver (`python/driver.py` of the editor's webview application, the
//! one session both applications run) reads one JSON request per line on its
//! stdin and writes one JSON message per line on its stdout. This only
//! carries them: each line of stdout reaches the application as a driver
//! message, and output on stderr (the driver's own errors, output from C or
//! from subprocesses) as a `sys` one.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;

use serde_json::{Value, json};

use crate::app::Msg;
use crate::paths;

const DRIVER: &str = include_str!("../../../clausters-editor/src-tauri/python/driver.py");

struct Proc {
    child: Child,
    stdin: ChildStdin,
    generation: u64,
}

/// The session: at most one process, started when it is first needed.
#[derive(Clone)]
pub struct Session {
    inner: Arc<Inner>,
}

struct Inner {
    proc: Mutex<Option<Proc>>,
    generation: AtomicU64,
    /// The process's id, outside the mutex: interrupting must not wait on a
    /// blocked write.
    pid: AtomicU32,
    tx: Sender<Msg>,
}

/// The interpreter used when no environment is active, and the one new
/// environments are created from: `CLAUSTERS_EDITOR_DEFAULT_PYTHON`, else the
/// system's Python.
pub fn default_python() -> String {
    std::env::var("CLAUSTERS_EDITOR_DEFAULT_PYTHON")
        .ok()
        .filter(|exe| !exe.is_empty())
        .unwrap_or_else(|| if cfg!(windows) { "python" } else { "python3" }.into())
}

/// The session's interpreter and its environment's name:
/// `CLAUSTERS_EDITOR_PYTHON`, else the active environment, else the default
/// Python.
pub fn python_exe() -> (String, Option<String>) {
    if let Ok(exe) = std::env::var("CLAUSTERS_EDITOR_PYTHON") {
        return (exe, None);
    }
    if let Some((name, python)) = crate::envs::active_python() {
        return (python.to_string_lossy().into_owned(), Some(name));
    }
    (default_python(), None)
}

impl Session {
    pub fn new(tx: Sender<Msg>) -> Self {
        Self {
            inner: Arc::new(Inner {
                proc: Mutex::new(None),
                generation: AtomicU64::new(0),
                pid: AtomicU32::new(0),
                tx,
            }),
        }
    }

    /// The process slot, taken whether or not a thread panicked holding it.
    fn slot(&self) -> MutexGuard<'_, Option<Proc>> {
        self.inner
            .proc
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Ends `proc` and waits for it, so it leaves no zombie behind.
    fn end(&self, mut proc: Proc) {
        self.inner.pid.store(0, Ordering::SeqCst);
        let _ = proc.child.kill();
        let _ = proc.child.wait();
    }

    fn spawn(&self) -> Result<Proc, String> {
        // A new session may import other packages: their books are asked again.
        crate::pkgdocs::forget();
        let dir = paths::cache_dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let driver = dir.join("driver.py");
        std::fs::write(&driver, DRIVER).map_err(|e| e.to_string())?;

        let (exe, env) = python_exe();
        let mut child = Command::new(&exe)
            .arg("-u")
            .arg(&driver)
            .env("PYTHONIOENCODING", "utf-8")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Could not start '{exe}': {e}"))?;

        let generation = self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.inner.pid.store(child.id(), Ordering::SeqCst);
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");

        let session = self.clone();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let msg = serde_json::from_str::<Value>(&line)
                    .unwrap_or_else(|_| json!({ "type": "sys", "text": line + "\n" }));
                let _ = session.inner.tx.send(Msg::Py(msg));
            }
            // stdout closed: the process ended (or was restarted).
            let code = {
                let mut guard = session.slot();
                match guard.as_mut() {
                    Some(p) if p.generation == generation => {
                        let code = p.child.wait().ok().and_then(|s| s.code());
                        *guard = None;
                        session.inner.pid.store(0, Ordering::SeqCst);
                        code
                    }
                    _ => None,
                }
            };
            let _ = session.inner.tx.send(Msg::PyExit { generation, code });
        });

        let tx = self.inner.tx.clone();
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let _ = tx.send(Msg::Py(json!({ "type": "sys", "text": line + "\n" })));
            }
        });

        let _ = self.inner.tx.send(Msg::PyStart { generation, env });
        Ok(Proc {
            child,
            stdin,
            generation,
        })
    }

    /// One request, the process started (or started again) first if it is
    /// not running.
    pub fn send(&self, request: &Value) -> Result<(), String> {
        let mut line = request.to_string();
        line.push('\n');
        let mut guard = self.slot();
        if guard.is_none() {
            *guard = Some(self.spawn()?);
        }
        let ok = guard
            .as_mut()
            .is_some_and(|p| p.stdin.write_all(line.as_bytes()).is_ok());
        if !ok {
            // Broken pipe: the process died. It is waited for, restarted, and
            // the write tried once more.
            if let Some(dead) = guard.take() {
                self.end(dead);
            }
            let proc = guard.insert(self.spawn()?);
            proc.stdin
                .write_all(line.as_bytes())
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Starts the process if it is not running.
    pub fn start(&self) -> Result<(), String> {
        let mut guard = self.slot();
        if guard.is_none() {
            *guard = Some(self.spawn()?);
        }
        Ok(())
    }

    /// Ends the process and starts another (with whichever interpreter
    /// applies now). The slot stays locked from one to the other, so a
    /// request in between waits for the new interpreter instead of starting
    /// one of its own.
    pub fn restart(&self) -> Result<(), String> {
        let mut slot = self.slot();
        if let Some(old) = slot.take() {
            self.end(old);
        }
        *slot = Some(self.spawn()?);
        Ok(())
    }

    /// Asks the running code to stop: a `KeyboardInterrupt` in it, and the
    /// packages' interrupt hooks.
    pub fn interrupt(&self) -> Result<(), String> {
        let pid = self.inner.pid.load(Ordering::SeqCst);
        if pid == 0 {
            return Ok(());
        }
        #[cfg(unix)]
        {
            // SAFETY: a signal sent to a process id this session started.
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGINT) };
            Ok(())
        }
        #[cfg(not(unix))]
        {
            Err("Interrupting is not supported on this system; restart the session.".into())
        }
    }

    pub fn kill(&self) {
        let taken = self.slot().take();
        if let Some(p) = taken {
            self.end(p);
        }
    }
}
