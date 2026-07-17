//! M19: MIDI-standalone operation — a server boots from a data directory with
//! its defs, MIDI bindings and boot preset already in place, playable with no
//! OSC programming. End-to-end across two real `OscServer` instances on one
//! data dir (no audio device, no MIDI transport): session 1 sets things up
//! over OSC, session 2 reloads them at boot. We observe the reload through
//! `/g_queryTree` (a restored GraphDef binding and a boot graph appear as
//! groups in the node tree).

#![cfg(feature = "synth")]

use std::net::{SocketAddr, UdpSocket};
use std::path::Path;
use std::time::Duration;

use clausters::osc::server::{OscServer, ServerInfo};
use clausters::rosc::{OscMessage, OscPacket, OscType, decoder, encoder};
use clausters::server::defstore::DefStore;
use clausters::server::engine::engine_pair;

const SR: f32 = 48_000.0;
const DEADLINE: Duration = Duration::from_secs(2);

/// A per-test temp dir, removed on drop.
struct TempDir(std::path::PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("clausters-m19-{}-{tag}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct TestServer {
    addr: SocketAddr,
    handle: std::thread::JoinHandle<std::io::Result<()>>,
    client: UdpSocket,
}

impl TestServer {
    fn spawn(data_dir: &Path) -> Self {
        let (_engine, engine_handle) = engine_pair(SR, 2);
        let info = ServerInfo {
            nominal_sample_rate: SR as f64,
            actual_sample_rate: SR as f64,
        };
        let mut server = OscServer::bind(("127.0.0.1", 0), info, engine_handle).unwrap();
        server.attach_store(DefStore::open(data_dir).unwrap());
        let addr = server.local_addr().unwrap();
        let handle = std::thread::spawn(move || server.run());
        let client = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        client.set_read_timeout(Some(DEADLINE)).unwrap();
        Self {
            addr,
            handle,
            client,
        }
    }

    fn send(&self, addr: &str, args: Vec<OscType>) {
        let packet = OscPacket::Message(OscMessage {
            addr: addr.into(),
            args,
        });
        self.client
            .send_to(&encoder::encode(&packet).unwrap(), self.addr)
            .unwrap();
    }

    fn recv_until(&self, addr: &str) -> OscMessage {
        let mut buf = [0u8; 65536];
        for _ in 0..200 {
            let (len, _) = self.client.recv_from(&mut buf).expect("reply timed out");
            if let (_, OscPacket::Message(msg)) = decoder::decode_udp(&buf[..len]).unwrap()
                && msg.addr == addr
            {
                return msg;
            }
        }
        panic!("never received {addr}");
    }

    /// The child count of the root group from `/g_queryTree 0` (arg index 2).
    fn root_child_count(&self) -> i32 {
        self.send("/g_queryTree", vec![OscType::Int(0)]);
        match self.recv_until("/g_queryTree.reply").args[2] {
            OscType::Int(n) => n,
            ref other => panic!("unexpected child count: {other:?}"),
        }
    }

    fn sync(&self) {
        self.send("/sync", vec![OscType::Int(1)]);
        self.recv_until("/synced");
    }

    fn quit(self) {
        self.send("/quit", vec![]);
        self.recv_until("/done");
        self.handle.join().unwrap().unwrap();
    }
}

const VTONE: &str = r#"{"name":"vtone","controls":[{"name":"out","default":0.0},{"name":"freq","default":440.0},{"name":"level","default":0.2}],"ugens":[{"kind":"Sine","inputs":[{"control":1}]},{"kind":"Mul","inputs":[{"ugen":0},{"control":2}]},{"kind":"Out","inputs":[{"control":0},{"ugen":1}]}]}"#;
const VGAIN: &str = r#"{"name":"vgain","controls":[{"name":"in","default":0.0},{"name":"gain","default":0.3}],"ugens":[{"kind":"In","inputs":[{"control":0}]},{"kind":"Mul","inputs":[{"ugen":0},{"control":1}]},{"kind":"Out","inputs":[{"const":0.0},{"ugen":0}]}]}"#;
const POLY: &str = r#"{"name":"poly","buses":[{"name":"mix","rate":"audio"}],"members":[{"def":"vgain","controls":{"in":"mix"}},{"def":"vtone","controls":{"out":"mix"},"voice":true}],"surface":{"gain":[{"member":0,"control":"gain"}],"freq":[{"member":1,"control":"freq"}],"amp":[{"member":1,"control":"level"}]},"defaults":{"gain":0.3,"amp":0.2}}"#;

fn s(v: &str) -> OscType {
    OscType::String(v.into())
}

/// Session 1 loads two synthdefs + a GraphDef and binds it to MIDI; session 2
/// boots from the same dir and the binding's shared instance is restored.
#[test]
fn a_graphdef_midi_binding_survives_a_restart() {
    let dir = TempDir::new("bind");

    let a = TestServer::spawn(dir.path());
    a.send("/d_recv", vec![s(VTONE)]);
    a.recv_until("/done");
    a.send("/d_recv", vec![s(VGAIN)]);
    a.recv_until("/done");
    a.send("/d_graph", vec![s(POLY)]);
    a.recv_until("/done");
    a.send("/midi_bind", vec![OscType::Int(0), s("poly")]);
    a.sync();
    // The binding spawned the shared instance now (one group at root).
    assert_eq!(a.root_child_count(), 1);
    a.quit();

    // The binding was persisted.
    assert!(dir.path().join("midi.json").exists());

    // Session 2: defs + GraphDef + binding reload at boot, re-instantiating the
    // shared instance — without any client sending a single command.
    let b = TestServer::spawn(dir.path());
    b.sync();
    assert_eq!(
        b.root_child_count(),
        1,
        "the restored binding's instance is missing"
    );
    b.quit();
}

/// A boot preset (`boot.json`) instantiates a standalone GraphDef at startup.
#[test]
fn a_boot_preset_instantiates_a_standalone_graph() {
    let dir = TempDir::new("boot");

    // Persist the member defs + a voice-capable GraphDef.
    {
        let a = TestServer::spawn(dir.path());
        a.send("/d_recv", vec![s(VTONE)]);
        a.recv_until("/done");
        a.send("/d_recv", vec![s(VGAIN)]);
        a.recv_until("/done");
        a.send("/d_graph", vec![s(POLY)]);
        a.recv_until("/done");
        a.quit();
    }
    // Author a boot preset naming that graph.
    std::fs::write(
        dir.path().join("boot.json"),
        br#"[{"graph":"poly","ports":{"gain":0.5}}]"#,
    )
    .unwrap();

    let b = TestServer::spawn(dir.path());
    b.sync();
    assert_eq!(
        b.root_child_count(),
        1,
        "the boot graph was not instantiated"
    );
    b.quit();
}
