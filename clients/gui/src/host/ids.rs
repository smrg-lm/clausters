//! **The host's ids**: the node ids, buses, buffers and transports it
//! allocates on the server it plays through, the replies that give them back,
//! and the ids of the widgets it opens for itself.
//!
//! A host is a client of its audio server like any script, so it allocates by
//! the one policy every client does ([`clausters_core::ids`]): the node table's
//! client range, the buses above the server's outputs, and a **share** when
//! another client allocates on the same server. A script that launches a host
//! keeps share 0 of 2 and hands the host share 1 (`--id-share 1/2`); a host on
//! its own takes the whole space.
//!
//! What this replaced was three fixed windows -- the voices at `0x1000_0000`,
//! the monitor's readers and its group just past them, the multitrack's nodes past
//! those -- and a buffer base carried by hand from the session's load. None of
//! them recycled, and none of them knew the server's size.
//!
//! Ids come back the way a script's do: a node on its `/node_end` (the host
//! registers for it when a link attaches), a bus or a buffer when whatever
//! held it frees it.

use clausters_core::ids::{IdError, IdShare, IdSpaces, ServerShape, Space};
use clausters_core::osc::{OscMessage, OscType};
use clausters_core::widgetids::WidgetIds;

use crate::host::instance::Leg;
use crate::host::{Host, diag};

impl Host {
    /// **Takes share `share` of every space**, keeping what is already
    /// allocated. What a launcher gives the host (`--id-share`) when a script
    /// allocates on the same server.
    pub fn set_id_share(&mut self, share: IdShare) -> Result<(), IdError> {
        self.ids.narrow(share)
    }

    /// The spaces this host allocates from.
    pub fn ids(&self) -> &IdSpaces {
        &self.ids
    }

    /// The spaces this host allocates from, for a caller that allocates on
    /// its behalf -- a session's load, which reads its takes into buffers
    /// before anything plays.
    pub fn ids_mut(&mut self) -> &mut IdSpaces {
        &mut self.ids
    }

    /// **Both of the host's id tables at once**, for a caller that allocates
    /// server ids and widget ids in one go: a bundle being mounted.
    pub fn mount_ids(&mut self) -> (&mut IdSpaces, &mut WidgetIds) {
        (&mut self.ids, &mut self.widget_ids)
    }

    /// **The id of a widget this host opens for itself**, by what it draws:
    /// the same number for as long as the name is held, and one no client of
    /// this host is handed. `None`, said out loud, when the table is full.
    pub(crate) fn own_widget(&mut self, structure: i64, role: &str, key: &str) -> Option<i32> {
        let id = self
            .widget_ids
            .id_for(self.drawer, structure, role, key)
            .and_then(|id| i32::try_from(id).ok());
        if id.is_none() {
            diag::warn!("out of widget ids for the host's own windows");
        }
        id
    }

    /// **Numbers the widgets of one of the host's own windows that came with
    /// none** -- an application's tools, which a client numbers on the way
    /// out -- each by its place in the tree, under `structure` and `role`, so
    /// a window opened again keeps its numbers. The root keeps none: a
    /// GuiDef's id is the one its `/gui_def` names.
    pub(crate) fn number_own(&mut self, def: &mut serde_json::Value, structure: i64, role: &str) {
        let mut n = 0;
        let mut next = |_: &serde_json::Value| {
            n += 1;
            self.own_widget(structure, role, &format!("free:{n}"))
        };
        number_children(def, &mut next);
    }

    /// Returns the id of one of the host's own widgets, by its name.
    pub(crate) fn forget_own_widget(&mut self, structure: i64, role: &str, key: &str) {
        self.widget_ids.forget(structure, role, key);
    }

    /// The id one of the host's own widgets has, when it has one.
    #[cfg(test)]
    pub(crate) fn own_widget_id(&self, structure: i64, role: &str, key: &str) -> Option<i32> {
        self.widget_ids
            .id_of(structure, role, key)
            .and_then(|id| i32::try_from(id).ok())
    }

    /// A run of `width` node ids, or `None` said out loud.
    pub(crate) fn alloc_nodes(&mut self, width: usize) -> Option<i32> {
        match self.ids.alloc(Space::Nodes, width) {
            Ok(first) => Some(first as i32),
            Err(e) => {
                diag::warn!("cannot make a node: {e}");
                None
            }
        }
    }

    /// **What a freshly attached link to the server that sounds is told**:
    /// `/server_notify 1`, so a node this host made comes back on its
    /// `/node_end`, and `/server_query`, so the spaces take the server's own
    /// shape rather than the default one. The monitor's defs go with its first
    /// play, sent by its playback -- so a server that persists defs never
    /// answers with an older host's copy.
    pub fn on_link_attached(&mut self) {
        for addr in ["/server_notify", "/server_query"] {
            self.send_to_player(OscMessage {
                addr: addr.into(),
                args: if addr == "/server_notify" {
                    vec![OscType::Int(1)]
                } else {
                    vec![]
                },
            });
        }
    }

