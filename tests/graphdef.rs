//! M18: GraphDef — a persistent node-graph "program" instantiated as a wired
//! group with private buses and a named parameter surface. Translator-level:
//! a GraphDef expands into existing primitives (group + member synths +
//! `/node_map`), so we assert on the mirrored node tree and the resolved surface.

#![cfg(feature = "synth")]

use clausters::dsp::NUM_AUDIO_BUSES;
use clausters::osc::graphdef::GRAPH_AUDIO_BUS_RESERVED;
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
    let nodes = &t.graph_instances.get(&group).unwrap().shared_nodes;
    (0..nodes.len()).map(|i| nodes[&i]).collect()
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
        mix as usize >= NUM_AUDIO_BUSES - GRAPH_AUDIO_BUS_RESERVED,
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
        "/node_set",
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
        "/node_set",
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
    run(&mut t, "/node_free", vec![OscType::Int(500)]);
    assert!(!t.graph_instances.contains_key(&500));

    // Every reclaimed bus is allocatable again: cycling instances far past
    // the reserved range's width (32 audio buses) never exhausts the pool.
    for i in 0..100 {
        let id = 501 + i;
        run(
            &mut t,
            "/graph_new",
            vec![
                OscType::String("chain".into()),
                OscType::Int(id),
                OscType::Int(0),
                OscType::Int(0),
            ],
        );
        assert!(t.graph_instances.contains_key(&id), "instance {id} built");
        run(&mut t, "/node_free", vec![OscType::Int(id)]);
        assert!(!t.graph_instances.contains_key(&id));
    }
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
    assert!(!t.graph_instances.contains_key(&600));
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

// ---- per-voice partition (/graph_newVoice) ----

/// A per-voice oscillator: writes `Sine(freq) * level` to the bus `out`.
const VTONE: &str = r#"{
    "name": "vtone",
    "controls": [{"name": "out", "default": 0.0}, {"name": "freq", "default": 440.0}, {"name": "level", "default": 0.2}],
    "ugens": [
        {"kind": "Sine", "inputs": [{"control": 1}]},
        {"kind": "Mul", "inputs": [{"ugen": 0}, {"control": 2}]},
        {"kind": "Out", "inputs": [{"control": 0}, {"ugen": 1}]}
    ]
}"#;

/// A shared mixer: reads the bus `in`, scales by `gain`, writes to hardware 0.
const VGAIN: &str = r#"{
    "name": "vgain",
    "controls": [{"name": "in", "default": 0.0}, {"name": "gain", "default": 0.4}],
    "ugens": [
        {"kind": "In", "inputs": [{"control": 0}]},
        {"kind": "Mul", "inputs": [{"ugen": 0}, {"control": 1}]},
        {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
    ]
}"#;

/// A polyphonic instrument: a shared mixer (member 0) + a per-voice oscillator
/// (member 1, `voice: true`). `gain` is a shared port; `freq`/`amp` are voice
/// ports (so they apply per `/graph_newVoice`).
const POLY: &str = r#"{
    "name": "poly",
    "buses": [{"name": "mix", "rate": "audio"}],
    "members": [
        {"def": "vgain", "controls": {"in": "mix"}},
        {"def": "vtone", "controls": {"out": "mix"}, "voice": true}
    ],
    "surface": {
        "gain": [{"member": 0, "control": "gain"}],
        "freq": [{"member": 1, "control": "freq"}],
        "amp":  [{"member": 1, "control": "level"}]
    },
    "defaults": {"gain": 0.4, "amp": 0.2}
}"#;

fn load_poly(t: &mut CmdTranslator) {
    t.d_recv(&[OscType::String(VTONE.into())]).unwrap();
    t.d_recv(&[OscType::String(VGAIN.into())]).unwrap();
    t.d_graph(&[OscType::String(POLY.into())]).unwrap();
}

/// The single tone node inside a voice sub-group.
fn voice_tone(t: &CmdTranslator, voice: i32) -> i32 {
    t.mirror.children(voice).unwrap()[0]
}

