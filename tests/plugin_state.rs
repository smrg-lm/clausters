//! Live opaque state belongs to one generation and participates in the barrier.
#![cfg(all(target_os = "linux", feature = "plugin-clap"))]
use clausters::node::SynthNode;
use clausters::plugin::{PluginDef, PluginReference, PluginSynth};
use clausters::rosc::OscType;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
#[path = "common/plugin_live.rs"]
mod plugin_live;
use plugin_live::Live;
static FIXTURE: OnceLock<PathBuf> = OnceLock::new();
fn reference() -> PluginReference {
    let path = FIXTURE.get_or_init(|| {
        let dir =
            std::env::temp_dir().join(format!("clausters-plugin-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("extended.clap");
        assert!(
            std::process::Command::new("cc")
                .args([
                    "-std=c11",
                    "-O2",
                    "-fPIC",
                    "-shared",
                    "-Ithird_party/clap/include",
                    "crates/clausters-plugin-host/tests/fixtures/extended.c",
                    "-o",
                ])
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        path
    });
    serde_json::from_value(serde_json::json!({"format":"clap", "path":path,
        "id":"org.clausters.fixture.state", "inputs":[{"id":0,"channels":1,"bus":4}],
        "outputs":[{"id":0,"channels":1,"bus":0}]}))
    .unwrap()
}
#[test]
fn two_nodes_capture_load_chunks_recover_and_keep_the_def_independent() {
    let live = Live::new();
    live.send(
        "/def_send",
        vec![
            OscType::String("plugin".into()),
            OscType::String("fx".into()),
            OscType::String(serde_json::to_string(&reference()).unwrap()),
        ],
    );
    live.done("/def_send");
    for (id, bus) in [(42, 0), (43, 1)] {
        live.send(
            "/synth_new",
            vec![
                OscType::String("fx".into()),
                OscType::Int(id),
                OscType::Int(1),
                OscType::Int(0),
                OscType::String("out:0".into()),
                OscType::Float(bus as f32),
            ],
        );
    }
    live.sync();
    live.output(0.5, 0.5);
    live.send(
        "/node_set",
        vec![
            OscType::Int(42),
            OscType::String("param:0".into()),
            OscType::Float(0.625),
        ],
    );
    live.output(0.625, 0.5);
    let snapshot = live.save_watching(42, Some(0));
    assert_eq!(&snapshot[..8], &0.625f64.to_ne_bytes());
    assert_eq!(snapshot.len(), 13008);
    assert_eq!(live.save(43), 0.5f64.to_ne_bytes());
    live.output(0.625, 0.5);
    live.send(
        "/plugin_loadState",
        vec![OscType::Int(43), OscType::Blob(snapshot.clone())],
    );
    live.done("/plugin_loadState");
    live.sync();
    live.output(0.625, 0.625);
    // The recipe still has its original defaults. A third node gets those.
    live.send("/def_query", vec![OscType::String("fx".into())]);
    assert_eq!(live.recv().args[4], OscType::Float(0.5));
    live.done("/def_query");
    // Refreshed native range allows a value the original surface rejected.
    live.send(
        "/plugin_loadState",
        vec![
            OscType::Int(42),
            OscType::Blob(0.75f64.to_ne_bytes().to_vec()),
        ],
    );
    live.done("/plugin_loadState");
    live.sync();
    live.output(0.75, 0.625);
    live.send("/node_query", vec![OscType::Int(42)]);
    assert_eq!(live.recv().args[8], OscType::Float(0.75));
    live.send(
        "/node_set",
        vec![
            OscType::Int(42),
            OscType::String("param:0".into()),
            OscType::Float(1.5),
        ],
    );
    live.output(1.5, 0.625);
    // Failed loads keep the last successful image available for recovery.
    for bytes in [vec![1], (-1.0f64).to_ne_bytes().to_vec()] {
        live.send(
            "/plugin_loadState",
            vec![OscType::Int(43), OscType::Blob(bytes)],
        );
        assert_eq!(live.recv().addr, "/fail");
        live.sync();
        live.output(1.5, 0.0);
        live.send("/plugin_restart", vec![OscType::Int(43)]);
        live.done("/plugin_restart");
        live.sync();
        live.output(1.5, 0.625);
    }
    // A result from a freed generation must not reach a reused node ID. One
    // bundle, so the capture cannot finish before the node is replaced.
    live.send_bundle(vec![
        ("/plugin_saveState", vec![OscType::Int(42)]),
        ("/node_free", vec![OscType::Int(42)]),
        (
            "/synth_new",
            vec![
                OscType::String("fx".into()),
                OscType::Int(42),
                OscType::Int(1),
                OscType::Int(0),
                OscType::String("param:0".into()),
                OscType::Float(0.125),
            ],
        ),
    ]);
    live.send("/server_sync", vec![OscType::Int(19)]);
    assert_eq!(live.recv().addr, "/fail");
    assert_eq!(live.recv().addr, "/server_sync.reply");
    live.output(0.125, 0.625);
    live.send("/bus_set", vec![OscType::Int(1), OscType::Float(0.375)]);
    live.send(
        "/node_map",
        vec![
            OscType::Int(43),
            OscType::String("param:0".into()),
            OscType::Int(1),
        ],
    );
    live.output(0.125, 0.375);
    live.send(
        "/plugin_loadState",
        vec![
            OscType::Int(43),
            OscType::Blob(0.75f64.to_ne_bytes().to_vec()),
        ],
    );
    live.done("/plugin_loadState");
    live.sync();
    live.output(0.125, 0.375);
    assert_eq!(live.save(43), 0.375f64.to_ne_bytes());
    live.output(0.125, 0.375);
    // Empty opaque images are real states, not an omitted initial snapshot.
    live.send(
        "/node_set",
        vec![
            OscType::Int(42),
            OscType::String("param:0".into()),
            OscType::Float(0.333),
        ],
    );
    live.output(0.333, 0.375);
    // Exact double fixture sentinel is reached by explicit native state load.
    live.send(
        "/plugin_loadState",
        vec![OscType::Int(42), OscType::Blob(Vec::new())],
    );
    live.done("/plugin_loadState");
    assert!(live.save(42).is_empty());
    live.send("/plugin_restart", vec![OscType::Int(42)]);
    live.done("/plugin_restart");
    live.sync();
    live.output(0.333, 0.375);
}
#[test]
fn initial_state_refreshes_latency_ports_and_parameter_metadata_before_activation() {
    let mut reference = reference();
    reference.state = 0.75f64.to_ne_bytes().to_vec();
    let def = Arc::new(PluginDef::probe(reference, 48000.0).unwrap());
    assert_eq!(def.defaults[0], 0.75);
    assert_eq!(def.info.parameters[0].max, 2.0);
    assert_eq!(def.info.inputs[0].name, "Restored port");
    let synth = PluginSynth::new(def, 48000.0, 0).unwrap();
    assert_eq!(synth.latency(), 256 + 23);
}
