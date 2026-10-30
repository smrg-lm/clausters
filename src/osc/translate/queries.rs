//! What the translator reports: the def tables, the buffer pool and the tree
//! mirror, as the arguments of a reply.
//!
//! Nothing here mutates or reaches the engine -- these read the network-side
//! state the rest of the module maintains, which is why a query answers
//! immediately instead of round-tripping through the audio thread.

use super::*;
use clausters_core::node_editor::Control;
#[cfg(any(
    feature = "faust",
    all(
        any(feature = "plugin-clap", feature = "plugin-vst3"),
        target_os = "linux"
    )
))]
use clausters_core::node_editor::ControlDisplay;

impl CmdTranslator {
    /// `/def_query.reply` arguments for `/def_query`, one vector per def -- the
    /// loaded defs and their control surface, which is what a patcher wires
    /// and what a generic editor draws.
    ///
    /// With `names`, details exactly those (an unknown one comes back with an
    /// empty family and no controls, the way `/buffer_query` reports an unallocated
    /// buffer as zeros rather than failing the whole batch); with `None`, every
    /// loaded def, ordered by family then name so the reply is deterministic.
    ///
    /// Layout per def: `name, family, numControls`, then per control the same
    /// record in every family (`push_control`). A **graph** def reports its
    /// surface **ports** as its controls, each followed by `numTargets` and per
    /// target `member, control, mul, add` -- the scaling the port applies
    /// inside, so a level-1 patch can draw the port's real connections.
    pub fn def_info(&self, names: Option<&[String]>) -> Vec<Vec<OscType>> {
        match names {
            Some(names) => names.iter().map(|n| self.one_def_info(n)).collect(),
            None => {
                let mut all: Vec<String> = Vec::new();
                #[cfg(feature = "synth")]
                all.extend(self.synth_defs.keys().cloned());
                #[cfg(feature = "faust")]
                all.extend(self.faust_defs.keys().cloned());
                #[cfg(all(
                    any(feature = "plugin-clap", feature = "plugin-vst3"),
                    target_os = "linux"
                ))]
                all.extend(self.plugin_defs.keys().cloned());
                all.extend(self.graph_defs.keys().cloned());
                all.sort();
                all.iter().map(|n| self.one_def_info(n)).collect()
            }
        }
    }

    fn one_def_info(&self, name: &str) -> Vec<OscType> {
        let head = |family: &str, count: usize| {
            vec![
                OscType::String(name.into()),
                OscType::String(family.into()),
                OscType::Int(count as i32),
            ]
        };
        #[cfg_attr(
            not(any(
                feature = "synth",
                feature = "faust",
                all(
                    any(feature = "plugin-clap", feature = "plugin-vst3"),
                    target_os = "linux"
                )
            )),
            allow(unused_variables)
        )]
        let described = |family: &str, controls: Vec<Control>| {
            let mut args = head(family, controls.len());
            for control in &controls {
                push_control(&mut args, control);
            }
            args
        };
        #[cfg(feature = "synth")]
        if let Some(def) = self.synth_defs.get(name) {
            return described("synth", synth_controls(def));
        }
        #[cfg(feature = "faust")]
        if let Some(def) = self.faust_defs.get(name) {
            return described("faust", faust_controls(def));
        }
        #[cfg(all(
            any(feature = "plugin-clap", feature = "plugin-vst3"),
            target_os = "linux"
        ))]
        if let Some(def) = self.plugin_defs.get(name) {
            return described("plugin", self.plugin_controls(def));
        }
        if let Some(spec) = self.graph_defs.get(name) {
            let mut ports: Vec<&String> = spec.surface.keys().collect();
            ports.sort();
            let mut args = head("graph", ports.len());
            for port in ports {
                let default = spec.defaults.get(port).copied().unwrap_or(0.0);
                push_control(&mut args, &Control::new(port.clone(), f64::from(default)));
                let targets = &spec.surface[port];
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
        head("", 0)
    }

    /// **A live node's controls, described** -- from the def it was built
    /// with, which outlives a `/def_free` or a replacement of its name. `None`
    /// for a node with no controls (a group, or no node at all). A `Voices`
    /// node is described by its def; its `notes` bus is not a control a
    /// person sets.
    pub fn node_controls(&self, node: i32) -> Option<Vec<Control>> {
        Some(self.def_controls(self.node_defs.get(&node)?))
    }

    #[cfg_attr(
        not(any(
            feature = "synth",
            feature = "faust",
            all(
                any(feature = "plugin-clap", feature = "plugin-vst3"),
                target_os = "linux"
            )
        )),
        allow(clippy::only_used_in_recursion)
    )]
    fn def_controls(&self, def: &NodeDef) -> Vec<Control> {
        match def {
            #[cfg(feature = "synth")]
            NodeDef::UGen(def) => synth_controls(def),
            #[cfg(feature = "faust")]
            NodeDef::Faust(def) => faust_controls(def),
            #[cfg(all(
                any(feature = "plugin-clap", feature = "plugin-vst3"),
                target_os = "linux"
            ))]
            NodeDef::Plugin(def, ..) => self.plugin_controls(def),
            #[cfg(any(feature = "synth", feature = "faust"))]
            NodeDef::Voices(voices) => self.def_controls(&voices.inner),
            #[cfg(not(any(
                feature = "synth",
                feature = "faust",
                all(
                    any(feature = "plugin-clap", feature = "plugin-vst3"),
                    target_os = "linux"
                )
            )))]
            _ => match *def {},
        }
    }

    /// A plugin def's controls: its parameters as the plugin describes them
    /// -- their own names, modules, flags and list labels -- then its ports,
    /// which are buses (an audio bus, or for a note input an event bus) and
    /// are grouped as such.
    #[cfg(all(
        any(feature = "plugin-clap", feature = "plugin-vst3"),
        target_os = "linux"
    ))]
    fn plugin_controls(&self, def: &crate::plugin::PluginDef) -> Vec<Control> {
        def.names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let default = f64::from(def.defaults[i]);
                match def.info.parameters.get(i) {
                    Some(p) => Control {
                        display: ControlDisplay {
                            min: Some(p.min),
                            max: Some(p.max),
                            step: Some(p.step),
                            ..ControlDisplay::default()
                        },
                        label: Some(p.name.clone()),
                        group: p.module.clone(),
                        flags: p.flags,
                        labels: p.labels.clone(),
                        ..Control::new(name.clone(), default)
                    },
                    None => {
                        let last = if i >= def.notes_control() {
                            self.limits.event_buses as f64 - 1.0
                        } else {
                            NUM_AUDIO_BUSES as f64 - 1.0
                        };
                        Control {
                            display: ControlDisplay {
                                min: Some(-1.0),
                                max: Some(last),
                                step: Some(1.0),
                                ..ControlDisplay::default()
                            },
                            group: "Buses".into(),
                            ..Control::new(name.clone(), default)
                        }
                    }
                }
            })
            .collect()
    }

    /// `/buffer_query.reply` arguments for an argument-less `/buffer_query`: every
    /// **allocated** buffer, four args each (`bufnum, frames, channels,
    /// sampleRate`) -- the same shape the per-index form replies with, so one
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
    /// (`-1` for synths) followed by a name -- the group's own (empty when it
    /// has none) or the synth's def name -- and per `detail` level the same
    /// payload `/node_query.reply` carries -- 1 adds the control count and (name, value)
    /// pairs (scsynth's `flag`), 2 adds the maps and the inferred bus lists,
    /// which is what makes every entry a full node info.
    ///
    /// Every node reads `ID, count, name` -- one shape for both kinds, rather
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
        if detail >= 2 {
            self.group_modes(group, &mut args);
        }
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
                if detail >= 2 {
                    self.group_modes(child, args);
                }
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

    /// The two modes a **group** runs under, as the pair of flags that follows
    /// its name: `/group_sortMode` (1 = the server orders the children by the
    /// buses they touch) and `/group_parallel` (1 = the independent ones run on
    /// the DSP workers). They are set by command and were readable nowhere but
    /// inside `/group_dumpGraph`'s debug string, which is the one thing a
    /// client must not parse -- so a client with both ways of ordering could not
    /// say which one it was looking at, and found out from a `/fail`.
    ///
    /// In the tree reply they ride at `detail >= 2`, which is where that reply
    /// stops being scsynth's; in `/node_query.reply` they follow the group's
    /// name, which is already past scsynth's record.
    fn group_modes(&self, group: i32, args: &mut Vec<OscType>) {
        args.push(OscType::Int(self.mirror.is_auto_group(group) as i32));
        args.push(OscType::Int(self.mirror.is_parallel_group(group) as i32));
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
        args.push(OscType::String(bus_list(usage.reads())));
        args.push(OscType::String(bus_list(usage.writes())));
    }

    /// `/node_query.reply` arguments for `/node_query`: per-node detail beyond the tree
    /// structure `/group_queryTree` gives. Layout: `nodeID, parentID, prevID,
    /// nextID, isGroup`; then for a **group** `headID, tailID` (`-1` if empty);
    /// for a **synth** `defName`, `numControls` + (name|index, value) pairs,
    /// `numMaps` + (controlIndex, bus, audio) triples, and the inferred
    /// `reads`/`writes` bus lists as two strings (same format as
    /// `/group_dumpGraph`). A group's `/group_name` follows its `tailID`,
    /// empty when it has none. Siblings are `-1` when absent, and a node the server
    /// does not hold answers `nodeID, -1, -1, -1, -1` -- `isGroup = -1` is how
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
                self.group_modes(id, &mut args);
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

    /// A group's `/group_name` as ` "mixer"`, or nothing when it has none --
    /// the introspection dumps' way of showing the label next to the ID.
    fn quoted_name(&self, id: i32) -> String {
        match self.mirror.name_of(id) {
            "" => String::new(),
            name => format!(" \"{name}\""),
        }
    }

    /// `/group_dumpGraph`: a human-readable view of the inferred bus graph of
    /// one group -- what each child reads/writes and the current order.
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
                bus_list(usage.reads()),
                bus_list(usage.writes()),
            ));
        }
        out.push_str("  compensation (global, samples):\n");
        for spec in &self.compensation_report.nodes {
            out.push_str(&format!(
                "    {} intrinsic {} inputs {:?} writers {:?}\n",
                spec.id, spec.latency, spec.reads, spec.writes
            ));
        }
        for (id, reason) in &self.compensation_report.excluded {
            out.push_str(&format!("    {id} uncompensated: {reason}\n"));
        }
        Ok(out)
    }
}