#[test]
fn graph_new_only_instantiates_shared_members() {
    let mut t = CmdTranslator::new(SR);
    load_poly(&mut t);
    run(
        &mut t,
        "/graph_new",
        vec![
            OscType::String("poly".into()),
            OscType::Int(700),
            OscType::Int(0),
            OscType::Int(0),
        ],
    );

    let inst = t.graph_instances.get(&700).unwrap();
    // Only the shared mixer (member 0); the per-voice osc is absent until /graph_newVoice.
    assert_eq!(inst.shared_nodes.len(), 1);
    assert!(inst.shared_nodes.contains_key(&0));
    assert!(inst.voices.is_empty());
    // The shared port resolved; the voice ports did not (no voice yet).
    assert!(inst.surface.contains_key("gain"));
    assert!(!inst.surface.contains_key("freq"));
}

#[test]
fn graph_voice_spawns_a_wired_voice_with_its_surface() {
    let mut t = CmdTranslator::new(SR);
    load_poly(&mut t);
    run(
        &mut t,
        "/graph_new",
        vec![
            OscType::String("poly".into()),
            OscType::Int(700),
            OscType::Int(0),
            OscType::Int(0),
        ],
    );
    let mix = control(&t, t.graph_instances.get(&700).unwrap().shared_nodes[&0], 0); // vgain.in

    run(
        &mut t,
        "/graph_newVoice",
        vec![
            OscType::Int(700),
            OscType::Int(710),
            OscType::String("freq".into()),
            OscType::Float(330.0),
        ],
    );

    assert!(t.graph_voices.contains_key(&710));
    assert_eq!(t.graph_voices.get(&710).unwrap().instance, 700);
    assert!(t.graph_instances.get(&700).unwrap().voices.contains(&710));

    let tone = voice_tone(&t, 710);
    assert_eq!(control(&t, tone, 0), mix); // vtone.out wired to the shared mix bus
    assert_eq!(control(&t, tone, 1), 330.0); // freq port override
    assert_eq!(control(&t, tone, 2), 0.2); // amp port default -> level
}

#[test]
fn n_set_on_a_voice_resolves_against_its_surface() {
    let mut t = CmdTranslator::new(SR);
    load_poly(&mut t);
    run(
        &mut t,
        "/graph_new",
        vec![
            OscType::String("poly".into()),
            OscType::Int(700),
            OscType::Int(0),
            OscType::Int(0),
        ],
    );
    run(
        &mut t,
        "/graph_newVoice",
        vec![OscType::Int(700), OscType::Int(710)],
    );
    let tone = voice_tone(&t, 710);

    run(
        &mut t,
        "/node_set",
        vec![
            OscType::Int(710),
            OscType::String("freq".into()),
            OscType::Float(550.0),
        ],
    );
    assert_eq!(control(&t, tone, 1), 550.0);
}

#[test]
fn graph_voice_needs_voice_members_and_a_live_instance() {
    let mut t = CmdTranslator::new(SR);
    load_defs(&mut t); // the voice-less "chain"
    load_poly(&mut t);
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

    let mut cmds = Vec::new();
    // "chain" has no voice members.
    assert!(
        t.translate(
            &msg("/graph_newVoice", vec![OscType::Int(500), OscType::Int(-1)]),
            &mut cmds
        )
        .is_err()
    );
    // unknown instance.
    assert!(
        t.translate(
            &msg("/graph_newVoice", vec![OscType::Int(999), OscType::Int(-1)]),
            &mut cmds
        )
        .is_err()
    );
}

#[test]
fn freeing_a_voice_and_the_instance_cleans_up() {
    let mut t = CmdTranslator::new(SR);
    load_poly(&mut t);
    run(
        &mut t,
        "/graph_new",
        vec![
            OscType::String("poly".into()),
            OscType::Int(700),
            OscType::Int(0),
            OscType::Int(0),
        ],
    );
    run(
        &mut t,
        "/graph_newVoice",
        vec![OscType::Int(700), OscType::Int(710)],
    );
    run(
        &mut t,
        "/graph_newVoice",
        vec![OscType::Int(700), OscType::Int(711)],
    );

    // Free one voice: it leaves graph_voices and the instance's set.
    run(&mut t, "/node_free", vec![OscType::Int(710)]);
    assert!(!t.graph_voices.contains_key(&710));
    assert!(!t.graph_instances.get(&700).unwrap().voices.contains(&710));

    // Free the instance: it takes the remaining voice with it.
    run(&mut t, "/node_free", vec![OscType::Int(700)]);
    assert!(!t.graph_instances.contains_key(&700));
    assert!(!t.graph_voices.contains_key(&711));
}

