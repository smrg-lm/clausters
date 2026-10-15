//! Native plugin discovery, always outside the network/audio threads.
use super::super::*;

#[cfg(all(
    any(feature = "plugin-clap", feature = "plugin-vst3"),
    target_os = "linux"
))]
pub(in crate::osc::server) struct ScanWorker {
    sender: std::sync::mpsc::SyncSender<ScanJob>,
    receiver: std::sync::mpsc::Receiver<ScanResult>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
#[cfg(all(
    any(feature = "plugin-clap", feature = "plugin-vst3"),
    target_os = "linux"
))]
struct ScanRequest {
    dirs: Vec<std::path::PathBuf>,
    cache: Option<std::path::PathBuf>,
    client: ClientId,
}
#[cfg(all(
    any(feature = "plugin-clap", feature = "plugin-vst3"),
    target_os = "linux"
))]
struct ScanReply {
    client: ClientId,
    result: Result<Vec<clausters_plugin_host::format::ScanEntry>, String>,
}
#[cfg(all(
    any(feature = "plugin-clap", feature = "plugin-vst3"),
    target_os = "linux"
))]
enum ScanJob {
    Editor {
        id: i32,
        expected: u64,
        admin: Arc<crate::plugin::PluginAdmin>,
        command: clausters_plugin_host::format::EditorCommand,
        address: &'static str,
        client: ClientId,
    },
    Scan(ScanRequest),
    Def {
        name: String,
        reference: crate::plugin::PluginReference,
        sample_rate: f32,
        client: ClientId,
        ticket: u64,
    },
    State {
        id: i32,
        expected: u64,
        def: Arc<crate::plugin::PluginDef>,
        admin: Arc<crate::plugin::PluginAdmin>,
        bytes: Option<Vec<u8>>,
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
}
#[cfg(all(
    any(feature = "plugin-clap", feature = "plugin-vst3"),
    target_os = "linux"
))]
struct PreparedState {
    def: Arc<crate::plugin::PluginDef>,
    synth: Box<crate::plugin::PluginSynth>,
    bytes: Vec<u8>,
}
#[cfg(all(
    any(feature = "plugin-clap", feature = "plugin-vst3"),
    target_os = "linux"
))]
enum ScanResult {
    Editor {
        id: i32,
        expected: u64,
        result: Result<clausters_plugin_host::format::EditorState, String>,
        address: &'static str,
        client: ClientId,
    },
    Scan(ScanReply),
    Def {
        name: String,
        result: Result<Box<crate::plugin::PluginDef>, String>,
        client: ClientId,
        ticket: u64,
    },
    State {
        id: i32,
        expected: u64,
        prior: Arc<crate::plugin::PluginDef>,
        admin: Arc<crate::plugin::PluginAdmin>,
        loading: bool,
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
}
#[cfg(all(
    any(feature = "plugin-clap", feature = "plugin-vst3"),
    target_os = "linux"
))]
impl ScanWorker {
    pub(in crate::osc::server) fn spawn(waker: Option<crate::osc::wake::Waker>) -> Self {
        let (sender, incoming) = std::sync::mpsc::sync_channel::<ScanJob>(4);
        let (outgoing, receiver) = std::sync::mpsc::sync_channel(4);
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop = Arc::clone(&cancelled);
        // At most four accepted jobs exist, enforced by submitted/drained counts.
        // No audio callback submits discovery or owns this worker/channel.
        let thread = std::thread::Builder::new()
            .name("plugin-scan".into())
            .spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    let job = match incoming.recv_timeout(Duration::from_millis(50)) {
                        Ok(job) => job,
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    };
                    let result = match job {
                        ScanJob::Editor {
                            id,
                            expected,
                            admin,
                            command,
                            address,
                            client,
                        } => ScanResult::Editor {
                            id,
                            expected,
                            result: admin.editor(command),
                            address,
                            client,
                        },
                        ScanJob::Scan(job) => {
                            let result = (|| {
                                let helper = clausters_plugin_host::runtime::helper_path()?;
                                let entries = clausters_plugin_host::runtime::scan_cancellable(
                                    &helper,
                                    &job.dirs,
                                    clausters_plugin_host::runtime::DEFAULT_TIMEOUT,
                                    &stop,
                                )?;
                                if let Some(dir) = &job.cache {
                                    clausters_plugin_host::runtime::cache(dir, &entries)?;
                                }
                                Ok(entries)
                            })();
                            ScanResult::Scan(ScanReply {
                                client: job.client,
                                result,
                            })
                        }
                        ScanJob::Def {
                            name,
                            reference,
                            sample_rate,
                            client,
                            ticket,
                        } => ScanResult::Def {
                            name,
                            result: crate::plugin::PluginDef::probe(reference, sample_rate)
                                .map(Box::new),
                            client,
                            ticket,
                        },
                        ScanJob::State {
                            id,
                            expected,
                            def,
                            admin,
                            bytes,
                            sample_rate,
                            device_frames,
                            client,
                        } => {
                            let loading = bytes.is_some();
                            let result = (|| {
                                let state = match bytes {
                                    Some(bytes) => {
                                        admin.wait_boundary()?;
                                        bytes
                                    }
                                    None => admin.capture()?,
                                };
                                let mut reference = def.reference.clone();
                                reference.state = state.clone();
                                let refreshed =
                                    crate::plugin::PluginDef::probe_state(reference, sample_rate)?;
                                if refreshed.names != def.names {
                                    return Err(
                                        "plugin state changed its control identities".into()
                                    );
                                }
                                let refreshed = Arc::new(refreshed);
                                let mut synth = crate::plugin::PluginSynth::new(
                                    Arc::clone(&refreshed),
                                    sample_rate,
                                    device_frames,
                                )?;
                                if loading {
                                    synth.loaded_state();
                                }
                                Ok(PreparedState {
                                    def: refreshed,
                                    synth: Box::new(synth),
                                    bytes: state,
                                })
                            })();
                            ScanResult::State {
                                id,
                                expected,
                                prior: def,
                                admin,
                                loading,
                                result,
                                client,
                            }
                        }
                        ScanJob::Restart {
                            id,
                            expected,
                            def,
                            sample_rate,
                            device_frames,
                            client,
                        } => {
                            let result = crate::plugin::PluginSynth::new(
                                Arc::clone(&def),
                                sample_rate,
                                device_frames,
                            )
                            .map(Box::new);
                            ScanResult::Restart {
                                id,
                                expected,
                                def,
                                result,
                                client,
                            }
                        }
                    };
                    if outgoing.try_send(result).is_err() {
                        break;
                    }
                    if let Some(waker) = &waker {
                        waker.wake();
                    }
                }
            })
            .expect("spawn plugin discovery worker");
        Self {
            sender,
            receiver,
            cancelled,
            thread: Some(thread),
        }
    }
}
#[cfg(all(
    any(feature = "plugin-clap", feature = "plugin-vst3"),
    target_os = "linux"
))]
impl Drop for ScanWorker {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
impl OscServer {
    pub(in crate::osc::server) fn handle_plugin_scan(
        &mut self,
        args: Args,
        from: ClientId,
    ) -> Answer {
        let mut dirs = Vec::new();
        for argument in args.rest() {
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
            if self.plugin_submitted - self.plugin_drained >= 4 {
                return Err("plugin scan queue full".into());
            }
            let cache = self
                .store
                .as_ref()
                .and_then(|s| s.defs_dir().parent())
                .map(std::path::Path::to_path_buf);
            self.plugin_scanner
                .sender
                .try_send(ScanJob::Scan(ScanRequest {
                    dirs,
                    cache,
                    client: from,
                }))
                .map_err(|_| "plugin scan worker unavailable")?;
            self.plugin_submitted += 1;
            Ok(())
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = from;
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
            let _ = from;
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
            if self.plugin_submitted - self.plugin_drained >= 4 {
                return Err("plugin worker queue full".into());
            }
            let ticket = self.plugin_submitted + 1;
            self.plugin_scanner
                .sender
                .try_send(ScanJob::Def {
                    name: name.clone(),
                    reference,
                    sample_rate: self.handle.sample_rate,
                    client: from,
                    ticket,
                })
                .map_err(|_| "plugin worker unavailable")?;
            self.claim_def_name(name, DefKind::Plugin);
            self.plugin_claims.insert(name.clone(), ticket);
            self.plugin_submitted += 1;
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
            if self.plugin_submitted - self.plugin_drained >= 4 {
                return Err("plugin worker queue full".into());
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
            self.plugin_scanner
                .sender
                .try_send(ScanJob::Editor {
                    id,
                    expected: *expected,
                    admin: Arc::clone(admin),
                    command,
                    address,
                    client: from,
                })
                .map_err(|_| "plugin worker unavailable")?;
            self.plugin_submitted += 1;
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
            if self.plugin_submitted - self.plugin_drained >= 4 {
                return Err("plugin worker queue full".into());
            }
            self.plugin_scanner
                .sender
                .try_send(ScanJob::Restart {
                    id,
                    expected: *expected,
                    def: Arc::clone(def),
                    sample_rate: self.handle.sample_rate,
                    device_frames: self.translator.device_frames(),
                    client: from,
                })
                .map_err(|_| "plugin worker unavailable")?;
            self.plugin_submitted += 1;
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
            if self.plugin_submitted - self.plugin_drained >= 4 {
                return Err("plugin worker queue full".into());
            }
            admin.reserve()?;
            if self
                .plugin_scanner
                .sender
                .try_send(ScanJob::State {
                    id,
                    expected: *expected,
                    def: Arc::clone(def),
                    admin: Arc::clone(admin),
                    bytes,
                    sample_rate: self.handle.sample_rate,
                    device_frames: self.translator.device_frames(),
                    client: from,
                })
                .is_err()
            {
                admin.abort();
                return Err("plugin worker unavailable".into());
            }
            self.plugin_submitted += 1;
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
    #[cfg(all(
        any(feature = "plugin-clap", feature = "plugin-vst3"),
        target_os = "linux"
    ))]
    pub(in crate::osc::server) fn collect_plugin_results(&mut self) {
        while let Ok(result) = self.plugin_scanner.receiver.try_recv() {
            self.plugin_drained += 1;
            let result = match result {
                ScanResult::Editor {
                    id,
                    expected,
                    result,
                    address,
                    client,
                } => {
                    if self.translator.mirror.get(id).is_none()
                        || !matches!(self.translator.node_defs.get(&id), Some(crate::osc::translate::NodeDef::Plugin(_, generation, _)) if *generation == expected)
                    {
                        self.fail(client, address, "plugin node no longer exists");
                    } else {
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
                    continue;
                }
                ScanResult::Scan(result) => result,
                ScanResult::Def {
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
                                vec![OscType::String("plugin".into()), OscType::String(name)],
                            );
                        }
                        Err(error) => self.fail(client, "/def_send", error),
                    }
                    continue;
                }
                ScanResult::State {
                    id,
                    expected,
                    prior,
                    admin,
                    loading,
                    result,
                    client,
                } => {
                    let command = if loading {
                        "/plugin_loadState"
                    } else {
                        "/plugin_saveState"
                    };
                    let current = matches!(self.translator.node_defs.get(&id), Some(crate::osc::translate::NodeDef::Plugin(def, generation, ..)) if *generation == expected && Arc::ptr_eq(def, &prior));
                    let result = result.and_then(|value| {
                        if current {
                            Ok(value)
                        } else {
                            Err("plugin node no longer exists".into())
                        }
                    });
                    match result {
                        Ok(PreparedState { def, synth, bytes }) => {
                            let generation = crate::node::SynthNode::incarnation(synth.as_ref());
                            let next_admin = synth.administration();
                            if self
                                .send_all([Cmd::RestartSynth {
                                    id,
                                    expected,
                                    synth,
                                }])
                                .is_err()
                            {
                                admin.abort();
                                self.fail(client, command, "command FIFO full");
                            } else {
                                self.translator.retire_plugin_generation(id, expected);
                                if loading {
                                    for (index, value) in def
                                        .defaults
                                        .iter()
                                        .take(def.info.parameters.len())
                                        .enumerate()
                                    {
                                        let mapped = matches!(self.translator.mirror.get(id).map(|node| &node.body), Some(crate::osc::graph::MirrorBody::Synth { maps, .. }) if maps.iter().any(|(control, ..)| *control == index as u32));
                                        if !mapped {
                                            self.translator.mirror.set_control(
                                                id,
                                                index as u32,
                                                *value,
                                            );
                                        }
                                    }
                                }
                                self.translator.node_defs.insert(
                                    id,
                                    crate::osc::translate::NodeDef::Plugin(
                                        def, generation, next_admin,
                                    ),
                                );
                                if !loading {
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
                                                OscType::Blob(
                                                    bytes[start..bytes.len().min(start + 6144)]
                                                        .to_vec(),
                                                ),
                                            ],
                                        );
                                    }
                                }
                                self.done_with(client, command, vec![OscType::Int(id)]);
                            }
                        }
                        Err(error) => {
                            admin.abort();
                            self.fail(client, command, error);
                        }
                    }
                    continue;
                }
                ScanResult::Restart {
                    id,
                    expected,
                    def,
                    result,
                    client,
                } => {
                    let current = matches!(self.translator.node_defs.get(&id), Some(crate::osc::translate::NodeDef::Plugin(current, generation, ..)) if *generation == expected && Arc::ptr_eq(current, &def));
                    if !current {
                        self.fail(client, "/plugin_restart", "plugin node no longer exists");
                        continue;
                    }
                    match result {
                        Ok(synth) => {
                            let generation = crate::node::SynthNode::incarnation(synth.as_ref());
                            let admin = synth.administration();
                            if self
                                .send_all([Cmd::RestartSynth {
                                    id,
                                    expected,
                                    synth,
                                }])
                                .is_err()
                            {
                                self.fail(client, "/plugin_restart", "command FIFO full");
                            } else {
                                self.translator.retire_plugin_generation(id, expected);
                                self.translator.node_defs.insert(
                                    id,
                                    crate::osc::translate::NodeDef::Plugin(def, generation, admin),
                                );
                                self.done_with(client, "/plugin_restart", vec![OscType::Int(id)]);
                            }
                        }
                        Err(error) => self.fail(client, "/plugin_restart", error),
                    }
                    continue;
                }
            };
            match result.result {
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
            }
        }
        self.resolve_syncs();
    }
}
