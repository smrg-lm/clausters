//! The patcher's cord → bus pass: a directed patch compiled to a GraphDef wiring.
//!
//! The GUI patcher (the P track) is a **directed, typed graph**: boxes with
//! typed inlets/outlets, a cord running `outlet → inlet`. A cord *is* a bus, but
//! the user never numbers one — that bookkeeping is this module's job, kept here
//! (and not per client) because every client that draws a patch needs the
//! identical translation. It is the front half a hand-wired GraphDef never
//! needed: when the user named the buses, there was nothing to allocate; when the
//! user draws cords, someone has to invent the bus names the connections imply.
//!
//! # What it computes
//!
//! In a GraphDef a member's control feeding an `Out` writes **one** bus and a
//! control feeding an `In` reads **one** bus, so a cord `A.out → B.in` means A's
//! outlet and B's inlet share a bus. Transitively, the buses are the **connected
//! components** of the cord graph over the set of all ports: every outlet in a
//! component writes the component's bus, every inlet reads it (getting the sum of
//! the writers). That single rule is the whole model:
//!
//! - **fan-in** (many outlets → one inlet) → one bus the writers **sum** onto;
//! - **fan-out** (one outlet → many inlets) → the readers share the outlet's bus.
//!
//! Every net is a private bus (`b0`, `b1`, …). There is **no hardware node**:
//! the buses are never drawn, so the hardware output is not one either — a signal
//! reaches the speakers through a **terminal def** (a `dac`: an inlet, and an
//! `Out.ar(0, …)` baked in, so no outlet), a member like any other.
//!
//! # What it does *not* do
//!
//! It names buses; it does **not** number or order them. The server resolves a
//! GraphDef's bus *names* to allocated indices and **auto-sorts** the members so
//! writers run before readers (see `src/osc/graphdef.rs`), so this pass adds no
//! ordering and the audio path is untouched. The graphic is a DAG: a genuine
//! feedback cycle is a code construction (nodes and groups in a control cycle),
//! never a cord, so the pass does not resolve cycles — it faithfully wires
//! whatever cords it is given.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// A port's signal rate — the cord type. Cords of different rates never connect:
/// the rate is checked at the gesture and again here.
///
/// `Audio` (`ar`) and `Control` (`kr`) are the two the **level-1** patcher wires,
/// because a server bus is one of those two rates; `Init` (`ir`) is the third the
/// **level-2** Def-view adds, where a cord is an internal UGen wire (never an
/// allocated bus) and a scalar/init-rate output is a legitimate connection. The
/// level-1 cord→bus pass ([`compile`]) is only ever handed audio/control ports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Rate {
    Audio,
    Control,
    Init,
}

/// A port's direction. An outlet writes its bus; an inlet reads it. A cord runs
/// `Out → In`; any other pairing is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    In,
    Out,
}

/// One port of a box: the def **control** it stands for, plus its direction and
/// rate (both derived from the def — a control feeding an `In` is an inlet, one
/// feeding an `Out` an outlet).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Port {
    /// The def control name — becomes the wired control in the output.
    pub name: String,
    pub dir: Dir,
    pub rate: Rate,
}

impl Port {
    /// An audio inlet named `name`.
    pub fn audio_in(name: impl Into<String>) -> Port {
        Port {
            name: name.into(),
            dir: Dir::In,
            rate: Rate::Audio,
        }
    }
    /// An audio outlet named `name`.
    pub fn audio_out(name: impl Into<String>) -> Port {
        Port {
            name: name.into(),
            dir: Dir::Out,
            rate: Rate::Audio,
        }
    }
}

/// One box on the canvas: a def with its typed ports. Every box is a member; a
/// **terminal** def (a `dac`, reaching hardware via a baked `Out.ar(0, …)`) is
/// one with inlets and no outlets — there is no special hardware box.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PatchBox {
    /// The member def name (a SynthDef/FaustDef the server has).
    pub def: String,
    pub ports: Vec<Port>,
}

impl PatchBox {
    /// A box: a def with its ports.
    pub fn member(def: impl Into<String>, ports: Vec<Port>) -> PatchBox {
        PatchBox {
            def: def.into(),
            ports,
        }
    }
}

/// One directed cord: `(from_box, from_port)` outlet → `(to_box, to_port)`
/// inlet. Boxes and ports are indices into [`Patch`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cord {
    pub from_box: usize,
    pub from_port: usize,
    pub to_box: usize,
    pub to_port: usize,
}

impl Cord {
    /// A cord from box `fb` port `fp` to box `tb` port `tp`.
    pub fn new(fb: usize, fp: usize, tb: usize, tp: usize) -> Cord {
        Cord {
            from_box: fb,
            from_port: fp,
            to_box: tb,
            to_port: tp,
        }
    }
}

