//! Booting a persisted bundle: the browser's replay, and the native mount.
//!
//! The native standalone mode hands the whole data directory to the embedded
//! server, which loads its own defs and boot preset (`attach_store` in the
//! server crate). A browser has no data directory and no embedded server: the
//! page fetches the same persisted files as URLs and replays them to the
//! in-page engine as ordinary OSC. This module owns that replay — the
//! **ordering and encoding** of the boot, platform-agnostic and natively
//! unit-tested; the fetching (a page concern) stays in JS.
//!
//! [`boot_packets`] mirrors the server's own boot order (defs → graphdefs →
//! boot preset → the GuiDef's `boot` messages) and brackets it with two
//! `/sync`s: the first marks the defs in (the barrier the native flow gets
//! implicitly by loading in-process), the second — arriving after everything,
//! since the engine serves strictly in order — is the page's "bundle is up"
//! signal (`/synced sync_id+1`).
//!
//! The second half is the **mount** ([`mount`], [`MountAllocator`]): a bundle
//! whose manifest declares the component contract is a *template*, and mounting
//! it means allocating its symbols and resolving its holes
//! ([`clausters_core::bundle`]). That is the same pass the browser runs, so one
//! directory behaves identically on all three legs — and a bundle written
//! before the contract existed (or with no manifest at all) mounts verbatim,
//! which is what keeps today's bundles running.

use clausters_core::bundle::{
    Allocation, Error as BundleError, Manifest, ParamInput, Requirements, Resolved, Template,
};
use clausters_core::osc::{OscMessage, OscPacket, OscType, encode};
use serde_json::Value;

/// The ordered OSC packets that boot a bundle on an audio server, from the
/// persisted files' bytes:
///
/// - `synthdefs`: each `defs/synthdefs/<name>.json` verbatim (a `/d_recv` spec);
/// - `graphdefs`: each `defs/graphdefs/<name>.json` verbatim (a `/d_graph` spec);
/// - `boot_json`: `boot.json` (the boot preset of standalone graphs), absent
///   when the bundle has none;
/// - `guidef_tree`: the GuiDef tree JSON (the `"gui"` field of the saved
///   record) — its root `boot` messages run last, as the native standalone
///   host runs them.
///
/// MIDI bindings — the one other thing the native data-dir boot restores — are
/// deliberately absent: the browser has no MIDI leg.
pub fn boot_packets(
    synthdefs: &[Vec<u8>],
    graphdefs: &[Vec<u8>],
    boot_json: Option<&[u8]>,
    guidef_tree: &[u8],
    sync_id: i32,
) -> Vec<Vec<u8>> {
    let mut messages = Vec::new();
    for spec in synthdefs {
        messages.push(OscMessage {
            addr: "/d_recv".into(),
            args: vec![OscType::Blob(spec.clone())],
        });
    }
    for spec in graphdefs {
        messages.push(OscMessage {
            addr: "/d_graph".into(),
            args: vec![OscType::Blob(spec.clone())],
        });
    }
    messages.push(sync(sync_id));
    if let Some(boot) = boot_json {
        messages.extend(boot_graphs(boot));
    }
    messages.extend(boot_messages(guidef_tree));
    messages.push(sync(sync_id + 1));
    messages
        .into_iter()
        .filter_map(|msg| encode(&OscPacket::Message(msg)).ok())
        .collect()
}

fn sync(id: i32) -> OscMessage {
    OscMessage {
        addr: "/sync".into(),
        args: vec![OscType::Int(id)],
    }
}

/// The `/graph_new` messages of a `boot.json` preset — one per entry, exactly
/// as the server's own boot builds them: `[{"graph": <name>, "ports":
/// {<port>: <value>, …}}, …]`, instantiated with id `-1` (server-allocated)
/// at the tail of the root group. Malformed entries are skipped, matching the
/// server's per-entry warn-and-continue.
fn boot_graphs(boot_json: &[u8]) -> Vec<OscMessage> {
    let Ok(Value::Array(list)) = serde_json::from_slice::<Value>(boot_json) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in list {
        let Value::Object(map) = entry else { continue };
        let Some(Value::String(graph)) = map.get("graph") else {
            continue;
        };
        let mut args = vec![
            OscType::String(graph.clone()),
            OscType::Int(-1),
            OscType::Int(0),
            OscType::Int(0),
        ];
        if let Some(Value::Object(ports)) = map.get("ports") {
            for (port, value) in ports {
                if let Some(v) = value.as_f64() {
                    args.push(OscType::String(port.clone()));
                    args.push(OscType::Float(v as f32));
                }
            }
        }
        out.push(OscMessage {
            addr: "/graph_new".into(),
            args,
        });
    }
    out
}