#[test]
fn d_graph_rejects_a_port_mixing_shared_and_voice_members() {
    let mut t = CmdTranslator::new(SR);
    let bad = r#"{"name":"mix","members":[{"def":"a"},{"def":"b","voice":true}],
                  "surface":{"p":[{"member":0,"control":"x"},{"member":1,"control":"y"}]}}"#;
    assert!(t.d_graph(&[OscType::String(bad.into())]).is_err());
}

// ---- MIDI binding a GraphDef ----

#[test]
fn midi_bind_to_a_graphdef_plays_voices() {
    use clausters::midi::ChannelVoiceMessage::{NoteOff, NoteOn};
    use clausters::midi::convert;

    let mut t = CmdTranslator::new(SR);
    load_poly(&mut t);

    // Binding spawns the shared instance.
    run(
        &mut t,
        "/midi_bind",
        vec![OscType::Int(0), OscType::String("poly".into())],
    );
    assert_eq!(t.graph_instances.len(), 1);
    let instance = *t.graph_instances.keys().next().unwrap();

    // A note spawns a voice into that instance; freq follows the note.
    let mut cmds = Vec::new();
    t.translate_midi(
        NoteOn {
            channel: 0,
            note: 69,
            velocity: 100,
        },
        &mut cmds,
    )
    .unwrap();
    assert_eq!(t.graph_voices.len(), 1);
    let voice = *t.graph_voices.keys().next().unwrap();
    assert_eq!(t.graph_voices.get(&voice).unwrap().instance, instance);
    let tone = voice_tone(&t, voice);
    assert!((control(&t, tone, 1) - convert::midi2freq(69.0)).abs() < 1e-3); // A4 = 440

    // Note off frees the voice.
    cmds.clear();
    t.translate_midi(
        NoteOff {
            channel: 0,
            note: 69,
            velocity: 0,
        },
        &mut cmds,
    )
    .unwrap();
    assert!(t.graph_voices.is_empty());

    // Unbind frees the shared instance.
    cmds.clear();
    t.translate(&msg("/midi_unbind", vec![OscType::Int(0)]), &mut cmds)
        .unwrap();
    assert!(t.graph_instances.is_empty());
}

/// M30: `/def_query` reports a GraphDef's **ports** — the named surface, which
/// is what a level-1 patch wires — each with its default and the inner targets
/// it drives, scaling included. Built at the translator, where the def tables
/// live; the server handler is a thin dispatch over this.
#[test]
fn def_info_reports_graph_ports_with_their_targets() {
    let mut t = CmdTranslator::new(SR);
    load_defs(&mut t);

    let infos = t.def_info(Some(&["chain".to_string()]));
    assert_eq!(infos.len(), 1);
    let a = &infos[0];
    assert_eq!(a[0], OscType::String("chain".into()));
    assert_eq!(a[1], OscType::String("graph".into()));
    assert_eq!(a[2], OscType::Int(1), "one surface port");
    // port name, default, rate, numTargets, then (member, control, mul, add).
    assert_eq!(a[3], OscType::String("gain".into()));
    assert_eq!(a[4], OscType::Float(0.5), "the def's declared default");
    assert_eq!(a[5], OscType::String("kr".into()));
    assert_eq!(a[6], OscType::Int(1), "one target");
    assert_eq!(a[7], OscType::Int(0), "member 0 (gsrc)");
    assert_eq!(a[8], OscType::String("level".into()));
    assert_eq!(a[9], OscType::Float(1.0), "identity mul");
    assert_eq!(a[10], OscType::Float(0.0), "identity add");
}

/// The listing form spans all three families at once: the member UGen defs and
/// the GraphDef built over them come back from one query, each tagged.
#[test]
fn def_info_lists_every_family_together() {
    let mut t = CmdTranslator::new(SR);
    load_defs(&mut t);

    let by_name: std::collections::HashMap<String, String> = t
        .def_info(None)
        .iter()
        .map(|a| match (&a[0], &a[1]) {
            (OscType::String(n), OscType::String(f)) => (n.clone(), f.clone()),
            other => panic!("expected (name, family), got {other:?}"),
        })
        .collect();

    assert_eq!(by_name.get("gsrc").map(String::as_str), Some("synth"));
    assert_eq!(by_name.get("gsink").map(String::as_str), Some("synth"));
    assert_eq!(by_name.get("chain").map(String::as_str), Some("graph"));
}