/// A directed patch: the boxes on the canvas and the cords between their ports.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Patch {
    pub boxes: Vec<PatchBox>,
    pub cords: Vec<Cord>,
}

/// A private internal bus of the compiled graph — one per connected net of cords.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bus {
    pub name: String,
    pub rate: Rate,
}

/// One control of a member wired to a bus (`control → bus`). Only connected
/// controls appear; an unwired control is omitted and keeps the def's default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Wiring {
    pub control: String,
    pub bus: String,
}

/// A compiled member: the box it came from, its def, and its wired controls.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Member {
    /// Index into [`Patch::boxes`] — so a driver maps the member back to the box
    /// the user drew.
    pub box_index: usize,
    pub def: String,
    /// Wired controls, sorted by control name (deterministic output).
    pub controls: Vec<Wiring>,
}

/// The result of [`compile`]: the buses to declare and the members to wire — the
/// ingredients of a GraphDef spec, minus the parameter surface (which is the
/// driver's, not the cord graph's).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Compiled {
    /// Private internal buses — one per connected net.
    pub buses: Vec<Bus>,
    /// One per box, in box order.
    pub members: Vec<Member>,
}

/// Compiles a directed [`Patch`] into its [`Compiled`] bus wiring: one bus per
/// connected net of cords, its writers summing, named `b0`, `b1`, …
/// Deterministic — the output does not depend on cord order.
///
/// Returns an error, naming the offending cord, when a cord references a missing
/// box/port, runs the wrong way (not `outlet → inlet`), or joins mismatched
/// rates.
pub fn compile(patch: &Patch) -> Result<Compiled, String> {
    // Global port index: box b's ports occupy `offsets[b] .. offsets[b+1]`.
    let mut offsets = Vec::with_capacity(patch.boxes.len() + 1);
    let mut total = 0usize;
    for b in &patch.boxes {
        offsets.push(total);
        total += b.ports.len();
    }
    offsets.push(total);

    // Reverse map: global port → (box, local port), for the rate lookup.
    let mut port_box = vec![0usize; total];
    let mut port_local = vec![0usize; total];
    for (bi, b) in patch.boxes.iter().enumerate() {
        for pi in 0..b.ports.len() {
            let g = offsets[bi] + pi;
            port_box[g] = bi;
            port_local[g] = pi;
        }
    }

    // Union the ports each cord joins — the connected components are the buses.
    let mut uf = UnionFind::new(total);
    for (ci, c) in patch.cords.iter().enumerate() {
        let from = port_at(patch, c.from_box, c.from_port).ok_or_else(|| {
            format!(
                "cord {ci}: source port ({}, {}) out of range",
                c.from_box, c.from_port
            )
        })?;
        let to = port_at(patch, c.to_box, c.to_port).ok_or_else(|| {
            format!(
                "cord {ci}: destination port ({}, {}) out of range",
                c.to_box, c.to_port
            )
        })?;
        if from.dir != Dir::Out {
            return Err(format!(
                "cord {ci}: source '{}' is an inlet, not an outlet",
                from.name
            ));
        }
        if to.dir != Dir::In {
            return Err(format!(
                "cord {ci}: destination '{}' is an outlet, not an inlet",
                to.name
            ));
        }
        if from.rate != to.rate {
            return Err(format!(
                "cord {ci}: rate mismatch, outlet '{}' is {:?} but inlet '{}' is {:?}",
                from.name, from.rate, to.name, to.rate
            ));
        }
        uf.union(
            offsets[c.from_box] + c.from_port,
            offsets[c.to_box] + c.to_port,
        );
    }

    // Group ports by component; a component with a cord (size >= 2) is a bus.
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for g in 0..total {
        groups.entry(uf.find(g)).or_default().push(g);
    }
    // Name buses deterministically: by the smallest port index in each net, so
    // the names do not depend on the order cords were drawn.
    let mut nets: Vec<(usize, usize)> = groups
        .iter()
        .filter(|(_, ports)| ports.len() >= 2)
        .map(|(&root, ports)| (*ports.iter().min().unwrap(), root))
        .collect();
    nets.sort_unstable();

    let mut bus_name: HashMap<usize, String> = HashMap::new();
    let mut buses: Vec<Bus> = Vec::new();
    for (n, (_first, root)) in nets.into_iter().enumerate() {
        let ports = &groups[&root];
        // Rate is consistent across a net: every cord that merged it checked it.
        let rate = patch.boxes[port_box[ports[0]]].ports[port_local[ports[0]]].rate;
        let name = format!("b{n}");
        buses.push(Bus {
            name: name.clone(),
            rate,
        });
        bus_name.insert(root, name);
    }

    // Emit one member per box, wiring each connected control.
    let mut members = Vec::new();
    for (bi, b) in patch.boxes.iter().enumerate() {
        let mut controls: Vec<Wiring> = Vec::new();
        for (pi, port) in b.ports.iter().enumerate() {
            if let Some(name) = bus_name.get(&uf.find(offsets[bi] + pi)) {
                controls.push(Wiring {
                    control: port.name.clone(),
                    bus: name.clone(),
                });
            }
        }
        controls.sort_by(|a, b| a.control.cmp(&b.control));
        members.push(Member {
            box_index: bi,
            def: b.def.clone(),
            controls,
        });
    }

    Ok(Compiled { buses, members })
}