/// The `boot` messages declared at a GuiDef's root: a list of `[addr, args…]`
/// the standalone host sends to the server right after the defs load, to bring
/// the instrument up (e.g. `["/s_new", "drone", 1000, 0, 0]`). The int/float
/// distinction is preserved (a JSON integer is an OSC `Int`, so node ids stay
/// integers). Empty when the GuiDef declares no `boot`.
pub fn boot_messages(tree_json: &[u8]) -> Vec<OscMessage> {
    let Ok(Value::Object(root)) = serde_json::from_slice::<Value>(tree_json) else {
        return Vec::new();
    };
    let Some(Value::Array(list)) = root.get("boot") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in list {
        if let Value::Array(items) = entry
            && let Some(Value::String(addr)) = items.first()
        {
            let args = items[1..].iter().filter_map(value_to_osc).collect();
            out.push(OscMessage {
                addr: addr.clone(),
                args,
            });
        }
    }
    out
}

// --- the mount ------------------------------------------------------------

/// Where a host's own id spaces start when it mounts bundles.
///
/// The standalone front is the **only** client of its embedded server, so a
/// bump allocator is the whole story: nothing else is handing out these ids and
/// a mount never gives one back. The bases keep out of the way of what a
/// hand-written def already uses — node ids follow the client range's `1000`
/// (scsynth convention), buses start above the low ones instruments write to
/// out of habit, buffers above the first few a `boot.json` may name.
const WIDGET_BASE: i32 = 1000;
const NODE_BASE: i32 = 1000;
const BUS_BASE: i32 = 64;
const BUFFER_BASE: i32 = 32;

/// The id spaces a host hands to the bundles it mounts, one bump each.
///
/// It is deliberately not a [`Registry`](clausters_core::registry::Registry):
/// nothing here is ever released, because a mounted component lives as long as
/// the process. The browser leg allocates from the page's real allocators
/// instead — the resolver takes an allocation either way, which is exactly why
/// it takes one rather than making it.
pub struct MountAllocator {
    widget: i32,
    node: i32,
    bus: i32,
    buffer: i32,
}

impl Default for MountAllocator {
    fn default() -> Self {
        Self {
            widget: WIDGET_BASE,
            node: NODE_BASE,
            bus: BUS_BASE,
            buffer: BUFFER_BASE,
        }
    }
}

impl MountAllocator {
    /// Allocates one instance's worth of ids. Two calls with the same
    /// requirements never overlap — which is what lets one bundle mount twice.
    pub fn allocate(&mut self, req: &Requirements) -> Allocation {
        let mut allocation = Allocation {
            widget_base: self.widget,
            ..Allocation::default()
        };
        self.widget += req.widgets.max(1) as i32;
        for name in &req.nodes {
            allocation.nodes.insert(name.clone(), self.node);
            self.node += 1;
        }
        for spec in &req.buses {
            allocation.buses.insert(spec.name.clone(), self.bus);
            self.bus += spec.channels.max(1) as i32;
        }
        for name in &req.buffers {
            allocation.buffers.insert(name.clone(), self.buffer);
            self.buffer += 1;
        }
        allocation
    }
}

/// One mounted instance, ready to send: the GuiDef to open and the messages
/// that bring its half of the server up.
pub struct Mount {
    /// The id to open the GuiDef under (`/gui_def <def_id> …`).
    pub def_id: i32,
    /// The resolved tree, as the JSON the `/gui_def` argument carries.
    pub tree: Vec<u8>,
    /// What to send before opening it: the declared buffers' `/b_allocRead`s,
    /// then the GuiDef's own `boot` list.
    pub messages: Vec<OscMessage>,
}

/// Whether a manifest carries the component contract at all.
///
/// A manifest written before it — or a directory with none — declares no
/// widgets, no symbols and no parameters, and mounts **verbatim**: its saved
/// widget ids are used as they are and nothing is substituted. That is the
/// compatibility hinge of the whole format.
pub fn is_symbolic(manifest: &Manifest) -> bool {
    manifest.widgets > 0 || !manifest.symbols.is_empty() || !manifest.params.is_empty()
}

