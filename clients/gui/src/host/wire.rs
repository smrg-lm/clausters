//! **The `/gui_*` protocol as it arrives**: a packet unwrapped, each message
//! dispatched to its handler, and the arguments read the one way every handler
//! reads them. What a handler *does* lives with its concern -- a definition in
//! [`super::define`], a set in [`super::set`] -- and the handlers here are the
//! ones that are only that: a load, a face, a theme, a size table, an
//! acknowledgement, a query.

use super::*;

impl Host {
    /// Handles one decoded packet from `from`, returning the effects its front
    /// should carry out (replies plus window open/close). A bundle is unwrapped
    /// and its messages run in order (the timetag is treated as immediate at this
    /// milestone -- no scheduling yet).
    pub fn handle_packet(&mut self, packet: OscPacket, from: ClientId) -> Vec<HostEffect> {
        let mut effects = Vec::new();
        self.dispatch_packet(packet, from, &mut effects);
        effects
    }

    pub(super) fn dispatch_packet(
        &mut self,
        packet: OscPacket,
        from: ClientId,
        effects: &mut Vec<HostEffect>,
    ) {
        match packet {
            OscPacket::Message(msg) => self.dispatch(msg, from, effects),
            OscPacket::Bundle(bundle) => {
                for inner in bundle.content {
                    self.dispatch_packet(inner, from, effects);
                }
            }
        }
    }

    pub(super) fn dispatch(
        &mut self,
        msg: OscMessage,
        from: ClientId,
        effects: &mut Vec<HostEffect>,
    ) {
        match msg.addr.as_str() {
            GUI_DEF => self.on_def(&msg.args, from, effects),
            GUI_SET => self.on_set(&msg.args, from, effects),
            GUI_FREE => self.on_free(&msg.args, from, effects),
            GUI_QUERY => self.on_query(&msg.args, from, effects),
            GUI_ACK => self.on_ack(&msg.args),
            GUI_BIND => self.on_bind(&msg.args, from),
            GUI_LOAD => self.on_load(&msg.args, from, effects),
            GUI_FONT => self.on_font(&msg.args, from, effects),
            GUI_THEME => self.on_theme(&msg.args, from, effects),
            GUI_METRICS => self.on_metrics(&msg.args, from, effects),
            GUI_CLOCK => self.on_clock(&msg.args, from, effects),
            _other => diag::debug!("{from}: ignoring unhandled address {_other}"),
        }
    }

    /// `/gui_load <name>` -- load a persisted GuiDef and instantiate it (build its
    /// tree and open its window), replaying it as a `/gui_def` under the id it was
    /// saved with.
    pub(super) fn on_load(
        &mut self,
        args: &[OscType],
        from: ClientId,
        effects: &mut Vec<HostEffect>,
    ) {
        let Some(name) = string_arg(args, 0) else {
            return diag::warn!("{from}: {GUI_LOAD} needs a name argument");
        };
        let Some(store) = self.store.as_ref() else {
            return diag::warn!("{from}: {GUI_LOAD} {name}: no data directory configured");
        };
        let (id, json) = match store.load(name) {
            Ok(loaded) => loaded,
            Err(e) => return diag::warn!("{from}: {GUI_LOAD} {name}: {e}"),
        };
        diag::info!("{from}: {GUI_LOAD} {name}: instantiating GuiDef {id}");
        self.on_def(
            &[
                OscType::Int(id),
                OscType::String(String::from_utf8_lossy(&json).into_owned()),
            ],
            from,
            effects,
        );
    }

