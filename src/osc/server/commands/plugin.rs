//! Native plugin discovery and administration, always outside the
//! network/audio threads.
//!
//! Three workers, so that no kind of request waits behind another: **scan**
//! (a discovery can take minutes), **build** (probing a def, preparing a
//! node's spare instance, restarting a node, loading a state -- each starts a
//! helper and loads a plugin, which can take seconds) and **control** (the
//! editor and a state capture, which a running helper answers at once).
use super::super::*;

#[cfg(all(
    any(feature = "plugin-clap", feature = "plugin-vst3"),
    target_os = "linux"
))]
mod workers {
    use super::*;

    /// Most jobs a worker holds, accepted or finished and not yet drained;
    /// its result channel has the same capacity, so a result never waits.
    const QUEUE: usize = 4;

    pub(in crate::osc::server) struct ScanRequest {
        pub dirs: Vec<std::path::PathBuf>,
        pub cache: Option<std::path::PathBuf>,
        /// Inspect every binary, cached or not.
        pub rescan: bool,
        pub client: ClientId,
    }
    pub(in crate::osc::server) struct ScanReply {
        pub client: ClientId,
        pub result: Result<Vec<clausters_plugin_host::format::ScanEntry>, String>,
    }
    pub(in crate::osc::server) enum PluginJob {
        Scan(ScanRequest),
        Editor {
            id: i32,
            expected: u64,
            admin: Arc<crate::plugin::PluginAdmin>,
            command: clausters_plugin_host::format::EditorCommand,
            address: &'static str,
            client: ClientId,
        },
        Save {
            id: i32,
            expected: u64,
            def: Arc<crate::plugin::PluginDef>,
            admin: Arc<crate::plugin::PluginAdmin>,
            client: ClientId,
        },
        Def {
            name: String,
            reference: crate::plugin::PluginReference,
            sample_rate: f32,
            client: ClientId,
            ticket: u64,
        },
        Load {
            id: i32,
            expected: u64,
            def: Arc<crate::plugin::PluginDef>,
            admin: Arc<crate::plugin::PluginAdmin>,
            bytes: Vec<u8>,
            sample_rate: f32,
            device_frames: u32,
            client: ClientId,
        },
        Restart {
            id: i32,
            expected: u64,
            def: Arc<crate::plugin::PluginDef>,
            sample_rate: f32,
            device_frames: u32,
            client: ClientId,
        },
        Spare {
            name: String,
            def: Arc<crate::plugin::PluginDef>,
            sample_rate: f32,
            device_frames: u32,
        },
    }
    pub(in crate::osc::server) struct PreparedState {
        pub def: Arc<crate::plugin::PluginDef>,
        pub synth: Box<crate::plugin::PluginSynth>,
    }
    pub(in crate::osc::server) enum PluginResult {
        Scan(ScanReply),
        Editor {
            id: i32,
            expected: u64,
            result: Result<clausters_plugin_host::format::EditorState, String>,
            address: &'static str,
            client: ClientId,
        },
        Save {
            id: i32,
            expected: u64,
            def: Arc<crate::plugin::PluginDef>,
            result: Result<Vec<u8>, String>,
            client: ClientId,
        },
        Def {
            name: String,
            result: Result<Box<crate::plugin::PluginDef>, String>,
            client: ClientId,
            ticket: u64,
        },
        Load {
            id: i32,
            expected: u64,
            prior: Arc<crate::plugin::PluginDef>,
            admin: Arc<crate::plugin::PluginAdmin>,
            result: Result<PreparedState, String>,
            client: ClientId,
        },
        Restart {
            id: i32,
            expected: u64,
            def: Arc<crate::plugin::PluginDef>,
            result: Result<Box<crate::plugin::PluginSynth>, String>,
            client: ClientId,
        },
        Spare {
            name: String,
            def: Arc<crate::plugin::PluginDef>,
            result: Result<Box<crate::plugin::PluginSynth>, String>,
        },
    }
    impl PluginResult {
        /// A spare is the server's own preparation: no `/server_sync` waits
        /// for it.
        fn counted(&self) -> bool {
            !matches!(self, Self::Spare { .. })
        }
    }