/// Mounts one instance of `template`: allocates what the manifest declares,
/// resolves the holes, and lays out what to send.
///
/// `dir` is the bundle's directory, used to resolve declared buffer files to
/// the paths `/b_allocRead` reads. `params` is what the caller supplies for the
/// declared parameters (nothing, on the desktop, so the defaults stand).
pub fn mount(
    manifest: &Manifest,
    template: &Template,
    dir: &str,
    alloc: &mut MountAllocator,
    params: &ParamInput,
) -> Result<Mount, BundleError> {
    let requirements = clausters_core::bundle::requirements(manifest);
    let allocation = alloc.allocate(&requirements);
    let Resolved {
        def_id, tree, boot, ..
    } = clausters_core::bundle::resolve(manifest, template, &allocation, params)?;

    let mut messages = Vec::new();
    // The samples first, so a boot message can already play one.
    for (name, file) in &manifest.buffers {
        let Some(&bufnum) = allocation.buffers.get(name) else {
            continue; // declared as a file but not as a symbol: nothing to fill
        };
        messages.push(OscMessage {
            addr: "/b_allocRead".into(),
            args: vec![
                OscType::Int(bufnum),
                OscType::String(format!("{dir}/{file}")),
            ],
        });
    }
    messages.extend(boot.iter().map(|entry| {
        OscMessage {
            addr: entry
                .first()
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            args: entry[1..].iter().filter_map(value_to_osc).collect(),
        }
    }));

    Ok(Mount {
        def_id,
        tree: serde_json::to_vec(&tree).unwrap_or_default(),
        messages,
    })
}

/// Reads a bundle directory's `bundle.json`, or `None` when it has none (the
/// native host then lists the directory, as it always did).
#[cfg(not(target_arch = "wasm32"))]
pub fn read_manifest(dir: &std::path::Path) -> Option<Manifest> {
    let bytes = std::fs::read(dir.join("bundle.json")).ok()?;
    match serde_json::from_slice(&bytes) {
        Ok(manifest) => Some(manifest),
        Err(e) => {
            tracing::warn!("{}/bundle.json is not a manifest: {e}", dir.display());
            None
        }
    }
}

