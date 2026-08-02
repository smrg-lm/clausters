//! What the translator reports: the def tables, the buffer pool and the tree
//! mirror, as the arguments of a reply.
//!
//! Nothing here mutates or reaches the engine — these read the network-side
//! state the rest of the module maintains, which is why a query answers
//! immediately instead of round-tripping through the audio thread.

use super::*;

impl CmdTranslator {
    /// `/def_query.reply` arguments for `/def_query`, one vector per def — the
    /// loaded defs and their control surface, which is what a patcher wires.
    ///
    /// With `names`, details exactly those (an unknown one comes back with an
    /// empty family and no controls, the way `/buffer_query` reports an unallocated
    /// buffer as zeros rather than failing the whole batch); with `None`, every
    /// loaded def, ordered by family then name so the reply is deterministic.
    ///
    /// Layout per def: `name, family, numControls`, then per control
    /// `name, default, rate` — `rate` naming the same `kr`/`tr`/`ir` control
    /// types a `/def_send synth` spec declares. A **faust** def appends `min, max,
    /// step` (its params carry a range; the reserved `out`/`in` bus controls
    /// are engine plumbing and stay out of the reported surface). A **graph**
    /// def reports its surface **ports** instead, each followed by
    /// `numTargets` and per target `member, control, mul, add` — the scaling
    /// the port applies inside, so a level-1 patch can draw the port's real
    /// connections.
    pub fn def_info(&self, names: Option<&[String]>) -> Vec<Vec<OscType>> {
        match names {
            Some(names) => names.iter().map(|n| self.one_def_info(n)).collect(),
            None => {
                let mut all: Vec<String> = Vec::new();
                #[cfg(feature = "synth")]
                all.extend(self.synth_defs.keys().cloned());
                #[cfg(feature = "faust")]
                all.extend(self.faust_defs.keys().cloned());
                all.extend(self.graph_defs.keys().cloned());
                all.sort();
                all.iter().map(|n| self.one_def_info(n)).collect()
            }
        }
    }

    fn one_def_info(&self, name: &str) -> Vec<OscType> {
        #[cfg(feature = "synth")]
        if let Some(def) = self.synth_defs.get(name) {
            let mut args = vec![
                OscType::String(name.into()),
                OscType::String("synth".into()),
                OscType::Int(def.control_names.len() as i32),
            ];
            for (i, cname) in def.control_names.iter().enumerate() {
                args.push(OscType::String(cname.clone()));
                args.push(OscType::Float(
                    def.control_defaults.get(i).copied().unwrap_or(0.0),
                ));
                args.push(OscType::String(
                    match def.control_types.get(i).copied().unwrap_or_default() {
                        crate::synthdef::ControlType::Control => "kr",
                        crate::synthdef::ControlType::Trigger => "tr",
                        crate::synthdef::ControlType::Scalar => "ir",
                    }
                    .into(),
                ));
            }
            return args;
        }
        #[cfg(feature = "faust")]
        if let Some(def) = self.faust_defs.get(name) {
            // The reserved `out`/`in` bus controls are engine plumbing, not a
            // parameter anyone patches, so the reported surface is the UI
            // params only. A Faust param carries its own range, appended after
            // the shared triple.
            let mut args = vec![
                OscType::String(name.into()),
                OscType::String("faust".into()),
                OscType::Int(def.params.len() as i32),
            ];
            for p in &def.params {
                args.push(OscType::String(p.name.clone()));
                args.push(OscType::Float(p.init));
                args.push(OscType::String("kr".into()));
                args.push(OscType::Float(p.min));
                args.push(OscType::Float(p.max));
                args.push(OscType::Float(p.step));
            }
            return args;
        }
        if let Some(spec) = self.graph_defs.get(name) {
            let mut ports: Vec<&String> = spec.surface.keys().collect();
            ports.sort();
            let mut args = vec![
                OscType::String(name.into()),
                OscType::String("graph".into()),
                OscType::Int(ports.len() as i32),
            ];
            for port in ports {
                let targets = &spec.surface[port];
                args.push(OscType::String(port.clone()));
                args.push(OscType::Float(
                    spec.defaults.get(port).copied().unwrap_or(0.0),
                ));
                args.push(OscType::String("kr".into()));
                args.push(OscType::Int(targets.len() as i32));
                for t in targets {
                    args.push(OscType::Int(t.member as i32));
                    args.push(OscType::String(t.control.clone()));
                    args.push(OscType::Float(t.mul));
                    args.push(OscType::Float(t.add));
                }
            }
            return args;
        }
        vec![
            OscType::String(name.into()),
            OscType::String(String::new()),
            OscType::Int(0),
        ]
    }

