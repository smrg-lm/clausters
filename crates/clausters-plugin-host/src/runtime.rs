//! Linux helper supervision, isolated discovery and administrative instance control.
//!
//! These operations are non-RT. An instance's borrowed producer alone is RT-safe;
//! its owning instance must be retired and dropped outside the audio callback.

use crate::format::*;
use crate::mapping::Mapping;
use crate::protocol::{self, Handshake, Request, Response};
use clausters_core::plugin_shm::{Config, Failure, Producer};
#[cfg(any(feature = "clap", feature = "vst3"))]
use clausters_core::plugin_shm::{EventList, FRAMES};
use std::collections::HashSet;
use std::io::ErrorKind;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

/// A scan/load/admin request may take this long before its helper is killed.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);
/// Most candidate binaries inspected in one scan.
pub const MAX_CANDIDATES: usize = 128;
static NEXT: AtomicU64 = AtomicU64::new(1);

/// Find the matching executable, allowing an explicit development override.
pub fn helper_path() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("CLAUSTERS_PLUGIN_HOST") {
        return Ok(path.into());
    }
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let adjacent = executable
        .parent()
        .ok_or("executable has no directory")?
        .join("clausters-plugin-host");
    if adjacent.is_file() {
        return Ok(adjacent);
    }
    // An embedded server runs inside another executable. Resolve beside the
    // loaded library, including the Python wheel's _libs/ and _bin/ pair.
    // SAFETY: dladdr inspects this live function address and returns borrowed paths.
    unsafe {
        let mut info: libc::Dl_info = std::mem::zeroed();
        if libc::dladdr(helper_path as *const () as *const libc::c_void, &mut info) != 0
            && !info.dli_fname.is_null()
        {
            let library = PathBuf::from(
                std::ffi::CStr::from_ptr(info.dli_fname)
                    .to_string_lossy()
                    .into_owned(),
            );
            if let Some(directory) = library.parent() {
                for candidate in [
                    directory.join("clausters-plugin-host"),
                    directory.join("../_bin/clausters-plugin-host"),
                    directory.join("../bin/clausters-plugin-host"),
                ] {
                    if candidate.is_file() {
                        return Ok(candidate);
                    }
                }
            }
        }
    }
    let workspace =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/clausters-plugin-host");
    if workspace.is_file() {
        return Ok(workspace);
    }
    Err("plugin helper not found; set CLAUSTERS_PLUGIN_HOST".into())
}
struct Helper {
    child: Child,
    socket: UnixStream,
    timeout: Duration,
}
impl Helper {
    fn spawn(executable: &Path, timeout: Duration) -> Result<Self, String> {
        let (parent, child_socket) = UnixStream::pair().map_err(|e| e.to_string())?;
        parent
            .set_read_timeout(Some(timeout))
            .map_err(|e| e.to_string())?;
        parent
            .set_write_timeout(Some(timeout))
            .map_err(|e| e.to_string())?;
        let fd = child_socket.as_raw_fd();
        let mut command = Command::new(executable);
        command
            .arg("--control-fd")
            .arg(fd.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: pre_exec only calls async-signal-safe fcntl and does not allocate.
        // All other inherited descriptors keep CLOEXEC; plugins cannot corrupt
        // the administrative protocol by printing to stdout.
        unsafe {
            command.pre_exec(move || {
                if libc::fcntl(fd, libc::F_SETFD, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().map_err(|e| format!("plugin helper: {e}"))?;
        drop(child_socket);
        let mut helper = Self {
            child,
            socket: parent,
            timeout,
        };
        let handshake: Handshake = helper
            .read()
            .map_err(|e| format!("incompatible plugin helper handshake: {e}"))?;
        if handshake != Handshake::current() {
            return Err("incompatible plugin helper handshake".into());
        }
        Ok(helper)
    }
    fn read<T: serde::de::DeserializeOwned>(&mut self) -> Result<T, String> {
        protocol::receive(&mut self.socket).map_err(|error| {
            if self.child.try_wait().ok().flatten().is_some() {
                format!("plugin helper crashed/exited: {error}")
            } else {
                format!("plugin helper timeout/protocol error: {error}")
            }
        })
    }
    fn request(&mut self, request: Request) -> Result<Response, String> {
        self.socket
            .set_read_timeout(Some(self.timeout))
            .map_err(|e| e.to_string())?;
        protocol::send(&mut self.socket, &request)?;
        let result = self.read()?;
        if let Response::Error(error) = result {
            return Err(error);
        }
        Ok(result)
    }
}
impl Drop for Helper {
    fn drop(&mut self) {
        // Never join a hung plugin. kill/wait and descriptor cleanup are non-RT.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Probe one stable identity without enumerating unrelated plugins in its binary.
pub fn probe(
    executable: &Path,
    path: &Path,
    id: &str,
    timeout: Duration,
) -> Result<PluginInfo, String> {
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    let mut helper = Helper::spawn(executable, timeout)?;
    let Response::Metadata(info) = helper.request(Request::Probe {
        path: path.to_string_lossy().into_owned(),
        id: id.into(),
    })?
    else {
        return Err("invalid plugin probe response".into());
    };
    info.validate()?;
    if info.id != id || info.path != path.to_string_lossy() {
        return Err("plugin probe identity mismatch".into());
    }
    Ok(*info)
}

/// Parent-side per-instance owner. Retire/drop it only off the audio thread.
pub struct Instance {
    helper: Helper,
    mapping: Mapping,
    metadata: PluginInfo,
    producer_claimed: bool,
}
impl Instance {
    /// Load and activate a single identity with exactly these negotiated capacities.
    pub fn load(
        executable: &Path,
        path: &Path,
        id: &str,
        config: Config,
        timeout: Duration,
    ) -> Result<Self, String> {
        Self::load_with_state(executable, path, id, config, timeout, None)
    }
    /// Load initial state before activation so ports, parameters and latency
    /// are negotiated from the restored instance, not stale processor metadata.
    pub fn load_with_state(
        executable: &Path,
        path: &Path,
        id: &str,
        config: Config,
        timeout: Duration,
        state: Option<&[u8]>,
    ) -> Result<Self, String> {
        if state.is_some_and(|bytes| bytes.len() > MAX_STATE_BYTES) {
            return Err("plugin state capacity exceeded".into());
        }
        let mapping = Mapping::create(config)?;
        let mut helper = Helper::spawn(executable, timeout)?;
        let path = path.canonicalize().map_err(|e| e.to_string())?;
        let response = helper.request(Request::Load {
            path: path.to_string_lossy().into_owned(),
            id: id.into(),
            segment: mapping.path().to_string_lossy().into_owned(),
            generation: config.generation,
            state: state.map(<[u8]>::to_vec),
        })?;
        let Response::Metadata(metadata) = response else {
            return Err("invalid plugin load response".into());
        };
        metadata.validate()?;
        if metadata.id != id
            || metadata.path != path.to_string_lossy()
            || !metadata.matches(config)
            || !mapping.view()?.ready()
            || mapping.view()?.failure() != 0
        {
            return Err("plugin load identity/capacity mismatch".into());
        }
        Ok(Self {
            helper,
            mapping,
            metadata: *metadata,
            producer_claimed: false,
        })
    }
    /// Lease administrative control before transferring the audio endpoint.
    /// The caller must quiesce its producer before using this socket.
    pub fn administration(&self) -> Result<Administration, String> {
        let socket = self.helper.socket.try_clone().map_err(|e| e.to_string())?;
        socket
            .set_read_timeout(Some(self.helper.timeout))
            .map_err(|e| e.to_string())?;
        socket
            .set_write_timeout(Some(self.helper.timeout))
            .map_err(|e| e.to_string())?;
        Ok(Administration { socket })
    }
    /// Lease the sole producer once; no allocation/unmapping on producer drop.
    pub fn producer(&mut self) -> Result<Producer<'_>, String> {
        if self.producer_claimed {
            return Err("plugin producer already leased".into());
        }
        self.producer_claimed = true;
        Ok(self.mapping.view()?.producer())
    }
    /// Validated metadata including declared intrinsic latency.
    pub fn metadata(&self) -> &PluginInfo {
        &self.metadata
    }
    /// Helper process identity, for supervision and diagnostics.
    pub fn pid(&self) -> u32 {
        self.helper.child.id()
    }
    /// Segment path, for administrative diagnostics only.
    pub fn segment_path(&self) -> &Path {
        self.mapping.path()
    }
    /// Save opaque state off RT, coordinating a pause of the helper audio thread.
    pub fn save_state(&mut self) -> Result<Vec<u8>, String> {
        match self.helper.request(Request::SaveState)? {
            Response::State(bytes) => Ok(bytes),
            _ => Err("invalid plugin state response".into()),
        }
    }
    /// Restore opaque state off RT and revalidate refreshed metadata.
    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() > MAX_STATE_BYTES {
            return Err("plugin state capacity exceeded".into());
        }
        self.helper.request(Request::LoadState(bytes.to_vec()))?;
        self.refresh()
    }
    /// Operate a native editor on the helper main thread.
    pub fn editor(&mut self, command: EditorCommand) -> Result<EditorState, String> {
        match self.helper.request(Request::Editor(command))? {
            Response::Editor(state) => Ok(state),
            _ => Err("invalid plugin editor response".into()),
        }
    }
    /// Query a parameter using its stable native identity and plain units.
    pub fn parameter(&mut self, id: u32) -> Result<f64, String> {
        match self.helper.request(Request::Parameter(id))? {
            Response::Parameter(value) if value.is_finite() => Ok(value),
            _ => Err("invalid plugin parameter response".into()),
        }
    }
    /// Revalidate native ports/parameters/latency; a changed layout fails this segment.
    pub fn refresh(&mut self) -> Result<(), String> {
        let Response::Metadata(info) = self.helper.request(Request::Metadata)? else {
            return Err("invalid plugin metadata response".into());
        };
        info.validate()?;
        if !info.matches(self.mapping.view()?.config())
            || info.id != self.metadata.id
            || info.path != self.metadata.path
        {
            self.mapping.view()?.fail(Failure::Protocol);
            return Err("plugin metadata changed its negotiated ports/identity".into());
        }
        self.metadata = *info;
        Ok(())
    }
}
impl Drop for Instance {
    fn drop(&mut self) {
        if let Ok(view) = self.mapping.view() {
            view.stop();
        }
        // Kill/reap before Mapping drops, even when the plugin never returns.
        let _ = self
            .helper
            .socket
            .set_read_timeout(Some(Duration::from_millis(300)));
        let _ = protocol::send(&mut self.helper.socket, &Request::Shutdown);
        let _: Result<Response, _> = self.helper.read();
        let _ = self.helper.child.kill();
        let _ = self.helper.child.wait();
    }
}

/// Non-RT administrative lease. The audio owner keeps the helper alive;
/// freeing it makes an outstanding operation fail on the bounded socket.
pub struct Administration {
    socket: UnixStream,
}
impl Administration {
    /// Operate the native editor on main while the audio worker keeps processing.
    pub fn editor(&mut self, command: EditorCommand) -> Result<EditorState, String> {
        protocol::send(&mut self.socket, &Request::Editor(command))?;
        match protocol::receive(&mut self.socket)? {
            Response::Editor(state) => Ok(state),
            Response::Error(error) => Err(error),
            _ => Err("invalid plugin editor response".into()),
        }
    }
    /// Capture a quiesced instance, on the helper's main thread.
    pub fn save_state(&mut self) -> Result<Vec<u8>, String> {
        protocol::send(&mut self.socket, &Request::SaveState)?;
        match protocol::receive(&mut self.socket)? {
            Response::State(bytes) if bytes.len() <= MAX_STATE_BYTES => Ok(bytes),
            Response::Error(error) => Err(error),
            _ => Err("invalid plugin state response".into()),
        }
    }
}

/// Owning audio endpoint. All construction and dropping must be non-RT.
/// The producer has no destructor; its mapping is retained by the private owner.
pub struct AudioInstance {
    producer: Producer<'static>,
    _owner: Instance,
}
impl AudioInstance {
    /// Transfer the sole producer together with the mapping and helper owner.
    pub fn new(mut owner: Instance) -> Result<Self, String> {
        // SAFETY: mmap has a stable address independent of its owner's moves.
        // No producer escapes this struct and it is dropped before the owner.
        let producer =
            unsafe { std::mem::transmute::<Producer<'_>, Producer<'static>>(owner.producer()?) };
        Ok(Self {
            producer,
            _owner: owner,
        })
    }
    /// Exclusive RT endpoint, valid while this owner is alive.
    pub fn receive(
        &mut self,
        output: &mut [f32],
        events: &mut clausters_core::plugin_shm::EventList,
    ) -> bool {
        self.producer.receive(output, events)
    }
    /// Publish one complete staged block without allocating.
    pub fn publish(&mut self, job: &clausters_core::plugin_shm::Job, input: &[f32]) {
        self.producer.publish(job, input);
    }
    /// Publish and wait for completion outside RT, retaining the normal due time.
    /// A hang or process exit latches failure instead of leaving a silent render.
    pub fn publish_offline(&mut self, job: &clausters_core::plugin_shm::Job, input: &[f32]) {
        self.producer.publish(job, input);
        let start = std::time::Instant::now();
        while self.failure() == 0 && !self.producer.last_completed() {
            if !matches!(self._owner.helper.child.try_wait(), Ok(None)) {
                self.fail(Failure::Process);
            } else if start.elapsed() >= DEFAULT_TIMEOUT {
                self.fail(Failure::Starved);
            } else {
                std::thread::sleep(Duration::from_micros(50));
            }
        }
    }
    /// Identity of this mapping incarnation.
    pub fn generation(&self) -> u64 {
        self.producer.view().config().generation
    }
    /// Latched audio-plane failure.
    pub fn failure(&self) -> u32 {
        self.producer.view().failure()
    }
    /// Stop submissions on a capacity or protocol failure.
    pub fn fail(&self, failure: Failure) {
        self.producer.view().fail(failure);
    }
    /// Administrative metadata, immutable during processing.
    pub fn metadata(&self) -> &PluginInfo {
        self._owner.metadata()
    }
}

/// Native format default search roots. Absent default directories are simply empty.
pub fn default_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<_> = std::env::var_os("CLAP_PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".clap"));
    }
    dirs.extend([
        PathBuf::from("/usr/lib/clap"),
        PathBuf::from("/usr/local/lib/clap"),
    ]);
    dirs.extend(
        std::env::var_os("VST3_PATH")
            .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
            .unwrap_or_default(),
    );
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".vst3"));
    }
    dirs.extend([
        PathBuf::from("/usr/lib/vst3"),
        PathBuf::from("/usr/local/lib/vst3"),
    ]);
    dirs.into_iter().filter(|p| p.exists()).collect()
}
/// Scan candidates without loading any plugin in this process. Each binary gets
/// a disposable helper, bounded timeout and crash/protocol diagnostics.
pub fn scan(
    executable: &Path,
    dirs: &[PathBuf],
    timeout: Duration,
) -> Result<Vec<ScanEntry>, String> {
    scan_cancellable(
        executable,
        dirs,
        timeout,
        &std::sync::atomic::AtomicBool::new(false),
    )
}
/// Same discovery operation, stopped between candidates when its owner closes.
pub fn scan_cancellable(
    executable: &Path,
    dirs: &[PathBuf],
    timeout: Duration,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<Vec<ScanEntry>, String> {
    if dirs.len() > 32 {
        return Err("plugin scan directory capacity exceeded".into());
    }
    let mut result = Vec::new();
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    let mut pending: Vec<_> = dirs.iter().cloned().map(|p| (p, 0usize)).collect();
    while let Some((path, depth)) = pending.pop() {
        if cancelled.load(Ordering::Acquire) {
            return Err("plugin scan cancelled".into());
        }
        let canonical = match path.canonicalize() {
            Ok(p) => p,
            Err(e) => {
                result.push(ScanEntry::Error {
                    path: path.to_string_lossy().into_owned(),
                    error: e.to_string(),
                });
                continue;
            }
        };
        if !seen.insert(canonical.clone()) {
            continue;
        }
        if canonical
            .extension()
            .is_some_and(|e| e == "vst3" || e == "clap")
        {
            if candidates.len() == MAX_CANDIDATES {
                return Err("plugin scan candidate capacity exceeded".into());
            }
            candidates.push(canonical);
        } else if canonical.is_dir() {
            if depth >= 8 {
                result.push(ScanEntry::Error {
                    path: canonical.to_string_lossy().into_owned(),
                    error: "plugin scan directory depth exceeded".into(),
                });
                continue;
            }
            match std::fs::read_dir(&canonical) {
                Ok(entries) => {
                    for entry in entries {
                        match entry {
                            Ok(e) => match e.file_type() {
                                Ok(kind) if !kind.is_symlink() => {
                                    pending.push((e.path(), depth + 1))
                                }
                                Ok(_) => {}
                                Err(error) => result.push(ScanEntry::Error {
                                    path: e.path().to_string_lossy().into_owned(),
                                    error: error.to_string(),
                                }),
                            },
                            Err(e) => result.push(ScanEntry::Error {
                                path: canonical.to_string_lossy().into_owned(),
                                error: e.to_string(),
                            }),
                        }
                        if pending.len() > 4096 {
                            return Err("plugin scan traversal capacity exceeded".into());
                        }
                    }
                }
                Err(e) => result.push(ScanEntry::Error {
                    path: canonical.to_string_lossy().into_owned(),
                    error: e.to_string(),
                }),
            }
        }
        if seen.len() > 8192 || result.len() > MAX_CANDIDATES {
            return Err("plugin scan traversal capacity exceeded".into());
        }
    }
    candidates.sort();
    for path in candidates {
        if cancelled.load(Ordering::Acquire) {
            return Err("plugin scan cancelled".into());
        }
        let outcome = (|| {
            let mut child = Helper::spawn(executable, timeout)?;
            let mut plugins = Vec::new();
            loop {
                if cancelled.load(Ordering::Acquire) {
                    return Err("plugin scan cancelled".into());
                }
                let Response::Scan(page) = child.request(Request::Scan {
                    path: path.to_string_lossy().into_owned(),
                    offset: plugins.len(),
                })?
                else {
                    return Err("invalid plugin scan response".into());
                };
                let count = page.len();
                if count > 4 || plugins.len() + count > MAX_PLUGINS {
                    return Err("plugin binary catalogue capacity exceeded".into());
                }
                plugins.extend(page);
                if count < 4 {
                    break;
                }
            }
            let mut ids = HashSet::new();
            for row in &plugins {
                match row {
                    ScanEntry::Plugin { plugin: p } => {
                        p.validate()?;
                        if p.path != path.to_string_lossy() || !ids.insert(p.id.clone()) {
                            return Err("invalid plugin scan identity".into());
                        }
                    }
                    ScanEntry::Error {
                        path: candidate, ..
                    } if *candidate == path.to_string_lossy() => {}
                    _ => return Err("invalid plugin scan diagnostic".into()),
                }
            }
            Ok(plugins)
        })();
        match outcome {
            Ok(plugins) => result.extend(plugins),
            Err(error) => result.push(ScanEntry::Error {
                path: path.to_string_lossy().into_owned(),
                error,
            }),
        }
        if serde_json::to_vec(&result)
            .map_err(|e| e.to_string())?
            .len()
            > 16 * 1024 * 1024
        {
            return Err("plugin scan catalogue capacity exceeded".into());
        }
    }
    if serde_json::to_vec(&result)
        .map_err(|e| e.to_string())?
        .len()
        > 16 * 1024 * 1024
    {
        return Err("plugin scan catalogue capacity exceeded".into());
    }
    Ok(result)
}
/// Atomically cache successful scan rows and diagnostics in the server data directory.
pub fn cache(data_dir: &Path, entries: &[ScanEntry]) -> Result<(), String> {
    std::fs::create_dir_all(data_dir).map_err(|e| e.to_string())?;
    let temp = data_dir.join(format!(
        ".plugins-{}-{}.json",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let json = serde_json::json!({"version": 1, "entries": entries});
    let result = (|| {
        std::fs::write(
            &temp,
            serde_json::to_vec_pretty(&json).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        std::fs::rename(&temp, data_dir.join("plugins.json")).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}

enum AudioControl {
    Pause,
    Resume,
    Stop,
}
struct Worker {
    controls: mpsc::SyncSender<AudioControl>,
    ack: mpsc::Receiver<Result<(), String>>,
    finished: mpsc::Receiver<Box<dyn std::any::Any + Send>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Worker {
    fn control(&self, command: AudioControl) -> Result<(), String> {
        self.controls
            .try_send(command)
            .map_err(|_| "plugin audio control capacity/disconnect")?;
        self.ack
            .recv_timeout(Duration::from_millis(500))
            .map_err(|_| "plugin audio thread timeout")?
    }
    fn stop(&mut self) -> Result<Box<dyn std::any::Any + Send>, String> {
        let _ = self.controls.try_send(AudioControl::Stop);
        let processor = self
            .finished
            .recv_timeout(Duration::from_millis(500))
            .map_err(|_| "plugin audio thread timeout")?;
        if let Some(thread) = self.thread.take() {
            thread.join().map_err(|_| "plugin audio thread panicked")?;
        }
        Ok(processor)
    }
}
#[cfg(any(feature = "clap", feature = "vst3"))]
fn audio_worker(
    mapping: Arc<Mapping>,
    mut processor: Box<dyn FormatProcessor>,
) -> Result<Worker, String> {
    let (tx, rx) = mpsc::sync_channel(4);
    let (ack_tx, ack) = mpsc::sync_channel(1);
    let (finish_tx, finished) = mpsc::sync_channel(1);
    let thread = std::thread::Builder::new()
        .name("plugin-audio".into())
        .spawn(move || {
            clausters_core::denormals::flush_to_zero();
            let Ok(view) = mapping.view() else {
                return;
            };
            let c = view.config();
            let mut consumer = view.consumer();
            let mut pending = EventList::default();
            let start = processor.start();
            if start.is_ok() {
                consumer.view().set_ready();
            } else {
                consumer.view().fail(Failure::Process);
            }
            let _ = ack_tx.send(start);
            let mut paused = false;
            let mut pause_requested = false;
            loop {
                if consumer.view().stopping() || consumer.view().failure() != 0 {
                    break;
                }
                match rx.try_recv() {
                    Ok(AudioControl::Pause) => {
                        pause_requested = true;
                    }
                    Ok(AudioControl::Resume) => {
                        let result = processor.start();
                        paused = result.is_err();
                        let _ = ack_tx.send(result);
                    }
                    Ok(AudioControl::Stop) | Err(mpsc::TryRecvError::Disconnected) => break,
                    Err(mpsc::TryRecvError::Empty) => {}
                }
                if paused {
                    std::thread::sleep(Duration::from_micros(50));
                    continue;
                }
                if let Some(mut slot) = consumer.next_job() {
                    let job = *slot.job();
                    let (input, output) = slot.audio();
                    output.fill(0.0);
                    let mut result = EventList::default();
                    let mut failed = processor
                        .editor_events(&mut result)
                        .err()
                        .map(|_| Failure::Process);
                    for span in job.spans.iter().take(job.span_count as usize).copied() {
                        let mut events = EventList::default();
                        if span.active != 0 {
                            events = pending;
                            pending.count = 0;
                            for e in events.values.iter_mut().take(events.count as usize) {
                                e.offset = 0;
                            }
                        }
                        let destination = if span.active != 0 {
                            &mut events
                        } else {
                            &mut pending
                        };
                        for mut e in job
                            .events
                            .values
                            .iter()
                            .take(job.events.count as usize)
                            .copied()
                        {
                            if e.offset >= span.offset && e.offset < span.offset + span.frames {
                                e.offset -= span.offset;
                                // Frozen spans preserve ordering across blocks even when their
                                // local offsets restart. They are rebased to zero at resume.
                                if span.active == 0 {
                                    e.offset = 0;
                                }
                                if !destination.push(e) {
                                    failed = Some(Failure::Capacity);
                                    break;
                                }
                            }
                        }
                        if failed.is_some() {
                            break;
                        }
                        if span.active != 0
                            && processor
                                .process(
                                    span,
                                    c.sample_origin
                                        + job.sequence * FRAMES as u64
                                        + span.offset as u64,
                                    input,
                                    output,
                                    &events,
                                    &mut result,
                                )
                                .is_err()
                        {
                            failed = Some(Failure::Process);
                            break;
                        }
                    }
                    if let Some(reason) = failed {
                        slot.fail(reason);
                    } else {
                        slot.finish(result);
                    }
                } else {
                    if pause_requested {
                        processor.pause();
                        paused = true;
                        pause_requested = false;
                        let _ = ack_tx.send(Ok(()));
                    }
                    std::thread::sleep(Duration::from_micros(50));
                }
            }
            let _ = finish_tx.send(processor.stop());
        })
        .map_err(|e| e.to_string())?;
    let worker = Worker {
        controls: tx,
        ack,
        finished,
        thread: Some(thread),
    };
    worker
        .ack
        .recv_timeout(DEFAULT_TIMEOUT)
        .map_err(|_| "plugin audio startup timeout")??;
    Ok(worker)
}
struct Loaded {
    instance: Box<dyn FormatInstance>,
    mapping: Arc<Mapping>,
    worker: Worker,
}
impl Loaded {
    fn metadata(&mut self) -> Result<PluginInfo, String> {
        let info = self.instance.metadata()?;
        if !info.matches(self.mapping.view()?.config()) {
            self.mapping.view()?.fail(Failure::Protocol);
            return Err("plugin ports changed".into());
        }
        Ok(info)
    }
    fn state(&mut self, bytes: Option<Vec<u8>>) -> Result<Response, String> {
        self.worker.control(AudioControl::Pause)?;
        let result = match bytes {
            Some(bytes) => self.instance.load_state(&bytes).map(|_| Response::Ok),
            None => self.instance.save_state().map(Response::State),
        };
        let metadata = self.metadata();
        let resume = self.worker.control(AudioControl::Resume);
        metadata?;
        resume?;
        result
    }
}
impl Drop for Loaded {
    fn drop(&mut self) {
        if let Ok(view) = self.mapping.view() {
            view.stop();
        }
        if let Ok(processor) = self.worker.stop() {
            let _ = self.instance.deactivate(processor);
        }
        // On hang, main returns/exits the helper and the parent reaps it. No
        // mapping is freed while its detached audio thread still holds its Arc.
    }
}

/// Run the helper's main-thread administrative loop on a single owned descriptor.
///
/// # Safety
/// `fd` is a unique open Unix socket handed to this process by its parent;
/// ownership is transferred here and must not be reused by the caller.
pub unsafe fn helper_main(fd: i32) -> Result<(), String> {
    // SAFETY: caller transfers unique ownership of this inherited socket.
    let mut socket = unsafe { UnixStream::from_raw_fd(fd) };
    protocol::send(&mut socket, &Handshake::current())?;
    let mut loaded: Option<Loaded> = None;
    loop {
        if let Some(plugin) = &mut loaded {
            plugin.instance.main_callback();
            if plugin.instance.restart_requested() {
                plugin.mapping.view()?.fail(Failure::Protocol);
            }
        }
        // poll only on the helper main thread; framed reads have a bounded
        // timeout so malformed/partial frames never desynchronize a retry.
        let mut poll = libc::pollfd {
            fd: socket.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: valid pollfd array for one owned socket.
        let count = unsafe { libc::poll(&mut poll, 1, 10) };
        if count < 0 {
            if std::io::Error::last_os_error().kind() == ErrorKind::Interrupted {
                continue;
            }
            return Err("plugin administrative poll failed".into());
        }
        if count == 0 {
            continue;
        }
        socket
            .set_read_timeout(Some(DEFAULT_TIMEOUT))
            .map_err(|e| e.to_string())?;
        let request: Request = protocol::receive(&mut socket)?;
        if matches!(request, Request::Shutdown) {
            drop(loaded);
            protocol::send(&mut socket, &Response::Ok)?;
            return Ok(());
        }
        let response = handle(request, &mut loaded).unwrap_or_else(Response::Error);
        protocol::send(&mut socket, &response)?;
    }
}
fn handle(request: Request, loaded: &mut Option<Loaded>) -> Result<Response, String> {
    match request {
        Request::Editor(command) => Ok(Response::Editor(
            loaded
                .as_mut()
                .ok_or("plugin not loaded")?
                .instance
                .editor(command)?,
        )),
        Request::Probe { path, id } => Ok(Response::Metadata(Box::new(
            load_format(Path::new(&path), &id)?.metadata()?,
        ))),
        Request::Scan { path, offset } => {
            if loaded.is_some() {
                return Err("scan helper already owns an instance".into());
            }
            Ok(Response::Scan(scan_format(Path::new(&path), offset)?))
        }
        Request::Load {
            path,
            id,
            segment,
            generation,
            state,
        } => {
            if loaded.is_some() {
                return Err("plugin helper already owns an instance".into());
            }
            #[cfg(any(feature = "clap", feature = "vst3"))]
            {
                let mapping = Arc::new(Mapping::open(Path::new(&segment), generation)?);
                let config = mapping.view()?.config();
                let mut instance = load_format(Path::new(&path), &id)?;
                if let Some(bytes) = state {
                    if bytes.len() > MAX_STATE_BYTES {
                        return Err("plugin state capacity exceeded".into());
                    }
                    instance.load_state(&bytes)?;
                    let _ = instance.restart_requested();
                }
                let processor = instance.activate(config)?;
                let worker = audio_worker(Arc::clone(&mapping), processor)?;
                let mut plugin = Loaded {
                    instance,
                    mapping,
                    worker,
                };
                let metadata = plugin.metadata()?;
                *loaded = Some(plugin);
                Ok(Response::Metadata(Box::new(metadata)))
            }
            #[cfg(not(any(feature = "clap", feature = "vst3")))]
            {
                let _ = (path, id, segment, generation, state);
                Err("plugin helper built without adapters".into())
            }
        }
        Request::Metadata => Ok(Response::Metadata(Box::new(
            loaded.as_mut().ok_or("no loaded plugin")?.metadata()?,
        ))),
        Request::SaveState => loaded.as_mut().ok_or("no loaded plugin")?.state(None),
        Request::LoadState(bytes) => loaded
            .as_mut()
            .ok_or("no loaded plugin")?
            .state(Some(bytes)),
        Request::Parameter(id) => Ok(Response::Parameter(
            loaded
                .as_mut()
                .ok_or("no loaded plugin")?
                .instance
                .parameter(id)?,
        )),
        Request::Shutdown => Ok(Response::Ok),
    }
}

fn load_format(path: &Path, id: &str) -> Result<Box<dyn FormatInstance>, String> {
    match path.extension().and_then(|e| e.to_str()) {
        #[cfg(feature = "clap")]
        Some("clap") => crate::clap::load(path, id),
        #[cfg(feature = "vst3")]
        Some("vst3") => crate::vst3::load(path, id),
        _ => {
            let _ = id;
            Err("plugin format unavailable in helper".into())
        }
    }
}
fn scan_format(path: &Path, offset: usize) -> Result<Vec<ScanEntry>, String> {
    match path.extension().and_then(|e| e.to_str()) {
        #[cfg(feature = "clap")]
        Some("clap") => crate::clap::scan(path, offset),
        #[cfg(feature = "vst3")]
        Some("vst3") => crate::vst3::scan(path, offset),
        _ => {
            let _ = offset;
            Err("plugin format unavailable in helper".into())
        }
    }
}