    fn run(job: PluginJob, stop: &std::sync::atomic::AtomicBool) -> PluginResult {
        match job {
            PluginJob::Scan(job) => {
                let result = (|| {
                    let helper = clausters_plugin_host::runtime::helper_path()?;
                    let mut cache = job
                        .cache
                        .as_deref()
                        .map(clausters_plugin_host::runtime::ScanCache::read);
                    let outcome = clausters_plugin_host::runtime::scan_cached(
                        &helper,
                        &job.dirs,
                        clausters_plugin_host::runtime::DEFAULT_TIMEOUT,
                        stop,
                        cache.as_mut(),
                        job.rescan,
                    )?;
                    if let (Some(dir), Some(cache)) = (&job.cache, &cache) {
                        cache.write(dir)?;
                    }
                    Ok(outcome.entries)
                })();
                PluginResult::Scan(ScanReply {
                    client: job.client,
                    result,
                })
            }
            PluginJob::Editor {
                id,
                expected,
                admin,
                command,
                address,
                client,
            } => PluginResult::Editor {
                id,
                expected,
                result: admin.editor(command),
                address,
                client,
            },
            PluginJob::Save {
                id,
                expected,
                def,
                admin,
                client,
            } => PluginResult::Save {
                id,
                expected,
                def,
                result: admin.save(),
                client,
            },
            PluginJob::Def {
                name,
                reference,
                sample_rate,
                client,
                ticket,
            } => PluginResult::Def {
                name,
                result: crate::plugin::PluginDef::probe(reference, sample_rate).map(Box::new),
                client,
                ticket,
            },
            PluginJob::Load {
                id,
                expected,
                def,
                admin,
                bytes,
                sample_rate,
                device_frames,
                client,
            } => {
                let result = (|| {
                    admin.wait_boundary()?;
                    let mut reference = def.reference.clone();
                    reference.state = bytes;
                    let refreshed = crate::plugin::PluginDef::probe_state(reference, sample_rate)?;
                    if refreshed.names != def.names {
                        return Err("plugin state changed its control identities".into());
                    }
                    let refreshed = Arc::new(refreshed);
                    let mut synth = crate::plugin::PluginSynth::new(
                        Arc::clone(&refreshed),
                        sample_rate,
                        device_frames,
                    )?;
                    synth.loaded_state();
                    Ok(PreparedState {
                        def: refreshed,
                        synth: Box::new(synth),
                    })
                })();
                PluginResult::Load {
                    id,
                    expected,
                    prior: def,
                    admin,
                    result,
                    client,
                }
            }
            PluginJob::Restart {
                id,
                expected,
                def,
                sample_rate,
                device_frames,
                client,
            } => PluginResult::Restart {
                id,
                expected,
                result: crate::plugin::PluginSynth::new(
                    Arc::clone(&def),
                    sample_rate,
                    device_frames,
                )
                .map(Box::new),
                def,
                client,
            },
            PluginJob::Spare {
                name,
                def,
                sample_rate,
                device_frames,
            } => PluginResult::Spare {
                name,
                result: crate::plugin::PluginSynth::new(
                    Arc::clone(&def),
                    sample_rate,
                    device_frames,
                )
                .map(Box::new),
                def,
            },
        }
    }