/// `u128` bus mask -> "0,1,16" (or "-" when empty).
fn bus_list(buses: impl Iterator<Item = usize>) -> String {
    let buses: Vec<String> = buses.map(|b| b.to_string()).collect();
    if buses.is_empty() {
        return "-".into();
    }
    buses.join(",")
}

/// A SynthDef's controls: a name, a default, a rate, and the range, step,
/// warp and unit the spec declared beside each, when it did.
#[cfg(feature = "synth")]
fn synth_controls(def: &SynthDef) -> Vec<Control> {
    def.control_names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let rate = match def.control_types.get(i).copied().unwrap_or_default() {
                crate::synthdef::ControlType::Control => "kr",
                crate::synthdef::ControlType::Trigger => "tr",
                crate::synthdef::ControlType::Scalar => "ir",
            };
            Control {
                rate: rate.into(),
                display: def.control_display.get(i).cloned().unwrap_or_default(),
                ..Control::new(name.clone(), f64::from(def.control_defaults[i]))
            }
        })
        .collect()
}

/// A FaustDef's controls: its UI parameters, each with the range and step
/// its slider declared, and the unit and log scale its metadata declares (a
/// log scale over a range that reaches zero has no ratios and stays linear,
/// as `ControlDisplay::validate` asks). The reserved `out`/`in` bus controls are engine
/// plumbing and stay out of the reported surface.
#[cfg(feature = "faust")]
fn faust_controls(def: &FaustDef) -> Vec<Control> {
    def.params
        .iter()
        .map(|p| Control {
            display: ControlDisplay {
                min: Some(f64::from(p.min)),
                max: Some(f64::from(p.max)),
                step: Some(f64::from(p.step)),
                warp: (p.log && p.min > 0.0).then(|| "exp".to_string()),
                unit: p.unit.clone(),
            },
            ..Control::new(p.name.clone(), f64::from(p.init))
        })
        .collect()
}

