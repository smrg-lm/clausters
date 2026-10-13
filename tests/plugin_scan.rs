//! The native discovery worker, persistence and async barrier over real OSC.
use clausters::osc::server::{OscServer, ServerInfo};
use clausters::rosc::{OscMessage, OscPacket, OscType, encoder};
use clausters::server::engine::engine_pair;
use std::net::UdpSocket;
use std::time::Duration;

#[test]
fn scan_capability_validation_and_barrier() {
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
    #[cfg(all(target_os = "linux", feature = "plugin-clap"))]
    let directory = {
        use clausters::server::defstore::DefStore;
        use std::path::Path;
        use std::process::Command;
        let directory =
            std::env::temp_dir().join(format!("clausters-scan-osc-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        for (source, name, mode) in [
            (
                "crates/clausters-plugin-host/tests/fixtures/clap.c",
                "fixture.clap",
                None,
            ),
            (
                "crates/clausters-plugin-host/tests/fixtures/failure.c",
                "a-hang.clap",
                Some(2),
            ),
        ] {
            let mut command = Command::new("cc");
            command
                .args(["-std=c11", "-O2", "-fPIC", "-shared"])
                .arg("-I")
                .arg(root.join("third_party/clap/include"));
            if let Some(mode) = mode {
                command.arg(format!("-DFAILURE_MODE={mode}"));
            }
            assert!(
                command
                    .arg(root.join(source))
                    .arg("-o")
                    .arg(directory.join(name))
                    .status()
                    .unwrap()
                    .success()
            );
        }
        server.attach_store(DefStore::open(&directory.join("data")).unwrap());
        directory
    };
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
            panic!("expected message")
        };
        msg
    };
    send("/plugin_scan", vec![OscType::Int(1)]);
    let invalid = recv();
    assert_eq!(invalid.addr, "/fail");
    assert!(matches!(&invalid.args[1], OscType::String(s) if s.contains("strings")));
    #[cfg(all(target_os = "linux", feature = "plugin-clap"))]
    {
        send(
            "/plugin_scan",
            vec![OscType::String(directory.to_string_lossy().into_owned())],
        );
        send("/server_status", Vec::new());
        send("/server_sync", vec![OscType::Int(38)]);
        // The deliberately hanging first candidate cannot block the network loop.
        let before = std::time::Instant::now();
        assert_eq!(recv().addr, "/server_status.reply");
        assert!(before.elapsed() < Duration::from_secs(1));
        let mut entries = Vec::new();
        let mut manifest = false;
        let mut done = false;
        loop {
            let msg = recv();
            match msg.addr.as_str() {
                "/plugin_scan.reply" => {
                    let [
                        OscType::Int(index),
                        OscType::Int(count),
                        OscType::Int(chunk),
                        OscType::Int(chunks),
                        OscType::Blob(bytes),
                    ] = msg.args.as_slice()
                    else {
                        panic!("invalid fragment")
                    };
                    assert_eq!(*count, 7);
                    if *index == -1 {
                        manifest = true;
                        assert!(bytes.is_empty());
                    } else {
                        assert_eq!((*chunk, *chunks), (0, 1));
                        entries.push(serde_json::from_slice::<serde_json::Value>(bytes).unwrap());
                    }
                }
                "/done" => {
                    assert_eq!(msg.args[0], OscType::String("/plugin_scan".into()));
                    done = true;
                }
                "/server_sync.reply" => {
                    assert_eq!(msg.args[0], OscType::Int(38));
                    break;
                }
                _ => panic!("unexpected reply: {msg:?}"),
            }
        }
        assert!(done && manifest);
        assert_eq!(entries.len(), 7);
        assert!(
            entries
                .iter()
                .any(|e| e["kind"] == "error" && e["error"].as_str().unwrap().contains("timeout"))
        );
        let cached: serde_json::Value =
            serde_json::from_slice(&std::fs::read(directory.join("data/plugins.json")).unwrap())
                .unwrap();
        assert_eq!(cached["entries"], serde_json::Value::Array(entries));
    }
    #[cfg(not(all(target_os = "linux", feature = "plugin-clap")))]
    {
        send("/plugin_scan", Vec::new());
        let reply = recv();
        assert_eq!(reply.addr, "/fail");
        assert!(
            matches!(&reply.args[1], OscType::String(s) if s.contains("CLAP support") || s.contains("in-page"))
        );
        send("/server_sync", vec![OscType::Int(38)]);
        assert_eq!(recv().addr, "/server_sync.reply");
    }
    send("/server_quit", Vec::new());
    thread.join().unwrap().unwrap();
}
