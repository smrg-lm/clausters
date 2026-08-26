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
                    // No client: this is the startup reload, and a def that no
                    // longer compiles would warn here at every boot. Same rule
                    // as the synchronous families (`retire_dead_defs`): named
                    // by default, dropped under `--prune-defs`.
                    None => {
                        warn!("persisted Faust def '{}' failed: {error}", result.name);
                        match (self.prune_dead_defs, &self.store) {
                            // `store` is unused in a page: there is no def
                            // store there, so this arm cannot be reached.
                            #[cfg_attr(target_arch = "wasm32", allow(unused_variables))]
                            (true, Some(store)) => {
                                // A page has no def store, so nothing lands
                                // in this arm there and there is no cache.
                                #[cfg(not(target_arch = "wasm32"))]
                                crate::faust::cache::remove(store.faustdefs_dir(), &result.name);
                                warn!("pruned the persisted def '{}'", result.name);
                            }
                            (false, Some(store)) => warn!(
                                "it will warn again at every boot; it is in {} — drop the dead \
                                 ones with `clausters --prune-defs`",
                                store.defs_dir().display()
                            ),
                            _ => {}
                        }
                    }
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
            if let std::collections::hash_map::Entry::Occupied(mut e) =
                self.nrt_in_flight.entry(result.index)
            {
                *e.get_mut() -= 1;
                if *e.get() == 0 {
                    e.remove();
                }
            }
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
                    // **Where a buffer lives is decided here, once.** With a
                    // segment attached the samples go into a region a peer
                    // can map by name, so an editor draws and writes them with no
                    // message at all; with none they stay the server's own
                    // memory, exactly as before. The copy is paid at
                    // *allocation*, which is where the data was being built
                    // anyway -- never per write.
                    let buffer = self.share_buffer(index, buffer);
                    self.translator.buffers[index] = Some(Arc::clone(&buffer));
                    Some(Some(buffer))
                }
                NrtAction::Clear => {
                    self.retire_buffer(index);
                    self.translator.buffers[index] = None;
                    Some(None)
                }
                // **A write in place, and the span it covered.** Nothing is
                // installed -- the cells the engine reads are already the new
                // ones -- so all this owes is the summary over them: the
                // overview beside the region follows the span, and every
                // *client* holding a picture is told by whoever asked for the
                // write, exactly as a peer's own edit is announced.
                NrtAction::Wrote { start, frames } => {
                    if let Some(buffer) = self
                        .translator
                        .buffers
                        .get(index)
                        .and_then(|b| b.as_ref().cloned())
                    {
                        self.overviews.wrote(index, &buffer, start, frames);
                    }
                    None
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

    /// Moves an installed buffer into a **mapped region** when this server has
    /// a segment, and publishes its row so a peer can find it.
    ///
    /// The region is named from the segment's path, the buffer number and the
    /// generation the directory hands back — so the file of a freed buffer and
    /// the file of its replacement can never be the same name, and a peer that
    /// kept the old mapping is writing into memory nobody reads rather than
    /// into somebody else's take.
    #[cfg(unix)]
    fn share_buffer(
        &mut self,
        index: usize,
        buffer: Arc<crate::dsp::buffer::Buffer>,
    ) -> Arc<crate::dsp::buffer::Buffer> {
        use crate::dsp::region::Region;
        if !self.owns_samples {
            // Somebody else's directory: this server's own allocations stay in
            // its own memory rather than taking a row and a buffer number that
            // are the owner's to hand out.
            return buffer;
        }
        let Some(path) = self.shm_path.clone() else {
            return buffer; // no segment: the server's own memory, as always
        };
        let Some(segment) = self.segment.clone() else {
            return buffer;
        };
        if self.shared_buffers.len() < self.translator.buffers.len() {
            self.shared_buffers
                .resize_with(self.translator.buffers.len(), || None);
        }
        let Some(generation) = segment.publish_buffer(
            index,
            buffer.frames(),
            buffer.channels(),
            buffer.sample_rate(),
        ) else {
            return buffer; // a buffer number the directory has no row for
        };
        let region_path = Region::path_for(&path, index, generation);
        let region = match Region::create(&region_path, buffer.len()) {
            Ok(region) => Arc::new(region),
            Err(e) => {
                tracing::warn!("buffer {index}: cannot share its samples: {e}");
                segment.retire_buffer(index);
                return buffer;
            }
        };
        // The one copy: what was just built, into the memory it will live in.
        let cells = region.cells();
        for (cell, value) in cells.iter().zip(buffer.cells()) {
            cell.store(
                value.load(std::sync::atomic::Ordering::Relaxed),
                std::sync::atomic::Ordering::Relaxed,
            );
        }
        // **The overview beside it**, built from the samples that were just
        // copied in: the one full pass, paid where the copy already is, so a
        // peer opening this take maps a summary instead of computing one.
        self.overviews.publish(index, &region_path, &buffer);
        self.shared_buffers[index] = Some(region_path);
        // Shared samples are samples somebody may be drawing, so the buffer publishes
        // how far it has been written: a recording fills a picture in another
        // process with one relaxed store per block and no message at all.
        Arc::new(
            crate::dsp::buffer::Buffer::shared(
                region,
                buffer.channels(),
                buffer.frames(),
                buffer.sample_rate(),
            )
            .with_frontier(segment.frontier_sink(index)),
        )
    }

    /// Sharing samples needs a mapped region, and a region is a file
    /// somebody else can open — which off Unix (the wasm engine, above all)
    /// there is no equivalent of. A buffer stays the server's own memory
    /// there, exactly as it does with no segment at all.
    #[cfg(not(unix))]
    fn share_buffer(
        &mut self,
        _index: usize,
        buffer: Arc<crate::dsp::buffer::Buffer>,
    ) -> Arc<crate::dsp::buffer::Buffer> {
        buffer
    }

    /// Empties a directory row and unlinks the region behind it.
    ///
    /// **Unlink, not delete**: every mapping a peer still holds stays valid
    /// until it drops it, which is what makes freeing a buffer safe while
    /// somebody is drawing it. What the peer sees is the row going even.
    fn retire_buffer(&mut self, index: usize) {
        if !self.owns_samples {
            // Freeing a buffer here frees this server's *mapping* of it. The
            // row and the region are the owner's, and a player retiring them
            // would free samples out from under whoever is editing it.
            return;
        }
        if let Some(segment) = self.segment.as_ref() {
            segment.retire_buffer(index);
        }
        #[cfg(unix)]
        if let Some(path) = self.shared_buffers.get_mut(index).and_then(Option::take) {
            crate::dsp::region::Region::unlink(&path);
            self.overviews.retire(index);
        }
    }

    /// Queues a built NRT job, failing back to the client if the thread is gone.
    pub(in crate::osc::server) fn submit_nrt(
        &mut self,
        cmd: &'static str,
        index: i32,
        from: ClientId,
        job: NrtJob,
    ) {
        // A job that rebuilds a buffer from its current contents must build on
        // what the *queue* last produced, not on the snapshot its parse took
        // from the mirror -- but only while the queue still owes work on that
        // index, since with nothing in flight the mirror has caught up and is
        // the authority (see `NrtChain`). That is exactly what this count says.
        let chained = *self.nrt_in_flight.get(&index).unwrap_or(&0) > 0;
        let request = NrtRequest {
            cmd,
            index,
            client: from,
            chained,
            job,
        };
        if self.nrt.submit(request).is_err() {
            self.fail(from, cmd, "NRT thread is down");
        } else {
            self.nrt_submitted += 1;
            *self.nrt_in_flight.entry(index).or_insert(0) += 1;
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
