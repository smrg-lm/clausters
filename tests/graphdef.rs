//! M18: GraphDef — a persistent node-graph "program" instantiated as a wired
//! group with private buses and a named parameter surface. Translator-level:
//! a GraphDef expands into existing primitives (group + member synths +
//! `/n_map`), so we assert on the mirrored node tree and the resolved surface.

use clausters::osc::graphdef::GRAPH_AUDIO_BUS_BASE;
use clausters::osc::translate::CmdTranslator;
use clausters::rosc::{OscMessage, OscType};
use clausters::server::engine::Cmd;

const SR: f32 = 48_000.0;

fn msg(addr: &str, args: Vec<OscType>) -> OscMessage {
    OscMessage {
        addr: addr.into(),
        args,
    }
}

fn run(t: &mut CmdTranslator, addr: &str, args: Vec<OscType>) -> Vec<Cmd> {
    let mut cmds = Vec::new();
    t.translate(&msg(addr, args), &mut cmds).unwrap();
    cmds
}

/// A source UGen def: writes its `level` (DC) to the audio bus named by `out`.
const GSRC: &str = r#"{
    "name": "gsrc",
    "controls": [{"name": "out", "default": 0.0}, {"name": "level", "default": 1.0}],
    "ugens": [{"kind": "Out", "inputs": [{"control": 0}, {"control": 1}]}]
}"#;

/// A sink UGen def: copies the audio bus `in` to the bus `out`.
const GSINK: &str = r#"{
    "name": "gsink",
    "controls": [{"name": "in", "default": 0.0}, {"name": "out", "default": 0.0}],
    "ugens": [
        {"kind": "In", "inputs": [{"control": 0}]},
        {"kind": "Out", "inputs": [{"control": 1}, {"ugen": 0}]}
    ]
}"#;

/// A two-member chain wired through one private audio bus "mix"; the surface
/// port "gain" maps to the source's `level`, defaulting to 0.5.
const CHAIN: &str = r#"{
    "name": "chain",
    "buses": [{"name": "mix", "rate": "audio"}],
    "members": [
        {"def": "gsrc", "controls": {"out": "mix"}},
        {"def": "gsink", "controls": {"in": "mix", "out": "OUT"}}
    ],
    "surface": {"gain": [{"member": 0, "control": "level"}]},
    "defaults": {"gain": 0.5}
}"#;

fn load_defs(t: &mut CmdTranslator) {
    t.d_recv(&[OscType::String(GSRC.into())]).unwrap();
    t.d_recv(&[OscType::String(GSINK.into())]).unwrap();
    t.d_graph(&[OscType::String(CHAIN.into())]).unwrap();
}

fn members(t: &CmdTranslator, group: i32) -> Vec<i32> {
    t.graph_instances.get(&group).unwrap().members.clone()
}

fn control(t: &CmdTranslator, id: i32, index: usize) -> f32 {
    t.mirror.synth_info(id).unwrap().1[index]
}

#[test]
fn graph_new_wires_members_to_private_buses() {
    let mut t = CmdTranslator::new(SR);
    load_defs(&mut t);
    run(
        &mut t,
        "/graph_new",
        vec![
            OscType::String("chain".into()),
            OscType::Int(500),
            OscType::Int(0),
            OscType::Int(0),
        ],
    );

    let m = members(&t, 500);
    assert_eq!(m.len(), 2);
    let (src, sink) = (m[0], m[1]);

    // The source writes to a private audio bus (top reserved range), and the
    // sink reads the *same* bus and writes to hardware bus 0 ("OUT").
    let mix = control(&t, src, 0); // gsrc.out
    assert!(
        mix as usize >= GRAPH_AUDIO_BUS_BASE,
        "mix bus {mix} is private"
    );
    assert_eq!(control(&t, sink, 0), mix); // gsink.in == mix
    assert_eq!(control(&t, sink, 1), 0.0); // gsink.out == OUT (bus 0)

    // The auto-sorted instance group orders the writer before the reader.
    assert_eq!(t.mirror.children(500).unwrap(), &[src, sink]);
}

#[test]
fn surface_default_reaches_the_mapped_member_control() {
    let mut t = CmdTranslator::new(SR);
    load_defs(&mut t);
    run(
        &mut t,
        "/graph_new",
        vec![
            OscType::String("chain".into()),
            OscType::Int(500),
            OscType::Int(0),
            OscType::Int(0),
        ],
    );
    // gsrc.level (index 1) defaults to 1.0 in the def, but the surface
    // default `gain=0.5` was applied through the port mapping.
    let src = members(&t, 500)[0];
    assert_eq!(control(&t, src, 1), 0.5);
}

