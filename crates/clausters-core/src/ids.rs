//! **The id spaces a client allocates from**, sized from the server and sliced
//! for the clients sharing it — the policy every endpoint used to restate.
//!
//! [`Registry`](crate::registry::Registry) is the occupancy map; this is what
//! stands on it. A client needs four spaces — node ids, audio buses, control
//! buses, buffers — and each has a shape the *server* decides: the node table's
//! client range, the output buses at the bottom of the audio space, the private
//! GraphDef windows at the top of both bus spaces. Two clients on one server
//! each take a share of what is left.
//!
//! It was written three times: the Python client's four allocator classes, the
//! web client's four and a second scheme of fixed bases for the page, and the
//! GUI host's own windows with no registry at all. They had drifted on a fact —
//! one client reserved two output buses by default where the other reserved the
//! server's output count — and the host's copy was the one that went silent.
//! One policy, here, bound by every endpoint.

use crate::registry::{
    NodeIdPartition, Registry, ReleaseError, graph_audio_reserved, graph_control_reserved,
};

/// **Which slice of a client id space a client takes.**
///
/// `index` of `of`: a server with one client gives it the whole of every space
/// (`0` of `1`), and a server shared between clients gives each a disjoint
/// slice of each, with no coordination protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdShare {
    /// This client's slice, from `0` to `of - 1`.
    pub index: u32,
    /// How many clients the spaces are split between.
    pub of: u32,
}

impl IdShare {
    /// The whole of every space: the one client of a server.
    pub const WHOLE: IdShare = IdShare { index: 0, of: 1 };

    /// A share, refused when it is not one (`of` of zero, or an `index` past
    /// the split).
    pub fn new(index: u32, of: u32) -> Result<IdShare, IdError> {
        if of == 0 {
            return Err(IdError::BadShare);
        }
        if index >= of {
            return Err(IdError::BadShare);
        }
        Ok(IdShare { index, of })
    }
}

impl Default for IdShare {
    fn default() -> Self {
        IdShare::WHOLE
    }
}

/// **The `(base, span)` of a share within `span` ids at `base`.**
///
/// The **last share takes the remainder**, so the slices tile the range exactly
/// rather than leaving a few ids nobody may allocate. A share of a range too
/// small to split yields an empty span, and an empty space reports exhaustion
/// from its first allocation — a client that cannot allocate says so.
pub fn share_of(base: i64, span: usize, share: IdShare) -> (i64, usize) {
    let of = share.of.max(1) as usize;
    let index = (share.index as usize).min(of - 1);
    let each = span / of;
    let first = base + (index * each) as i64;
    if index == of - 1 {
        (first, span - index * each)
    } else {
        (first, each)
    }
}

/// Which of the four spaces an id belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    /// Node ids.
    Nodes,
    /// Audio buses.
    AudioBuses,
    /// Control buses.
    ControlBuses,
    /// Buffer numbers.
    Buffers,
}

impl Space {
    /// The space a wire name spells: `"nodes"`, `"audio"`, `"control"`,
    /// `"buffers"`.
    pub fn parse(name: &str) -> Option<Space> {
        Some(match name {
            "nodes" => Space::Nodes,
            "audio" => Space::AudioBuses,
            "control" => Space::ControlBuses,
            "buffers" => Space::Buffers,
            _ => return None,
        })
    }

    /// What a message about this space calls it.
    fn noun(self) -> &'static str {
        match self {
            Space::Nodes => "node ids",
            Space::AudioBuses => "audio buses",
            Space::ControlBuses => "control buses",
            Space::Buffers => "buffer slots",
        }
    }
}

/// Why an allocation or a release was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdError {
    /// No run of the width asked for is free.
    Exhausted(Space),
    /// A release of ids this space never handed out, or has already taken back.
    NotAllocated(Space),
    /// A share that is not one.
    BadShare,
}

impl std::fmt::Display for IdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdError::Exhausted(space) => write!(f, "out of {}", space.noun()),
            IdError::NotAllocated(space) => write!(
                f,
                "double free of {}: not currently allocated here",
                space.noun()
            ),
            IdError::BadShare => write!(f, "an id share is `index` of `of`, with index < of"),
        }
    }
}

impl std::error::Error for IdError {}

