//! `/node_*`, `/group_*`, `/synth_*`: the node tree, and what it reports.
//!
//! The mutations are all one path -- [`OscServer::handle_via_translate`], which
//! runs the command through [`CmdTranslator`] so the tree mirror stays in step
//! with the engine -- and the queries read that mirror. Nothing here touches
//! the engine directly.

use super::super::*;

impl OscServer {
    /// Immediate form of every translator-covered command: translate (which
    /// also updates the tree mirror and may append re-sort moves), then
    /// ship the whole batch.
    pub(in crate::osc::server) fn handle_via_translate(
        &mut self,
        msg: &OscMessage,
        from: ClientId,
    ) {
        let mut cmds = Vec::new();
        if let Err(e) = self.translator.translate(msg, &mut cmds) {
            return self.fail(from, &msg.addr, e);
        }
        for cmd in cmds {
            if self.handle.send(cmd).is_err() {
                return self.fail(from, &msg.addr, "command FIFO full");
            }
        }
    }

    /// write the current MIDI bindings to disk after a mutation, if
    /// persistence is on. Best-effort; a write error is logged, never fatal.
    pub(in crate::osc::server) fn persist_bindings(&self) {
        if let Some(store) = &self.store
            && let Err(e) = store.save_bindings(&self.translator.midi.persist())
        {
            error!("could not persist MIDI bindings: {e}");
        }
    }

    /// The node tree as seen by the network-side mirror, in scsynth's
    /// `/group_queryTree.reply` format. Args: [groupID = 0, detail = 0]; detail 1
    /// includes control names and values (scsynth's flag), detail 2 also the
    /// maps and inferred bus lists, which makes each entry a full node info.
    pub(in crate::osc::server) fn handle_group_query_tree(
        &mut self,
        msg: &OscMessage,
        from: ClientId,
    ) {
        let group = int_arg(&msg.args, 0).unwrap_or(0);
        let detail = int_arg(&msg.args, 1).unwrap_or(0).clamp(0, 2);
        match self.translator.query_tree(group, detail) {
            Ok(args) => self.reply(from, "/group_queryTree.reply", args),
            Err(e) => self.fail(from, "/group_queryTree", e),
        }
    }

    /// Per-node detail: replies `/node_query.reply` for each queried node ID (scsynth's
    /// `/node_query`, extended with the def name, controls, maps and inferred
    /// bus usage — see [`CmdTranslator::node_info`]). An id the server does
    /// not hold answers with an absent record, not `/fail`: only a malformed
    /// request is a protocol error.
    pub(in crate::osc::server) fn handle_node_query(&mut self, msg: &OscMessage, from: ClientId) {
        for arg in &msg.args {
            let OscType::Int(id) = arg else {
                return self.fail(from, "/node_query", "expected int node ids");
            };
            let args = self.translator.node_info(*id);
            self.reply(from, "/node_query.reply", args);
        }
    }

    /// `/group_query path...`: resolves each path to the node it names,
    /// replying `/group_query.reply <path> <nodeID>` — the one place a path is
    /// interpreted. A path nothing answers to resolves to `-1` (absence is a
    /// state, as in `/node_query`), so one dead path does not abort the rest.
    pub(in crate::osc::server) fn handle_group_query(&mut self, msg: &OscMessage, from: ClientId) {
        for arg in &msg.args {
            let OscType::String(path) = arg else {
                return self.fail(from, "/group_query", "expected string paths");
            };
            let id = self.translator.resolve_path(path);
            self.reply(
                from,
                "/group_query.reply",
                vec![OscType::String(path.clone()), OscType::Int(id)],
            );
        }
    }

    /// Debug: the inferred bus graph of one group as a string reply.
    pub(in crate::osc::server) fn handle_group_dump_graph(
        &mut self,
        msg: &OscMessage,
        from: ClientId,
    ) {
        let group = int_arg(&msg.args, 0).unwrap_or(0);
        match self.translator.dump_graph(group) {
            Ok(dump) => self.reply(
                from,
                "/group_dumpGraph.reply",
                vec![OscType::Int(group), OscType::String(dump)],
            ),
            Err(e) => self.fail(from, "/group_dumpGraph", e),
        }
    }