#[test]
fn n_set_on_the_instance_resolves_against_the_surface() {
    let mut t = CmdTranslator::new(SR);
    load_defs(&mut t);
    run(
        &mut t,
        "/graph_new",
        vec![
            OscType::String("chain".into()),
            OscType::Int(500),
            OscType::Int(0),
            OscType::Int(0),
        ],
    );
    let src = members(&t, 500)[0];

    let cmds = run(
        &mut t,
        "/n_set",
        vec![
            OscType::Int(500),
            OscType::String("gain".into()),
            OscType::Float(0.8),
        ],
    );
    // The port "gain" routed to gsrc.level (index 1), never the member id.
    assert_eq!(control(&t, src, 1), 0.8);
    assert!(cmds.iter().any(|c| matches!(
        c,
        Cmd::SetControl { id, index, value } if *id == src && *index == 1 && *value == 0.8
    )));
}

#[test]
fn n_set_unknown_port_is_ignored() {
    let mut t = CmdTranslator::new(SR);
    load_defs(&mut t);
    run(
        &mut t,
        "/graph_new",
        vec![
            OscType::String("chain".into()),
            OscType::Int(500),
            OscType::Int(0),
            OscType::Int(0),
        ],
    );
    let cmds = run(
        &mut t,
        "/n_set",
        vec![
            OscType::Int(500),
            OscType::String("nonesuch".into()),
            OscType::Float(1.0),
        ],
    );
    assert!(cmds.is_empty());
}

#[test]
fn instantiation_port_override_beats_the_default() {
    let mut t = CmdTranslator::new(SR);
    load_defs(&mut t);
    run(
        &mut t,
        "/graph_new",
        vec![
            OscType::String("chain".into()),
            OscType::Int(500),
            OscType::Int(0),
            OscType::Int(0),
            OscType::String("gain".into()),
            OscType::Float(0.25),
        ],
    );
    let src = members(&t, 500)[0];
    assert_eq!(control(&t, src, 1), 0.25);
}

#[test]
fn free_reclaims_private_buses() {
    let mut t = CmdTranslator::new(SR);
    load_defs(&mut t);
    run(
        &mut t,
        "/graph_new",
        vec![
            OscType::String("chain".into()),
            OscType::Int(500),
            OscType::Int(0),
            OscType::Int(0),
        ],
    );
    let first_mix = control(&t, members(&t, 500)[0], 0);

    run(&mut t, "/n_free", vec![OscType::Int(500)]);
    assert!(t.graph_instances.get(&500).is_none());

    // A fresh instance reuses the reclaimed bus.
    run(
        &mut t,
        "/graph_new",
        vec![
            OscType::String("chain".into()),
            OscType::Int(501),
            OscType::Int(0),
            OscType::Int(0),
        ],
    );
    assert_eq!(control(&t, members(&t, 501)[0], 0), first_mix);
}

#[test]
fn unknown_member_def_fails_atomically() {
    let mut t = CmdTranslator::new(SR);
    t.d_recv(&[OscType::String(GSRC.into())]).unwrap();
    // A graph referencing a missing member def.
    let bad = r#"{"name":"bad","members":[{"def":"missing"}]}"#;
    t.d_graph(&[OscType::String(bad.into())]).unwrap();

    let mut cmds = Vec::new();
    let res = t.translate(
        &msg(
            "/graph_new",
            vec![
                OscType::String("bad".into()),
                OscType::Int(600),
                OscType::Int(0),
                OscType::Int(0),
            ],
        ),
        &mut cmds,
    );
    assert!(res.is_err());
    // No partial instance, no group, no leaked commands.
    assert!(t.graph_instances.get(&600).is_none());
    assert!(t.mirror.get(600).is_none());
    assert!(cmds.is_empty());
}

#[test]
fn d_graph_rejects_bad_surface_and_bus_refs() {
    let mut t = CmdTranslator::new(SR);
    // Surface points at a member index that does not exist.
    let bad_surface = r#"{"name":"x","members":[],"surface":{"p":[{"member":3,"control":"y"}]}}"#;
    assert!(t.d_graph(&[OscType::String(bad_surface.into())]).is_err());
    // A member references an undeclared internal bus.
    let bad_bus = r#"{"name":"y","members":[{"def":"d","controls":{"out":"ghost"}}]}"#;
    assert!(t.d_graph(&[OscType::String(bad_bus.into())]).is_err());
}