/// **What a server says about itself** that decides the shape of its spaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServerShape {
    /// The node table's capacity (`--max-nodes`).
    pub max_nodes: usize,
    /// How many audio buses the server has.
    pub audio_buses: usize,
    /// How many of them are its **outputs**, at the bottom of the space.
    ///
    /// The server's output channel count, which is the answer that is a fact:
    /// a client that reserved a fixed two left a stereo-plus server's third
    /// output allocatable, and a mono one two buses short.
    pub outputs: usize,
    /// How many control buses it has.
    pub control_buses: usize,
    /// How many buffer slots it has.
    pub buffers: usize,
}

/// **The four spaces one client allocates from.**
pub struct IdSpaces {
    shape: ServerShape,
    score: bool,
    nodes: Registry,
    audio: Option<Registry>,
    control: Option<Registry>,
    buffers: Registry,
}

impl IdSpaces {
    /// The spaces of a live client of a server of this shape, taking `share`
    /// of each.
    pub fn new(shape: ServerShape, share: IdShare) -> IdSpaces {
        let part = NodeIdPartition::from_max_nodes(shape.max_nodes);
        let (base, span) = share_of(part.client_base, part.client_capacity, share);
        IdSpaces {
            shape,
            score: false,
            nodes: Registry::new(base, span),
            audio: bus_space(
                shape.audio_buses,
                shape.outputs,
                graph_audio_reserved(shape.audio_buses),
                share,
            ),
            control: bus_space(
                shape.control_buses,
                0,
                graph_control_reserved(shape.control_buses),
                share,
            ),
            buffers: {
                let (base, span) = share_of(0, shape.buffers, share);
                Registry::new(base, span)
            },
        }
    }

    /// The spaces of an **offline score**: node ids ascend from the client base
    /// and never run out, because a score has no `/node_end` stream to recycle
    /// from and one author by construction. The other three are the live ones.
    pub fn score(shape: ServerShape) -> IdSpaces {
        let mut spaces = IdSpaces::new(shape, IdShare::WHOLE);
        spaces.score = true;
        spaces.nodes =
            Registry::unbounded(NodeIdPartition::from_max_nodes(shape.max_nodes).client_base);
        spaces
    }

    fn registry(&mut self, space: Space) -> Option<&mut Registry> {
        match space {
            Space::Nodes => Some(&mut self.nodes),
            Space::AudioBuses => self.audio.as_mut(),
            Space::ControlBuses => self.control.as_mut(),
            Space::Buffers => Some(&mut self.buffers),
        }
    }

    fn registry_ref(&self, space: Space) -> Option<&Registry> {
        match space {
            Space::Nodes => Some(&self.nodes),
            Space::AudioBuses => self.audio.as_ref(),
            Space::ControlBuses => self.control.as_ref(),
            Space::Buffers => Some(&self.buffers),
        }
    }

    /// A run of `width` contiguous ids of `space`, or [`IdError::Exhausted`].
    /// Never wraps into ids that may still be alive. `width` 0 counts as 1.
    pub fn alloc(&mut self, space: Space, width: usize) -> Result<i64, IdError> {
        self.registry(space)
            .and_then(|r| r.alloc(width))
            .ok_or(IdError::Exhausted(space))
    }

    /// Returns a run of `space` to the pool, or [`IdError::NotAllocated`] for
    /// ids it never handed out — losing track of an id is a client bug and is
    /// reported, never absorbed.
    pub fn release(&mut self, space: Space, first: i64, width: usize) -> Result<(), IdError> {
        match self.registry(space) {
            Some(r) => r.release(first, width).map_err(|e| match e {
                ReleaseError::OutOfRange | ReleaseError::NotAllocated => {
                    IdError::NotAllocated(space)
                }
            }),
            None => Err(IdError::NotAllocated(space)),
        }
    }

    /// **A node the server reports gone** (`/node_end`, or the id of a
    /// `/synth_new` it refused), taken back if it was this client's.
    ///
    /// Every node death on a server is reported to every client that asked, not
    /// only those of nodes this client made, so an id outside this space — the
    /// server's own, another client's — or one already taken back is not an
    /// error here: it is ignored, and the answer says whether it was ours.
    pub fn node_ended(&mut self, node: i64) -> bool {
        self.nodes.is_allocated(node) && self.nodes.release(node, 1).is_ok()
    }

