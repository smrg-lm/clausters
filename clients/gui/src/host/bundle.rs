//! Booting a persisted bundle **over the wire** — the browser standalone path.
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
