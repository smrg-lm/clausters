//! The work that does not finish inside one turn of the loop.
//!
//! Buffer commands go to the NRT thread and Faust defs to the compiler thread;
//! both reply `/done` or `/fail` whenever they are done, out of order with
//! everything else. `/server_sync` is the barrier over both: it records the
//! submitted counts as its targets and answers once the drained counts have
//! caught up, which is what makes "wait until my defs are loaded" expressible
//! without blocking the network thread.

use super::*;

impl OscServer {
    /// Drains finished compilations: stores factories and sends the async
    /// `/done`/`/fail` replies. Called from the same places as
    /// `collect_garbage` (after each packet and on the GC tick).
    #[cfg(feature = "faust")]
    pub(in crate::osc::server) fn collect_faust_results(&mut self) {
        while let Some(result) = self.faust_compiler.try_result() {
            self.faust_drained += 1;
            match result.outcome {
                Ok(def) => {
                    self.translator
                        .faust_defs
                        .insert(result.name.clone(), Arc::new(def));
                    // No client on a startup reload: nothing to answer.
                    if let Some(client) = result.client {
                        self.reply(
                            client,
                            "/done",
                            vec![
                                OscType::String("/def_send".into()),
                                OscType::String("faust".into()),
                                OscType::String(result.name),
                            ],
                        );
                    }
                }
                Err(error) => match result.client {
                    Some(client) => self.fail(client, "/def_send", error),
                    None => warn!("persisted Faust def '{}' failed: {error}", result.name),
                },
            }
        }
        self.resolve_syncs();
    }

    /// `/def_send faust <name> <def>`: queue an async Faust compilation. The def
    /// format is sniffed by [`CompilePayload::classify`]: raw Faust source,
    /// a JSON box graph (`faust::boxes`), or a JSON signal tree
    /// (`faust::signals`, root `{"signals": …}`).
    #[cfg(feature = "faust")]
    pub(in crate::osc::server) fn handle_def_send_faust(
        &mut self,
        args: &[OscType],
        from: ClientId,
    ) {
        let (name, def) = match crate::osc::translate::parse_def_send_faust(args) {
            Ok(pair) => pair,
            Err(e) => return self.fail(from, "/def_send", e),
        };
        let payload = CompilePayload::classify(def);
        self.claim_def_name(&name, DefKind::Faust);
        // A live faust /def_send always compiles fresh from the given def and, with
        // persistence on, (re)writes the cache (restore = None). An ephemeral
        // def never reaches the store: its bitcode speed-cache goes to the OS
        // temp directory instead, so replaying the same expression still skips
        // the recompile without leaving a record behind.
        let cache = if defstore::is_ephemeral(&name) {
            let dir = defstore::ephemeral_dir();
            std::fs::create_dir_all(&dir)
                .is_ok()
                .then(|| Box::new(CacheJob { dir, restore: None }))
        } else {
            self.store.as_ref().map(|s| {
                Box::new(CacheJob {
                    dir: s.faustdefs_dir().to_path_buf(),
                    restore: None,
                })
            })
        };
        let request = CompileRequest {
            name,
            payload,
            client: Some(from),
            cache,
        };
        if self.faust_compiler.submit(request).is_err() {
            self.fail(from, "/def_send", "compiler thread is down");
        } else {
            self.faust_submitted += 1;
        }
    }

    #[cfg(not(feature = "faust"))]
    pub(in crate::osc::server) fn handle_def_send_faust(
        &mut self,
        _args: &[OscType],
        from: ClientId,
    ) {
        self.fail(from, "/def_send", "server built without faust support");
    }