    /// One worker thread with its bounded queue. `submitted`/`drained` count
    /// the jobs a `/server_sync` waits for; each worker completes in order,
    /// so per-worker counts are a sufficient barrier.
    pub(in crate::osc::server) struct PluginWorker {
        sender: std::sync::mpsc::SyncSender<PluginJob>,
        receiver: std::sync::mpsc::Receiver<PluginResult>,
        cancelled: Arc<std::sync::atomic::AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
        in_flight: usize,
        pub submitted: u64,
        pub drained: u64,
    }
    impl PluginWorker {
        fn spawn(name: &str, waker: Option<crate::osc::wake::Waker>) -> Self {
            let (sender, incoming) = std::sync::mpsc::sync_channel::<PluginJob>(QUEUE);
            let (outgoing, receiver) = std::sync::mpsc::sync_channel(QUEUE);
            let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let stop = Arc::clone(&cancelled);
            // No audio callback submits a job or owns this worker or channel.
            let thread = std::thread::Builder::new()
                .name(format!("plugin-{name}"))
                .spawn(move || {
                    while !stop.load(Ordering::Acquire) {
                        let job = match incoming.recv_timeout(Duration::from_millis(50)) {
                            Ok(job) => job,
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                        };
                        if outgoing.try_send(run(job, &stop)).is_err() {
                            break;
                        }
                        if let Some(waker) = &waker {
                            waker.wake();
                        }
                    }
                })
                .expect("spawn plugin worker");
            Self {
                sender,
                receiver,
                cancelled,
                thread: Some(thread),
                in_flight: 0,
                submitted: 0,
                drained: 0,
            }
        }
        /// Queue a job, or refuse it while the worker holds `QUEUE` of them.
        pub fn submit(&mut self, job: PluginJob) -> Result<(), &'static str> {
            if self.in_flight >= QUEUE {
                return Err("plugin worker queue full");
            }
            let counted = !matches!(job, PluginJob::Spare { .. });
            self.sender
                .try_send(job)
                .map_err(|_| "plugin worker unavailable")?;
            self.in_flight += 1;
            if counted {
                self.submitted += 1;
            }
            Ok(())
        }
        pub fn try_recv(&mut self) -> Option<PluginResult> {
            let result = self.receiver.try_recv().ok()?;
            self.in_flight -= 1;
            if result.counted() {
                self.drained += 1;
            }
            Some(result)
        }
    }
    impl Drop for PluginWorker {
        fn drop(&mut self) {
            self.cancelled.store(true, Ordering::Release);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    pub(in crate::osc::server) struct PluginWorkers {
        pub scan: PluginWorker,
        pub build: PluginWorker,
        pub control: PluginWorker,
    }
    impl PluginWorkers {
        pub fn spawn(waker: Option<crate::osc::wake::Waker>) -> Self {
            Self {
                scan: PluginWorker::spawn("scan", waker.clone()),
                build: PluginWorker::spawn("build", waker.clone()),
                control: PluginWorker::spawn("control", waker),
            }
        }
        pub fn all(&mut self) -> [&mut PluginWorker; 3] {
            [&mut self.scan, &mut self.build, &mut self.control]
        }
    }
}
#[cfg(all(
    any(feature = "plugin-clap", feature = "plugin-vst3"),
    target_os = "linux"
))]
pub(in crate::osc::server) use workers::PluginWorkers;
#[cfg(all(
    any(feature = "plugin-clap", feature = "plugin-vst3"),
    target_os = "linux"
))]
use workers::{PluginJob, PluginResult, PreparedState, ScanRequest};

