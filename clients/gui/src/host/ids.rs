//! **The host's ids**: the node ids, buses and buffers it allocates on the
//! server it plays through, and the replies that give them back.
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

use crate::host::instance::Leg;
use crate::host::{Host, diag, play};

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
    /// the take monitor's def, `/server_notify 1`, so a node this host made
    /// comes back on its `/node_end`, and `/server_query`, so the spaces take
    /// the server's own shape rather than the default one.
    ///
    /// The def goes first and goes on every link, whoever launched the server:
    /// a def is asynchronous, so it has to be there before anything can press
    /// the space bar -- and a server that persists defs would otherwise answer
    /// with whatever copy an older host left on its disk.
    pub fn on_link_attached(&mut self) {
        self.send_to_player(play::take_def_message());
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

    /// **Binds a group of this host's own to the server's transport** and
    /// answers it: where the take monitor's readers are made **when no multitrack
    /// plays**. A multitrack makes the transport's group itself, the same as it does
    /// for every endpoint, and the monitor's group then goes inside that one
    /// instead ([`Host::monitor_group`]).
    ///
    /// Only a host that owns its server's transport does this -- an editor with
    /// its own player. A host that is a guest on a script's server leaves the
    /// transport to the script and makes its nodes in the root group.
    pub fn govern_transport(&mut self) -> Option<i32> {
        if let Some(group) = self.governed {
            return Some(group);
        }
        let group = self.alloc_nodes(1)?;
        for message in play::take_group_messages(group) {
            self.send_sound(message);
        }
        self.governed = Some(group);
        self.owns_transport = true;
        Some(group)
    }

    /// The group the transport governs, when this host bound one.
    pub fn governed_group(&self) -> Option<i32> {
        self.governed
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
            _ => {}
        }
        self.multitrack_reply(from, msg);
    }
}

/// The shape a `/server_query.reply` states: audio buses, control buses and
/// outputs first, the node table and the buffer slots at 7 and 8. `None` for
/// a reply too short to carry them.
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
    })
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

    /// **A link attached by any path sends the monitor's def first.** It was
    /// sent by the standalone session alone, so a host launched against a
    /// script's server played the space bar through whatever copy of the def
    /// that server had persisted -- an older one, whose gate never closed past
    /// the end of the take.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn an_attached_link_is_sent_the_monitors_def_first() {
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
        assert_eq!(msg, play::take_def_message());
    }

    /// The governed group is allocated once, like any node, and bound once.
    #[test]
    fn the_governed_group_is_one_of_the_hosts_nodes() {
        let mut host = Host::new();
        let group = host.govern_transport().unwrap();
        assert!(host.ids().contains(Space::Nodes, i64::from(group)));
        assert_eq!(host.govern_transport(), Some(group));
        assert!(host.owns_transport());
    }
}
