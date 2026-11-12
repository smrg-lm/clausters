//! The second native adapter binds the same plugin node, OSC and recovery contract.
#![cfg(all(target_os = "linux", feature = "plugin-vst3"))]
#[path = "common/plugin_live.rs"]
mod plugin_live;
#[path = "common/vst3_fixture.rs"]
mod vst3_fixture;
use clausters::rosc::OscType;
use plugin_live::Live;
#[test]
fn vst3_def_live_state_barrier_recovery_and_free_use_the_existing_family() {
    let live = Live::new();
    let reference = serde_json::json!({"format":"vst3","path":vst3_fixture::fixture(),"id":"6E33225254224A00AA69301AF318797D","inputs":[{"id":0,"channels":2,"bus":4}],"outputs":[{"id":0,"channels":2,"bus":0}]});
    live.send(
        "/def_send",
        vec![
            OscType::String("plugin".into()),
            OscType::String("fx".into()),
            OscType::String(reference.to_string()),
        ],
    );
    live.done("/def_send");
    live.send(
        "/synth_new",
        vec![
            OscType::String("fx".into()),
            OscType::Int(42),
            OscType::Int(1),
            OscType::Int(0),
        ],
    );
    live.sync();
    live.output(0.5, 0.5);
    live.send(
        "/node_set",
        vec![
            OscType::Int(42),
            OscType::String("param:0".into()),
            OscType::Float(0.75),
        ],
    );
    live.output(0.75, 0.75);
    let state = live.save(42);
    assert_eq!(&state[..4], b"VST3");
    live.send(
        "/node_set",
        vec![
            OscType::Int(42),
            OscType::String("param:0".into()),
            OscType::Float(0.2),
        ],
    );
    live.output(0.2, 0.2);
    live.send(
        "/plugin_loadState",
        vec![OscType::Int(42), OscType::Blob(state)],
    );
    live.done("/plugin_loadState");
    live.sync();
    live.output(0.75, 0.75);
    live.send("/plugin_restart", vec![OscType::Int(42)]);
    live.done("/plugin_restart");
    live.sync();
    live.output(0.75, 0.75);
    live.send("/node_free", vec![OscType::Int(42)]);
    live.sync();
    live.output(0.0, 0.0);
}
#[test]
fn offline_vst3_state_and_notes_render_deterministically() {
    use clausters::rosc::OscMessage;
    use clausters::server::render::{OscScore, RenderConfig, render_to_vec};
    let reference = serde_json::json!({"format":"vst3","path":vst3_fixture::fixture(),"id":"6E33225254224A00AA69301AF318797D","inputs":[{"id":0,"channels":2,"bus":4}],"outputs":[{"id":0,"channels":2,"bus":0}]});
    let message = |addr: &str, args: Vec<OscType>| OscMessage {
        addr: addr.into(),
        args,
    };
    let event = |kind: &str, value: f64| {
        message("/plugin_event",vec![OscType::Int(42),OscType::String(serde_json::json!({"kind":kind,"port":0,"note_id":7,"channel":0,"key":60,"value":value}).to_string())])
    };
    let score = OscScore::new(vec![
        (
            0.0,
            vec![
                message(
                    "/def_send",
                    vec![
                        OscType::String("plugin".into()),
                        OscType::String("fx".into()),
                        OscType::String(reference.to_string()),
                    ],
                ),
                message(
                    "/synth_new",
                    vec![
                        OscType::String("fx".into()),
                        OscType::Int(42),
                        OscType::Int(1),
                        OscType::Int(0),
                    ],
                ),
            ],
        ),
        (17.0 / 48000.0, vec![event("note_on", 0.8)]),
        (91.0 / 48000.0, vec![event("note_off", 0.0)]),
        (
            1024.0 / 48000.0,
            vec![message("/node_free", vec![OscType::Int(42)])],
        ),
    ])
    .unwrap();
    let config = RenderConfig {
        channels: 2,
        ..Default::default()
    };
    let samples = render_to_vec(&score, &config).unwrap().0;
    assert_eq!(samples, render_to_vec(&score, &config).unwrap().0);
    for sample in 0..samples.len() / 2 {
        let expected = if (256 + 17..256 + 91).contains(&sample) {
            0.4
        } else {
            0.0
        };
        assert!((samples[sample * 2] - expected).abs() < 1e-6);
        assert_eq!(samples[sample * 2], samples[sample * 2 + 1]);
    }
}
