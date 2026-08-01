//! Def persistence: SynthDefs and (with the `faust` feature) FaustDefs are
//! saved to a data directory and reloaded when the server restarts. The core
//! tests cover the [`clausters::server::defstore`] layout; the `faust` module
//! covers the bitcode cache and the end-to-end reload across two server
//! instances on one data directory.

#![cfg(feature = "synth")]

use std::path::PathBuf;

use clausters::server::defstore::{DefKind, DefStore, resolve_data_dir, sanitize_name};

/// A unique temp directory per test, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "clausters-persist-{}-{tag}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn cli_override_wins_data_dir() {
    assert_eq!(
        resolve_data_dir(Some("/tmp/explicit")),
        Some(PathBuf::from("/tmp/explicit"))
    );
}

#[test]
fn sanitize_keeps_safe_chars_and_escapes_the_rest() {
    assert_eq!(sanitize_name("my-Def_1.0"), "my-Def_1.0");
    // A slash would escape the directory; spaces and slashes are encoded.
    assert_eq!(sanitize_name("a/b c"), "a%2Fb%20c");
}

#[test]
fn synthdef_specs_round_trip_through_disk() {
    let dir = TempDir::new("synthdef");
    let store = DefStore::open(dir.path()).unwrap();

    let spec = br#"{"name":"foo","ugens":[]}"#;
    store.save_synthdef("foo", spec).unwrap();

    let loaded = store.load_synthdef_specs();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0], spec);

    store.remove_synthdef("foo");
    assert!(store.load_synthdef_specs().is_empty());
}

// ---- M19: persisted MIDI bindings + boot preset ----

#[test]
fn midi_bindings_round_trip_through_disk() {
    use clausters::osc::translate::CmdTranslator;
    use clausters::rosc::{OscMessage, OscType};

    let dir = TempDir::new("bindings");
    let store = DefStore::open(dir.path()).unwrap();
    assert!(store.load_bindings().is_empty()); // absent file -> empty

    // Build a binding (channel 3 -> default, with a cc->amp map) and persist it.
    let mut t = CmdTranslator::new(48_000.0);
    let mut cmds = Vec::new();
    let bind = OscMessage {
        addr: "/midi_bind".into(),
        args: vec![OscType::Int(3), OscType::String("default".into())],
    };
    t.translate(&bind, &mut cmds).unwrap();
    let map = OscMessage {
        addr: "/midi_map".into(),
        args: vec![
            OscType::Int(3),
            OscType::String("cc7".into()),
            OscType::String("amp".into()),
        ],
    };
    t.translate(&map, &mut cmds).unwrap();

    store.save_bindings(&t.midi.persist()).unwrap();
    assert!(dir.path().join("midi.json").exists());

    let loaded = store.load_bindings();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].channel, 3);
    assert_eq!(loaded[0].binding.instrument, "default");
    assert_eq!(
        loaded[0].binding.cc.get(&7).map(String::as_str),
        Some("amp")
    );
}

#[test]
fn boot_preset_loads_when_present() {
    use clausters::osc::graphdef::BootInstance;

    let dir = TempDir::new("boot");
    let store = DefStore::open(dir.path()).unwrap();
    assert!(store.load_boot().is_empty()); // absent -> empty

    std::fs::write(
        dir.path().join("boot.json"),
        br#"[{"graph":"reverb","ports":{"mix":0.3}}]"#,
    )
    .unwrap();
    let boot: Vec<BootInstance> = store.load_boot();
    assert_eq!(boot.len(), 1);
    assert_eq!(boot[0].graph, "reverb");
    assert_eq!(boot[0].ports.get("mix"), Some(&0.3));
}

#[test]
fn the_tmp_prefix_marks_a_def_ephemeral() {
    use clausters::server::defstore::is_ephemeral;

    // What `clausters.defs.as_def` generates for an expression nobody named.
    assert!(is_ephemeral("tmp_synthdef_9f2a1c40be71"));
    assert!(is_ephemeral("tmp_faustdef_9f2a1c40be71"));
    // Anything a user would name is not.
    assert!(!is_ephemeral("bass"));
    assert!(!is_ephemeral("my_tmp_def"));
}

#[test]
fn an_ephemeral_defs_artifacts_stay_out_of_the_data_dir() {
    use clausters::server::defstore::ephemeral_dir;

    // Whatever an ephemeral def must write goes under the OS temp directory,
    // never the store that outlives the process.
    let tmp = ephemeral_dir();
    assert!(tmp.starts_with(std::env::temp_dir()));
    let dir = TempDir::new("ephemeral");
    assert!(!tmp.starts_with(dir.path()));
}

