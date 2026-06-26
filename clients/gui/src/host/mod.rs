//! The GUI host: an OSC front with a widget command interpreter.
//!
//! `clausters-gui` is **two roles in one process**: a *GUI server (host)* for
//! the language clients — it owns the windows, the widgets and (later) the GPU,
//! and speaks the `/gui_*` widget protocol — and a *client of the audio server*
//! — it reads buffers/buses/the node tree and sends control, exactly as the
//! Python client does. This milestone is the **headless skeleton**: the server
//! front and a scaffolded client leg, a widget [`Registry`], and a command loop
//! that interprets `/gui_def`/`/gui_set`/`/gui_free`/`/gui_query`, logs them and
//! answers `/gui_query` with `/gui_info`. No GPU yet (G3 brings the first
//! pixels).
//!
//! ## Transport decision (recorded here per the milestone)
//!
//! The host does **not** extract or link the audio server's transport layer
//! (`src/osc/{server,tcp,ws}.rs`): that code is tangled with the audio
//! `ServerState`, the engine wake and the IPC ring, so lifting it now would drag
//! server concerns into this independent crate for no gain. Instead the host
//! **links `clausters-core`** — a path dependency that pulls only `rosc`, never
//! the server crate — for the shared OSC seam (the single
//! [`clausters_core::osc::decode_packet`] door, plus encode/bundle/message), and
//! owns a **thin transport front** of its own ([`transport`]). G2 ships the
//! **UDP** front (the default Clausters carrier, minimal to drive from a Python
//! client); TCP/WebSocket/ring are added in later milestones behind the same
//! [`transport::ClientId`] and reply seam, which is shaped to generalize. The
//! client leg ([`client::ServerLeg`]) reuses that same encode door, so the gui
//! talks to the audio server with one encoder, not a parallel one.

pub mod client;
pub mod controls;
pub mod font;
pub mod guidef;
pub mod layout;
pub mod meters;
pub mod paint;
pub mod registry;
pub mod transport;
pub mod widget;

// Reading the audio server's shared-memory segment for zero-message meters/
// scopes (G5). Unix-only, as the server's segment is.
#[cfg(unix)]
pub mod shm;

// The windowed host (winit + wgpu) is native-only; a wasm build swaps it for a
// `<canvas>` surface. Everything above is windowing-agnostic and web-portable.
#[cfg(not(target_arch = "wasm32"))]
pub mod gui;

use std::collections::HashMap;

use clausters_core::osc::{OscMessage, OscPacket, OscType};
use serde_json::Value;
use tracing::{debug, info, warn};

pub use client::ServerLeg;
pub use guidef::GuiNode;
pub use registry::Registry;
pub use transport::ClientId;
pub use widget::Widget;

/// A source of live control-bus values for the meter/scope views. Implemented by
/// the shared-memory segment ([`shm::SharedSegment`]) on Unix; the trait lets the
/// windowed front hold the source without platform `cfg`s and read a bus each
/// frame with no OSC traffic.
pub trait BusSource: Send + Sync {
    /// The current value of control bus `index` (`0.0` if out of range).
    fn control(&self, index: usize) -> f32;
}

// The `/gui_*` vocabulary (canonical tables in clients/gui/PLAN.md).
pub const GUI_DEF: &str = "/gui_def";
pub const GUI_SET: &str = "/gui_set";
pub const GUI_FREE: &str = "/gui_free";
pub const GUI_QUERY: &str = "/gui_query";
pub const GUI_BIND: &str = "/gui_bind";
pub const GUI_LOAD: &str = "/gui_load";
pub const GUI_INFO: &str = "/gui_info";
pub const GUI_EVENT: &str = "/gui_event";
pub const GUI_CLOSED: &str = "/gui_closed";

