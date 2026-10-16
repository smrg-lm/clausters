//! Isolated nodes use the engine's slice, control, bus and garbage contracts.
#![cfg(all(target_os = "linux", feature = "plugin-clap"))]
use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use clausters::dsp::{Buses, ControlBuses, ProcessCtx, TransportCtx};
use clausters::node::SynthNode;
use clausters::osc::translate::CmdTranslator;
use clausters::plugin::{PluginDef, PluginReference, PluginSynth};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
#[global_allocator]
static ALLOC: AllocDisabler = AllocDisabler;
static FIXTURE: OnceLock<PathBuf> = OnceLock::new();
fn fixture() -> &'static PathBuf {
    FIXTURE.get_or_init(|| {
        let dir =
            std::env::temp_dir().join(format!("clausters-node-plugin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fixture.clap");
        assert!(
            std::process::Command::new("cc")
                .args([
                    "-std=c11",
                    "-O2",
                    "-fPIC",
                    "-shared",
                    "-Ithird_party/clap/include",
                    "crates/clausters-plugin-host/tests/fixtures/clap.c",
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
fn def(kind: &str) -> Arc<PluginDef> {
    let reference: PluginReference = serde_json::from_value(serde_json::json!({"format":"clap", "path":fixture(), "id":format!("org.clausters.fixture.{kind}"), "inputs":[{"id":0,"channels":1,"bus":4}], "outputs":[{"id":0,"channels":1,"bus":0}]})).unwrap();
    Arc::new(PluginDef::probe(reference, 48000.0).unwrap())
}
#[test]
fn exact_due_audio_slices_native_controls_maps_and_latency_without_allocations() {
    let def = def("gain");
    assert_eq!(def.names, ["param:0", "in:0", "out:0"]);
    let (usage, indices) = def.usage(&def.defaults);
    assert_eq!(indices, [1, 2]);
    let _ = usage;
    let mut synth = PluginSynth::new(Arc::clone(&def), 48000.0, 0).unwrap();
    assert_eq!(synth.latency(), 256);
    let mut buses = Buses::new(ControlBuses::new(16), 16);
    for block in 0..20 {
        buses.clear_audio();
        if block == 10 {
            buses.control.set(1, 0.25);
            synth.map_control(0, 1, false);
        }
        // SAFETY: this test is the sole bus owner.
        unsafe { buses.audio_mut(4) }.fill(1.0);
        for (offset, frames) in [(0, 13), (13, 51)] {
            let mut ctx = ProcessCtx {
                sample_rate: 48000.0,
                full_sample_rate: 48000.0,
                buses: &buses,
                buffers: &[],
                offset,
                frames,
                transport: TransportCtx {
                    position: block * 64 + offset as u64,
                    rolling: true,
                    ..TransportCtx::default()
                },
            };
            assert_no_alloc(|| {
                synth.prepare_slice(&ctx, true);
                synth.process(&mut ctx);
            });
            if block == 0 && offset == 0 {
                synth.set_control(0, 0.75);
            }
        }
        assert_no_alloc(|| synth.finish_block());
        std::thread::sleep(Duration::from_millis(3));
        if (6..14).contains(&block) {
            assert!(buses.audio(0).iter().all(|x| *x == 0.75));
        }
        if block >= 14 {
            assert!(buses.audio(0).iter().all(|x| *x == 0.25));
        }
    }
    let mut translator = CmdTranslator::new(48000.0);
    translator.plugin_defs.insert("fx".into(), def);
    let info = translator.def_info(Some(&["fx".into()]));
    assert_eq!(
        info[0][1],
        clausters::rosc::OscType::String("plugin".into())
    );
    translator
        .d_free(&[clausters::rosc::OscType::String("fx".into())])
        .unwrap();
    // The live instance retains its reference and helper after freeing the def.
    assert_eq!(synth.latency(), 256);
}
#[test]
fn capacity_and_process_death_latch_a_single_node_fault() {
    for kind in ["gain", "crash", "hang", "error"] {
        let mut synth = PluginSynth::new(def(kind), 48000.0, 0).unwrap();
        let buses = Buses::new(ControlBuses::new(16), 16);
        for block in 0..22 {
            let mut ctx = ProcessCtx {
                sample_rate: 48000.0,
                full_sample_rate: 48000.0,
                buses: &buses,
                buffers: &[],
                offset: 0,
                frames: 64,
                transport: TransportCtx::default(),
            };
            assert_no_alloc(|| {
                synth.prepare_slice(&ctx, true);
                synth.process(&mut ctx);
                synth.finish_block();
            });
            if kind == "gain" && block == 0 {
                // Nine spans exceed the negotiated span capacity explicitly.
                for offset in 0..9 {
                    ctx.offset = offset;
                    ctx.frames = 1;
                    synth.prepare_slice(&ctx, true);
                }
                synth.finish_block();
            }
            std::thread::sleep(Duration::from_millis(3));
        }
        let mut faults = Vec::new();
        synth.drain_replies(12, &mut |msg| faults.push(msg));
        synth.drain_replies(12, &mut |msg| faults.push(msg));
        assert_eq!(faults.len(), 1, "{kind}");
        assert_eq!(faults[0].node_id, 12);
    }
}

#[test]
fn osc_family_barrier_persistence_queries_restart_and_unavailable_records() {
    use clausters::osc::server::{OscServer, ServerInfo};
    use clausters::rosc::{OscMessage, OscPacket, OscType, encoder};
    use clausters::server::defstore::DefStore;
    use clausters::server::engine::engine_pair;
    use std::net::UdpSocket;
    let directory = fixture().parent().unwrap().join("defs-data");
    let store = DefStore::open(&directory).unwrap();
    store
        .save_plugindef(
            "unavailable",
            br#"{"format":"clap","path":"/missing.clap","id":"missing","inputs":[],"outputs":[]}"#,
            None,
        )
        .unwrap();
    let (mut engine, handle) = engine_pair(48000.0, 2);
    let mut server = OscServer::bind(
        ("127.0.0.1", 0),
        ServerInfo {
            nominal_sample_rate: 48000.0,
            actual_sample_rate: 48000.0,
        },
        handle,
    )
    .unwrap();
    server.attach_store(store);
    assert!(directory.join("defs/plugindefs/unavailable.json").exists());
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
            .unwrap();
    };
    let recv = || {
        let mut bytes = [0; 65536];
        let n = client.recv(&mut bytes).unwrap();
        let OscPacket::Message(msg) = clausters::osc::decode_packet(&bytes[..n]).unwrap() else {
            panic!("message required")
        };
        msg
    };
    let reference = serde_json::to_string(&def("gain").reference).unwrap();
    send(
        "/def_send",
        vec![
            OscType::String("plugin".into()),
            OscType::String("fx".into()),
            OscType::String(reference),
        ],
    );
    send("/server_sync", vec![OscType::Int(17)]);
    assert_eq!(
        recv().args,
        [
            OscType::String("/def_send".into()),
            OscType::String("plugin".into()),
            OscType::String("fx".into())
        ]
    );
    assert_eq!(recv().addr, "/server_sync.reply");
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.join("defs/plugindefs/fx.json")).unwrap())
            .unwrap();
    assert!(record["surface"]["info"]["parameters"].is_array());
    send(
        "/synth_new",
        vec![
            OscType::String("fx".into()),
            OscType::Int(42),
            OscType::Int(0),
            OscType::Int(0),
        ],
    );
    send("/def_query", vec![OscType::String("fx".into())]);
    assert_eq!(recv().args[1], OscType::String("plugin".into()));
    assert_eq!(recv().addr, "/done");
    assert_no_alloc(|| engine.process_block(&mut [0.0; 128]));
    for _ in 0..2 {
        send("/plugin_restart", vec![OscType::Int(42)]);
        assert_eq!(recv().args[0], OscType::String("/plugin_restart".into()));
        assert_no_alloc(|| engine.process_block(&mut [0.0; 128]));
    }
    send("/def_free", vec![OscType::String("fx".into())]);
    send("/node_query", vec![OscType::Int(42)]);
    assert_eq!(recv().addr, "/node_query.reply");
    assert!(!directory.join("defs/plugindefs/fx.json").exists());
    send("/node_free", vec![OscType::Int(42)]);
    assert_no_alloc(|| engine.process_block(&mut [0.0; 128]));
    send("/server_quit", Vec::new());
    assert_eq!(recv().addr, "/done");
    thread.join().unwrap().unwrap();
}

#[test]
fn a_stored_surface_restores_without_loading_the_plugin() {
    use clausters::osc::server::{OscServer, ServerInfo};
    use clausters::rosc::{OscMessage, OscPacket, OscType, encoder};
    use clausters::server::defstore::DefStore;
    use clausters::server::engine::engine_pair;
    use std::net::UdpSocket;
    let directory = fixture().parent().unwrap().join("lazy-data");
    let store = DefStore::open(&directory).unwrap();
    // The binary exists but holds no plugin of this id, so a probe would
    // fail: the def is present only if restoring loaded nothing.
    let probed = def("gain");
    let mut reference = probed.reference.clone();
    reference.id = "org.clausters.fixture.absent".into();
    store
        .save_plugindef(
            "lazy",
            &serde_json::to_vec(&reference).unwrap(),
            Some(&serde_json::to_vec(&probed.surface()).unwrap()),
        )
        .unwrap();
    let (_engine, handle) = engine_pair(48000.0, 2);
    let mut server = OscServer::bind(
        ("127.0.0.1", 0),
        ServerInfo {
            nominal_sample_rate: 48000.0,
            actual_sample_rate: 48000.0,
        },
        handle,
    )
    .unwrap();
    server.attach_store(store);
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
            .unwrap();
    };
    let recv = || {
        let mut bytes = [0; 65536];
        let n = client.recv(&mut bytes).unwrap();
        let OscPacket::Message(msg) = clausters::osc::decode_packet(&bytes[..n]).unwrap() else {
            panic!("message required")
        };
        msg
    };
    send("/def_query", vec![OscType::String("lazy".into())]);
    let reply = recv();
    assert_eq!(reply.args[1], OscType::String("plugin".into()));
    assert!(reply.args.contains(&OscType::String("param:0".into())));
    assert_eq!(recv().addr, "/done");
    // The first node is what loads the plugin, and it finds none.
    send(
        "/synth_new",
        vec![
            OscType::String("lazy".into()),
            OscType::Int(7),
            OscType::Int(0),
            OscType::Int(0),
        ],
    );
    let failed = recv();
    assert_eq!(failed.addr, "/fail");
    send("/server_quit", Vec::new());
    assert_eq!(recv().addr, "/done");
    thread.join().unwrap().unwrap();
}
