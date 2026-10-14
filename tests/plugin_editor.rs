//! Native editors use the node's administrative lease while its DSP continues.
#![cfg(all(
    target_os = "linux",
    any(feature = "plugin-clap", feature = "plugin-vst3")
))]
use clausters::rosc::OscType;
#[allow(dead_code)] // This fixture also provides state capture for the state suite.
#[path = "common/plugin_live.rs"]
mod plugin_live;
#[cfg(feature = "plugin-vst3")]
#[path = "common/vst3_fixture.rs"]
mod vst3_fixture;
use plugin_live::Live;
fn exercise(format: &str, path: &std::path::Path, identity: &str, channels: u32) {
    let live = Live::new();
    let reference = serde_json::json!({"format":format,"path":path,"id":identity,
        "inputs":[{"id":0,"channels":channels,"bus":4}],"outputs":[{"id":0,"channels":channels,"bus":0}]});
    live.send(
        "/def_send",
        vec![
            OscType::String("editor".into()),
            OscType::String("plugin".into()),
        ],
    );
    // Above invalid ordering must not claim a name or start a helper.
    assert_eq!(live.recv().addr, "/fail");
    live.send(
        "/def_send",
        vec![
            OscType::String("plugin".into()),
            OscType::String("editor".into()),
            OscType::String(reference.to_string()),
        ],
    );
    live.done("/def_send");
    live.send(
        "/synth_new",
        vec![
            OscType::String("editor".into()),
            OscType::Int(42),
            OscType::Int(1),
            OscType::Int(0),
        ],
    );
    live.sync();
    let query = |live: &Live| {
        live.send("/plugin_queryEditor", vec![OscType::Int(42)]);
        let reply = live.recv();
        assert_eq!(reply.addr, "/done", "{reply:?}");
        assert_eq!(reply.args[0], OscType::String("/plugin_queryEditor".into()));
        assert_eq!(reply.args[1], OscType::Int(42));
        let OscType::String(state) = &reply.args[2] else {
            panic!("editor JSON")
        };
        serde_json::from_str::<serde_json::Value>(state).unwrap()
    };
    assert_eq!(query(&live)["open"], false);
    live.send(
        "/plugin_resizeEditor",
        vec![OscType::Int(42), OscType::Int(0), OscType::Int(200)],
    );
    assert_eq!(live.recv().addr, "/fail");
    if std::env::var_os("DISPLAY").is_none() {
        return;
    }
    live.send("/node_run", vec![OscType::Int(42), OscType::Int(0)]);
    live.sync();
    live.send("/server_notify", vec![OscType::Int(1)]);
    live.done("/server_notify");
    live.send("/plugin_openEditor", vec![OscType::Int(42)]);
    // Native callback notifications can arrive before the administrative reply.
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut changed = false;
    let mut opened = false;
    while !changed || !opened {
        assert!(std::time::Instant::now() < until);
        let message = live.recv();
        if message.addr == "/done" {
            assert_eq!(
                message.args[0],
                OscType::String("/plugin_openEditor".into())
            );
            opened = true;
        }
        if message.addr == "/plugin_edit" && message.args.contains(&OscType::Float(0.75)) {
            changed = true;
        }
    }
    // Unsubscribe after consuming any already queued gesture notifications.
    live.send("/server_notify", vec![OscType::Int(0)]);
    loop {
        if live.recv().addr == "/done" {
            break;
        }
    }
    live.send(
        "/synth_get",
        vec![OscType::Int(42), OscType::String("param:0".into())],
    );
    let values = live.recv();
    assert_eq!(values.addr, "/node_set");
    assert!(values.args.contains(&OscType::Float(0.75)), "{values:?}");
    live.output(0.0, 0.0);
    live.send("/node_run", vec![OscType::Int(42), OscType::Int(1)]);
    live.sync();
    live.output(0.75, if channels == 2 { 0.75 } else { 0.0 });
    live.send(
        "/plugin_resizeEditor",
        vec![OscType::Int(42), OscType::Int(480), OscType::Int(280)],
    );
    live.done("/plugin_resizeEditor");
    assert_eq!(query(&live)["width"], 480);
    live.send("/plugin_closeEditor", vec![OscType::Int(42)]);
    live.done("/plugin_closeEditor");
    assert_eq!(query(&live)["open"], false);
    live.send("/plugin_openEditor", vec![OscType::Int(42)]);
    live.done("/plugin_openEditor");
    live.send("/node_free", vec![OscType::Int(42)]);
    live.sync();
    live.send("/plugin_queryEditor", vec![OscType::Int(42)]);
    assert_eq!(live.recv().addr, "/fail");
}
#[cfg(feature = "plugin-clap")]
#[test]
fn clap_editor_controls_and_notifications_share_the_existing_node() {
    let path =
        std::env::temp_dir().join(format!("clausters-osc-editor-{}.clap", std::process::id()));
    let result = std::process::Command::new("cc")
        .args([
            "-std=c11",
            "-O2",
            "-fPIC",
            "-shared",
            "-Ithird_party/clap/include",
            "crates/clausters-plugin-host/tests/fixtures/editor.c",
            "-o",
        ])
        .arg(&path)
        .arg("-lX11")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    exercise("clap", &path, "org.clausters.fixture.gain", 1);
}
#[cfg(feature = "plugin-vst3")]
#[test]
fn vst3_editor_controls_and_notifications_share_the_existing_node() {
    exercise(
        "vst3",
        vst3_fixture::fixture(),
        "6E33225254224A00AA69301AF318797D",
        2,
    );
}