/// What handling a packet asks the host's *front* to do, beyond mutating the
/// host's own state. The protocol logic stays transport- and GPU-agnostic and
/// *returns* these, so the caller decides how to act: the windowed front opens
/// and closes OS windows and sends replies; the headless front sends replies and
/// logs the window effects (no display). That keeps the logic unit-testable
/// without a socket or a GPU.
#[derive(Debug)]
pub enum HostEffect {
    /// Send this message back to the requesting client.
    Reply(OscMessage),
    /// Open (or rebuild) the window for the GuiDef rooted at this id.
    OpenWindow(i32),
    /// Close the window for the GuiDef rooted at this id, if any.
    CloseWindow(i32),
    /// A live `/gui_set` changed a widget in the window rooted at this id; the
    /// front should repaint it (the typed tree is already updated in place).
    Redraw(i32),
}

/// The widget-protocol interpreter (transport- and GPU-agnostic). See
/// [`handle_packet`](Self::handle_packet) and [`HostEffect`].
pub struct Host {
    registry: Registry,
    /// Typed widget trees for window-rooted defs, by def id — the renderable
    /// documents the windowed front builds windows from. Non-window roots live
    /// only in the generic registry.
    window_defs: HashMap<i32, Widget>,
    /// The audio-server client leg (the third topology leg). Present when the
    /// host was started with a `--server` target; bindings (a later milestone)
    /// forward bound-widget values through it.
    server: Option<ServerLeg>,
}

impl Default for Host {
    fn default() -> Self {
        Self::new()
    }
}

impl Host {
    pub fn new() -> Self {
        Self {
            registry: Registry::new(),
            window_defs: HashMap::new(),
            server: None,
        }
    }

    /// Attaches the audio-server client leg (host -> audio server).
    pub fn with_server(mut self, server: ServerLeg) -> Self {
        self.server = Some(server);
        self
    }

    /// The audio-server client leg, if one was attached (`--server`). The
    /// windowed front uses it to query and fetch server buffers.
    pub fn server(&self) -> Option<&ServerLeg> {
        self.server.as_ref()
    }

    /// Read access to the widget tree (for tests and introspection).
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// The typed window document for def `id`, if it is a window-rooted def the
    /// front should render. The single source of truth: the windowed front
    /// renders and hit-tests from it, and live `/gui_set`s mutate it in place
    /// (see [`window_def_mut`](Self::window_def_mut)).
    pub fn window_def(&self, id: i32) -> Option<&Widget> {
        self.window_defs.get(&id)
    }

    /// Mutable access to a window document, for the front to write back a value
    /// a user interaction produced (a turned knob, a moved slider).
    pub fn window_def_mut(&mut self, id: i32) -> Option<&mut Widget> {
        self.window_defs.get_mut(&id)
    }

    /// Handles one decoded packet from `from`, returning the effects its front
    /// should carry out (replies plus window open/close). A bundle is unwrapped
    /// and its messages run in order (the timetag is treated as immediate at this
    /// milestone — no scheduling yet).
    pub fn handle_packet(&mut self, packet: OscPacket, from: ClientId) -> Vec<HostEffect> {
        let mut effects = Vec::new();
        self.dispatch_packet(packet, from, &mut effects);
        effects
    }