    /// `/buffer_query.reply` arguments for an argument-less `/buffer_query`: every
    /// **allocated** buffer, four args each (`bufnum, frames, channels,
    /// sampleRate`) — the same shape the per-index form replies with, so one
    /// parser reads both.
    pub fn buffer_list(&self) -> Vec<OscType> {
        let mut args = Vec::new();
        for (index, slot) in self.buffers.iter().enumerate() {
            if let Some(buf) = slot {
                args.push(OscType::Int(index as i32));
                args.push(OscType::Int(buf.frames() as i32));
                args.push(OscType::Int(buf.channels() as i32));
                args.push(OscType::Float(buf.sample_rate() as f32));
            }
        }
        args
    }

    /// `/group_queryTree.reply` arguments: `detail`, the queried group, its
    /// child count and its name, then depth-first per node: ID and child count
    /// (`-1` for synths) followed by a name — the group's own (empty when it
    /// has none) or the synth's def name — and per `detail` level the same
    /// payload `/node_query.reply` carries — 1 adds the control count and (name, value)
    /// pairs (scsynth's `flag`), 2 adds the maps and the inferred bus lists,
    /// which is what makes every entry a full node info.
    ///
    /// Every node reads `ID, count, name` — one shape for both kinds, rather
    /// than a name only where it is new.
    pub fn query_tree(&self, group: i32, detail: i32) -> Result<Vec<OscType>, String> {
        let Some(children) = self.mirror.children(group) else {
            return Err(match self.mirror.get(group) {
                Some(_) => format!("node {group} is not a group"),
                None => format!("group {group} not found"),
            });
        };
        let mut args = vec![
            OscType::Int(detail),
            OscType::Int(group),
            OscType::Int(children.len() as i32),
            OscType::String(self.mirror.name_of(group).into()),
        ];
        self.query_children(group, detail, &mut args);
        Ok(args)
    }

    /// `/group_query`: the node a path names, or `-1` when nothing answers to
    /// it. Absence is a state, not a protocol error (the `/node_query`
    /// convention), so an unresolved path replies rather than failing.
    pub fn resolve_path(&self, path: &str) -> i32 {
        self.mirror.resolve_path(path).unwrap_or(-1)
    }

    fn query_children(&self, group: i32, detail: i32, args: &mut Vec<OscType>) {
        let children = self.mirror.children(group).unwrap_or(&[]).to_vec();
        for child in children {
            args.push(OscType::Int(child));
            if let Some(grandchildren) = self.mirror.children(child) {
                args.push(OscType::Int(grandchildren.len() as i32));
                args.push(OscType::String(self.mirror.name_of(child).into()));
                self.query_children(child, detail, args);
            } else if let Some((def_name, _)) = self.mirror.synth_info(child) {
                args.push(OscType::Int(-1));
                args.push(OscType::String(def_name.into()));
                if detail >= 1 {
                    self.synth_payload(child, detail >= 2, args);
                }
            }
        }
    }

    /// The per-synth payload shared by `/node_query.reply` and a detailed
    /// `/group_queryTree.reply`: the control count and its (name|index, value)
    /// pairs and, with `full`, the map count and its (controlIndex, bus,
    /// audio) triples plus the inferred `reads`/`writes` bus lists.
    fn synth_payload(&self, id: i32, full: bool, args: &mut Vec<OscType>) {
        let Some(MirrorBody::Synth { controls, maps, .. }) = self.mirror.get(id).map(|n| &n.body)
        else {
            return;
        };
        args.push(OscType::Int(controls.len() as i32));
        let def = self.node_defs.get(&id);
        for (i, value) in controls.iter().enumerate() {
            let name = def.and_then(|d| d.control_name(i)).unwrap_or("");
            if name.is_empty() {
                args.push(OscType::Int(i as i32));
            } else {
                args.push(OscType::String(name.into()));
            }
            args.push(OscType::Float(*value));
        }
        if !full {
            return;
        }
        args.push(OscType::Int(maps.len() as i32));
        for (ctl, bus, audio) in maps {
            args.push(OscType::Int(*ctl as i32));
            args.push(OscType::Int(*bus));
            args.push(OscType::Int(*audio as i32));
        }
        let usage = self.mirror.usage_of(id);
        args.push(OscType::String(bus_list(usage.reads)));
        args.push(OscType::String(bus_list(usage.writes)));
    }