    /// `/server_sync id`: the async barrier (scsynth semantics). Records the current
    /// submitted counts as targets and is answered with `/server_sync.reply id` once both
    /// async pipelines (NRT buffers, Faust compiles) have drained up to them —
    /// i.e. every async command received before this `/server_sync` has finished.
    /// Each pipeline completes FIFO, so the counters are a sufficient barrier.
    pub(in crate::osc::server) fn handle_server_sync(&mut self, msg: &OscMessage, from: ClientId) {
        let id = match msg.args.first() {
            Some(OscType::Int(id)) => *id,
            _ => return self.fail(from, "/server_sync", "expected an int id"),
        };
        self.pending_syncs.push(PendingSync {
            client: from,
            id,
            nrt_target: self.nrt_submitted,
            faust_target: self.faust_submitted,
        });
        self.resolve_syncs(); // answer at once if nothing is outstanding
    }

    /// Answers every pending `/server_sync` whose target counts have been reached.
    /// Called after each async drain (and from [`Self::handle_server_sync`]).
    fn resolve_syncs(&mut self) {
        if self.pending_syncs.is_empty() {
            return;
        }
        let (nrt, faust) = (self.nrt_drained, self.faust_drained);
        let mut ready = Vec::new();
        self.pending_syncs.retain(|p| {
            let done = nrt >= p.nrt_target && faust >= p.faust_target;
            if done {
                ready.push((p.client, p.id));
            }
            !done
        });
        for (client, id) in ready {
            self.reply(client, "/server_sync.reply", vec![OscType::Int(id)]);
        }
    }

    pub(in crate::osc::server) fn collect_nrt_results(&mut self) {
        while let Some(result) = self.nrt.try_result() {
            self.nrt_drained += 1;
            let action = match result.outcome {
                Ok(action) => action,
                Err(error) => {
                    self.fail(result.client, result.cmd, error);
                    continue;
                }
            };
            let index = result.index as usize;
            let swap = match action {
                NrtAction::Install(buffer) => {
                    self.translator.buffers[index] = Some(Arc::clone(&buffer));
                    Some(Some(buffer))
                }
                NrtAction::Clear => {
                    self.translator.buffers[index] = None;
                    Some(None)
                }
                NrtAction::None => None,
            };
            if let Some(buffer) = swap
                && self.handle.send(Cmd::SetBuffer { index, buffer }).is_err()
            {
                self.fail(result.client, result.cmd, "command FIFO full");
                continue;
            }
            self.reply(
                result.client,
                "/done",
                vec![
                    OscType::String(result.cmd.into()),
                    OscType::Int(result.index),
                ],
            );
        }
        self.resolve_syncs();
    }

    /// Queues a built NRT job, failing back to the client if the thread is gone.
    pub(in crate::osc::server) fn submit_nrt(
        &mut self,
        cmd: &'static str,
        index: i32,
        from: ClientId,
        job: NrtJob,
    ) {
        let request = NrtRequest {
            cmd,
            index,
            client: from,
            job,
        };
        if self.nrt.submit(request).is_err() {
            self.fail(from, cmd, "NRT thread is down");
        } else {
            self.nrt_submitted += 1;
        }
    }

    /// Drains finished NRT jobs: installs/clears buffers in the engine and
    /// the mirror, and sends the async `/done cmd bufnum` / `/fail` replies.
    /// Installs a host-built buffer at `index`: the network-side mirror and
    /// the engine swap, exactly the `NrtAction::Install` path minus the OSC
    /// reply. The embed `buffer_load` door: a headless host hands the server
    /// samples it decoded itself (the browser's `/buffer_allocRead` replacement,
    /// where there is no filesystem).
    pub fn install_buffer(
        &mut self,
        index: usize,
        buffer: Arc<crate::dsp::buffer::Buffer>,
    ) -> Result<(), String> {
        if index >= self.translator.buffers.len() {
            return Err(format!(
                "buffer index {index} out of range (max {})",
                self.translator.buffers.len() - 1
            ));
        }
        self.translator.buffers[index] = Some(Arc::clone(&buffer));
        self.handle
            .send(Cmd::SetBuffer {
                index,
                buffer: Some(buffer),
            })
            .map_err(|_| "command FIFO full".to_string())
    }
}
