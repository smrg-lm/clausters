//! `/def_*`: sending, loading and freeing defs of both families.
//!
//! A name is claimed before the work starts, so two defs racing for one name
//! resolve here rather than in the store. SynthDefs and GraphDefs are installed
//! synchronously; a FaustDef leaves for the compiler thread and comes back
//! through [`super::super::async_pipes`].

use super::super::*;

impl OscServer {
    /// Gives `name` to `kind`, freeing it in the other two def kinds — in
    /// memory and on disk.
    ///
    /// A name identifies **one** def: sending a def under a name another kind
    /// holds replaces it, last one wins. Without this the two entries coexist
    /// and lookup order decides which answers, which is silently wrong
    /// everywhere the name is resolved — instancing, `/def_query`, and the bus
    /// usage the parallel scheduler reads.
    ///
    /// For a Faust def this runs at **submit** time, before the compile
    /// finishes, so a compile that then fails still leaves the name free. That
    /// is the honest reading of the request: the client said this name is a
    /// Faust def now.
    pub(in crate::osc::server) fn claim_def_name(&mut self, name: &str, kind: DefKind) {
        #[cfg(feature = "synth")]
        if kind != DefKind::Synth {
            self.translator.synth_defs.remove(name);
        }
        #[cfg(feature = "faust")]
        if kind != DefKind::Faust {
            self.translator.faust_defs.remove(name);
        }
        if kind != DefKind::Graph {
            self.translator.graph_defs.remove(name);
        }
        if let Some(store) = &self.store {
            store.remove_other_kinds(name, kind);
        }
    }

    /// `/def_send <family> <payload…>` — sends a def of any family: `"synth"`
    /// (one `SynthDefSpec` JSON blob), `"faust"` (a name and a def payload) or
    /// `"graph"` (one `GraphDefSpec` JSON blob). The family is a wire argument
    /// rather than three commands because it is already a datum of a def — it
    /// is what [`Self::handle_def_query`] reports back under the same name and
    /// the same three spellings.
    ///
    /// The ack echoes both: `/done "/def_send" <family>` (a faust compile,
    /// which finishes asynchronously, appends the def name).
    pub(in crate::osc::server) fn handle_def_send(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let family = args.str()?.to_string();
        let rest = args.rest();
        match family.as_str() {
            "synth" => self.handle_def_send_synth(rest, from),
            "faust" => self.handle_def_send_faust(rest, from),
            "graph" => self.handle_def_send_graph(rest, from),
            other => return Err(format!("unknown def family '{other}'")),
        }
        Ok(())
    }

    fn handle_def_send_synth(&mut self, args: &[OscType], from: ClientId) {
        match self.translator.d_recv(args) {
            Ok(name) => {
                self.claim_def_name(&name, DefKind::Synth);
                if let Some(store) = &self.store
                    && !defstore::is_ephemeral(&name)
                    && let Some(spec) = synthdef_spec_bytes(args)
                    && let Err(e) = store.save_synthdef(&name, spec)
                {
                    error!("could not persist SynthDef '{name}': {e}");
                }
                self.reply(
                    from,
                    "/done",
                    vec![
                        OscType::String("/def_send".into()),
                        OscType::String("synth".into()),
                    ],
                );
            }
            Err(e) => self.fail(from, "/def_send", e),
        }
    }

    /// `/def_send graph <json>`: load a GraphDef (validate + store), persist its
    /// spec verbatim, and reply `/done`. Cheap — no JIT, just validation.
    fn handle_def_send_graph(&mut self, args: &[OscType], from: ClientId) {
        match self.translator.d_graph(args) {
            Ok(name) => {
                self.claim_def_name(&name, DefKind::Graph);
                if let Some(store) = &self.store
                    && !defstore::is_ephemeral(&name)
                    && let Some(spec) = synthdef_spec_bytes(args)
                    && let Err(e) = store.save_graphdef(&name, spec)
                {
                    error!("could not persist GraphDef '{name}': {e}");
                }
                self.reply(
                    from,
                    "/done",
                    vec![
                        OscType::String("/def_send".into()),
                        OscType::String("graph".into()),
                    ],
                );
            }
            Err(e) => self.fail(from, "/def_send", e),
        }
    }

    pub(in crate::osc::server) fn handle_def_free(&mut self, msg: &OscMessage, from: ClientId) {
        if let Err(e) = self.translator.d_free(&msg.args) {
            return self.fail(from, "/def_free", e);
        }
        if let Some(store) = &self.store {
            for arg in &msg.args {
                if let OscType::String(name) = arg {
                    store.remove_synthdef(name);
                    store.remove_graphdef(name);
                    #[cfg(all(feature = "faust", not(target_arch = "wasm32")))]
                    crate::faust::cache::remove(store.faustdefs_dir(), name);
                }
            }
        }
    }

    /// `/def_load path`: loads a SynthDef from a JSON spec file on disk (the
    /// Clausters def format — the same body `/def_send synth` carries), on demand,
    /// complementing the boot-time reload. GraphDefs load through `/def_send graph`.
    pub(in crate::osc::server) fn handle_def_load(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let path = args.str()?;
        self.load_synthdef_file(std::path::Path::new(path))?;
        self.reply(from, "/done", vec![OscType::String("/def_load".into())]);
        Ok(())
    }

    /// `/def_loadDir dir`: loads every `*.json` SynthDef spec in a directory. A
    /// single unreadable/invalid file fails the whole command (like scsynth
    /// aborting on a bad def), naming the offending file.
    pub(in crate::osc::server) fn handle_def_load_dir(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let dir = args.str()?;
        let entries = std::fs::read_dir(dir).map_err(|e| format!("{dir}: {e}"))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json")
                && let Err(e) = self.load_synthdef_file(&path)
            {
                return Err(e);
            }
        }
        self.reply(from, "/done", vec![OscType::String("/def_loadDir".into())]);
        Ok(())
    }

    /// Reads one SynthDef spec file, compiles it through the `/def_send synth` path and
    /// persists it under its name. Shared by `/def_load` and `/def_loadDir`.
    fn load_synthdef_file(&mut self, path: &std::path::Path) -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let args = [OscType::Blob(bytes.clone())];
        let name = self
            .translator
            .d_recv(&args)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if let Some(store) = &self.store
            && let Err(e) = store.save_synthdef(&name, &bytes)
        {
            error!("could not persist SynthDef '{name}': {e}");
        }
        Ok(())
    }

    /// `/def_query [name...]` → one `/def_query.reply` per def, then `/done "/def_query"`
    ///. No argument lists every loaded def. The reply is one message per
    /// def because the control surface is variable-length: an aggregate would
    /// nest, and a large catalog would outgrow a UDP datagram.
    ///
    /// Retrieval only — the def store persists across sessions, so this is how
    /// a client learns what a running server actually holds.
    pub(in crate::osc::server) fn handle_def_query(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let mut names = Vec::with_capacity(args.len());
        while !args.is_empty() {
            names.push(args.str()?.to_string());
        }
        let requested = (!names.is_empty()).then_some(names.as_slice());
        for info in self.translator.def_info(requested) {
            self.reply(from, "/def_query.reply", info);
        }
        self.reply(from, "/done", vec![OscType::String("/def_query".into())]);
        Ok(())
    }
}