    /// **Every reply from the audio server passes here first**, from both
    /// fronts, with the leg it came in on: a node this host made that ended,
    /// the server's shape, and whatever the multitrack's steps are waiting on.
    /// Anything else is not the ids' and costs a match.
    pub fn on_server_reply(&mut self, from: Leg, msg: &OscMessage) {
        match msg.addr.as_str() {
            "/node_end" => {
                if let Some(OscType::Int(node)) = msg.args.first() {
                    self.ids.node_ended(i64::from(*node));
                }
            }
            "/server_query.reply" => {
                // The engine's rate, which a take's frames are converted to.
                match msg.args.get(4) {
                    Some(OscType::Double(rate)) if *rate > 0.0 => self.server_rate = *rate,
                    Some(OscType::Float(rate)) if *rate > 0.0 => {
                        self.server_rate = f64::from(*rate)
                    }
                    Some(OscType::Int(rate)) if *rate > 0 => self.server_rate = f64::from(*rate),
                    _ => {}
                }
                if let Some(shape) = shape_of(&msg.args) {
                    let share = self.ids.share();
                    if let Err(e) = self.ids.reshape(shape, share) {
                        diag::warn!(
                            "the server's shape ({shape:?}) does not hold the ids this host \
                             already has, which stay as they were: {e}"
                        );
                    }
                }
            }
            "/transport_query.reply" => self.on_transport_state(&msg.args),
            _ => {}
        }
        self.multitrack_reply(from, msg);
    }
}

/// The shape a `/server_query.reply` states: audio buses, control buses and
/// outputs first, the node table and the buffer slots at 7 and 8, the
/// transports at 15 -- the default count for a reply that stops short of
/// them. `None` for a reply too short to carry the rest.
fn shape_of(args: &[OscType]) -> Option<ServerShape> {
    let int = |i: usize| match args.get(i) {
        Some(OscType::Int(v)) if *v >= 0 => Some(*v as usize),
        _ => None,
    };
    Some(ServerShape {
        audio_buses: int(0)?,
        control_buses: int(1)?,
        outputs: int(2)?,
        max_nodes: int(7)?,
        buffers: int(8)?,
        transports: int(15).unwrap_or(ServerShape::DEFAULT.transports),
    })
}

/// **Numbers every widget under `def` that came with no id**, in the order
/// the tree is written, by what `next` hands out; the root is left as it is.
pub(crate) fn number_children(
    def: &mut serde_json::Value,
    next: &mut dyn FnMut(&serde_json::Value) -> Option<i32>,
) {
    let Some(children) = def.get_mut("children").and_then(|c| c.as_array_mut()) else {
        return;
    };
    for child in children {
        if child.get("id").is_none_or(serde_json::Value::is_null)
            && let Some(id) = next(child)
            && let Some(map) = child.as_object_mut()
        {
            map.insert("id".into(), serde_json::json!(id));
        }
        number_children(child, next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(args: Vec<OscType>) -> OscMessage {
        OscMessage {
            addr: "/server_query.reply".into(),
            args,
        }
    }

    /// **The server's answer shapes the spaces**, and the share given at
    /// launch holds across it.
    #[test]
    fn the_servers_answer_shapes_the_spaces_and_keeps_the_share() {
        let mut host = Host::new();
        host.set_id_share(IdShare::new(1, 2).unwrap()).unwrap();
        let ints = |v: &[i32]| v.iter().map(|n| OscType::Int(*n)).collect::<Vec<_>>();
        host.on_server_reply(
            Leg::Server,
            &reply(ints(&[512, 4096, 6, 64, 48000, 48000, 2, 2048, 128])),
        );
        let shape = host.ids().shape();
        assert_eq!(
            (shape.outputs, shape.max_nodes, shape.buffers),
            (6, 2048, 128)
        );
        assert_eq!(host.ids().share(), IdShare::new(1, 2).unwrap());
        assert_eq!(
            host.ids_mut().alloc(Space::Buffers, 1),
            Ok(64),
            "the second half of 128 slots"
        );
    }

    /// **A node the host made comes back on its `/node_end`**, and one it did
    /// not make is nobody's business here.
    #[test]
    fn a_node_end_gives_the_id_back() {
        let mut host = Host::new();
        let node = host.alloc_nodes(1).unwrap();
        assert_eq!(host.ids().in_use(Space::Nodes), 1);
        let end = |n: i32| OscMessage {
            addr: "/node_end".into(),
            args: vec![OscType::Int(n)],
        };
        host.on_server_reply(Leg::Server, &end(3));
        assert_eq!(host.ids().in_use(Space::Nodes), 1);
        host.on_server_reply(Leg::Server, &end(node));
        assert_eq!(host.ids().in_use(Space::Nodes), 0);
    }

    /// **A link attached by any path asks to be told**: `/server_notify` first,
    /// so a node this host makes comes back on its `/node_end`.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn an_attached_link_is_asked_to_notify() {
        use clausters_core::osc::{OscPacket, decode_packet};
        use std::net::UdpSocket;
        use std::time::Duration;

        let fake_server = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        fake_server
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let leg = crate::host::ServerLeg::connect(fake_server.local_addr().unwrap()).unwrap();
        let mut host = Host::new().with_server(leg);
        host.on_link_attached();

        let mut buf = [0u8; 8192];
        let (len, _) = fake_server.recv_from(&mut buf).expect("a datagram");
        let OscPacket::Message(msg) = decode_packet(&buf[..len]).unwrap() else {
            panic!("expected a message");
        };
        assert_eq!(msg.addr, "/server_notify");
    }
}