fn port_at(patch: &Patch, box_i: usize, port_i: usize) -> Option<&Port> {
    patch.boxes.get(box_i)?.ports.get(port_i)
}

/// A disjoint-set forest with union-by-size and path halving — the connected
/// components of the cord graph are the buses.
struct UnionFind {
    parent: Vec<usize>,
    size: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        UnionFind {
            parent: (0..n).collect(),
            size: vec![1; n],
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        let (big, small) = if self.size[ra] >= self.size[rb] {
            (ra, rb)
        } else {
            (rb, ra)
        };
        self.parent[small] = big;
        self.size[big] += self.size[small];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `tone` (a source: outlet only), `trem` (in → out), `dac` (a terminal sink:
    /// an inlet, no outlet — it reaches hardware via a baked `Out.ar(0, …)`).
    fn tone() -> PatchBox {
        PatchBox::member("tone", vec![Port::audio_out("out")])
    }
    fn trem() -> PatchBox {
        PatchBox::member("trem", vec![Port::audio_in("in"), Port::audio_out("out")])
    }
    fn dac() -> PatchBox {
        PatchBox::member("dac", vec![Port::audio_in("in")])
    }

    /// The wired controls of the member on box `bi`, as `(control, bus)` pairs.
    fn wiring(c: &Compiled, bi: usize) -> Vec<(String, String)> {
        let m = c.members.iter().find(|m| m.box_index == bi).unwrap();
        m.controls
            .iter()
            .map(|w| (w.control.clone(), w.bus.clone()))
            .collect()
    }

    #[test]
    fn a_chain_wires_through_with_a_bus_per_link() {
        // tone[0].out -> trem[1].in ; trem[1].out -> dac[2].in (dac is terminal).
        let patch = Patch {
            boxes: vec![tone(), trem(), dac()],
            cords: vec![
                Cord::new(0, 0, 1, 0), // tone.out -> trem.in
                Cord::new(1, 1, 2, 0), // trem.out -> dac.in
            ],
        };
        let c = compile(&patch).unwrap();
        // Two private buses, one per link. There is no hardware bus — dac reaches
        // the speakers on its own.
        assert_eq!(
            c.buses,
            vec![
                Bus {
                    name: "b0".into(),
                    rate: Rate::Audio
                },
                Bus {
                    name: "b1".into(),
                    rate: Rate::Audio
                },
            ]
        );
        assert_eq!(wiring(&c, 0), vec![("out".into(), "b0".into())]);
        assert_eq!(
            wiring(&c, 1),
            vec![("in".into(), "b0".into()), ("out".into(), "b1".into())]
        );
        assert_eq!(wiring(&c, 2), vec![("in".into(), "b1".into())]);
        assert_eq!(c.members.len(), 3);
    }

    #[test]
    fn fan_in_sums_onto_one_bus() {
        // two tones into one dac inlet: both write the same bus, dac reads it.
        let patch = Patch {
            boxes: vec![tone(), tone(), dac()],
            cords: vec![
                Cord::new(0, 0, 2, 0), // tone0.out -> dac.in
                Cord::new(1, 0, 2, 0), // tone1.out -> dac.in
            ],
        };
        let c = compile(&patch).unwrap();
        assert_eq!(wiring(&c, 0), vec![("out".into(), "b0".into())]);
        assert_eq!(wiring(&c, 1), vec![("out".into(), "b0".into())]); // same bus -> sum
        assert_eq!(wiring(&c, 2), vec![("in".into(), "b0".into())]);
        assert_eq!(c.buses.len(), 1);
    }

    #[test]
    fn fan_out_shares_the_outlet_bus() {
        // one tone into two readers: both read the tone's single bus.
        let patch = Patch {
            boxes: vec![tone(), trem(), dac()],
            cords: vec![
                Cord::new(0, 0, 1, 0), // tone.out -> trem.in
                Cord::new(0, 0, 2, 0), // tone.out -> dac.in
            ],
        };
        let c = compile(&patch).unwrap();
        assert_eq!(wiring(&c, 0), vec![("out".into(), "b0".into())]);
        assert_eq!(wiring(&c, 1), vec![("in".into(), "b0".into())]);
        assert_eq!(wiring(&c, 2), vec![("in".into(), "b0".into())]);
        assert_eq!(c.buses.len(), 1);
    }

    #[test]
    fn a_shared_outlet_makes_transitive_readers_share_one_sum() {
        // A.out->B.in, A.out->D.in, C.out->B.in: A and C share B's bus (a sum),
        // and because A writes one bus, D reads A+C too — the honest consequence
        // of a member's outlet being a single bus.
        let patch = Patch {
            boxes: vec![tone(), tone(), dac(), dac()], // A=0, C=1, B=2, D=3
            cords: vec![
                Cord::new(0, 0, 2, 0), // A.out -> B.in
                Cord::new(0, 0, 3, 0), // A.out -> D.in
                Cord::new(1, 0, 2, 0), // C.out -> B.in
            ],
        };
        let c = compile(&patch).unwrap();
        let bus = &wiring(&c, 0)[0].1;
        assert_eq!(wiring(&c, 1)[0].1, *bus); // C on the same bus
        assert_eq!(wiring(&c, 2)[0].1, *bus); // B reads it (A + C)
        assert_eq!(wiring(&c, 3)[0].1, *bus); // D reads it too (A + C)
        assert_eq!(c.buses.len(), 1);
    }

    #[test]
    fn a_control_cord_makes_a_control_bus() {
        let src = PatchBox::member(
            "lfo",
            vec![Port {
                name: "out".into(),
                dir: Dir::Out,
                rate: Rate::Control,
            }],
        );
        let dst = PatchBox::member(
            "amp",
            vec![Port {
                name: "gain".into(),
                dir: Dir::In,
                rate: Rate::Control,
            }],
        );
        let patch = Patch {
            boxes: vec![src, dst],
            cords: vec![Cord::new(0, 0, 1, 0)],
        };
        let c = compile(&patch).unwrap();
        assert_eq!(
            c.buses,
            vec![Bus {
                name: "b0".into(),
                rate: Rate::Control
            }]
        );
    }

    #[test]
    fn an_unconnected_port_keeps_its_default_and_a_bare_box_is_still_a_member() {
        // trem -> dac; a lone tone wired to nothing, and trem's `in` left open.
        let patch = Patch {
            boxes: vec![tone(), trem(), dac()],
            cords: vec![Cord::new(1, 1, 2, 0)], // trem.out -> dac.in only
        };
        let c = compile(&patch).unwrap();
        assert_eq!(wiring(&c, 0), Vec::<(String, String)>::new()); // tone: all default
        assert_eq!(wiring(&c, 1), vec![("out".into(), "b0".into())]); // trem.in omitted
        assert_eq!(wiring(&c, 2), vec![("in".into(), "b0".into())]);
        assert_eq!(c.buses.len(), 1);
        assert_eq!(c.members.len(), 3);
    }

    #[test]
    fn bus_names_are_deterministic_regardless_of_cord_order() {
        let boxes = vec![tone(), trem(), dac()];
        let forward = Patch {
            boxes: boxes.clone(),
            cords: vec![Cord::new(0, 0, 1, 0), Cord::new(1, 1, 2, 0)],
        };
        let shuffled = Patch {
            boxes,
            cords: vec![Cord::new(1, 1, 2, 0), Cord::new(0, 0, 1, 0)],
        };
        assert_eq!(compile(&forward).unwrap(), compile(&shuffled).unwrap());
    }

    #[test]
    fn a_reversed_cord_is_refused() {
        // inlet -> outlet: the destination is an outlet.
        let patch = Patch {
            boxes: vec![tone(), dac()],
            cords: vec![Cord::new(1, 0, 0, 0)], // dac.in (inlet) -> tone.out (outlet)
        };
        let err = compile(&patch).unwrap_err();
        assert!(err.contains("source 'in' is an inlet"), "{err}");
    }

    #[test]
    fn a_rate_mismatch_is_refused() {
        let a = PatchBox::member("a", vec![Port::audio_out("out")]);
        let b = PatchBox::member(
            "b",
            vec![Port {
                name: "in".into(),
                dir: Dir::In,
                rate: Rate::Control,
            }],
        );
        let patch = Patch {
            boxes: vec![a, b],
            cords: vec![Cord::new(0, 0, 1, 0)],
        };
        assert!(compile(&patch).unwrap_err().contains("rate mismatch"));
    }

    #[test]
    fn an_out_of_range_cord_is_refused() {
        let patch = Patch {
            boxes: vec![tone()],
            cords: vec![Cord::new(0, 0, 9, 0)],
        };
        assert!(compile(&patch).unwrap_err().contains("out of range"));
    }

    #[test]
    fn the_empty_patch_compiles_to_nothing() {
        assert_eq!(compile(&Patch::default()).unwrap(), Compiled::default());
    }
}