    /// `/gui_font <blob>` -- draw text with this typeface from now on.
    ///
    /// The bytes are a raw TrueType/OpenType file. Loading one relayouts
    /// nothing (the size table never followed the face), so every open window
    /// redraws and comes up the same size. A build without a rasterizer (the
    /// `font-atlas` feature) says so and keeps drawing with its embedded bitmap
    /// face, which is what a refused face does too.
    pub(super) fn on_font(
        &mut self,
        args: &[OscType],
        from: ClientId,
        effects: &mut Vec<HostEffect>,
    ) {
        let Some(bytes) = json_arg(args, 0) else {
            return diag::warn!(
                "{from}: {GUI_FONT} needs a blob argument (a TrueType/OpenType file)"
            );
        };
        #[cfg(feature = "font-atlas")]
        {
            struct WireFace<'a>(&'a [u8]);
            impl FontSource for WireFace<'_> {
                fn face(&self) -> Option<Vec<u8>> {
                    Some(self.0.to_vec())
                }
            }
            if !self.load_face(&WireFace(bytes)) {
                return diag::warn!(
                    "{from}: {GUI_FONT}: those {} bytes are not a typeface this host can read",
                    bytes.len()
                );
            }
            diag::info!("{from}: {GUI_FONT}: drawing text with the face it handed over");
            for id in self.window_def_ids() {
                effects.push(HostEffect::Redraw(id));
            }
        }
        #[cfg(not(feature = "font-atlas"))]
        {
            let _ = (bytes, effects);
            diag::warn!(
                "{from}: {GUI_FONT}: this host was built without a rasterizer (the `font-atlas` \
                 feature); drawing with the embedded bitmap face"
            );
        }
    }

    /// `/gui_theme <json>` -- draw the chrome from these colors from now on.
    ///
    /// The table is partial and overlays the host's own; unknown roles and
    /// unreadable colors are reported and skipped, exactly as the launch-time
    /// table's are. Every window then re-resolves its theme **groups** over the
    /// new base -- a group overlays what it inherits, so changing the base
    /// changes what a group means -- and redraws.
    pub(super) fn on_theme(
        &mut self,
        args: &[OscType],
        from: ClientId,
        effects: &mut Vec<HostEffect>,
    ) {
        let Some(table) = json_table(args, 0) else {
            return diag::warn!("{from}: {GUI_THEME} needs a JSON object of role -> color");
        };
        for w in self.theme.overlay_json(&table) {
            diag::warn!("{from}: {GUI_THEME}: {w}");
        }
        // The base moved under the resolved references: a group's colors are
        // its own table over the inherited one, so they are re-resolved rather
        // than kept.
        let base = Arc::new(self.theme.clone());
        for id in self.window_def_ids() {
            if let Some(tree) = self.window_def_mut(id) {
                widget::resolve_style(tree, &base);
            }
            effects.push(HostEffect::Redraw(id));
        }
        diag::info!("{from}: {GUI_THEME}: {} role(s) overlaid", table.len());
    }

    /// `/gui_metrics <json>` -- lay out with these sizes from now on.
    ///
    /// The theme's counterpart, and the same rules: partial, warned about role
    /// by role, applied to the host's table. `scale` is the reserved key that
    /// regenerates the whole set at a density. Every canvas re-resolves the new
    /// roles at its own scale, and sizes are read per frame from that one
    /// table, so a redraw is the rest of the update.
    pub(super) fn on_metrics(
        &mut self,
        args: &[OscType],
        from: ClientId,
        effects: &mut Vec<HostEffect>,
    ) {
        let Some(table) = json_table(args, 0) else {
            return diag::warn!("{from}: {GUI_METRICS} needs a JSON object of role -> number");
        };
        let entries: Vec<(&str, f64)> = table
            .iter()
            .filter_map(|(k, v)| v.as_f64().map(|n| (k.as_str(), n)))
            .collect();
        for w in self.metrics.overlay(entries) {
            diag::warn!("{from}: {GUI_METRICS}: {w}");
        }
        self.refresh_metrics();
        for id in self.window_def_ids() {
            effects.push(HostEffect::Redraw(id));
        }
        diag::info!("{from}: {GUI_METRICS}: {} role(s) overlaid", table.len());
    }

    /// `/gui_set <id> <k> <v> ...` -- update one live widget's properties, in the
    /// generic registry (for `/gui_query`) and, if it is inside an open window,
    /// in the typed render tree (so the change shows live).
    pub(super) fn on_set(
        &mut self,
        args: &[OscType],
        from: ClientId,
        effects: &mut Vec<HostEffect>,
    ) {
        let Some(id) = int_arg(args, 0) else {
            return diag::warn!("{from}: {GUI_SET} needs an integer id");
        };
        let props = key_value_pairs(&args[1..]);
        if props.is_empty() {
            return diag::warn!("{from}: {GUI_SET} {id}: no key/value pairs");
        }
        let keys: Vec<String> = props.iter().map(|(k, _)| k.clone()).collect();
        if !self.set_props(id, props, effects) {
            return diag::warn!("{from}: {GUI_SET} {id}: no such widget");
        }
        diag::info!("{from}: {GUI_SET} {id}: updated {keys:?}");
    }

    /// `/gui_ack <seq> <docVersion> [<source> <generation>...] [<reason>]` -- the
    /// owner reports how far it has processed and what state that left.
    ///
    /// One rule and no branch: retire every pending edit at or below `seq`. The
    /// values the owner pushed arrive as ordinary `/gui_set`s in the same
    /// bundle, so *applied*, *applied transformed* and *refused* need no
    /// distinction here -- the state is whatever was pushed, and a refusal is
    /// the previous value.
    ///
    /// Trailing pairs are source generations, which is the only thing that can
    /// say a destructive edit changed samples whose identity did not move. A
    /// trailing string is a reason, informational and read by nothing in the
    /// mechanism.
    pub(super) fn on_ack(&mut self, args: &[OscType]) {
        let Some(OscType::Int(seq)) = args.first() else {
            diag::warn!("/gui_ack without a sequence number");
            return;
        };
        let mut acked = ack::Acked {
            seq: *seq,
            doc_version: match args.get(1) {
                Some(OscType::Int(v)) => i64::from(*v),
                Some(OscType::Long(v)) => *v,
                _ => 0,
            },
            ..ack::Acked::default()
        };
        let mut rest = &args[args.len().min(2)..];
        if let Some(OscType::String(reason)) = rest.last() {
            acked.reason = Some(reason.clone());
            rest = &rest[..rest.len() - 1];
        }
        for pair in rest.chunks(2) {
            if let [OscType::Int(source), generation] = pair {
                let generation = match generation {
                    OscType::Int(g) => i64::from(*g),
                    OscType::Long(g) => *g,
                    _ => continue,
                };
                acked.generations.insert(*source, generation);
            }
        }
        self.settle(acked);
    }

    /// `/gui_query <id>` -- reply `/gui_info <id> <type> <k> <v> ...`.
    pub(super) fn on_query(
        &mut self,
        args: &[OscType],
        from: ClientId,
        effects: &mut Vec<HostEffect>,
    ) {
        let Some(id) = int_arg(args, 0) else {
            return diag::warn!("{from}: {GUI_QUERY} needs an integer id");
        };
        let mut out = vec![OscType::Int(id)];
        // What the widget *is now*, before the document is read: a gesture
        // edits the render tree and never the document, so a widget that was
        // dragged answers with what it was defined as unless the live state
        // overlays it (see `live_props`).
        let live = self.live_props(id);
        match self.registry.get(id) {
            Some(widget) => {
                out.push(OscType::String(widget.kind.clone()));
                let mut props = widget.props.clone();
                props.extend(live);
                for (k, v) in &props {
                    if let Some(arg) = scalar_arg(v) {
                        out.push(OscType::String(k.clone()));
                        out.push(arg);
                    }
                }
                diag::info!("{from}: {GUI_QUERY} {id} -> {GUI_INFO} ({})", widget.kind);
            }
            None => {
                // An empty type string means "no such widget" -- the query still
                // gets an answer, the way the server replies even on a miss. A
                // miss is *not* a warning: it is how a client pings a host that is
                // still empty (the launcher's readiness check does exactly that).
                out.push(OscType::String(String::new()));
                diag::debug!("{from}: {GUI_QUERY} {id}: no such widget");
            }
        }
        effects.push(HostEffect::Reply(OscMessage {
            addr: GUI_INFO.into(),
            args: out,
        }));
    }

    /// The props widget `id` currently holds that its **document does not** --
    /// what a gesture changed since the def was sent.
    ///
    /// Two surfaces answer "what is this widget", and only one of them a
    /// gesture writes. The registry holds the document: what the script sent,
    /// kept current by every `/gui_set`, and it is the base a query answers
    /// from because it carries props the render tree does not model. The render
    /// tree holds the widget as the user has since left it -- a slider dragged, a
    /// clip moved, a curve edited -- and that is the divergence this closes, in
    /// the props' **own vocabulary**: a key here is one a script could set, with
    /// the value it would have to set to reproduce what is on screen.
    ///
    /// Empty for a widget nothing edits, which is most of them.
    pub(super) fn live_props(&self, id: i32) -> serde_json::Map<String, Value> {
        let live = self
            .registry
            .root_of(id)
            .and_then(|root| self.window_defs.get(&root))
            .and_then(|tree| tree.find(id))
            .map(|w| w.info())
            .unwrap_or_default();
        live.into_iter().collect()
    }
}

