//! M6 tests: sample-accurate scheduling of timed bundles. Engine-level
//! tests assert exactness to the sample (DC signals make the edges visible);
//! the OSC test covers NTP timetag → sample conversion against a live
//! server with a manually ticked engine.

use std::sync::Arc;
use std::time::Duration;

use clausters::node::{AddAction, ROOT_NODE_ID, SynthNode};
use clausters::server::engine::{BLOCK_SIZE, Cmd, Engine, engine_pair};
use clausters::synthdef::SynthDefSpec;
use clausters::synthdef::instance::UGenSynth;
use serde_json::json;

const SR: f32 = 48_000.0;
const CHANNELS: usize = 2;

/// DC level to bus 0, driven by the `level` control: scheduling edits show
/// up as exact steps in the output.
fn dc_synth(level: f32) -> Box<dyn SynthNode> {
    let spec: SynthDefSpec = serde_json::from_value(json!({
        "name": "dc",
        "controls": [{"name": "level", "default": 1.0}],
        "ugens": [
            {"kind": "Out", "inputs": [{"const": 0.0}, {"control": 0}]}
        ]
    }))
    .unwrap();
    let mut synth = Box::new(UGenSynth::new(Arc::new(
        clausters::synthdef::compile(spec).unwrap(),
    )));
    synth.set_control(0, level);
    synth
}

fn add_dc(id: i32, level: f32) -> Cmd {
    Cmd::AddSynth {
        id,
        target: ROOT_NODE_ID,
        action: AddAction::Tail,
        synth: dc_synth(level),
        usage: Default::default(),
    }
}

