//! Persistent notes, source ownership, decoder routing and transport cuts.
#![cfg(all(target_os = "linux", feature = "plugin-clap"))]
use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use clausters::dsp::{Limits, ReplyKind};
use clausters::osc::translate::CmdTranslator;
use clausters::plugin::{PluginDef, PluginReference};
use clausters::rosc::{OscMessage, OscType};
use clausters::server::engine::{Cmd, Engine, EngineHandle, engine_pair_full};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
#[global_allocator]
static ALLOC: AllocDisabler = AllocDisabler;
static FIXTURE: OnceLock<PathBuf> = OnceLock::new();
fn fixture() -> &'static PathBuf {
    FIXTURE.get_or_init(|| {
        let dir =
            std::env::temp_dir().join(format!("clausters-plugin-events-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("instrument.clap");
        assert!(
            std::process::Command::new("cc")
                .args([
                    "-std=c11",
                    "-O2",
                    "-fPIC",
                    "-shared",
                    "-Ithird_party/clap/include",
                    "crates/clausters-plugin-host/tests/fixtures/polyphonic.c",
                    "-o"
                ])
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        path
    })
}
fn build_translator() -> CmdTranslator {
    let reference: PluginReference = serde_json::from_value(serde_json::json!({"format":"clap","path":fixture(),"id":"org.clausters.fixture.instrument","inputs":[],"outputs":[{"id":0,"channels":1,"bus":0}]})).unwrap();
    let mut translator = CmdTranslator::new(48000.0);
    translator.plugin_defs.insert(
        "keys".into(),
        Arc::new(PluginDef::probe(reference, 48000.0).unwrap()),
    );
    translator
}
fn engine() -> (Engine, EngineHandle) {
    engine_pair_full(
        48000.0,
        2,
        0,
        None,
        16,
        16,
        Limits {
            max_nodes: 32,
            ..Limits::default()
        },
    )
}
fn event(source: u32, note: i32, kind: &str, key: i16, value: f64) -> OscMessage {
    OscMessage { addr:"/plugin_event".into(), args:vec![OscType::Int(42),OscType::String(serde_json::json!({"kind":kind,"source":source,"note_id":note,"port":0,"channel":0,"key":key,"value":value,"id":6}).to_string())] }
}
fn new_node(translator: &mut CmdTranslator, target: i32) -> Vec<Cmd> {
    let mut cmds = Vec::new();
    translator
        .translate(
            &OscMessage {
                addr: "/synth_new".into(),
                args: vec![
                    OscType::String("keys".into()),
                    OscType::Int(42),
                    OscType::Int(1),
                    OscType::Int(target),
                ],
            },
            &mut cmds,
        )
        .unwrap();
    cmds
}
fn translate(translator: &mut CmdTranslator, msg: OscMessage) -> Vec<Cmd> {
    let mut cmds = Vec::new();
    translator.translate(&msg, &mut cmds).unwrap();
    cmds
}
fn send(handle: &mut EngineHandle, cmds: Vec<Cmd>) {
    for cmd in cmds {
        assert!(handle.send(cmd).is_ok());
    }
}
fn blocks(engine: &mut Engine, handle: &mut EngineHandle, count: usize) -> Vec<f32> {
    let mut samples = Vec::new();
    for _ in 0..count {
        let mut out = [0.0; 128];
        assert_no_alloc(|| engine.process_block(&mut out));
        samples.extend(out.as_chunks::<2>().0.iter().map(|f| f[0]));
        std::thread::sleep(Duration::from_millis(3));
        while let Some(msg) = handle.pop_reply() {
            assert_ne!(msg.kind, ReplyKind::Fault, "{msg:?}");
        }
        while handle.pop_garbage().is_some() {}
    }
    samples
}
#[test]
fn overlapping_same_key_notes_keep_sources_expression_and_exact_bundle_offsets() {
    let mut translator = build_translator();
    let (mut engine, mut handle) = engine();
    send(&mut handle, new_node(&mut translator, 0));
    let mut attack = translate(&mut translator, event(3, 7, "note_on", 60, 0.3));
    attack.extend(translate(&mut translator, event(4, 7, "note_on", 60, 0.4)));
    assert!(
        handle
            .send(Cmd::Schedule {
                time: 17,
                cmds: attack
            })
            .is_ok()
    );
    assert!(
        handle
            .send(Cmd::Schedule {
                time: 31,
                cmds: translate(&mut translator, event(3, 7, "expression", 60, 1.0))
            })
            .is_ok()
    );
    assert!(
        handle
            .send(Cmd::Schedule {
                time: 44,
                cmds: translate(&mut translator, event(3, 7, "note_off", 60, 0.0))
            })
            .is_ok()
    );
    assert!(
        handle
            .send(Cmd::Schedule {
                time: 99,
                cmds: translate(&mut translator, event(4, 7, "note_off", 60, 0.0))
            })
            .is_ok()
    );
    let samples = blocks(&mut engine, &mut handle, 9);
    assert!(samples[..256 + 17].iter().all(|v| *v == 0.0));
    for value in &samples[256 + 17..256 + 31] {
        assert!((*value - 0.712).abs() < 1e-5);
    }
    for value in &samples[256 + 31..256 + 44] {
        assert!((*value - 0.812).abs() < 1e-5);
    }
    for value in &samples[256 + 44..256 + 99] {
        assert!((*value - 0.412).abs() < 1e-5);
    }
    assert!(samples[256 + 99..].iter().all(|v| *v == 0.0));
    assert_eq!(translator.node_defs.len(), 1);
}
#[test]
fn midi_binding_is_one_instance_and_key_releases_are_fifo_with_sustain() {
    let mut translator = build_translator();
    let (mut engine, mut handle) = engine();
    let mut cmds = Vec::new();
    translator
        .translate(
            &OscMessage {
                addr: "/midi_bind".into(),
                args: vec![OscType::Int(0), OscType::String("keys".into())],
            },
            &mut cmds,
        )
        .unwrap();
    let node = cmds
        .iter()
        .find_map(|cmd| {
            if let Cmd::AddSynth { id, .. } = cmd {
                Some(*id)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(
        cmds.iter()
            .filter(|cmd| matches!(cmd, Cmd::AddSynth { .. }))
            .count(),
        1
    );
    send(&mut handle, cmds);
    let mut attack = Vec::new();
    translator
        .translate_midi_bytes(&[0x90, 60, 127], &mut attack)
        .unwrap();
    translator
        .translate_midi_bytes(&[0x90, 60, 64], &mut attack)
        .unwrap();
    // The binding's node reads the bus the channel writes: a key is an event
    // there, never a node of its own.
    let bus = translator.midi.channels[&0].bus.unwrap();
    assert_eq!(translator.midi.channels[&0].node, Some(node));
    assert!(
        attack
            .iter()
            .all(|cmd| matches!(cmd, Cmd::BusEvent { bus: b, .. } if *b == bus))
    );
    assert_eq!(attack.len(), 2);
    send(&mut handle, attack);
    blocks(&mut engine, &mut handle, 6);
    let mut cmds = Vec::new();
    translator
        .translate_midi_bytes(&[0xb0, 64, 127], &mut cmds)
        .unwrap();
    translator
        .translate_midi_bytes(&[0x80, 60, 0], &mut cmds)
        .unwrap();
    send(&mut handle, cmds);
    let held = blocks(&mut engine, &mut handle, 6);
    assert!(
        *held.last().unwrap() > 1.5,
        "held output: {:?}",
        &held[held.len() - 10..]
    );
    let mut cmds = Vec::new();
    translator
        .translate_midi_bytes(&[0xb0, 64, 0], &mut cmds)
        .unwrap();
    send(&mut handle, cmds);
    let remaining = blocks(&mut engine, &mut handle, 6);
    assert!((*remaining.last().unwrap() - (64.0 / 127.0 + 0.012)).abs() < 1e-4);
    let mut cmds = Vec::new();
    translator
        .translate_midi_bytes(&[0x80, 60, 0], &mut cmds)
        .unwrap();
    send(&mut handle, cmds);
    let released = blocks(&mut engine, &mut handle, 6);
    assert_eq!(*released.last().unwrap(), 0.0);
    assert_eq!(translator.node_defs.len(), 1);
}
#[test]
fn governing_freezes_release_notes_and_resume_does_not_restore_held_keys() {
    let mut translator = build_translator();
    let (mut engine, mut handle) = engine();
    send(
        &mut handle,
        translate(
            &mut translator,
            OscMessage {
                addr: "/group_new".into(),
                args: vec![OscType::Int(10), OscType::Int(0), OscType::Int(0)],
            },
        ),
    );
    send(&mut handle, new_node(&mut translator, 10));
    assert!(
        handle
            .send(Cmd::TransportGroup {
                transport: 0,
                id: 10
            })
            .is_ok()
    );
    assert!(
        handle
            .send(Cmd::PluginTempo {
                transport: 0,
                bpm: 90.0
            })
            .is_ok()
    );
    assert!(
        handle
            .send(Cmd::TransportRun {
                transport: 0,
                rolling: true
            })
            .is_ok()
    );
    send(
        &mut handle,
        translate(&mut translator, event(0, 1, "note_on", 60, 0.5)),
    );
    let active = blocks(&mut engine, &mut handle, 6);
    assert!(*active.last().unwrap() > 0.509);
    assert!(
        handle
            .send(Cmd::TransportRun {
                transport: 0,
                rolling: false
            })
            .is_ok()
    );
    let frozen = blocks(&mut engine, &mut handle, 8);
    assert!(frozen.iter().all(|v| *v == 0.0));
    assert!(
        handle
            .send(Cmd::TransportLocate {
                transport: 0,
                position: 48000
            })
            .is_ok()
    );
    assert!(
        handle
            .send(Cmd::TransportRun {
                transport: 0,
                rolling: true
            })
            .is_ok()
    );
    let resumed = blocks(&mut engine, &mut handle, 6);
    assert!(resumed.iter().all(|v| *v == 0.0));
    send(
        &mut handle,
        translate(&mut translator, event(0, 2, "note_on", 64, 0.5)),
    );
    let jumped = blocks(&mut engine, &mut handle, 6);
    assert!((*jumped.last().unwrap() - 0.519).abs() < 0.0002);
}
#[test]
fn unsupported_events_are_reported_before_publication() {
    let mut translator = build_translator();
    let _ = new_node(&mut translator, 0);
    let mut msg = event(0, 1, "expression", 60, 0.5);
    msg.args[1] =
        OscType::String(serde_json::json!({"kind":"expression","id":99,"port":0}).to_string());
    assert!(
        translator
            .translate(&msg, &mut Vec::new())
            .unwrap_err()
            .contains("expression")
    );
    msg.args[1] = OscType::String(
        serde_json::json!({"kind":"note_on","port":77,"key":60,"value":0.5}).to_string(),
    );
    assert!(
        translator
            .translate(&msg, &mut Vec::new())
            .unwrap_err()
            .contains("port")
    );
}

#[test]
fn old_midi_release_cannot_release_a_new_attack_after_a_transport_stop() {
    let mut translator = build_translator();
    let (mut engine, mut handle) = engine();
    send(
        &mut handle,
        translate(
            &mut translator,
            OscMessage {
                addr: "/group_new".into(),
                args: vec![OscType::Int(10), OscType::Int(0), OscType::Int(0)],
            },
        ),
    );
    let mut cmds = Vec::new();
    translator
        .translate(
            &OscMessage {
                addr: "/midi_bind".into(),
                args: vec![
                    OscType::Int(0),
                    OscType::String("keys".into()),
                    OscType::Int(10),
                ],
            },
            &mut cmds,
        )
        .unwrap();
    send(&mut handle, cmds);
    assert!(
        handle
            .send(Cmd::TransportGroup {
                transport: 0,
                id: 10
            })
            .is_ok()
    );
    assert!(
        handle
            .send(Cmd::TransportRun {
                transport: 0,
                rolling: true
            })
            .is_ok()
    );
    let mut cmds = Vec::new();
    translator
        .translate_midi_bytes(&[0x90, 60, 127], &mut cmds)
        .unwrap();
    send(&mut handle, cmds);
    blocks(&mut engine, &mut handle, 6);
    assert!(
        handle
            .send(Cmd::TransportRun {
                transport: 0,
                rolling: false
            })
            .is_ok()
    );
    blocks(&mut engine, &mut handle, 6);
    assert!(
        handle
            .send(Cmd::TransportRun {
                transport: 0,
                rolling: true
            })
            .is_ok()
    );
    let mut cmds = Vec::new();
    translator
        .translate_midi_bytes(&[0x90, 60, 64], &mut cmds)
        .unwrap();
    translator
        .translate_midi_bytes(&[0x80, 60, 0], &mut cmds)
        .unwrap();
    send(&mut handle, cmds);
    let new = blocks(&mut engine, &mut handle, 6);
    assert!(*new.last().unwrap() > 0.5);
    let mut cmds = Vec::new();
    translator
        .translate_midi_bytes(&[0x80, 60, 0], &mut cmds)
        .unwrap();
    send(&mut handle, cmds);
    let released = blocks(&mut engine, &mut handle, 6);
    assert_eq!(*released.last().unwrap(), 0.0);
}

#[test]
fn mpe_expression_and_master_sustain_use_the_shared_decoder() {
    let mut translator = build_translator();
    let (mut engine, mut handle) = engine();
    let mut cmds = Vec::new();
    translator
        .translate(
            &OscMessage {
                addr: "/midi_bindZone".into(),
                args: vec![
                    OscType::Int(0),
                    OscType::Int(1),
                    OscType::String("keys".into()),
                ],
            },
            &mut cmds,
        )
        .unwrap();
    send(&mut handle, cmds);
    let mut cmds = Vec::new();
    translator
        .translate_midi_bytes(&[0x91, 60, 127], &mut cmds)
        .unwrap();
    send(&mut handle, cmds);
    blocks(&mut engine, &mut handle, 6);
    let mut cmds = Vec::new();
    translator
        .translate_midi_bytes(&[0xd1, 127], &mut cmds)
        .unwrap();
    send(&mut handle, cmds);
    let pressure = blocks(&mut engine, &mut handle, 6);
    assert!((*pressure.last().unwrap() - 1.112).abs() < 1e-5);
    let mut cmds = Vec::new();
    translator
        .translate_midi_bytes(&[0xb0, 64, 127], &mut cmds)
        .unwrap();
    translator
        .translate_midi_bytes(&[0x81, 60, 0], &mut cmds)
        .unwrap();
    send(&mut handle, cmds);
    let sustain = blocks(&mut engine, &mut handle, 6);
    assert!(*sustain.last().unwrap() > 1.1);
    let mut cmds = Vec::new();
    translator
        .translate_midi_bytes(&[0xb0, 64, 0], &mut cmds)
        .unwrap();
    send(&mut handle, cmds);
    let released = blocks(&mut engine, &mut handle, 6);
    assert_eq!(*released.last().unwrap(), 0.0);
    assert_eq!(translator.node_defs.len(), 1);
}

#[test]
fn osc_lane_replacement_releases_its_origin_without_releasing_the_keyboard() {
    use clausters::osc::server::{OscServer, ServerInfo};
    use clausters::rosc::{OscPacket, encoder};
    use std::net::UdpSocket;
    let reference: PluginReference=serde_json::from_value(serde_json::json!({"format":"clap","path":fixture(),"id":"org.clausters.fixture.instrument","inputs":[],"outputs":[{"id":0,"channels":1,"bus":0}]})).unwrap();
    let (mut engine, handle) = engine();
    let mut server = OscServer::bind(
        ("127.0.0.1", 0),
        ServerInfo {
            nominal_sample_rate: 48000.0,
            actual_sample_rate: 48000.0,
        },
        handle,
    )
    .unwrap();
    let address = server.local_addr().unwrap();
    let thread = std::thread::spawn(move || server.run());
    let client = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let send = |addr: &str, args| {
        client
            .send_to(
                &encoder::encode(&OscPacket::Message(OscMessage {
                    addr: addr.into(),
                    args,
                }))
                .unwrap(),
                address,
            )
            .unwrap()
    };
    let recv = || {
        let mut bytes = [0; 65536];
        let n = client.recv(&mut bytes).unwrap();
        let OscPacket::Message(msg) = clausters::osc::decode_packet(&bytes[..n]).unwrap() else {
            panic!("message required")
        };
        assert_ne!(msg.addr, "/fail", "{msg:?}");
        msg
    };
    send(
        "/def_send",
        vec![
            OscType::String("plugin".into()),
            OscType::String("keys".into()),
            OscType::String(serde_json::to_string(&reference).unwrap()),
        ],
    );
    assert_eq!(recv().addr, "/done");
    send(
        "/group_new",
        vec![OscType::Int(10), OscType::Int(0), OscType::Int(0)],
    );
    send(
        "/synth_new",
        vec![
            OscType::String("keys".into()),
            OscType::Int(42),
            OscType::Int(1),
            OscType::Int(0),
        ],
    );
    send(
        "/transport_set",
        vec![OscType::Int(0), OscType::Long(0), OscType::Double(2.0)],
    );
    assert_eq!(recv().addr, "/done");
    send("/transport_group", vec![OscType::Int(0), OscType::Int(10)]);
    assert_eq!(recv().addr, "/done");
    send(
        "/lane_new",
        vec![OscType::Int(0), OscType::Int(-1), OscType::Int(10)],
    );
    assert_eq!(recv().addr, "/done");
    send("/lane_set",vec![OscType::Int(-1),OscType::String(serde_json::json!({"notes":[[0,4096,{"node":42},{"key":60,"velocity":0.4},"gate"]]}).to_string())]);
    assert_eq!(recv().addr, "/done");
    let note = event(7, 1, "note_on", 60, 0.2);
    send(&note.addr, note.args);
    send("/transport_play", vec![OscType::Int(0)]);
    assert_eq!(recv().addr, "/done");
    let mut output = [0.0; 128];
    for _ in 0..8 {
        assert_no_alloc(|| engine.process_block(&mut output));
        std::thread::sleep(Duration::from_millis(3));
    }
    assert!(output[0] > 0.61, "{:?}", &output[..4]);
    send(
        "/lane_set",
        vec![OscType::Int(-1), OscType::String("{}".into())],
    );
    assert_eq!(recv().addr, "/done");
    for _ in 0..6 {
        assert_no_alloc(|| engine.process_block(&mut output));
        std::thread::sleep(Duration::from_millis(3));
    }
    assert!((0.212..0.213).contains(&output[0]), "{:?}", &output[..4]);
    let note = event(7, 1, "note_off", 60, 0.0);
    send(&note.addr, note.args);
    send("/server_status", Vec::new());
    assert_eq!(recv().addr, "/server_status.reply");
    for _ in 0..6 {
        assert_no_alloc(|| engine.process_block(&mut output));
        std::thread::sleep(Duration::from_millis(3));
    }
    assert!(output.iter().all(|v| *v == 0.0));
    send("/server_quit", Vec::new());
    assert_eq!(recv().addr, "/done");
    thread.join().unwrap().unwrap();
}

#[test]
fn ump_channel_and_per_note_messages_preserve_precision_and_the_whole_packet() {
    let mut translator = build_translator();
    let (mut engine, mut handle) = engine();
    let mut cmds = Vec::new();
    translator
        .translate(
            &OscMessage {
                addr: "/midi_bind".into(),
                args: vec![OscType::Int(0), OscType::String("keys".into())],
            },
            &mut cmds,
        )
        .unwrap();
    send(&mut handle, cmds);
    let mut cmds = Vec::new();
    translator
        .translate_ump(&[0x40903c00, 0x80000000, 0x40d00000, 0x12345678], &mut cmds)
        .unwrap();
    assert_eq!(cmds.len(), 2);
    send(&mut handle, cmds);
    let samples = blocks(&mut engine, &mut handle, 6);
    let expected = 32768.0 / 65535.0 + 0x12345678_u32 as f64 / u32::MAX as f64 * 0.1 + 0.012;
    assert!((*samples.last().unwrap() as f64 - expected).abs() < 1e-6);
    let mut cmds = Vec::new();
    translator
        .translate_ump(&[0x40603c00, 0xc0000000, 0x40003c4a, 0xabcdef12], &mut cmds)
        .unwrap();
    assert_eq!(cmds.len(), 2);
    send(&mut handle, cmds);
    let samples = blocks(&mut engine, &mut handle, 6);
    assert!((*samples.last().unwrap() as f64 - (expected + 0.24)).abs() < 1e-6);
    let mut cmds = Vec::new();
    translator
        .translate_ump(&[0x40803c00, 0], &mut cmds)
        .unwrap();
    send(&mut handle, cmds);
    let samples = blocks(&mut engine, &mut handle, 6);
    assert_eq!(*samples.last().unwrap(), 0.0);
    assert_eq!(translator.node_defs.len(), 1);
}

#[test]
fn bounded_note_ownership_and_input_event_overflow_are_explicit() {
    let mut translator = build_translator();
    let (mut engine, mut handle) = engine();
    send(&mut handle, new_node(&mut translator, 0));
    let mut many = Vec::new();
    for id in 0..clausters_core::plugin_shm::EVENTS as i32 + 1 {
        many.extend(translate(&mut translator, event(0, id, "note_on", 60, 0.1)));
    }
    send(&mut handle, many);
    let mut out = [0.0; 128];
    assert_no_alloc(|| engine.process_block(&mut out));
    assert!(out.iter().all(|v| *v == 0.0));
    let fault = handle.pop_reply().unwrap();
    assert_eq!(fault.kind, ReplyKind::Fault);
    assert_eq!(fault.id, 1);
    let mut translator = build_translator();
    let mut cmds = Vec::new();
    translator
        .translate(
            &OscMessage {
                addr: "/midi_bind".into(),
                args: vec![OscType::Int(0), OscType::String("keys".into())],
            },
            &mut cmds,
        )
        .unwrap();
    // Past the 256 notes an instance holds, a key on its bus is not played
    // -- a shared bus's excess, not the instance's fault.
    for _ in 0..257 {
        translator
            .translate_midi_bytes(&[0x90, 60, 64], &mut cmds)
            .unwrap();
    }
    assert_eq!(translator.node_defs.len(), 1);
    let (mut engine, mut handle) = self::engine();
    let (attacks, setup): (Vec<Cmd>, Vec<Cmd>) = cmds
        .into_iter()
        .partition(|c| matches!(c, Cmd::BusEvent { .. }));
    send(&mut handle, setup);
    blocks(&mut engine, &mut handle, 2);
    // A block holds at most the bus's own capacity of events: send them in
    // a few blocks.
    for chunk in attacks.chunks(100) {
        for cmd in chunk {
            if let Cmd::BusEvent { bus, event } = cmd {
                assert!(
                    handle
                        .send(Cmd::BusEvent {
                            bus: *bus,
                            event: *event
                        })
                        .is_ok()
                );
            }
        }
        let held = blocks(&mut engine, &mut handle, 1);
        assert!(held.iter().all(|v| v.is_finite()));
    }
    blocks(&mut engine, &mut handle, 6);
}