    fn dispatch_packet(
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

    fn dispatch(&mut self, msg: OscMessage, from: ClientId, effects: &mut Vec<HostEffect>) {
        match msg.addr.as_str() {
            GUI_DEF => self.on_def(&msg.args, from, effects),
            GUI_SET => self.on_set(&msg.args, from, effects),
            GUI_FREE => self.on_free(&msg.args, from, effects),
            GUI_QUERY => self.on_query(&msg.args, from, effects),
            GUI_BIND => warn!("{from}: {GUI_BIND} is not implemented yet (a later milestone)"),
            GUI_LOAD => warn!("{from}: {GUI_LOAD} is not implemented yet (a later milestone)"),
            other => debug!("{from}: ignoring unhandled address {other}"),
        }
    }

    /// `/gui_def <id> <json> [blob…]` — build a whole widget tree from one JSON
    /// GuiDef (with any bulk data, e.g. waveform samples, as trailing blobs). A
    /// `window` root also opens (or rebuilds) a window.
    fn on_def(&mut self, args: &[OscType], from: ClientId, effects: &mut Vec<HostEffect>) {
        let Some(id) = int_arg(args, 0) else {
            return warn!("{from}: {GUI_DEF} needs an integer id");
        };
        let Some(bytes) = json_arg(args, 1) else {
            return warn!("{from}: {GUI_DEF} needs a JSON string or blob argument");
        };
        let node = match GuiNode::parse(bytes) {
            Ok(node) => node,
            Err(e) => return warn!("{from}: {GUI_DEF} {id}: invalid GuiDef JSON: {e}"),
        };
        let outcome = self.registry.define(id, &node);
        // The acceptance criterion: log the parsed tree.
        info!(
            "{from}: {GUI_DEF} {id}: {} widget(s){}{}\n{}",
            outcome.inserted,
            if outcome.replaced { " (replaced)" } else { "" },
            if outcome.skipped > 0 {
                format!(", {} skipped", outcome.skipped)
            } else {
                String::new()
            },
            node.dump(id).trim_end(),
        );
        // A window root becomes a renderable typed document; the front opens it.
        if node.kind == "window" {
            let blobs = blob_args(&args[2.min(args.len())..]);
            match Widget::from_node(id, &node, &blobs) {
                Ok(tree) => {
                    self.window_defs.insert(id, tree);
                    effects.push(HostEffect::OpenWindow(id));
                }
                Err(e) => warn!("{from}: {GUI_DEF} {id}: cannot build window: {e}"),
            }
        }
    }

    /// `/gui_set <id> <k> <v> ...` — update one live widget's properties, in the
    /// generic registry (for `/gui_query`) and, if it is inside an open window,
    /// in the typed render tree (so the change shows live).
    fn on_set(&mut self, args: &[OscType], from: ClientId, effects: &mut Vec<HostEffect>) {
        let Some(id) = int_arg(args, 0) else {
            return warn!("{from}: {GUI_SET} needs an integer id");
        };
        let props = key_value_pairs(&args[1..]);
        if props.is_empty() {
            return warn!("{from}: {GUI_SET} {id}: no key/value pairs");
        }
        let keys: Vec<&String> = props.iter().map(|(k, _)| k).collect();
        if !self.registry.set(id, props.clone()) {
            return warn!("{from}: {GUI_SET} {id}: no such widget");
        }
        info!("{from}: {GUI_SET} {id}: updated {keys:?}");
        // Mirror the change into the typed window tree the front renders.
        if let Some(root) = self.registry.root_of(id)
            && let Some(tree) = self.window_defs.get_mut(&root)
            && let Some(widget) = tree.find_mut(id)
        {
            let mut changed = false;
            for (k, v) in &props {
                changed |= widget.kind.apply(k, v);
            }
            if changed {
                effects.push(HostEffect::Redraw(root));
            }
        }
    }

    /// `/gui_free <id>` — destroy a widget and its subtree (and its window, if
    /// `id` is a window-rooted def).
    fn on_free(&mut self, args: &[OscType], from: ClientId, effects: &mut Vec<HostEffect>) {
        let Some(id) = int_arg(args, 0) else {
            return warn!("{from}: {GUI_FREE} needs an integer id");
        };
        let removed = self.registry.free(id);
        if self.window_defs.remove(&id).is_some() {
            effects.push(HostEffect::CloseWindow(id));
        }
        if removed > 0 {
            info!("{from}: {GUI_FREE} {id}: freed {removed} widget(s)");
        } else {
            warn!("{from}: {GUI_FREE} {id}: no such widget");
        }
    }

    /// `/gui_query <id>` — reply `/gui_info <id> <type> <k> <v> ...`.
    fn on_query(&mut self, args: &[OscType], from: ClientId, effects: &mut Vec<HostEffect>) {
        let Some(id) = int_arg(args, 0) else {
            return warn!("{from}: {GUI_QUERY} needs an integer id");
        };
        let mut out = vec![OscType::Int(id)];
        match self.registry.get(id) {
            Some(widget) => {
                out.push(OscType::String(widget.kind.clone()));
                for (k, v) in &widget.props {
                    if let Some(arg) = scalar_arg(v) {
                        out.push(OscType::String(k.clone()));
                        out.push(arg);
                    }
                }
                info!("{from}: {GUI_QUERY} {id} -> {GUI_INFO} ({})", widget.kind);
            }
            None => {
                // An empty type string means "no such widget" — the query still
                // gets an answer, the way the server replies even on a miss.
                out.push(OscType::String(String::new()));
                warn!("{from}: {GUI_QUERY} {id}: no such widget");
            }
        }
        effects.push(HostEffect::Reply(OscMessage {
            addr: GUI_INFO.into(),
            args: out,
        }));
    }
}

/// Collects the trailing OSC blob arguments of a `/gui_def` (the bulk data, e.g.
/// waveform samples) into a list a `Widget` can index by `"blob"`.
fn blob_args(args: &[OscType]) -> Vec<Vec<u8>> {
    args.iter()
        .filter_map(|a| match a {
            OscType::Blob(b) => Some(b.clone()),
            _ => None,
        })
        .collect()
}

/// The i-th argument as an `i32`, if present and integer-typed.
fn int_arg(args: &[OscType], i: usize) -> Option<i32> {
    match args.get(i) {
        Some(OscType::Int(n)) => Some(*n),
        Some(OscType::Long(n)) => Some(*n as i32),
        _ => None,
    }
}

/// The i-th argument as JSON bytes: a string or a blob (both accepted, as
/// `/d_recv` accepts a SynthDef either way).
fn json_arg(args: &[OscType], i: usize) -> Option<&[u8]> {
    match args.get(i) {
        Some(OscType::String(s)) => Some(s.as_bytes()),
        Some(OscType::Blob(b)) => Some(b.as_slice()),
        _ => None,
    }
}

/// Turns a flat `k, v, k, v, ...` OSC tail into `(String, Value)` pairs,
/// preserving the int/float distinction (an OSC `Int` stays an integer JSON
/// number, a `Float` a floating one). A trailing unpaired key is ignored.
fn key_value_pairs(tail: &[OscType]) -> Vec<(String, Value)> {
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

/// One OSC primitive as a JSON value, keeping integers and floats apart.
fn osc_to_value(arg: &OscType) -> Option<Value> {
    match arg {
        OscType::Int(n) => Some(Value::from(*n)),
        OscType::Long(n) => Some(Value::from(*n)),
        OscType::Float(x) => Some(Value::from(*x)),
        OscType::Double(x) => Some(Value::from(*x)),
        OscType::String(s) => Some(Value::from(s.clone())),
        _ => None,
    }
}

/// One scalar JSON value as an OSC primitive for a `/gui_info` reply, keeping
/// integers (`Int`) and floats (`Float`) apart; `None` for structural values.
fn scalar_arg(v: &Value) -> Option<OscType> {
    match v {
        Value::Bool(b) => Some(OscType::Int(*b as i32)),
        Value::Number(n) if n.is_i64() || n.is_u64() => Some(OscType::Int(n.as_i64()? as i32)),
        Value::Number(n) => Some(OscType::Float(n.as_f64()? as f32)),
        Value::String(s) => Some(OscType::String(s.clone())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddr};

    fn from() -> ClientId {
        ClientId::Udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 9000)))
    }

