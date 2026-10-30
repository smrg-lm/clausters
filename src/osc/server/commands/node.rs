//! `/node_*`, `/group_*`, `/synth_*`: the node tree, and what it reports.
//!
//! The mutations are all one path -- [`OscServer::handle_via_translate`], which
//! runs the command through [`CmdTranslator`] so the tree mirror stays in step
//! with the engine -- and the queries read that mirror. Nothing here touches
//! the engine directly.

use super::super::*;
use clausters_core::node_editor;

impl OscServer {
    /// Immediate form of every translator-covered command: translate (which
    /// also updates the tree mirror and may append re-sort moves), then
    /// ship the whole batch.
    pub(in crate::osc::server) fn handle_via_translate(
        &mut self,
        _addr: &'static str,
        msg: &OscMessage,
        _from: ClientId,
    ) -> Answer {
        let mut cmds = Vec::new();
        self.translator.translate(msg, &mut cmds)?;
        self.send_all(cmds)
    }

    /// Write the current MIDI bindings to disk after a mutation, if
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
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let group = args.opt_int()?.unwrap_or(0);
        let detail = args.opt_int()?.unwrap_or(0).clamp(0, 2);
        let tree = self.translator.query_tree(group, detail)?;
        self.reply(from, "/group_queryTree.reply", tree);
        Ok(())
    }

    /// Per-node detail: replies `/node_query.reply` for each queried node ID (scsynth's
    /// `/node_query`, extended with the def name, controls, maps and inferred
    /// bus usage -- see [`CmdTranslator::node_info`]). An id the server does
    /// not hold answers with an absent record, not `/fail`: only a malformed
    /// request is a protocol error.
    pub(in crate::osc::server) fn handle_node_query(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        while !args.is_empty() {
            let id = args.int()?;
            let info = self.translator.node_info(id);
            self.reply(from, "/node_query.reply", info);
        }
        Ok(())
    }

    /// `/group_query path...`: resolves each path to the node it names,
    /// replying `/group_query.reply <path> <nodeID>` -- the one place a path is
    /// interpreted. A path nothing answers to resolves to `-1` (absence is a
    /// state, as in `/node_query`), so one dead path does not abort the rest.
    pub(in crate::osc::server) fn handle_group_query(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        while !args.is_empty() {
            let path = args.str()?.to_string();
            let id = self.translator.resolve_path(&path);
            self.reply(
                from,
                "/group_query.reply",
                vec![OscType::String(path), OscType::Int(id)],
            );
        }
        Ok(())
    }

    /// Debug: the inferred bus graph of one group as a string reply.
    pub(in crate::osc::server) fn handle_group_dump_graph(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let group = args.opt_int()?.unwrap_or(0);
        let dump = self.translator.dump_graph(group)?;
        self.reply(
            from,
            "/group_dumpGraph.reply",
            vec![OscType::Int(group), OscType::String(dump)],
        );
        Ok(())
    }

    /// `/synth_get nodeID control...` / `/synth_getRange nodeID control numControls...`:
    /// reads a synth's current control values from the mirror -- or, for a
    /// control mapped to a control bus, from the bus it follows -- and replies
    /// `/node_set nodeID control value ...` (`/synth_getRange` echoes each range's
    /// `(control, numControls, val...)`), the query counterpart of `/node_set`.
    pub(in crate::osc::server) fn handle_synth_get(
        &mut self,
        mut args: Args,
        from: ClientId,
        ranged: bool,
    ) -> Answer {
        let id = args.int()?;
        if !ranged && self.translator.is_graph(id) {
            return self.graph_get(id, args, from);
        }
        let def = self
            .translator
            .node_defs
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("synth {id} not found"))?;
        let (_, controls) = self
            .translator
            .mirror
            .synth_info(id)
            .ok_or_else(|| format!("node {id} is not a synth"))?;
        let read = |index: u32| -> Result<f32, String> {
            controls
                .get(index as usize)
                .map(|&set| self.control_now(id, index, set))
                .ok_or_else(|| format!("control index {index} out of range"))
        };
        // A control is named or numbered, so it is the one read `Args` cannot
        // do on its own: resolving it needs the def. It is answered as it was
        // asked for, so the reply is the `/node_set` a binding to that name
        // (or that index) would send.
        let control = |args: &mut Args| -> Result<(OscType, u32), String> {
            let arg = args.one()?;
            let index = control_key(arg, &def).ok_or_else(|| format!("unknown control {arg:?}"))?;
            Ok((arg.clone(), index))
        };
        let mut out = vec![OscType::Int(id)];
        while !args.is_empty() {
            let (asked, base) = control(&mut args)?;
            out.push(asked);
            if ranged {
                let count = u32::try_from(args.int()?).map_err(|_| "numControls must be >= 0")?;
                out.push(OscType::Int(count as i32));
                for offset in 0..count {
                    out.push(OscType::Float(read(base + offset)?));
                }
            } else {
                out.push(OscType::Float(read(base)?));
            }
        }
        self.reply(from, "/node_set", out);
        Ok(())
    }

    /// **A control's value now**: what the bus it is mapped to carries, for
    /// a control mapped to a control bus (an atomic this thread can read),
    /// and otherwise `set`, the value the mirror holds -- the last one a
    /// `/node_set` wrote. An audio-mapped control has no single value to
    /// read and answers `set` too.
    fn control_now(&self, id: i32, index: u32, set: f32) -> f32 {
        match self.translator.mirror.control_bus(id, index) {
            Some(bus) => self.handle.control_buses().get(bus),
            None => set,
        }
    }

    /// `/synth_get` on a **GraphDef instance** or one of its voices: each
    /// surface port read back as the `/node_set` that set it wrote it
    /// (`CmdTranslator::graph_port_value`).
    fn graph_get(&mut self, id: i32, mut args: Args, from: ClientId) -> Answer {
        let mut out = vec![OscType::Int(id)];
        while !args.is_empty() {
            let asked = args.one()?.clone();
            let OscType::String(port) = &asked else {
                return Err(format!("a graph's port is named, not {asked:?}"));
            };
            let value = self
                .translator
                .graph_port_value(id, port)
                .ok_or_else(|| format!("graph {id} has no port {port:?} to read"))?;
            out.push(asked);
            out.push(OscType::Float(value));
        }
        self.reply(from, "/node_set", out);
        Ok(())
    }

    /// `/synth_getText nodeID control...`: each control's current value **as
    /// text** -- the plugin's own words for it (`"-6.0 dB"`, `"Hall"`), or for a
    /// SynthDef or a FaustDef the number with its unit -- answered as
    /// `/node_setText nodeID (control text)...`, the message that would write
    /// it back. A plugin is asked on its control worker, never from audio, so
    /// its answer comes later; while one for the same node and asker is on its
    /// way, another is not started (the asker polls, and the next one will be).
    pub(in crate::osc::server) fn handle_synth_get_text(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let id = args.int()?;
        let controls = self
            .translator
            .node_controls(id)
            .ok_or_else(|| format!("synth {id} not found"))?;
        let (_, values) = self
            .translator
            .mirror
            .synth_info(id)
            .ok_or_else(|| format!("node {id} is not a synth"))?;
        let def = self
            .translator
            .node_defs
            .get(&id)
            .cloned()
            .ok_or("no def")?;
        let mut asked = Vec::with_capacity(args.len());
        while !args.is_empty() {
            let arg = args.one()?;
            let index = control_key(arg, &def).ok_or_else(|| format!("unknown control {arg:?}"))?;
            let value = values
                .get(index as usize)
                .map(|&set| self.control_now(id, index, set))
                .ok_or_else(|| format!("control index {index} out of range"))?;
            asked.push((arg.clone(), index, f64::from(value)));
        }
        #[cfg(all(
            any(feature = "plugin-clap", feature = "plugin-vst3"),
            target_os = "linux"
        ))]
        if self.plugin_text_read(id, &asked, from)? {
            return Ok(());
        }
        let mut out = vec![OscType::Int(id)];
        for (asked, index, value) in asked {
            let unit = controls.get(index as usize).map_or("", |c| c.unit());
            out.push(asked);
            out.push(OscType::String(node_editor::format_value(value, unit)));
        }
        self.reply(from, "/node_setText", out);
        Ok(())
    }

    /// `/node_setText nodeID (control text)...`: sets each control from text,
    /// read the way `/synth_getText` writes it -- by the plugin, which takes
    /// its own words for a value, or as a number with its unit or without. Text
    /// that does not read as a value fails and changes nothing.
    pub(in crate::osc::server) fn handle_node_set_text(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let id = args.int()?;
        let controls = self
            .translator
            .node_controls(id)
            .ok_or_else(|| format!("synth {id} not found"))?;
        let def = self
            .translator
            .node_defs
            .get(&id)
            .cloned()
            .ok_or("no def")?;
        let mut pairs = Vec::new();
        while !args.is_empty() {
            let arg = args.one()?.clone();
            let index =
                control_key(&arg, &def).ok_or_else(|| format!("unknown control {arg:?}"))?;
            let text = args.str()?.to_owned();
            pairs.push((arg, index, text));
        }
        let mut set = vec![OscType::Int(id)];
        for (arg, index, text) in pairs {
            #[cfg(all(
                any(feature = "plugin-clap", feature = "plugin-vst3"),
                target_os = "linux"
            ))]
            if self.plugin_text_write(id, &arg, index, &text, from)? {
                continue;
            }
            let unit = controls.get(index as usize).map_or("", |c| c.unit());
            let value = node_editor::parse_value(&text, unit)
                .ok_or_else(|| format!("{text:?} is not a value of {arg:?}"))?;
            set.push(arg);
            set.push(OscType::Float(value as f32));
        }
        if set.len() > 1 {
            let msg = OscMessage {
                addr: "/node_set".into(),
                args: set,
            };
            self.handle_via_translate("/node_set", &msg, from)?;
        }
        Ok(())
    }

    /// `/synth_forgetId nodeID...`: in scsynth this releases the integer node IDs so the
    /// server may reuse them. Clausters allocates IDs per client (auto IDs are
    /// server-assigned, negative-free), never reclaims an in-use ID, and never
    /// reuses a freed one under a live node, so there is nothing to release; we
    /// validate the IDs name live synths and acknowledge. Deliberate deviation
    /// (the plan's "compatibility of model, not literal copy").
    pub(in crate::osc::server) fn handle_synth_forget_id(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        if args.is_empty() {
            return Err("expected node IDs".into());
        }
        let ids: Vec<i32> = {
            let mut ids = Vec::with_capacity(args.len());
            while !args.is_empty() {
                ids.push(args.int()?);
            }
            ids
        };
        for id in &ids {
            if !self.translator.node_defs.contains_key(id) {
                return Err(format!("synth {id} not found"));
            }
        }
        self.done(from, "/synth_forgetId");
        Ok(())
    }

    /// `/node_trace nodeID...`: debug-traces a node by logging its current control
    /// values (from the mirror) to the server console -- the introspection
    /// counterpart of scsynth's per-block node trace. Network-thread only, no
    /// reply (matches scsynth).
    pub(in crate::osc::server) fn handle_node_trace(&mut self, mut args: Args) -> Answer {
        while !args.is_empty() {
            let id = args.int()?;
            match self.translator.mirror.synth_info(id) {
                Some((name, controls)) => {
                    info!(target: crate::logging::OSC_TARGET, "/node_trace node {id} synth {name:?} controls {controls:?}");
                }
                None if self.translator.mirror.get(id).is_some() => {
                    let children = self.translator.mirror.children(id).unwrap_or(&[]);
                    info!(target: crate::logging::OSC_TARGET, "/node_trace node {id} group children {children:?}");
                }
                None => {
                    info!(target: crate::logging::OSC_TARGET, "/node_trace node {id} not found")
                }
            }
        }
        Ok(())
    }
}