    /// `/node_query.reply` arguments for `/node_query`: per-node detail beyond the tree
    /// structure `/group_queryTree` gives. Layout: `nodeID, parentID, prevID,
    /// nextID, isGroup`; then for a **group** `headID, tailID` (`-1` if empty);
    /// for a **synth** `defName`, `numControls` + (name|index, value) pairs,
    /// `numMaps` + (controlIndex, bus, audio) triples, and the inferred
    /// `reads`/`writes` bus lists as two strings (same format as
    /// `/group_dumpGraph`). A group's `/group_name` follows its `tailID`,
    /// empty when it has none. Siblings are `-1` when absent, and a node the server
    /// does not hold answers `nodeID, -1, -1, -1, -1` — `isGroup = -1` is how
    /// the record says the node is gone.
    pub fn node_info(&self, id: i32) -> Vec<OscType> {
        let Some(node) = self.mirror.get(id) else {
            // A node that is not there is a *state*, not a protocol error:
            // `isGroup = -1` says so in the record itself, so one dead id
            // does not abort a multi-id query (the `/def_query` convention).
            return vec![
                OscType::Int(id),
                OscType::Int(-1),
                OscType::Int(-1),
                OscType::Int(-1),
                OscType::Int(-1),
            ];
        };
        let parent = self.mirror.parent(id).unwrap_or(-1);
        let (prev, next) = self.siblings(id, parent);
        let mut args = vec![
            OscType::Int(id),
            OscType::Int(parent),
            OscType::Int(prev),
            OscType::Int(next),
        ];
        match &node.body {
            MirrorBody::Group { children, name, .. } => {
                args.push(OscType::Int(1));
                args.push(OscType::Int(children.first().copied().unwrap_or(-1)));
                args.push(OscType::Int(children.last().copied().unwrap_or(-1)));
                args.push(OscType::String(name.as_deref().unwrap_or("").into()));
            }
            MirrorBody::Synth { def_name, .. } => {
                args.push(OscType::Int(0));
                args.push(OscType::String(def_name.clone()));
                self.synth_payload(id, true, &mut args);
            }
        }
        args
    }

    /// Previous and next sibling of `id` within `parent`'s children (`-1` if
    /// there is none or `id` is the root).
    fn siblings(&self, id: i32, parent: i32) -> (i32, i32) {
        let Some(sibs) = self.mirror.children(parent) else {
            return (-1, -1);
        };
        let Some(pos) = sibs.iter().position(|&c| c == id) else {
            return (-1, -1);
        };
        let prev = if pos > 0 { sibs[pos - 1] } else { -1 };
        let next = sibs.get(pos + 1).copied().unwrap_or(-1);
        (prev, next)
    }

    /// A group's `/group_name` as ` "mixer"`, or nothing when it has none —
    /// the introspection dumps' way of showing the label next to the ID.
    fn quoted_name(&self, id: i32) -> String {
        match self.mirror.name_of(id) {
            "" => String::new(),
            name => format!(" \"{name}\""),
        }
    }

    /// `/group_dumpGraph`: a human-readable view of the inferred bus graph of
    /// one group — what each child reads/writes and the current order.
    pub fn dump_graph(&self, group: i32) -> Result<String, String> {
        let Some(children) = self.mirror.children(group) else {
            return Err(match self.mirror.get(group) {
                Some(_) => format!("node {group} is not a group"),
                None => format!("group {group} not found"),
            });
        };
        let auto = if self.mirror.is_auto_group(group) {
            "auto"
        } else {
            "manual"
        };
        let parallel = if self.mirror.is_parallel_group(group) {
            ", parallel"
        } else {
            ""
        };
        let mut out = format!(
            "group {group}{} ({auto}{parallel})\n",
            self.quoted_name(group)
        );
        for &child in children {
            let usage = self.mirror.usage_of(child);
            let kind = match self.mirror.synth_info(child) {
                Some((def_name, _)) => def_name.to_string(),
                None if self.mirror.is_auto_group(child) => {
                    format!("group{} (auto)", self.quoted_name(child))
                }
                None => format!("group{}", self.quoted_name(child)),
            };
            let dynamic = if usage.dynamic { "  dynamic" } else { "" };
            out.push_str(&format!(
                "  {child} {kind}  reads {}  writes {}{dynamic}\n",
                bus_list(usage.reads),
                bus_list(usage.writes),
            ));
        }
        Ok(out)
    }
}

/// `u128` bus mask → "0,1,16" (or "-" when empty).
fn bus_list(mask: u128) -> String {
    if mask == 0 {
        return "-".into();
    }
    let buses: Vec<String> = (0..128)
        .filter(|b| mask & (1 << b) != 0)
        .map(|b| b.to_string())
        .collect();
    buses.join(",")
}