    fn def_msg(id: i32, json: &str) -> OscPacket {
        OscPacket::Message(OscMessage {
            addr: GUI_DEF.into(),
            args: vec![OscType::Int(id), OscType::String(json.into())],
        })
    }

    /// The reply messages among a batch of effects.
    fn replies(effects: Vec<HostEffect>) -> Vec<OscMessage> {
        effects
            .into_iter()
            .filter_map(|e| match e {
                HostEffect::Reply(m) => Some(m),
                _ => None,
            })
            .collect()
    }

    /// The def ids of any OpenWindow effects.
    fn opened(effects: &[HostEffect]) -> Vec<i32> {
        effects
            .iter()
            .filter_map(|e| match e {
                HostEffect::OpenWindow(id) => Some(*id),
                _ => None,
            })
            .collect()
    }

    const TREE: &str = r#"{"type":"window","title":"Filter","children":[
        {"id":10,"type":"knob","label":"cutoff","min":20.0,"max":20000.0,"value":800.0}
    ]}"#;

    #[test]
    fn window_def_opens_a_window_and_stores_the_typed_def() {
        let mut host = Host::new();
        let effects = host.handle_packet(def_msg(1, TREE), from());
        assert_eq!(opened(&effects), vec![1], "a window root opens a window");
        assert_eq!(host.registry().len(), 2, "window + knob in the registry");
        assert!(
            host.window_def(1).is_some(),
            "the typed window def is stored"
        );
    }

    #[test]
    fn def_then_query_replies_with_gui_info() {
        let mut host = Host::new();
        host.handle_packet(def_msg(1, TREE), from());

        let query = OscPacket::Message(OscMessage {
            addr: GUI_QUERY.into(),
            args: vec![OscType::Int(10)],
        });
        let out = replies(host.handle_packet(query, from()));
        assert_eq!(out.len(), 1);
        let info = &out[0];
        assert_eq!(info.addr, GUI_INFO);
        assert_eq!(info.args[0], OscType::Int(10));
        assert_eq!(info.args[1], OscType::String("knob".into()));
        // The reply carries the knob's props as k/v pairs, ints and floats kept
        // apart; `value` is a float.
        let pos = info
            .args
            .iter()
            .position(|a| *a == OscType::String("value".into()))
            .expect("value key present");
        assert_eq!(info.args[pos + 1], OscType::Float(800.0));
    }

    #[test]
    fn query_for_unknown_id_still_answers() {
        let mut host = Host::new();
        let query = OscPacket::Message(OscMessage {
            addr: GUI_QUERY.into(),
            args: vec![OscType::Int(42)],
        });
        let out = replies(host.handle_packet(query, from()));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].args[0], OscType::Int(42));
        assert_eq!(out[0].args[1], OscType::String(String::new()));
    }

    #[test]
    fn set_updates_a_live_widget() {
        let mut host = Host::new();
        host.handle_packet(def_msg(1, TREE), from());
        let set = OscPacket::Message(OscMessage {
            addr: GUI_SET.into(),
            args: vec![
                OscType::Int(10),
                OscType::String("value".into()),
                OscType::Float(440.0),
            ],
        });
        host.handle_packet(set, from());
        assert_eq!(
            host.registry().get(10).unwrap().props["value"],
            Value::from(440.0)
        );
    }

    #[test]
    fn free_drops_the_subtree_and_closes_the_window() {
        let mut host = Host::new();
        host.handle_packet(def_msg(1, TREE), from());
        let free = OscPacket::Message(OscMessage {
            addr: GUI_FREE.into(),
            args: vec![OscType::Int(1)],
        });
        let effects = host.handle_packet(free, from());
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, HostEffect::CloseWindow(1))),
            "freeing a window def closes its window"
        );
        assert!(host.registry().is_empty());
        assert!(host.window_def(1).is_none());
    }

    #[test]
    fn waveform_blob_rides_the_def_message() {
        let mut host = Host::new();
        let blob: Vec<u8> = [0.5f32, -0.5]
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect();
        let json = r#"{"type":"window","children":[{"id":9,"type":"waveform","blob":0}]}"#;
        let msg = OscPacket::Message(OscMessage {
            addr: GUI_DEF.into(),
            args: vec![
                OscType::Int(2),
                OscType::String(json.into()),
                OscType::Blob(blob),
            ],
        });
        let effects = host.handle_packet(msg, from());
        assert_eq!(opened(&effects), vec![2]);
        let tree = host.window_def(2).unwrap();
        match &tree.children[0].kind {
            widget::WidgetKind::Waveform { samples, .. } => assert_eq!(&samples[..], &[0.5, -0.5]),
            other => panic!("expected a waveform, got {other:?}"),
        }
    }

    #[test]
    fn bundle_is_unwrapped_in_order() {
        use clausters_core::osc::{IMMEDIATE, OscBundle};
        let mut host = Host::new();
        let bundle = OscPacket::Bundle(OscBundle {
            timetag: IMMEDIATE,
            content: vec![
                def_msg(1, TREE),
                OscPacket::Message(OscMessage {
                    addr: GUI_QUERY.into(),
                    args: vec![OscType::Int(1)],
                }),
            ],
        });
        let out = replies(host.handle_packet(bundle, from()));
        assert_eq!(out.len(), 1, "the query inside the bundle is answered");
        assert_eq!(out[0].args[1], OscType::String("window".into()));
    }
}