    /// **Takes a narrower share of every space, keeping what is allocated.**
    ///
    /// What a client does when a second client arrives on its server after it
    /// has already allocated — a script that opens a GUI host once its takes
    /// are loaded. Every id it holds keeps its number, because the server
    /// already knows it by that number; what changes is where the next one may
    /// come from.
    ///
    /// Refused whole, leaving the spaces as they were, when something allocated
    /// lies outside the new slice: an id the other client may now be handed is
    /// a collision, and one that is reported is one that can be avoided. A
    /// score's node space stays unbounded, since a score has one author.
    pub fn narrow(&mut self, share: IdShare) -> Result<(), IdError> {
        let mut next = IdSpaces::new(self.shape, share);
        next.score = self.score;
        let bounded: &[Space] = if self.score {
            &[Space::AudioBuses, Space::ControlBuses, Space::Buffers]
        } else {
            &[
                Space::Nodes,
                Space::AudioBuses,
                Space::ControlBuses,
                Space::Buffers,
            ]
        };
        for &space in bounded {
            let Some(old) = self.registry_ref(space) else {
                continue;
            };
            let Some(capacity) = old.capacity() else {
                continue;
            };
            let held: Vec<i64> = (old.base()..old.base() + capacity as i64)
                .filter(|&id| old.is_allocated(id))
                .collect();
            for id in held {
                let claimed = next.registry(space).is_some_and(|new| new.claim(id, 1));
                if !claimed {
                    return Err(IdError::NotAllocated(space));
                }
            }
        }
        // A score's node space has one author and no share: it moves over whole,
        // its next id still past everything it handed out.
        if self.score {
            next.nodes = std::mem::replace(&mut self.nodes, Registry::unbounded(0));
        }
        *self = next;
        Ok(())
    }

    /// Whether `id` falls inside this client's slice of `space`.
    pub fn contains(&self, space: Space, id: i64) -> bool {
        self.registry_ref(space).is_some_and(|r| r.contains(id))
    }

    /// How many ids of `space` are allocated now — what makes a leak visible.
    pub fn in_use(&self, space: Space) -> usize {
        self.registry_ref(space).map_or(0, Registry::in_use)
    }
}