/// One JSON value as an OSC primitive, keeping integers and floats apart.
fn value_to_osc(v: &Value) -> Option<OscType> {
    match v {
        Value::Number(n) if n.is_i64() || n.is_u64() => Some(OscType::Int(n.as_i64()? as i32)),
        Value::Number(n) => Some(OscType::Float(n.as_f64()? as f32)),
        Value::String(s) => Some(OscType::String(s.clone())),
        Value::Bool(b) => Some(OscType::Int(*b as i32)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_core::osc::decode_packet;

    fn decode(packets: &[Vec<u8>]) -> Vec<OscMessage> {
        packets
            .iter()
            .map(|bytes| match decode_packet(bytes).unwrap() {
                OscPacket::Message(m) => m,
                _ => panic!("boot packets are plain messages"),
            })
            .collect()
    }

    /// The full boot in the server's own order, bracketed by the two syncs.
    #[test]
    fn boot_order_matches_the_native_data_dir_boot() {
        let synthdef = br#"{"name":"drone","ugens":[]}"#.to_vec();
        let graphdef = br#"{"name":"rig","members":[]}"#.to_vec();
        let boot = br#"[{"graph":"rig","ports":{"level":0.5}}]"#;
        let gui = br#"{"type":"window","boot":[["/s_new","drone",1000,0,0]]}"#;
        let packets = boot_packets(
            std::slice::from_ref(&synthdef),
            &[graphdef],
            Some(boot),
            gui,
            700,
        );
        let msgs = decode(&packets);
        let addrs: Vec<&str> = msgs.iter().map(|m| m.addr.as_str()).collect();
        assert_eq!(
            addrs,
            [
                "/d_recv",
                "/d_graph",
                "/sync",
                "/graph_new",
                "/s_new",
                "/sync"
            ]
        );
        assert_eq!(msgs[0].args, vec![OscType::Blob(synthdef)]);
        assert_eq!(msgs[2].args, vec![OscType::Int(700)]);
        assert_eq!(
            msgs[3].args,
            vec![
                OscType::String("rig".into()),
                OscType::Int(-1),
                OscType::Int(0),
                OscType::Int(0),
                OscType::String("level".into()),
                OscType::Float(0.5),
            ]
        );
        // The boot /s_new keeps its node id an Int (the JSON was an integer).
        assert_eq!(msgs[4].args[1], OscType::Int(1000));
        assert_eq!(msgs[5].args, vec![OscType::Int(701)]);
    }

    /// A minimal bundle: no graphdefs, no boot.json, no GuiDef boot — just the
    /// def and the two syncs.
    #[test]
    fn empty_parts_are_skipped() {
        let packets = boot_packets(&[b"{}".to_vec()], &[], None, b"{}", 1);
        let addrs: Vec<String> = decode(&packets).into_iter().map(|m| m.addr).collect();
        assert_eq!(addrs, ["/d_recv", "/sync", "/sync"]);
    }

    fn symbolic() -> (Manifest, Template) {
        let manifest = serde_json::from_value(serde_json::json!({
            "name": "fm", "gui": "fm", "widgets": 2,
            "symbols": {
                "nodes": ["graph"],
                "buses": [{ "name": "lfo", "rate": "control", "channels": 1 }],
                "buffers": ["hit"]
            },
            "params": { "freq": { "type": "float", "default": 220.0 } },
            "buffers": { "hit": "audio/hit.wav" }
        }))
        .unwrap();
        let template = serde_json::from_value(serde_json::json!({
            "id": 1,
            "gui": {
                "type": "window",
                "boot": [["/s_new", "fm.voice", "@graph", 0, 0, "freq", "$freq"]],
                "children": [{ "id": 2, "type": "meter", "bus": "@lfo" }]
            }
        }))
        .unwrap();
        (manifest, template)
    }

    /// The milestone's whole point on this leg: one bundle, mounted twice, with
    /// nothing shared between the instances.
    #[test]
    fn one_bundle_mounts_twice_without_colliding() {
        let (manifest, template) = symbolic();
        let mut alloc = MountAllocator::default();
        let first = mount(
            &manifest,
            &template,
            "/data/fm",
            &mut alloc,
            &ParamInput::default(),
        )
        .unwrap();
        let second = mount(
            &manifest,
            &template,
            "/data/fm",
            &mut alloc,
            &ParamInput::default(),
        )
        .unwrap();

        assert_ne!(first.def_id, second.def_id);
        // Distinct node ids in the boot /s_new ...
        let node_of = |m: &Mount| m.messages.last().unwrap().args[1].clone();
        assert_ne!(node_of(&first), node_of(&second));
        // ... and distinct buses in the meter each one draws.
        let bus_of = |m: &Mount| {
            let tree: Value = serde_json::from_slice(&m.tree).unwrap();
            tree["children"][0]["bus"].clone()
        };
        assert_ne!(bus_of(&first), bus_of(&second));
        // The declared sample is read into each instance's own buffer.
        assert_eq!(first.messages[0].addr, "/b_allocRead");
        assert_eq!(
            first.messages[0].args[1],
            OscType::String("/data/fm/audio/hit.wav".into())
        );
        assert_ne!(first.messages[0].args[0], second.messages[0].args[0]);
    }

    /// A bundle written before the contract existed declares nothing, so it
    /// mounts verbatim — the compatibility hinge, asserted rather than assumed.
    #[test]
    fn a_pre_contract_bundle_is_not_symbolic() {
        let old: Manifest = serde_json::from_value(serde_json::json!({
            "gui": "piano", "synthdefs": ["piano_voice"], "graphdefs": []
        }))
        .unwrap();
        assert!(!is_symbolic(&old));
        let (symbolic, _) = symbolic();
        assert!(is_symbolic(&symbolic));
    }

    /// Malformed boot.json entries are skipped, not fatal (the server's own
    /// warn-and-continue posture).
    #[test]
    fn malformed_boot_entries_are_skipped() {
        let boot = br#"[{"no_graph":1},{"graph":"ok"},"junk"]"#;
        let packets = boot_packets(&[], &[], Some(boot), b"{}", 1);
        let msgs = decode(&packets);
        assert_eq!(msgs.iter().filter(|m| m.addr == "/graph_new").count(), 1);
        assert_eq!(msgs[1].args[0], OscType::String("ok".into()));
    }
}