    /// `/synth_get nodeID control...` / `/synth_getRange nodeID control numControls...`:
    /// reads a synth's current control values from the mirror and replies
    /// `/node_set nodeID control value ...` (`/synth_getRange` echoes each range's
    /// `(control, numControls, val...)`), the query counterpart of `/node_set`.
    pub(in crate::osc::server) fn handle_synth_get(
        &mut self,
        msg: &OscMessage,
        from: ClientId,
        ranged: bool,
    ) {
        let addr = if ranged {
            "/synth_getRange"
        } else {
            "/synth_get"
        };
        let Some(OscType::Int(id)) = msg.args.first() else {
            return self.fail(from, addr, "expected: nodeID, then controls");
        };
        let Some(def) = self.translator.node_defs.get(id).cloned() else {
            return self.fail(from, addr, format!("synth {id} not found"));
        };
        let Some((_, controls)) = self.translator.mirror.synth_info(*id) else {
            return self.fail(from, addr, format!("node {id} is not a synth"));
        };
        let mut args = vec![OscType::Int(*id)];
        let read = |index: u32| -> Result<f32, String> {
            controls
                .get(index as usize)
                .copied()
                .ok_or_else(|| format!("control index {index} out of range"))
        };
        if ranged {
            for pair in msg.args[1..].chunks(2) {
                let (Some(base), Some(OscType::Int(count))) =
                    (pair.first().and_then(|a| control_key(a, &def)), pair.get(1))
                else {
                    return self.fail(from, addr, "expected (control, numControls) pairs");
                };
                let Ok(count) = u32::try_from(*count) else {
                    return self.fail(from, addr, "numControls must be >= 0");
                };
                args.push(OscType::Int(base as i32));
                args.push(OscType::Int(count as i32));
                for offset in 0..count {
                    match read(base + offset) {
                        Ok(v) => args.push(OscType::Float(v)),
                        Err(e) => return self.fail(from, addr, e),
                    }
                }
            }
        } else {
            for arg in &msg.args[1..] {
                let Some(index) = control_key(arg, &def) else {
                    return self.fail(from, addr, "unknown control");
                };
                match read(index) {
                    Ok(v) => {
                        args.push(OscType::Int(index as i32));
                        args.push(OscType::Float(v));
                    }
                    Err(e) => return self.fail(from, addr, e),
                }
            }
        }
        self.reply(from, "/node_set", args);
    }

    /// `/synth_forgetId nodeID...`: in scsynth this releases the integer node IDs so the
    /// server may reuse them. Clausters allocates IDs per client (auto IDs are
    /// server-assigned, negative-free), never reclaims an in-use ID, and never
    /// reuses a freed one under a live node, so there is nothing to release; we
    /// validate the IDs name live synths and acknowledge. Deliberate deviation
    /// (the plan's "compatibility of model, not literal copy").
    pub(in crate::osc::server) fn handle_synth_forget_id(
        &mut self,
        msg: &OscMessage,
        from: ClientId,
    ) {
        if msg.args.is_empty() {
            return self.fail(from, "/synth_forgetId", "expected node IDs");
        }
        for arg in &msg.args {
            let OscType::Int(id) = arg else {
                return self.fail(from, "/synth_forgetId", "expected int node IDs");
            };
            if !self.translator.node_defs.contains_key(id) {
                return self.fail(from, "/synth_forgetId", format!("synth {id} not found"));
            }
        }
        self.reply(
            from,
            "/done",
            vec![OscType::String("/synth_forgetId".into())],
        );
    }

    /// `/node_trace nodeID...`: debug-traces a node by logging its current control
    /// values (from the mirror) to the server console — the introspection
    /// counterpart of scsynth's per-block node trace. Network-thread only, no
    /// reply (matches scsynth).
    pub(in crate::osc::server) fn handle_node_trace(&mut self, msg: &OscMessage, from: ClientId) {
        for arg in &msg.args {
            let OscType::Int(id) = arg else {
                return self.fail(from, "/node_trace", "expected int node IDs");
            };
            match self.translator.mirror.synth_info(*id) {
                Some((name, controls)) => {
                    info!(target: crate::logging::OSC_TARGET, "/node_trace node {id} synth {name:?} controls {controls:?}");
                }
                None if self.translator.mirror.get(*id).is_some() => {
                    let children = self.translator.mirror.children(*id).unwrap_or(&[]);
                    info!(target: crate::logging::OSC_TARGET, "/node_trace node {id} group children {children:?}");
                }
                None => {
                    info!(target: crate::logging::OSC_TARGET, "/node_trace node {id} not found")
                }
            }
        }
    }
}
