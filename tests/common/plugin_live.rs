//! Shared paced, device-free OSC acceptance server for native adapters.
use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use clausters::dsp::{ProcessCtx, StageMask};
use clausters::node::{AddAction, SynthNode};
use clausters::osc::server::{OscServer, ServerInfo};
use clausters::rosc::{OscMessage, OscPacket, OscType, encoder};
use clausters::server::engine::{Cmd, engine_pair};
use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};
#[global_allocator]
static ALLOC: AllocDisabler = AllocDisabler;
struct Source;
impl SynthNode for Source {
    fn process(&mut self, ctx: &mut ProcessCtx) {
        // SAFETY: processed before both effects in this sequential tree.
        for bus in [4, 5] {
            (unsafe { ctx.buses.audio_mut(bus) })[ctx.offset..ctx.offset + ctx.frames].fill(1.0);
        }
    }
    fn set_control(&mut self, _: u32, _: f32) {}
    fn map_control(&mut self, _: u32, _: i32, _: bool) {}
    fn ugen_count(&self) -> usize {
        1
    }
}
pub struct Live {
    socket: UdpSocket,
    address: std::net::SocketAddr,
    stop: Arc<AtomicBool>,
    outputs: Arc<[AtomicU32; 2]>,
    audio: Option<std::thread::JoinHandle<()>>,
    network: Option<std::thread::JoinHandle<std::io::Result<()>>>,
}
impl Live {
    pub fn new() -> Self {
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
    pub fn send(&self, addr: &str, args: Vec<OscType>) {
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
    /// Several messages in one immediate bundle, handled in one turn.
    // Each test binary compiles this module; only some of them send bundles.
    #[allow(dead_code)]
    pub fn send_bundle(&self, messages: Vec<(&str, Vec<OscType>)>) {
        let content = messages
            .into_iter()
            .map(|(addr, args)| {
                OscPacket::Message(OscMessage {
                    addr: addr.into(),
                    args,
                })
            })
            .collect();
        let bundle = OscPacket::Bundle(clausters::rosc::OscBundle {
            timetag: clausters::rosc::OscTime::from((0, 1)),
            content,
        });
        self.socket
            .send_to(&encoder::encode(&bundle).unwrap(), self.address)
            .unwrap();
    }
    pub fn recv(&self) -> OscMessage {
        let mut bytes = [0; 65536];
        let n = self.socket.recv(&mut bytes).unwrap();
        let OscPacket::Message(message) = clausters::osc::decode_packet(&bytes[..n]).unwrap()
        else {
            panic!("message required")
        };
        message
    }
    pub fn done(&self, command: &str) {
        let message = self.recv();
        assert_eq!(message.addr, "/done", "{message:?}");
        assert_eq!(message.args[0], OscType::String(command.into()));
    }
    pub fn sync(&self) {
        self.send("/server_sync", vec![OscType::Int(17)]);
        assert_eq!(self.recv().addr, "/server_sync.reply");
    }
    pub fn save(&self, node: i32) -> Vec<u8> {
        self.save_watching(node, None)
    }
    /// Save while checking, on every reply, that output `channel` never fell
    /// silent: a capture must not interrupt the node it captures.
    pub fn save_watching(&self, node: i32, channel: Option<usize>) -> Vec<u8> {
        self.send("/plugin_saveState", vec![OscType::Int(node)]);
        // Barrier must arrive after all fragments and completion.
        self.send("/server_sync", vec![OscType::Int(18)]);
        let mut bytes = Vec::new();
        let mut count = 0;
        let watch = || {
            if let Some(channel) = channel {
                let value = f32::from_bits(self.outputs[channel].load(Ordering::Acquire));
                assert_ne!(value, 0.0, "output {channel} fell silent during a save");
            }
        };
        loop {
            watch();
            let message = self.recv();
            watch();
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
    pub fn output(&self, left: f32, right: f32) {
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