/// A bus space: the reserved buses at the bottom, the GraphDef window at the
/// top, and this client's share of what is left — `None` when the
/// reservations swallow it whole, which reports exhaustion from the first call.
fn bus_space(size: usize, reserved: usize, graph: usize, share: IdShare) -> Option<Registry> {
    let top = size - graph.min(size);
    let span = top.saturating_sub(reserved);
    let (base, width) = share_of(reserved as i64, span, share);
    (width > 0).then(|| Registry::new(base, width))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape() -> ServerShape {
        ServerShape {
            max_nodes: 1024,
            audio_buses: 1024,
            outputs: 2,
            control_buses: 16384,
            buffers: 1024,
        }
    }

    /// The last share takes the remainder, so the slices tile the range.
    #[test]
    fn shares_tile_the_range_and_the_last_takes_the_remainder() {
        assert_eq!(share_of(1000, 10, IdShare::WHOLE), (1000, 10));
        assert_eq!(share_of(1000, 10, IdShare::new(0, 3).unwrap()), (1000, 3));
        assert_eq!(share_of(1000, 10, IdShare::new(1, 3).unwrap()), (1003, 3));
        assert_eq!(share_of(1000, 10, IdShare::new(2, 3).unwrap()), (1006, 4));
        assert_eq!(IdShare::new(3, 3), Err(IdError::BadShare));
        assert_eq!(IdShare::new(0, 0), Err(IdError::BadShare));
    }

    /// **The outputs are the server's, not a default.** A server with six
    /// outputs keeps all six out of the client's reach.
    #[test]
    fn the_audio_space_starts_above_the_servers_own_outputs() {
        let mut spaces = IdSpaces::new(
            ServerShape {
                outputs: 6,
                ..shape()
            },
            IdShare::WHOLE,
        );
        assert_eq!(spaces.alloc(Space::AudioBuses, 1), Ok(6));
        assert!(!spaces.contains(Space::AudioBuses, 5));
    }

    /// The GraphDef windows sit at the top of both bus spaces and a client
    /// never allocates into them.
    #[test]
    fn a_bus_space_stays_clear_of_the_graph_window() {
        let spaces = IdSpaces::new(shape(), IdShare::WHOLE);
        let top = 16384 - graph_control_reserved(16384);
        assert!(spaces.contains(Space::ControlBuses, top as i64 - 1));
        assert!(!spaces.contains(Space::ControlBuses, top as i64));
    }

    /// Node ids come from the partition's client range, and two shares of one
    /// server never hand out the same id.
    #[test]
    fn two_clients_of_one_server_never_share_an_id() {
        let mut a = IdSpaces::new(shape(), IdShare::new(0, 2).unwrap());
        let mut b = IdSpaces::new(shape(), IdShare::new(1, 2).unwrap());
        let (na, nb) = (
            a.alloc(Space::Nodes, 1).unwrap(),
            b.alloc(Space::Nodes, 1).unwrap(),
        );
        assert_eq!(na, 1000, "the client range starts at the partition's base");
        assert_ne!(na, nb);
        assert!(!a.contains(Space::Nodes, nb) && !b.contains(Space::Nodes, na));
    }

    /// Exhaustion is loud and a double free is refused; a node that ends and
    /// was never ours is not an error.
    #[test]
    fn exhaustion_and_double_free_are_refused_and_a_foreign_node_end_is_not() {
        let mut spaces = IdSpaces::new(
            ServerShape {
                buffers: 2,
                ..shape()
            },
            IdShare::WHOLE,
        );
        let first = spaces.alloc(Space::Buffers, 1).unwrap();
        spaces.alloc(Space::Buffers, 1).unwrap();
        assert_eq!(
            spaces.alloc(Space::Buffers, 1),
            Err(IdError::Exhausted(Space::Buffers))
        );
        spaces.release(Space::Buffers, first, 1).unwrap();
        assert_eq!(
            spaces.release(Space::Buffers, first, 1),
            Err(IdError::NotAllocated(Space::Buffers))
        );

        let node = spaces.alloc(Space::Nodes, 1).unwrap();
        assert!(spaces.node_ended(node), "ours, taken back");
        assert!(!spaces.node_ended(node), "already back: ignored");
        assert!(!spaces.node_ended(5), "the server's own: ignored");
    }

    /// **A share narrowed after allocating keeps what is held.** A script that
    /// opens a GUI host with its takes already loaded keeps their numbers, and
    /// the next id comes from its half.
    #[test]
    fn narrowing_keeps_every_id_already_held() {
        let mut spaces = IdSpaces::new(shape(), IdShare::WHOLE);
        let node = spaces.alloc(Space::Nodes, 1).unwrap();
        let bus = spaces.alloc(Space::ControlBuses, 4).unwrap();
        let buf = spaces.alloc(Space::Buffers, 1).unwrap();
        spaces.narrow(IdShare::new(0, 2).unwrap()).unwrap();
        assert_eq!(spaces.in_use(Space::Nodes), 1);
        assert_eq!(spaces.in_use(Space::ControlBuses), 4);
        assert!(spaces.release(Space::Buffers, buf, 1).is_ok(), "still ours");
        assert!(spaces.node_ended(node));
        assert!(spaces.release(Space::ControlBuses, bus, 4).is_ok());
        // ...and the second client's half is out of reach.
        let other = IdSpaces::new(shape(), IdShare::new(1, 2).unwrap());
        assert!(!spaces.contains(
            Space::Nodes,
            other.registry_ref(Space::Nodes).unwrap().base()
        ));
    }

    /// Narrowing is refused, and nothing changes, when something held would
    /// land in the other client's half.
    #[test]
    fn narrowing_refuses_when_a_held_id_is_in_the_other_half() {
        let mut spaces = IdSpaces::new(
            ServerShape {
                buffers: 4,
                ..shape()
            },
            IdShare::WHOLE,
        );
        for _ in 0..3 {
            spaces.alloc(Space::Buffers, 1).unwrap(); // 0, 1, 2
        }
        assert_eq!(
            spaces.narrow(IdShare::new(0, 2).unwrap()),
            Err(IdError::NotAllocated(Space::Buffers)),
            "buffer 2 is in the upper half"
        );
        assert_eq!(spaces.in_use(Space::Buffers), 3, "untouched");
    }

    /// A score's node space never runs out; the rest are the live spaces.
    #[test]
    fn a_score_allocates_node_ids_without_end() {
        let mut spaces = IdSpaces::score(ServerShape {
            max_nodes: 1,
            ..shape()
        });
        for _ in 0..100 {
            spaces.alloc(Space::Nodes, 1).unwrap();
        }
        assert_eq!(spaces.in_use(Space::Nodes), 100);
    }
}
