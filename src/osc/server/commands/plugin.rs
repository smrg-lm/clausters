//! Native plugin discovery, always outside the network/audio threads.
use super::super::*;

#[cfg(all(feature = "plugin-clap", target_os = "linux"))]
pub(in crate::osc::server) struct ScanWorker {
    sender: std::sync::mpsc::SyncSender<ScanJob>,
    receiver: std::sync::mpsc::Receiver<ScanResult>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
#[cfg(all(feature = "plugin-clap", target_os = "linux"))]
struct ScanJob {
    dirs: Vec<std::path::PathBuf>,
    cache: Option<std::path::PathBuf>,
    client: ClientId,
}
#[cfg(all(feature = "plugin-clap", target_os = "linux"))]
struct ScanResult {
    client: ClientId,
    result: Result<Vec<clausters_plugin_host::format::ScanEntry>, String>,
}
#[cfg(all(feature = "plugin-clap", target_os = "linux"))]
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
                    if outgoing
                        .try_send(ScanResult {
                            client: job.client,
                            result,
                        })
                        .is_err()
                    {
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
#[cfg(all(feature = "plugin-clap", target_os = "linux"))]
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
        #[cfg(all(feature = "plugin-clap", target_os = "linux"))]
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
                .try_send(ScanJob {
                    dirs,
                    cache,
                    client: from,
                })
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
            not(all(feature = "plugin-clap", target_os = "linux"))
        ))]
        {
            let _ = from;
            Err("server built without native CLAP support".into())
        }
    }
    #[cfg(all(feature = "plugin-clap", target_os = "linux"))]
    pub(in crate::osc::server) fn collect_plugin_results(&mut self) {
        while let Ok(result) = self.plugin_scanner.receiver.try_recv() {
            self.plugin_drained += 1;
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