impl OscServer {
    /// What a `/server_sync` issued now waits for, per plugin worker.
    pub(in crate::osc::server) fn plugin_targets(&self) -> [u64; 3] {
        #[cfg(all(
            any(feature = "plugin-clap", feature = "plugin-vst3"),
            target_os = "linux"
        ))]
        {
            let w = &self.plugin_workers;
            [w.scan.submitted, w.build.submitted, w.control.submitted]
        }
        #[cfg(not(all(
            any(feature = "plugin-clap", feature = "plugin-vst3"),
            target_os = "linux"
        )))]
        {
            [0; 3]
        }
    }
    /// Whether every plugin job a sync waits for has finished.
    pub(in crate::osc::server) fn plugin_reached(&self, targets: &[u64; 3]) -> bool {
        #[cfg(all(
            any(feature = "plugin-clap", feature = "plugin-vst3"),
            target_os = "linux"
        ))]
        {
            let w = &self.plugin_workers;
            w.scan.drained >= targets[0]
                && w.build.drained >= targets[1]
                && w.control.drained >= targets[2]
        }
        #[cfg(not(all(
            any(feature = "plugin-clap", feature = "plugin-vst3"),
            target_os = "linux"
        )))]
        {
            let _ = targets;
            true
        }
    }
    pub(in crate::osc::server) fn handle_plugin_scan(
        &mut self,
        args: Args,
        from: ClientId,
    ) -> Answer {
        let mut dirs = Vec::new();
        let mut rescan = false;
        for (i, argument) in args.rest().iter().enumerate() {
            if let (0, OscType::Int(flag)) = (i, argument) {
                rescan = *flag != 0;
                continue;
            }
            let OscType::String(path) = argument else {
                return Err("plugin scan directories must be strings".into());
            };
            dirs.push(std::path::PathBuf::from(path));
        }
        if dirs.len() > 32 {
            return Err("plugin scan directory capacity exceeded".into());
        }
        #[cfg(all(
            any(feature = "plugin-clap", feature = "plugin-vst3"),
            target_os = "linux"
        ))]
        {
            if dirs.is_empty() {
                dirs = clausters_plugin_host::runtime::default_dirs();
            }
            let cache = self
                .store
                .as_ref()
                .and_then(|s| s.defs_dir().parent())
                .map(std::path::Path::to_path_buf);
            self.plugin_workers
                .scan
                .submit(PluginJob::Scan(ScanRequest {
                    dirs,
                    cache,
                    rescan,
                    client: from,
                }))?;
            Ok(())
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (from, rescan);
            Err("native plugins unavailable in the in-page engine".into())
        }
        #[cfg(all(
            not(target_arch = "wasm32"),
            not(all(
                any(feature = "plugin-clap", feature = "plugin-vst3"),
                target_os = "linux"
            ))
        ))]
        {
            let _ = (from, rescan);
            Err("server built without native plugin support".into())
        }
    }
    pub(in crate::osc::server) fn handle_def_send_plugin(
        &mut self,
        args: &[OscType],
        from: ClientId,
    ) -> Answer {
        #[cfg(all(
            any(feature = "plugin-clap", feature = "plugin-vst3"),
            target_os = "linux"
        ))]
        {
            let Some(OscType::String(name)) = args.first() else {
                return Err("expected plugin name and reference JSON".into());
            };
            let reference = serde_json::from_slice(crate::osc::args::json_payload(&args[1..])?)
                .map_err(|e| format!("invalid plugin reference: {e}"))?;
            let ticket = self.plugin_tickets + 1;
            self.plugin_workers.build.submit(PluginJob::Def {
                name: name.clone(),
                reference,
                sample_rate: self.handle.sample_rate,
                client: from,
                ticket,
            })?;
            self.plugin_tickets = ticket;
            self.claim_def_name(name, DefKind::Plugin);
            self.plugin_claims.insert(name.clone(), ticket);
            Ok(())
        }
        #[cfg(not(all(
            any(feature = "plugin-clap", feature = "plugin-vst3"),
            target_os = "linux"
        )))]
        {
            let _ = (args, from);
            Err("server built without native plugin support".into())
        }
    }
    pub(in crate::osc::server) fn handle_plugin_editor(
        &mut self,
        mut args: Args,
        from: ClientId,
        address: &'static str,
    ) -> Answer {
        let id = args.int()?;
        let size = if address == "/plugin_resizeEditor" {
            let width = args.int()?;
            let height = args.int()?;
            if !(1..=16384).contains(&width) || !(1..=16384).contains(&height) {
                return Err("invalid plugin editor size".into());
            }
            Some((width as u32, height as u32))
        } else {
            None
        };
        if !args.is_empty() {
            return Err("unexpected plugin editor arguments".into());
        }
        #[cfg(all(
            any(feature = "plugin-clap", feature = "plugin-vst3"),
            target_os = "linux"
        ))]
        {
            use clausters_plugin_host::format::EditorCommand;
            if self.translator.mirror.get(id).is_none() {
                return Err("not a plugin node".into());
            }
            let Some(crate::osc::translate::NodeDef::Plugin(def, expected, admin)) =
                self.translator.node_defs.get(&id)
            else {
                return Err("not a plugin node".into());
            };
            if !def.info.editor {
                return Err("plugin has no native editor".into());
            }
            if admin.pending() {
                return Err("plugin state operation already pending".into());
            }
            let command = match address {
                "/plugin_openEditor" => EditorCommand::Open,
                "/plugin_closeEditor" => EditorCommand::Close,
                "/plugin_resizeEditor" => {
                    let (width, height) = size.unwrap();
                    EditorCommand::Resize { width, height }
                }
                _ => EditorCommand::Query,
            };
            let job = PluginJob::Editor {
                id,
                expected: *expected,
                admin: Arc::clone(admin),
                command,
                address,
                client: from,
            };
            self.plugin_workers.control.submit(job)?;
            Ok(())
        }
        #[cfg(not(all(
            any(feature = "plugin-clap", feature = "plugin-vst3"),
            target_os = "linux"
        )))]
        {
            let _ = (id, size, from);
            Err("server built without native plugin support".into())
        }
    }
    pub(in crate::osc::server) fn handle_plugin_restart(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let id = args.int()?;
        if !args.is_empty() {
            return Err("expected one plugin node id".into());
        }
        #[cfg(all(
            any(feature = "plugin-clap", feature = "plugin-vst3"),
            target_os = "linux"
        ))]
        {
            let Some(crate::osc::translate::NodeDef::Plugin(def, expected, admin)) =
                self.translator.node_defs.get(&id)
            else {
                return Err("not a plugin node".into());
            };
            if admin.pending() {
                return Err("plugin state operation already pending".into());
            }
            let job = PluginJob::Restart {
                id,
                expected: *expected,
                def: Arc::clone(def),
                sample_rate: self.handle.sample_rate,
                device_frames: self.translator.device_frames(),
                client: from,
            };
            self.plugin_workers.build.submit(job)?;
            Ok(())
        }
        #[cfg(not(all(
            any(feature = "plugin-clap", feature = "plugin-vst3"),
            target_os = "linux"
        )))]
        {
            let _ = (id, from);
            Err("server built without native plugin support".into())
        }
    }
    pub(in crate::osc::server) fn handle_plugin_state(
        &mut self,
        mut args: Args,
        from: ClientId,
        loading: bool,
    ) -> Answer {
        let id = args.int()?;
        let bytes = if loading {
            let Some(OscType::Blob(bytes)) = args.rest().first() else {
                return Err("expected plugin state blob".into());
            };
            if bytes.len() > 1024 * 1024 {
                return Err("plugin state capacity exceeded".into());
            }
            if args.rest().len() != 1 {
                return Err("expected one plugin state blob".into());
            }
            Some(bytes.clone())
        } else {
            if !args.is_empty() {
                return Err("expected one plugin node id".into());
            }
            None
        };
        #[cfg(all(
            any(feature = "plugin-clap", feature = "plugin-vst3"),
            target_os = "linux"
        ))]
        {
            let Some(crate::osc::translate::NodeDef::Plugin(def, expected, admin)) =
                self.translator.node_defs.get(&id)
            else {
                return Err("not a plugin node".into());
            };
            if !def.info.state {
                return Err("plugin has no state extension".into());
            }
            let (def, expected, admin) = (Arc::clone(def), *expected, Arc::clone(admin));
            let Some(bytes) = bytes else {
                // A capture keeps the node playing: the control worker asks
                // the running helper, and nothing is swapped.
                if admin.pending() {
                    return Err("plugin state operation already pending".into());
                }
                self.plugin_workers.control.submit(PluginJob::Save {
                    id,
                    expected,
                    def,
                    admin,
                    client: from,
                })?;
                return Ok(());
            };
            admin.reserve()?;
            let job = PluginJob::Load {
                id,
                expected,
                def,
                admin: Arc::clone(&admin),
                bytes,
                sample_rate: self.handle.sample_rate,
                device_frames: self.translator.device_frames(),
                client: from,
            };
            if let Err(error) = self.plugin_workers.build.submit(job) {
                admin.abort();
                return Err(error.into());
            }
            Ok(())
        }
        #[cfg(not(all(
            any(feature = "plugin-clap", feature = "plugin-vst3"),
            target_os = "linux"
        )))]
        {
            let _ = (id, from, bytes);
            Err("server built without native plugin support".into())
        }
    }
    /// Whether node `id` is still the generation a job was submitted for.
    #[cfg(all(
        any(feature = "plugin-clap", feature = "plugin-vst3"),
        target_os = "linux"
    ))]
    fn plugin_current(
        &self,
        id: i32,
        expected: u64,
        def: Option<&Arc<crate::plugin::PluginDef>>,
    ) -> bool {
        matches!(
            self.translator.node_defs.get(&id),
            Some(crate::osc::translate::NodeDef::Plugin(current, generation, ..))
                if *generation == expected && def.is_none_or(|def| Arc::ptr_eq(current, def))
        )
    }
    /// Swap a prepared instance into node `id` at a block boundary.
    #[cfg(all(
        any(feature = "plugin-clap", feature = "plugin-vst3"),
        target_os = "linux"
    ))]
    fn install_plugin_generation(
        &mut self,
        id: i32,
        expected: u64,
        def: Arc<crate::plugin::PluginDef>,
        synth: Box<crate::plugin::PluginSynth>,
    ) -> Result<(), &'static str> {
        let generation = crate::node::SynthNode::incarnation(synth.as_ref());
        let admin = synth.administration();
        self.send_all([Cmd::RestartSynth {
            id,
            expected,
            synth,
        }])
        .map_err(|_| "command FIFO full")?;
        self.translator.retire_plugin_generation(id, expected);
        self.translator.node_defs.insert(
            id,
            crate::osc::translate::NodeDef::Plugin(def, generation, admin),
        );
        Ok(())
    }
    /// Ask the build worker for a spare of every def a `/synth_new` drew
    /// from (and of `also`), and release the spares of defs that are gone.
    #[cfg(all(
        any(feature = "plugin-clap", feature = "plugin-vst3"),
        target_os = "linux"
    ))]
    fn refill_plugin_spares(&mut self, also: Option<String>) {
        drop(self.translator.stale_spares());
        for name in self
            .translator
            .take_spare_requests()
            .into_iter()
            .chain(also)
        {
            if self.translator.has_spare(&name) || self.plugin_spares_building.contains(&name) {
                continue;
            }
            let Some(def) = self.translator.plugin_defs.get(&name) else {
                continue;
            };
            let job = PluginJob::Spare {
                name: name.clone(),
                def: Arc::clone(def),
                sample_rate: self.handle.sample_rate,
                device_frames: self.translator.device_frames(),
            };
            // A full queue skips the spare: the next /synth_new builds its
            // instance itself and asks again.
            if self.plugin_workers.build.submit(job).is_ok() {
                self.plugin_spares_building.insert(name);
            }
        }
    }
    #[cfg(all(
        any(feature = "plugin-clap", feature = "plugin-vst3"),
        target_os = "linux"
    ))]
    pub(in crate::osc::server) fn collect_plugin_results(&mut self) {
        let mut results = Vec::new();
        for worker in self.plugin_workers.all() {
            while let Some(result) = worker.try_recv() {
                results.push(result);
            }
        }
        let mut installed = None;
        for result in results {
            match result {
                PluginResult::Editor {
                    id,
                    expected,
                    result,
                    address,
                    client,
                } => {
                    if self.translator.mirror.get(id).is_none()
                        || !self.plugin_current(id, expected, None)
                    {
                        self.fail(client, address, "plugin node no longer exists");
                        continue;
                    }
                    match result {
                        Ok(state) => self.done_with(
                            client,
                            address,
                            vec![
                                OscType::Int(id),
                                OscType::String(
                                    serde_json::to_string(&state).expect("editor state JSON"),
                                ),
                            ],
                        ),
                        Err(error) => self.fail(client, address, error),
                    }
                }
                PluginResult::Save {
                    id,
                    expected,
                    def,
                    result,
                    client,
                } => {
                    const COMMAND: &str = "/plugin_saveState";
                    let bytes = match result {
                        Ok(bytes) if self.plugin_current(id, expected, Some(&def)) => bytes,
                        Ok(_) => {
                            self.fail(client, COMMAND, "plugin node no longer exists");
                            continue;
                        }
                        Err(error) => {
                            self.fail(client, COMMAND, error);
                            continue;
                        }
                    };
                    // The image is the node's recovery point: a restart
                    // restores it. The recipe on disk does not change.
                    if let Some(crate::osc::translate::NodeDef::Plugin(current, ..)) =
                        self.translator.node_defs.get_mut(&id)
                    {
                        *current = Arc::new(current.with_state(bytes.clone()));
                    }
                    let chunks = bytes.len().div_ceil(6144).max(1);
                    for chunk in 0..chunks {
                        let start = chunk * 6144;
                        self.reply(
                            client,
                            "/plugin_saveState.reply",
                            vec![
                                OscType::Int(id),
                                OscType::Int(chunk as i32),
                                OscType::Int(chunks as i32),
                                OscType::Blob(bytes[start..bytes.len().min(start + 6144)].to_vec()),
                            ],
                        );
                    }
                    self.done_with(client, COMMAND, vec![OscType::Int(id)]);
                }
                PluginResult::Scan(result) => match result.result {
                    Err(error) => self.fail(result.client, "/plugin_scan", error),
                    Ok(entries) => {
                        let count = entries.len() as i32;
                        self.reply(
                            result.client,
                            "/plugin_scan.reply",
                            vec![
                                OscType::Int(-1),
                                OscType::Int(count),
                                OscType::Int(0),
                                OscType::Int(0),
                                OscType::Blob(Vec::new()),
                            ],
                        );
                        for (index, entry) in entries.into_iter().enumerate() {
                            let bytes =
                                serde_json::to_vec(&entry).expect("validated plugin metadata JSON");
                            let chunks = bytes.len().div_ceil(6144);
                            for (chunk, bytes) in bytes.chunks(6144).enumerate() {
                                self.reply(
                                    result.client,
                                    "/plugin_scan.reply",
                                    vec![
                                        OscType::Int(index as i32),
                                        OscType::Int(count),
                                        OscType::Int(chunk as i32),
                                        OscType::Int(chunks as i32),
                                        OscType::Blob(bytes.to_vec()),
                                    ],
                                );
                            }
                        }
                        self.done(result.client, "/plugin_scan");
                    }
                },
                PluginResult::Def {
                    name,
                    result,
                    client,
                    ticket,
                } => {
                    if self.plugin_claims.get(&name) != Some(&ticket) {
                        self.fail(client, "/def_send", "plugin def request superseded");
                        continue;
                    }
                    match result {
                        Ok(def) => {
                            if let Some(store) = &self.store
                                && !defstore::is_ephemeral(&name)
                                && let Err(error) = store.save_plugindef(
                                    &name,
                                    &serde_json::to_vec(&def.reference)
                                        .expect("plugin reference JSON"),
                                    Some(
                                        &serde_json::to_vec(&def.surface())
                                            .expect("plugin surface JSON"),
                                    ),
                                )
                            {
                                self.fail(client, "/def_send", error.to_string());
                                continue;
                            }
                            self.translator
                                .plugin_defs
                                .insert(name.clone(), Arc::from(def));
                            self.done_with(
                                client,
                                "/def_send",
                                vec![
                                    OscType::String("plugin".into()),
                                    OscType::String(name.clone()),
                                ],
                            );
                            // A def just sent is about to be played: start its
                            // first instance now rather than on /synth_new.
                            installed = Some(name);
                        }
                        Err(error) => self.fail(client, "/def_send", error),
                    }
                }
                PluginResult::Load {
                    id,
                    expected,
                    prior,
                    admin,
                    result,
                    client,
                } => {
                    const COMMAND: &str = "/plugin_loadState";
                    let result = result.and_then(|value| {
                        if self.plugin_current(id, expected, Some(&prior)) {
                            Ok(value)
                        } else {
                            Err("plugin node no longer exists".into())
                        }
                    });
                    match result {
                        Ok(PreparedState { def, synth }) => {
                            let values: Vec<f32> = def
                                .defaults
                                .iter()
                                .take(def.info.parameters.len())
                                .copied()
                                .collect();
                            if let Err(error) =
                                self.install_plugin_generation(id, expected, def, synth)
                            {
                                admin.abort();
                                self.fail(client, COMMAND, error);
                                continue;
                            }
                            // A load takes the loaded parameter values, except
                            // where a control is mapped.
                            for (index, value) in values.into_iter().enumerate() {
                                let mapped = matches!(self.translator.mirror.get(id).map(|node| &node.body), Some(crate::osc::graph::MirrorBody::Synth { maps, .. }) if maps.iter().any(|(control, ..)| *control == index as u32));
                                if !mapped {
                                    self.translator.mirror.set_control(id, index as u32, value);
                                }
                            }
                            self.done_with(client, COMMAND, vec![OscType::Int(id)]);
                        }
                        Err(error) => {
                            admin.abort();
                            self.fail(client, COMMAND, error);
                        }
                    }
                }
                PluginResult::Restart {
                    id,
                    expected,
                    def,
                    result,
                    client,
                } => {
                    const COMMAND: &str = "/plugin_restart";
                    if !self.plugin_current(id, expected, Some(&def)) {
                        self.fail(client, COMMAND, "plugin node no longer exists");
                        continue;
                    }
                    match result.and_then(|synth| {
                        self.install_plugin_generation(id, expected, def, synth)
                            .map_err(String::from)
                    }) {
                        Ok(()) => self.done_with(client, COMMAND, vec![OscType::Int(id)]),
                        Err(error) => self.fail(client, COMMAND, error),
                    }
                }
                PluginResult::Spare { name, def, result } => {
                    self.plugin_spares_building.remove(&name);
                    match result {
                        Ok(synth) => drop(self.translator.offer_spare(name, def, synth)),
                        Err(error) => tracing::warn!(
                            target: crate::logging::OSC_TARGET,
                            "plugin def {name}: spare instance failed: {error}"
                        ),
                    }
                }
            }
        }
        self.refill_plugin_spares(installed);
        self.resolve_syncs();
    }
}