/// `Impulse.ar(freq)` straight to bus 0: a single-sample 1.0 on its first
/// frame, then the train (or, with freq 0, silence forever).
fn add_impulse(id: i32, freq: f32) -> Cmd {
    let spec: SynthDefSpec = serde_json::from_value(json!({
        "name": "imp",
        "ugens": [
            {"kind": "Impulse", "inputs": [{"const": freq}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    }))
    .unwrap();
    let synth = Box::new(UGenSynth::new(Arc::new(
        clausters::synthdef::compile(spec).unwrap(),
    )));
    Cmd::AddSynth {
        id,
        target: ROOT_NODE_ID,
        action: AddAction::Tail,
        synth,
        usage: Default::default(),
    }
}

fn at(time: u64, cmds: Vec<Cmd>) -> Cmd {
    Cmd::Schedule { time, cmds }
}

/// Renders `blocks` blocks and returns channel 0.
fn render(engine: &mut Engine, blocks: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; BLOCK_SIZE * CHANNELS];
    let mut buf = Vec::with_capacity(blocks * BLOCK_SIZE);
    for _ in 0..blocks {
        engine.process_block(&mut out);
        buf.extend(out.iter().step_by(CHANNELS).copied());
    }
    buf
}

#[test]
fn bundle_fires_at_the_exact_sample_mid_block() {
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    // Sample 100 = block 1, offset 36: the engine must split that block.
    handle.send(at(100, vec![add_dc(1000, 1.0)])).ok().unwrap();

    let left = render(&mut engine, 4);
    for (i, s) in left.iter().enumerate() {
        let expected = if i < 100 { 0.0 } else { 1.0 };
        assert_eq!(*s, expected, "sample {i}");
    }
    // The spent bundle shell comes back through the garbage FIFO.
    assert_eq!(handle.collect_garbage(), 1);
}

#[test]
fn several_events_split_the_same_block() {
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle.send(at(10, vec![add_dc(1000, 1.0)])).ok().unwrap();
    handle
        .send(at(
            30,
            vec![Cmd::SetControl {
                id: 1000,
                index: 0,
                value: 0.5,
            }],
        ))
        .ok()
        .unwrap();
    handle
        .send(at(50, vec![Cmd::FreeNode { id: 1000 }]))
        .ok()
        .unwrap();

    let left = render(&mut engine, 2);
    for (i, s) in left.iter().enumerate() {
        let expected = match i {
            0..10 => 0.0,
            10..30 => 1.0,
            30..50 => 0.5,
            _ => 0.0,
        };
        assert_eq!(*s, expected, "sample {i}");
    }
}

#[test]
fn one_bundle_executes_its_commands_atomically() {
    // Two synths in one bundle: both appear at the same sample, summing.
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(at(70, vec![add_dc(1000, 0.25), add_dc(1001, 0.5)]))
        .ok()
        .unwrap();

    let left = render(&mut engine, 2);
    for (i, s) in left.iter().enumerate() {
        let expected = if i < 70 { 0.0 } else { 0.75 };
        assert_eq!(*s, expected, "sample {i}");
    }
}

#[test]
fn bundles_at_the_same_time_run_in_send_order() {
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle.send(add_dc(1000, 1.0)).ok().unwrap();
    handle
        .send(at(
            100,
            vec![Cmd::SetControl {
                id: 1000,
                index: 0,
                value: 0.3,
            }],
        ))
        .ok()
        .unwrap();
    handle
        .send(at(
            100,
            vec![Cmd::SetControl {
                id: 1000,
                index: 0,
                value: 0.7,
            }],
        ))
        .ok()
        .unwrap();

    let left = render(&mut engine, 3);
    assert_eq!(left[99], 1.0);
    assert_eq!(left[100], 0.7, "the later send must win the tie");
}

#[test]
fn earlier_times_fire_first_regardless_of_send_order() {
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle.send(at(40, vec![add_dc(1001, 0.5)])).ok().unwrap();
    handle.send(at(20, vec![add_dc(1000, 0.25)])).ok().unwrap();

    let left = render(&mut engine, 1);
    for (i, s) in left.iter().enumerate() {
        let expected = match i {
            0..20 => 0.0,
            20..40 => 0.25,
            _ => 0.75,
        };
        assert_eq!(*s, expected, "sample {i}");
    }
}

#[test]
fn scheduled_impulse_lands_on_its_exact_sample() {
    // The example's mechanism: a `/sched`'d Impulse(0) splits the block at
    // the target and fires its single impulse on that exact frame — unlike
    // SinOsc (which starts at sin(0) = 0), the marked sample itself is 1.0.
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    let target = 100u64; // mid-block: block 1, offset 36
    handle
        .send(at(target, vec![add_impulse(1000, 0.0)]))
        .ok()
        .unwrap();

    let left = render(&mut engine, 4);
    for (i, s) in left.iter().enumerate() {
        let expected = if i as u64 == target { 1.0 } else { 0.0 };
        assert_eq!(*s, expected, "sample {i}");
    }
}

#[test]
fn impulse_train_is_periodic_to_the_sample() {
    // freq = SR / 64 → an impulse exactly every 64 samples, the first on the
    // synth's first frame; the f64 phase keeps it drift-free.
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle.send(add_impulse(1000, SR / 64.0)).ok().unwrap();

    let left = render(&mut engine, 4);
    for (i, s) in left.iter().enumerate() {
        let expected = if i % 64 == 0 { 1.0 } else { 0.0 };
        assert_eq!(*s, expected, "sample {i}");
    }
}

#[test]
fn late_bundles_execute_at_the_start_of_the_next_block() {
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    render(&mut engine, 2); // now = 128
    handle.send(at(50, vec![add_dc(1000, 1.0)])).ok().unwrap();

    let left = render(&mut engine, 1);
    assert_eq!(left[0], 1.0, "late bundle must apply at offset 0");
}

#[test]
fn scheduled_control_bus_write_lands_on_its_sample() {
    // InCtl reads control bus 7 each slice: the scheduled /c_set becomes a
    // mid-block step.
    let spec: SynthDefSpec = serde_json::from_value(json!({
        "name": "ctlreader",
        "ugens": [
            {"kind": "InCtl", "inputs": [{"const": 7.0}]},
            {"kind": "Out",   "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    }))
    .unwrap();
    let synth = Box::new(UGenSynth::new(Arc::new(
        clausters::synthdef::compile(spec).unwrap(),
    )));
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(Cmd::AddSynth {
            id: 1000,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            synth,
            usage: Default::default(),
        })
        .ok()
        .unwrap();
    handle
        .send(at(
            32,
            vec![Cmd::SetControlBus {
                index: 7,
                value: 0.9,
            }],
        ))
        .ok()
        .unwrap();

    let left = render(&mut engine, 1);
    for (i, s) in left.iter().enumerate() {
        let expected = if i < 32 { 0.0 } else { 0.9 };
        assert_eq!(*s, expected, "sample {i}");
    }
}

#[test]
fn far_future_bundles_wait_in_the_queue() {
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(at(BLOCK_SIZE as u64 * 10, vec![add_dc(1000, 1.0)]))
        .ok()
        .unwrap();

    let left = render(&mut engine, 12);
    let start = BLOCK_SIZE * 10;
    assert!(left[..start].iter().all(|s| *s == 0.0));
    assert!(left[start..].iter().all(|s| *s == 1.0));
}

#[test]
fn a_full_schedule_queue_rejects_extra_bundles() {
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    // The queue holds 1024; the command FIFO also holds 1024, so feed it in
    // two rounds with a tick in between.
    let far = 1_000_000u64;
    for i in 0..512 {
        handle.send(at(far + i, vec![])).ok().unwrap();
    }
    render(&mut engine, 1);
    for i in 512..1024 {
        handle.send(at(far + i, vec![])).ok().unwrap();
    }
    render(&mut engine, 1);
    assert_eq!(handle.collect_garbage(), 0, "queue holds exactly 1024");

    handle
        .send(at(far + 1024, vec![add_dc(1000, 1.0)]))
        .ok()
        .unwrap();
    render(&mut engine, 1);
    // The rejected bundle comes back whole (synth still inside) and is
    // dropped here.
    assert_eq!(handle.collect_garbage(), 1);
}

/// A Faust synth split mid-block: `compute` must handle partial frame
/// counts. A constant-output def makes the scheduled edge sample-exact.
#[cfg(feature = "faust")]
#[test]
fn faust_synths_survive_block_splits() {
    use clausters::faust::compiler::{CompilePayload, CompileRequest, CompilerThread};
    use clausters::faust::synth::FaustSynth;

    let compiler = CompilerThread::spawn();
    compiler
        .submit(CompileRequest {
            name: "fdc".into(),
            payload: CompilePayload::Source("process = 0.8;".into()),
            client: Some(clausters::osc::ClientId::Udp(
                "127.0.0.1:1".parse().unwrap(),
            )),
            cache: None,
        })
        .ok()
        .unwrap();
    let def = Arc::new(
        compiler
            .recv_result_timeout(Duration::from_secs(10))
            .expect("compilation must finish")
            .outcome
            .expect("def must compile"),
    );

    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    let synth = Box::new(FaustSynth::new(Arc::clone(&def), SR).unwrap());
    handle
        .send(at(
            100,
            vec![Cmd::AddSynth {
                id: 1000,
                target: ROOT_NODE_ID,
                action: AddAction::Tail,
                synth,
                usage: Default::default(),
            }],
        ))
        .ok()
        .unwrap();
    handle
        .send(at(200, vec![Cmd::FreeNode { id: 1000 }]))
        .ok()
        .unwrap();

    let left = render(&mut engine, 4);
    for (i, s) in left.iter().enumerate() {
        let expected = if (100..200).contains(&i) { 0.8 } else { 0.0 };
        assert_eq!(*s, expected, "sample {i}");
    }
}

// ---- OSC round trip: NTP timetags against a live server ----

mod osc {
    use super::*;
    use std::net::UdpSocket;

    use clausters::osc::server::{OscServer, ServerInfo};
    use clausters::rosc::{OscBundle, OscMessage, OscPacket, OscTime, OscType, decoder, encoder};

    const NTP_UNIX_OFFSET: f64 = 2_208_988_800.0;

    fn ntp_in(seconds_ahead: f64) -> OscTime {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let target = now + seconds_ahead + NTP_UNIX_OFFSET;
        OscTime {
            seconds: target as u32,
            fractional: (target.fract() * 2f64.powi(32)) as u32,
        }
    }

    #[test]
    fn timed_bundles_over_osc_fire_by_the_stream_clock() {
        let (mut engine, engine_handle) = engine_pair(SR, CHANNELS);
        let info = ServerInfo {
            nominal_sample_rate: SR as f64,
            actual_sample_rate: SR as f64,
        };
        let mut server = OscServer::bind(("127.0.0.1", 0), info, engine_handle).unwrap();
        let addr = server.local_addr().unwrap();
        let server_thread = std::thread::spawn(move || server.run());
        let client = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();

        let send_bundle = |timetag: OscTime, content: Vec<OscPacket>| {
            let packet = OscPacket::Bundle(OscBundle { timetag, content });
            client
                .send_to(&encoder::encode(&packet).unwrap(), addr)
                .unwrap();
        };
        let s_new = OscPacket::Message(OscMessage {
            addr: "/s_new".into(),
            args: vec![
                OscType::String("default".into()),
                OscType::Int(1000),
                OscType::Int(1),
                OscType::Int(0),
            ],
        });

        // Scheduled 0.25 s ahead while the published clock reads 0: the
        // target lands near sample 12000. We tick much faster than wall
        // time, so the sound must start in that neighborhood — well after
        // the first blocks, well before the 24000 mark.
        send_bundle(ntp_in(0.25), vec![s_new.clone()]);
        std::thread::sleep(Duration::from_millis(200)); // let the server parse it
        let left = render(&mut engine, 375); // 24000 samples
        let first = left.iter().position(|s| *s != 0.0);
        let first = first.expect("the scheduled synth must have started");
        assert!(
            (1000..13000).contains(&first),
            "default synth started at sample {first}, expected ≈ 12000"
        );

        // An immediate-tag bundle plays right away.
        send_bundle(
            OscTime {
                seconds: 0,
                fractional: 1,
            },
            vec![OscPacket::Message(OscMessage {
                addr: "/n_free".into(),
                args: vec![OscType::Int(1000)],
            })],
        );
        let mut silent = false;
        for _ in 0..100 {
            silent = render(&mut engine, 2).iter().all(|s| *s == 0.0);
            if silent {
                break;
            }
        }
        assert!(silent, "immediate bundle must free the synth");

        // Non-schedulable commands in a timed bundle reply /fail.
        send_bundle(
            ntp_in(0.5),
            vec![OscPacket::Message(OscMessage {
                addr: "/quit".into(),
                args: vec![],
            })],
        );
        let mut buf = [0u8; 65536];
        let (len, _) = client.recv_from(&mut buf).expect("expected /fail");
        let (_, OscPacket::Message(reply)) = decoder::decode_udp(&buf[..len]).unwrap() else {
            panic!("expected a message reply");
        };
        assert_eq!(reply.addr, "/fail");

        // Shut down (immediate message, not in a bundle).
        let quit = OscPacket::Message(OscMessage {
            addr: "/quit".into(),
            args: vec![],
        });
        client
            .send_to(&encoder::encode(&quit).unwrap(), addr)
            .unwrap();
        server_thread.join().unwrap().unwrap();
    }

    /// M8: `/sched` carries an *absolute* sample target, so unlike the NTP
    /// test above there is no wall-clock neighborhood to allow for — the
    /// note must start on that exact frame. This precision is the point of
    /// scheduling on the sample clock.
    #[test]
    fn sched_message_fires_at_the_exact_sample() {
        let (mut engine, engine_handle) = engine_pair(SR, CHANNELS);
        let info = ServerInfo {
            nominal_sample_rate: SR as f64,
            actual_sample_rate: SR as f64,
        };
        let mut server = OscServer::bind(("127.0.0.1", 0), info, engine_handle).unwrap();
        let addr = server.local_addr().unwrap();
        let server_thread = std::thread::spawn(move || server.run());
        let client = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();

        let target: i64 = 5_025; // deliberately mid-block (5025 % 64 != 0)
        let s_new = OscPacket::Message(OscMessage {
            addr: "/s_new".into(),
            args: vec![
                OscType::String("default".into()),
                OscType::Int(1000),
                OscType::Int(1),
                OscType::Int(0),
            ],
        });
        let sched = OscPacket::Message(OscMessage {
            addr: "/sched".into(),
            args: vec![
                OscType::Long(target),
                OscType::Blob(encoder::encode(&s_new).unwrap()),
            ],
        });
        client
            .send_to(&encoder::encode(&sched).unwrap(), addr)
            .unwrap();
        // Let the server thread parse and push the command; the engine is
        // not ticking, so unlike NTP there is no clock racing against us.
        std::thread::sleep(Duration::from_millis(200));

        let left = render(&mut engine, 100); // 6400 samples
        let first = left.iter().position(|s| *s != 0.0);
        assert_eq!(
            first,
            Some(target as usize + 1),
            "first audible sample (the target frame itself is sin(0) = 0)"
        );

        let quit = OscPacket::Message(OscMessage {
            addr: "/quit".into(),
            args: vec![],
        });
        client
            .send_to(&encoder::encode(&quit).unwrap(), addr)
            .unwrap();
        server_thread.join().unwrap().unwrap();
    }
}
