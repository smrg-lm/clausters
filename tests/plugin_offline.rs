//! Native helpers retain the same delayed processing contract in offline renders.
#![cfg(all(target_os = "linux", feature = "plugin-clap"))]
use clausters::dsp::ReplyKind;
use clausters::osc::translate::CmdTranslator;
use clausters::plugin::{PluginDef, PluginReference};
use clausters::rosc::{OscMessage, OscType};
use clausters::server::engine::{Cmd, engine_pair};
use clausters::server::render::{OscScore, RenderConfig, render_to_vec};
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
const SR: f64 = 48000.0;
static FIXTURES: OnceLock<[PathBuf; 3]> = OnceLock::new();
fn fixtures() -> &'static [PathBuf; 3] {
    FIXTURES.get_or_init(|| {
        let dir =
            std::env::temp_dir().join(format!("clausters-plugin-offline-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        [
            "crates/clausters-plugin-host/tests/fixtures/clap.c",
            "crates/clausters-plugin-host/tests/fixtures/polyphonic.c",
            "crates/clausters-plugin-host/tests/fixtures/extended.c",
        ]
        .map(|source| {
            let path = dir.join(format!(
                "{}.clap",
                PathBuf::from(source).file_stem().unwrap().to_str().unwrap()
            ));
            assert!(
                std::process::Command::new("cc")
                    .args([
                        "-std=c11",
                        "-O2",
                        "-fPIC",
                        "-shared",
                        "-Ithird_party/clap/include",
                        source,
                        "-o"
                    ])
                    .arg(&path)
                    .status()
                    .unwrap()
                    .success()
            );
            path
        })
    })
}
fn msg(addr: &str, args: Vec<OscType>) -> OscMessage {
    OscMessage {
        addr: addr.into(),
        args,
    }
}
fn reference(kind: &str, depth: u32) -> PluginReference {
    let (binary, inputs, outputs) = match kind {
        "instrument" => (1, json!([]), json!([{"id":0,"channels":1,"bus":0}])),
        "ports" => (
            2,
            json!([{"id":10,"channels":2,"bus":4},{"id":11,"channels":1,"bus":6}]),
            json!([{"id":20,"channels":2,"bus":0},{"id":21,"channels":1,"bus":2}]),
        ),
        _ => (
            0,
            json!([{"id":0,"channels":1,"bus":4}]),
            json!([{"id":0,"channels":1,"bus":0}]),
        ),
    };
    serde_json::from_value(json!({"format":"clap", "path":fixtures()[binary], "id":format!("org.clausters.fixture.{kind}"), "inputs":inputs, "outputs":outputs, "depth":depth})).unwrap()
}
fn new_node() -> OscMessage {
    msg(
        "/synth_new",
        vec![
            OscType::String("subject".into()),
            OscType::Int(42),
            OscType::Int(1),
            OscType::Int(0),
        ],
    )
}
fn event(kind: &str, value: f64) -> OscMessage {
    msg("/plugin_event", vec![OscType::Int(42), OscType::String(json!({"kind":kind,"source":1,"note_id":7,"port":0,"channel":0,"key":60,"value":value,"id":6}).to_string())])
}
fn compare(
    reference: PluginReference,
    events: Vec<(u64, Vec<OscMessage>)>,
    channels: usize,
) -> Vec<f32> {
    let mut translator = CmdTranslator::new(SR as f32);
    translator.plugin_defs.insert(
        "subject".into(),
        Arc::new(PluginDef::probe(reference.clone(), SR as f32).unwrap()),
    );
    let (mut engine, mut handle) = engine_pair(SR as f32, channels);
    for (sample, messages) in &events {
        let mut cmds = Vec::new();
        for message in messages {
            if message.addr == "/def_send" {
                translator.d_recv(&message.args[1..]).unwrap();
            } else {
                translator.translate(message, &mut cmds).unwrap();
            }
        }
        assert!(
            handle
                .send(Cmd::Schedule {
                    time: *sample,
                    cmds
                })
                .is_ok()
        );
    }
    let mut live = Vec::new();
    for _ in 0..24 {
        let mut block = vec![0.0; 64 * channels];
        engine.process_block(&mut block);
        live.extend(block);
        // Give the asynchronous helper time to finish; a missed deadline is a
        // failure of this comparison's explicit precondition, checked below.
        std::thread::sleep(Duration::from_millis(5));
        while let Some(reply) = handle.pop_reply() {
            assert_ne!(reply.kind, ReplyKind::Fault, "{reply:?}");
        }
        while handle.pop_garbage().is_some() {}
    }
    let mut score = vec![(
        0.0,
        vec![msg(
            "/def_send",
            vec![
                OscType::String("plugin".into()),
                OscType::String("subject".into()),
                OscType::Blob(serde_json::to_vec(&reference).unwrap()),
            ],
        )],
    )];
    score.extend(
        events
            .into_iter()
            .map(|(sample, messages)| (sample as f64 / SR, messages)),
    );
    score.push((
        24.0 * 64.0 / SR,
        vec![msg("/node_free", vec![OscType::Int(42)])],
    ));
    let score = OscScore::new(score).unwrap();
    let cfg = RenderConfig {
        channels,
        seed: Some(1),
        ..Default::default()
    };
    let offline = render_to_vec(&score, &cfg).unwrap().0;
    assert!(offline.iter().all(|x| x.is_finite()));
    assert_eq!(live, offline);
    assert_eq!(offline, render_to_vec(&score, &cfg).unwrap().0);
    offline
}
#[test]
fn persistent_notes_expression_transport_and_split_blocks_match_live() {
    for depth in [1, 4, 8] {
        let samples = compare(
            reference("instrument", depth),
            vec![
                (0, vec![new_node()]),
                (17, vec![event("note_on", 0.4)]),
                (31, vec![event("expression", 0.7)]),
                (91, vec![event("note_off", 0.0)]),
                (145, vec![event("note_on", 0.2)]),
                (333, vec![event("choke", 0.0)]),
            ],
            1,
        );
        assert!(
            samples[..depth as usize * 64 + 17]
                .iter()
                .all(|x| *x == 0.0)
        );
        assert!(samples.iter().any(|x| *x != 0.0));
    }
}
#[cfg(feature = "synth")]
#[test]
fn gain_intrinsic_delay_and_all_audio_ports_keep_bridge_latency() {
    for kind in ["gain", "delay", "ports"] {
        let source = json!({"name":"input","ugens":[{"kind":"Out","inputs":[{"const":4.0},{"const":1.0}]},{"kind":"Out","inputs":[{"const":5.0},{"const":2.0}]},{"kind":"Out","inputs":[{"const":6.0},{"const":3.0}]}]});
        let channels = if kind == "ports" { 3 } else { 1 };
        let samples = compare(
            reference(kind, 4),
            vec![
                (
                    0,
                    vec![
                        msg(
                            "/def_send",
                            vec![
                                OscType::String("synth".into()),
                                OscType::Blob(source.to_string().into_bytes()),
                            ],
                        ),
                        msg(
                            "/synth_new",
                            vec![
                                OscType::String("input".into()),
                                OscType::Int(41),
                                OscType::Int(1),
                                OscType::Int(0),
                            ],
                        ),
                        new_node(),
                    ],
                ),
                (
                    13,
                    vec![msg(
                        "/node_set",
                        vec![
                            OscType::Int(42),
                            OscType::String("param:0".into()),
                            OscType::Float(0.75),
                        ],
                    )],
                ),
            ],
            channels,
        );
        let delay = 256 + if kind == "delay" { 7 } else { 0 };
        assert!(samples[..delay * channels].iter().all(|x| *x == 0.0));
        assert_eq!(
            samples[delay * channels],
            if kind == "delay" { 1.0 } else { 0.5 }
        );
        if kind == "ports" {
            assert_eq!(&samples[256 * 3..256 * 3 + 3], &[0.5, 1.0, 1.5]);
        }
    }
}
#[test]
fn process_error_crash_and_hang_abort_with_node_and_processing_time() {
    for kind in ["error", "crash", "hang"] {
        let reference = reference(kind, 4);
        let score = OscScore::new([
            (
                0.0,
                vec![
                    msg(
                        "/def_send",
                        vec![
                            OscType::String("plugin".into()),
                            OscType::String("subject".into()),
                            OscType::String(serde_json::to_string(&reference).unwrap()),
                        ],
                    ),
                    new_node(),
                ],
            ),
            (0.02, vec![msg("/node_free", vec![OscType::Int(42)])]),
        ])
        .unwrap();
        let error = render_to_vec(&score, &RenderConfig::default())
            .err()
            .unwrap();
        assert!(error.contains("node 42"), "{kind}: {error}");
        assert!(error.contains("0.002667s"), "{kind}: {error}");
    }
}