#[test]
fn a_name_belongs_to_one_kind_and_the_last_def_wins() {
    let dir = TempDir::new("crosskind");
    let store = DefStore::open(dir.path()).unwrap();

    // A SynthDef and a GraphDef claim the same name, in that order.
    store
        .save_synthdef("clash", br#"{"name":"clash","ugens":[]}"#)
        .unwrap();
    store
        .save_graphdef("clash", br#"{"name":"clash","members":[]}"#)
        .unwrap();
    // Both on disk is exactly the state that lets a reload answer with the
    // wrong one, so claiming the name for the graph frees it everywhere else.
    store.remove_other_kinds("clash", DefKind::Graph);

    assert!(
        store.load_synthdef_specs().is_empty(),
        "the SynthDef lost the name"
    );
    assert_eq!(store.load_graphdef_specs().len(), 1, "the GraphDef kept it");
}

#[cfg(feature = "faust")]
mod faust {
    use super::TempDir;
    use std::net::{SocketAddr, UdpSocket};
    use std::time::Duration;

    use clausters::faust::cache;
    use clausters::faust::compiler::{self, CompilePayload};
    use clausters::faust::ffi;
    use clausters::faust::synth::FaustDef;
    use clausters::osc::server::{OscServer, ServerInfo};
    use clausters::rosc::{OscMessage, OscPacket, OscType, decoder, encoder};
    use clausters::server::defstore::DefStore;
    use clausters::server::engine::engine_pair;

    const SR: f32 = 48_000.0;
    const BLOCK: usize = 64;
    const DEADLINE: Duration = Duration::from_secs(10);

    /// A fixed-frequency sine: 0 inputs, 1 output, fully deterministic, so two
    /// instances of the same compiled DSP are sample-identical.
    const SINE: &str = r#"
        wrap(x) = x - floor(x);
        process = sin(6.283185307179586 * ((+(440.0/48000.0) : wrap) ~ _)) * 0.2;
    "#;

    fn payload() -> CompilePayload {
        CompilePayload::Source(SINE.into())
    }

    /// Renders a 0-in/1-out def, audio-thread style (see faust_json.rs).
    fn render_mono(def: &FaustDef, blocks: usize) -> Vec<f32> {
        let dsp = unsafe { ffi::createCDSPInstance(def.factory().as_ptr()) };
        assert!(!dsp.is_null());
        unsafe { ffi::initCDSPInstance(dsp, SR as i32) };
        let mut block = [0.0f32; BLOCK];
        let mut out = Vec::with_capacity(blocks * BLOCK);
        for _ in 0..blocks {
            let mut outputs: [*mut f32; 1] = [block.as_mut_ptr()];
            unsafe {
                ffi::computeCDSPInstance(
                    dsp,
                    BLOCK as i32,
                    std::ptr::null_mut(),
                    outputs.as_mut_ptr(),
                )
            };
            out.extend_from_slice(&block);
        }
        unsafe { ffi::deleteCDSPInstance(dsp) };
        out
    }

    #[test]
    fn bitcode_round_trip_is_sample_identical() {
        let dir = TempDir::new("bc");
        let original = compiler::compile("sine", &payload()).expect("compile");
        let bc = dir.path().join("sine.bc");
        assert!(
            cache::write_bitcode(original.factory(), &bc),
            "write_bitcode failed"
        );

        let restored = FaustDef::probe(cache::read_bitcode(&bc).expect("read_bitcode"))
            .expect("probe restored factory");
        assert_eq!(restored.num_inputs, original.num_inputs);
        assert_eq!(restored.num_outputs, original.num_outputs);

        // Same compiled DSP → bit-for-bit identical output.
        assert_eq!(render_mono(&original, 8), render_mono(&restored, 8));
    }

    #[test]
    fn record_persists_and_restores_from_cache() {
        let dir = TempDir::new("rec");
        let def = compiler::compile("sine", &payload()).expect("compile");
        cache::persist(def.factory(), "sine", &payload(), dir.path());

        let records = cache::load_records(dir.path());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "sine");

        // The bitcode is there and restores into a working def.
        let restored = FaustDef::probe(
            cache::try_restore(&records[0], dir.path()).expect("restore from cache"),
        )
        .expect("probe");
        assert_eq!(render_mono(&def, 4), render_mono(&restored, 4));
    }

    #[test]
    fn restore_rejects_version_mismatch() {
        let dir = TempDir::new("ver");
        let def = compiler::compile("sine", &payload()).expect("compile");
        cache::persist(def.factory(), "sine", &payload(), dir.path());

        let mut record = cache::load_records(dir.path()).pop().unwrap();
        record.faust_version = "0.0.0-stale".into();
        assert!(
            cache::try_restore(&record, dir.path()).is_err(),
            "a version mismatch must invalidate the cache"
        );
    }

    #[test]
    fn restore_falls_back_on_corrupt_bitcode() {
        let dir = TempDir::new("corrupt");
        let def = compiler::compile("sine", &payload()).expect("compile");
        cache::persist(def.factory(), "sine", &payload(), dir.path());

        // Truncate every .bc to garbage.
        for entry in std::fs::read_dir(dir.path()).unwrap().flatten() {
            if entry.path().extension().is_some_and(|e| e == "bc") {
                std::fs::write(entry.path(), b"not bitcode").unwrap();
            }
        }
        let record = cache::load_records(dir.path()).pop().unwrap();
        assert!(
            cache::try_restore(&record, dir.path()).is_err(),
            "corrupt bitcode must fail so the caller recompiles"
        );
    }

    // ---- end-to-end reload across two server instances ----

    struct TestServer {
        addr: SocketAddr,
        handle: std::thread::JoinHandle<std::io::Result<()>>,
        client: UdpSocket,
    }

    impl TestServer {
        /// Binds a server with persistence rooted at `data_dir`, reloading
        /// whatever it already holds, and runs it on its own thread.
        fn spawn(data_dir: &std::path::Path) -> Self {
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

        /// `/server_status.reply` def count (arg index 4).
        fn def_count(&self) -> i32 {
            self.send("/server_status", vec![]);
            let status = self.recv_until("/server_status.reply");
            match status.args[4] {
                OscType::Int(n) => n,
                ref other => panic!("unexpected def count arg: {other:?}"),
            }
        }

        /// Polls `/server_status` until the def count reaches `want` (defs reload
        /// incrementally on the compiler thread) or the deadline passes.
        fn wait_for_def_count(&self, want: i32) {
            let start = std::time::Instant::now();
            while start.elapsed() < DEADLINE {
                if self.def_count() == want {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            panic!("def count never reached {want} (last {})", self.def_count());
        }

        fn quit(self) {
            self.send("/server_quit", vec![]);
            self.recv_until("/done");
            self.handle.join().unwrap().unwrap();
        }
    }

    #[test]
    fn faust_def_survives_a_restart() {
        let dir = TempDir::new("e2e");

        // Session 1: define a Faust def; it gets persisted.
        let a = TestServer::spawn(dir.path());
        a.send(
            "/def_send",
            vec![
                OscType::String("faust".into()),
                OscType::String("psine".into()),
                OscType::String(SINE.into()),
            ],
        );
        let done = a.recv_until("/done");
        assert_eq!(done.args[1], OscType::String("faust".into()));
        assert_eq!(done.args[2], OscType::String("psine".into()));
        a.quit();

        // The record and a bitcode file landed in defs/faustdefs/.
        let faustdefs = dir.path().join("defs").join("faustdefs");
        assert!(faustdefs.join("psine.json").exists(), "record missing");
        let bc = std::fs::read_dir(&faustdefs)
            .unwrap()
            .flatten()
            .any(|e| e.path().extension().is_some_and(|x| x == "bc"));
        assert!(bc, "bitcode missing");

        // Session 2: the def reloads without re-sending it (built-in default
        // + psine = 2).
        let b = TestServer::spawn(dir.path());
        b.wait_for_def_count(2);
        b.quit();
    }

    #[test]
    fn d_free_deletes_persisted_files() {
        let dir = TempDir::new("free");
        let s = TestServer::spawn(dir.path());
        s.send(
            "/def_send",
            vec![
                OscType::String("faust".into()),
                OscType::String("gone".into()),
                OscType::String(SINE.into()),
            ],
        );
        s.recv_until("/done");

        let faustdefs = dir.path().join("defs").join("faustdefs");
        assert!(faustdefs.join("gone.json").exists());

        s.send("/def_free", vec![OscType::String("gone".into())]);
        // /def_free has no reply; a following round-trip flushes it.
        let _ = s.def_count();
        assert!(
            !faustdefs.join("gone.json").exists(),
            "record should be deleted"
        );
        let any_bc = std::fs::read_dir(&faustdefs)
            .unwrap()
            .flatten()
            .any(|e| e.path().extension().is_some_and(|x| x == "bc"));
        assert!(!any_bc, "bitcode should be deleted");
        s.quit();
    }
}