/// Collects the trailing OSC blob arguments of a `/gui_def` (the bulk data, e.g.
/// waveform samples) into a list a `Widget` can index by `"blob"`.
pub(super) fn blob_args(args: &[OscType]) -> Vec<Vec<u8>> {
    args.iter()
        .filter_map(|a| match a {
            OscType::Blob(b) => Some(b.clone()),
            _ => None,
        })
        .collect()
}

/// The i-th argument as an `i32`, if present and integer-typed.
pub(super) fn int_arg(args: &[OscType], i: usize) -> Option<i32> {
    match args.get(i) {
        Some(OscType::Int(n)) => Some(*n),
        Some(OscType::Long(n)) => Some(*n as i32),
        _ => None,
    }
}

/// The i-th argument as a string slice, if present and string-typed.
pub(super) fn string_arg(args: &[OscType], i: usize) -> Option<&str> {
    match args.get(i) {
        Some(OscType::String(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// One argument read as a JSON **object** -- what the two host-wide tables
/// cross as, the way every other structured value on this wire does.
pub(super) fn json_table(
    args: &[OscType],
    i: usize,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let bytes = json_arg(args, i)?;
    match serde_json::from_slice::<serde_json::Value>(bytes) {
        Ok(serde_json::Value::Object(table)) => Some(table),
        _ => None,
    }
}

/// The i-th argument as JSON bytes: a string or a blob (both accepted, as
/// `/def_send synth` accepts a SynthDef either way).
pub(super) fn json_arg(args: &[OscType], i: usize) -> Option<&[u8]> {
    match args.get(i) {
        Some(OscType::String(s)) => Some(s.as_bytes()),
        Some(OscType::Blob(b)) => Some(b.as_slice()),
        _ => None,
    }
}

/// Turns a flat `k, v, k, v, ...` OSC tail into `(String, Value)` pairs,
/// preserving the int/float distinction (an OSC `Int` stays an integer JSON
/// number, a `Float` a floating one). A trailing unpaired key is ignored.
pub(super) fn key_value_pairs(tail: &[OscType]) -> Vec<(String, Value)> {
    let mut pairs = Vec::new();
    let mut it = tail.iter();
    while let (Some(k), Some(v)) = (it.next(), it.next()) {
        if let OscType::String(key) = k
            && let Some(value) = osc_to_value(v)
        {
            pairs.push((key.clone(), value));
        }
    }
    pairs
}

/// A **blob** in a value slot is bulk samples -- the same raw little-endian
/// `f32` a `/gui_def`'s trailing blobs carry, and the one payload a scalar wire
/// cannot spell out.
///
/// It expands to the array the inline `data` prop would have held, so nothing
/// downstream learns a second shape: a live view's samples are replaced by the
/// path that already replaces them. This is what lets a client past the inline
/// ceiling change what a widget draws without redefining the window -- a native
/// one rewrites the file it spilled to, and a page has no file.
pub(super) fn blob_to_samples(bytes: &[u8]) -> Option<Value> {
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    Some(Value::Array(
        clausters_core::osc::blob_samples(bytes)
            .map(Value::from)
            .collect(),
    ))
}

/// One OSC primitive as a JSON value, keeping integers and floats apart -- and
/// a blob as the sample array it carries (see [`blob_to_samples`]).
pub(super) fn osc_to_value(arg: &OscType) -> Option<Value> {
    match arg {
        OscType::Int(n) => Some(Value::from(*n)),
        OscType::Long(n) => Some(Value::from(*n)),
        OscType::Float(x) => Some(Value::from(*x)),
        OscType::Double(x) => Some(Value::from(*x)),
        OscType::String(s) => Some(Value::from(s.clone())),
        OscType::Blob(b) => blob_to_samples(b),
        _ => None,
    }
}

/// One scalar JSON value as an OSC primitive for a `/gui_info` reply, keeping
/// integers (`Int`) and floats (`Float`) apart; `None` for structural values.
pub(super) fn scalar_arg(v: &Value) -> Option<OscType> {
    match v {
        Value::Bool(b) => Some(OscType::Int(*b as i32)),
        Value::Number(n) if n.is_i64() || n.is_u64() => Some(OscType::Int(n.as_i64()? as i32)),
        Value::Number(n) => Some(OscType::Float(n.as_f64()? as f32)),
        Value::String(s) => Some(OscType::String(s.clone())),
        _ => None,
    }
}
