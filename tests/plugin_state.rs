//! Live opaque state belongs to one generation and participates in the barrier.
#![cfg(all(target_os = "linux", feature = "plugin-clap"))]
use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use clausters::dsp::{ProcessCtx, StageMask};
use clausters::node::{AddAction, SynthNode};
use clausters::osc::server::{OscServer, ServerInfo};
use clausters::plugin::{PluginDef, PluginReference, PluginSynth};
use clausters::rosc::{OscMessage, OscPacket, OscType, encoder};
use clausters::server::engine::{Cmd, engine_pair};
use std::net::UdpSocket;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
#[global_allocator]
static ALLOC: AllocDisabler = AllocDisabler;
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
struct Source;
impl SynthNode for Source {
    fn process(&mut self, ctx: &mut ProcessCtx) {
        // SAFETY: processed before both effects in this sequential tree.
        (unsafe { ctx.buses.audio_mut(4) })[ctx.offset..ctx.offset + ctx.frames].fill(1.0);
    }
    fn set_control(&mut self, _: u32, _: f32) {}
    fn map_control(&mut self, _: u32, _: i32, _: bool) {}
    fn ugen_count(&self) -> usize {
        1
    }
}
struct Live {
    socket: UdpSocket,
    address: std::net::SocketAddr,
    stop: Arc<AtomicBool>,
    outputs: Arc<[AtomicU32; 2]>,
    audio: Option<std::thread::JoinHandle<()>>,
    network: Option<std::thread::JoinHandle<std::io::Result<()>>>,
}
impl Live {
    fn new() -> Self {
        let (mut engine, mut handle) = engine_pair(48000.0, 2);
        handle
            .send(Cmd::AddSynth {
                id: 10,
                target: 0,
                action: AddAction::Head,
                synth: Box::new(Source),
                usage: StageMask::default(),
            })
            .ok()
            .unwrap();
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
        let network = Some(std::thread::spawn(move || server.run()));
        let stop = Arc::new(AtomicBool::new(false));
        let outputs = Arc::new([AtomicU32::new(0), AtomicU32::new(0)]);
        let audio = {
            let stop = Arc::clone(&stop);
            let outputs = Arc::clone(&outputs);
            Some(std::thread::spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    let mut out = [0.0; 128];
                    assert_no_alloc(|| engine.process_block(&mut out));
                    for (index, value) in outputs.iter().enumerate() {
                        value.store(out[index].to_bits(), Ordering::Release);
                    }
                    std::thread::sleep(Duration::from_millis(3));
                }
            }))
        };
        let socket = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(8)))
            .unwrap();
        Self {
            socket,
            address,
            stop,
            outputs,
            audio,
            network,
        }
    }
    fn send(&self, addr: &str, args: Vec<OscType>) {
        self.socket
            .send_to(
                &encoder::encode(&OscPacket::Message(OscMessage {
                    addr: addr.into(),
                    args,
                }))
                .unwrap(),
                self.address,
            )
            .unwrap();
    }
    fn recv(&self) -> OscMessage {
        let mut bytes = [0; 65536];
        let n = self.socket.recv(&mut bytes).unwrap();
        let OscPacket::Message(message) = clausters::osc::decode_packet(&bytes[..n]).unwrap()
        else {
            panic!("message required")
        };
        message
    }
    fn done(&self, command: &str) {
        let message = self.recv();
        assert_eq!(message.addr, "/done", "{message:?}");
        assert_eq!(message.args[0], OscType::String(command.into()));
    }
    fn sync(&self) {
        self.send("/server_sync", vec![OscType::Int(17)]);
        assert_eq!(self.recv().addr, "/server_sync.reply");
    }
    fn save(&self, node: i32) -> Vec<u8> {
        self.send("/plugin_saveState", vec![OscType::Int(node)]);
        // Barrier must arrive after all fragments and completion, even while
        // the worker awaits the callback boundary and prepares a new helper.
        self.send("/server_sync", vec![OscType::Int(18)]);
        let mut bytes = Vec::new();
        let mut count = 0;
        loop {
            let message = self.recv();
            if message.addr == "/done" {
                assert_eq!(message.args[0], OscType::String("/plugin_saveState".into()));
                break;
            }
            assert_eq!(message.addr, "/plugin_saveState.reply", "{message:?}");
            assert_eq!(message.args[0], OscType::Int(node));
            assert_eq!(message.args[1], OscType::Int(count));
            count += 1;
            let OscType::Blob(payload) = &message.args[3] else {
                panic!("blob required")
            };
            bytes.extend_from_slice(payload);
        }
        assert_eq!(self.recv().addr, "/server_sync.reply");
        bytes
    }
    fn output(&self, left: f32, right: f32) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let values = self
                .outputs
                .each_ref()
                .map(|value| f32::from_bits(value.load(Ordering::Acquire)));
            if values == [left, right] {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "expected {left}/{right}, got {values:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for Live {
    fn drop(&mut self) {
        self.send("/server_quit", vec![]);
        if let Some(network) = self.network.take() {
            network.join().unwrap().unwrap();
        }
        self.stop.store(true, Ordering::Release);
        if let Some(audio) = self.audio.take() {
            audio.join().unwrap();
        }
    }
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
    let snapshot = live.save(42);
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
    // A result from a freed generation must not reach a reused node ID.
    live.send("/plugin_saveState", vec![OscType::Int(42)]);
    live.send("/node_free", vec![OscType::Int(42)]);
    live.send(
        "/synth_new",
        vec![
            OscType::String("fx".into()),
            OscType::Int(42),
            OscType::Int(1),
            OscType::Int(0),
            OscType::String("param:0".into()),
            OscType::Float(0.125),
        ],
    );
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
    let synth = PluginSynth::new(def, 48000.0).unwrap();
    assert_eq!(synth.latency(), 256 + 23);
}