/// **One control's record in `/def_query.reply`**, the same in every family:
/// `name, default, rate, min, max, step, warp, unit, label, group, flags,
/// numLabels, label...`. A def that declares no range reports `min` and `max`
/// equal (both zero); `step` zero is continuous; an empty `warp` is linear, an
/// empty `unit` none, an empty `label` the name and an empty `group` none.
/// `flags` are a plugin parameter's ([`clausters_core::plugin_catalog::Parameter`]'s
/// bits) and zero elsewhere, and the labels name a list's steps.
fn push_control(args: &mut Vec<OscType>, c: &Control) {
    let (min, max) = c.display.min.zip(c.display.max).unwrap_or((0.0, 0.0));
    let text = |s: &Option<String>| OscType::String(s.clone().unwrap_or_default());
    args.extend([
        OscType::String(c.name.clone()),
        OscType::Float(c.default as f32),
        OscType::String(c.rate.clone()),
        OscType::Float(min as f32),
        OscType::Float(max as f32),
        OscType::Float(c.display.step.unwrap_or(0.0) as f32),
        text(&c.display.warp),
        text(&c.display.unit),
        text(&c.label),
        OscType::String(c.group.clone()),
        OscType::Int(c.flags as i32),
        OscType::Int(c.labels.len() as i32),
    ]);
    args.extend(c.labels.iter().cloned().map(OscType::String));
}
