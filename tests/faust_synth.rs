//! F3 tests: `FaustSynth` in the node tree. Gated behind the `faust`
//! feature: `cargo test --features faust --test faust_synth`.
//!
//! Engine-level tests drive the same command FIFO the network thread uses
//! and listen with signal asserts; the OSC test at the end covers the whole
//! `/d_faust` → `/s_new` → `/n_set` → `/n_free` round trip with a manually
//! ticked engine.

#![cfg(feature = "faust")]

use std::sync::Arc;
use std::time::Duration;

use clausters::faust::compiler::{CompilePayload, CompileRequest, CompilerThread};
use clausters::faust::synth::{FaustDef, FaustSynth};
use clausters::node::{AddAction, ROOT_NODE_ID, SynthNode};
use clausters::server::engine::{BLOCK_SIZE, Cmd, Engine, engine_pair};

const SR: f32 = 48_000.0;
const CHANNELS: usize = 2;
const COMPILE_DEADLINE: Duration = Duration::from_secs(10);

/// Sine with named controls, stdlib-free (SR baked to 48 kHz like the
/// engine's).
const SINE_SRC: &str = r#"
wrap(x) = x - floor(x);
freq = hslider("freq", 440.0, 20.0, 20000.0, 0.01);
amp = hslider("amp", 0.2, 0.0, 1.0, 0.001);
phasor = (+(freq/48000.0) : wrap) ~ _;
process = sin(6.283185307179586 * phasor) * amp;
"#;

/// 1-in/1-out gain stage: exercises the input-bus mapping.
const GAIN_SRC: &str = r#"process = _ * hslider("gain", 0.5, 0.0, 1.0, 0.001);"#;

fn compile_def(name: &str, src: &str) -> Arc<FaustDef> {
    let compiler = CompilerThread::spawn();
    compiler
        .submit(CompileRequest {
            name: name.into(),
            payload: CompilePayload::Source(src.into()),
            client: "127.0.0.1:1".parse().unwrap(),
        })
        .ok()
        .unwrap();
    let result = compiler
        .recv_result_timeout(COMPILE_DEADLINE)
        .expect("compilation must finish");
    Arc::new(result.outcome.expect("def must compile"))
}

/// Builds the instance on this (network-style) thread, with named controls
/// resolved exactly like `/s_new` does.
fn add_faust(id: i32, def: &Arc<FaustDef>, controls: &[(&str, f32)]) -> Cmd {
    let mut synth = Box::new(FaustSynth::new(Arc::clone(def), SR).expect("instantiation"));
    for (name, value) in controls {
        let index = def.control_index(name).expect("control must exist");
        synth.set_control(index, *value);
    }
    Cmd::AddSynth {
        id,
        target: ROOT_NODE_ID,
        action: AddAction::Tail,
        synth,
        usage: Default::default(),
    }
}

/// Renders `blocks` blocks and returns the chosen interleaved channel.
fn render_channel(engine: &mut Engine, blocks: usize, channel: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; BLOCK_SIZE * CHANNELS];
    let mut buf = Vec::with_capacity(blocks * BLOCK_SIZE);
    for _ in 0..blocks {
        engine.process_block(&mut out);
        buf.extend(out.iter().skip(channel).step_by(CHANNELS).copied());
    }
    buf
}

fn rms(buf: &[f32]) -> f32 {
    (buf.iter().map(|x| x * x).sum::<f32>() / buf.len() as f32).sqrt()
}

fn estimated_freq(buf: &[f32]) -> f32 {
    let crossings = buf
        .windows(2)
        .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
        .count();
    crossings as f32 * SR / buf.len() as f32
}

#[test]
fn probed_def_exposes_params_and_reserved_bus_controls() {
    let def = compile_def("fsine", SINE_SRC);
    assert_eq!(def.num_inputs, 0);
    assert_eq!(def.num_outputs, 1);
    let names: Vec<_> = def.params.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"freq") && names.contains(&"amp"), "{names:?}");
    assert_eq!(def.control_index("out"), Some(def.params.len() as u32));
    assert_eq!(def.control_index("in"), Some(def.params.len() as u32 + 1));
    assert_eq!(def.control_index("nope"), None);
    let freq = &def.params[def.control_index("freq").unwrap() as usize];
    assert_eq!(freq.init, 440.0);
    assert_eq!((freq.min, freq.max), (20.0, 20_000.0));
}

#[test]
fn faust_synth_plays_in_the_tree() {
    let def = compile_def("fsine", SINE_SRC);
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle.send(add_faust(1000, &def, &[])).ok().unwrap();

    let left = render_channel(&mut engine, 750, 0); // exactly 1 s
    assert!(left.iter().all(|x| x.is_finite()));
    let freq = estimated_freq(&left);
    assert!((freq - 440.0).abs() < 5.0, "estimated freq = {freq}");
    let expected_rms = 0.2 * std::f32::consts::FRAC_1_SQRT_2;
    assert!(
        (rms(&left) - expected_rms).abs() < 0.005,
        "rms = {}, expected ≈ {expected_rms}",
        rms(&left)
    );
}

#[test]
fn set_control_writes_the_named_zone() {
    let def = compile_def("fsine", SINE_SRC);
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(add_faust(1000, &def, &[("freq", 660.0)]))
        .ok()
        .unwrap();
    render_channel(&mut engine, 100, 0); // warmup at 660

    handle
        .send(Cmd::SetControl {
            id: 1000,
            index: def.control_index("freq").unwrap(),
            value: 880.0,
        })
        .ok()
        .unwrap();
    let left = render_channel(&mut engine, 750, 0);
    let freq = estimated_freq(&left);
    assert!((freq - 880.0).abs() < 8.0, "estimated freq = {freq}");
}

#[test]
fn out_control_routes_to_another_bus() {
    let def = compile_def("fsine", SINE_SRC);
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(add_faust(1000, &def, &[("out", 1.0)]))
        .ok()
        .unwrap();

    let left = render_channel(&mut engine, 200, 0);
    assert!(rms(&left) < 1e-9, "bus 0 must stay clean");
    let (mut engine2, mut handle2) = engine_pair(SR, CHANNELS);
    handle2
        .send(add_faust(1000, &def, &[("out", 1.0)]))
        .ok()
        .unwrap();
    let right = render_channel(&mut engine2, 200, 1);
    assert!(rms(&right) > 0.1, "bus 1 must carry the sine");
}

#[test]
fn input_buses_feed_faust_synths() {
    // Sine onto private bus 4, then a Faust gain stage 4 → 0. Tree order
    // (both at the tail, gain added second) makes the chain causal within
    // the block.
    let sine = compile_def("fsine", SINE_SRC);
    let gain = compile_def("fgain", GAIN_SRC);
    assert_eq!(gain.num_inputs, 1);
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(add_faust(1000, &sine, &[("out", 4.0)]))
        .ok()
        .unwrap();
    handle
        .send(add_faust(1001, &gain, &[("in", 4.0), ("out", 0.0)]))
        .ok()
        .unwrap();

    let left = render_channel(&mut engine, 750, 0);
    let freq = estimated_freq(&left);
    assert!((freq - 440.0).abs() < 5.0, "estimated freq = {freq}");
    let expected_rms = 0.2 * 0.5 * std::f32::consts::FRAC_1_SQRT_2;
    assert!(
        (rms(&left) - expected_rms).abs() < 0.005,
        "rms = {}, expected ≈ {expected_rms}",
        rms(&left)
    );
}

#[test]
fn ugen_and_faust_synths_mix_on_the_same_bus() {
    use clausters::synthdef::instance::UGenSynth;
    use clausters::synthdef::{compile, default_spec};

    let fdef = compile_def("fsine", SINE_SRC);
    let udef = Arc::new(compile(default_spec()).unwrap());
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);

    // Same freq and phase, both summing into bus 0: amplitudes add.
    let mut usynth = Box::new(UGenSynth::new(Arc::clone(&udef)));
    usynth.set_control(0, 440.0); // freq
    usynth.set_control(1, 0.2); // amp
    handle
        .send(Cmd::AddSynth {
            id: 1000,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            synth: usynth,
            usage: Default::default(),
        })
        .ok()
        .unwrap();
    handle.send(add_faust(1001, &fdef, &[])).ok().unwrap();

    let left = render_channel(&mut engine, 750, 0);
    let expected_rms = 0.4 * std::f32::consts::FRAC_1_SQRT_2;
    assert!(
        (rms(&left) - expected_rms).abs() < 0.01,
        "rms = {}, expected ≈ {expected_rms} (the two sines must mix)",
        rms(&left)
    );
}

#[test]
fn freed_faust_synth_drops_on_this_thread_and_factory_survives() {
    let def = compile_def("fsine", SINE_SRC);
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle.send(add_faust(1000, &def, &[])).ok().unwrap();
    render_channel(&mut engine, 10, 0);

    // `/d_free` semantics: the table's Arc goes away while the instance is
    // still playing — its own clone keeps the factory alive.
    drop(def);
    let left = render_channel(&mut engine, 100, 0);
    assert!(rms(&left) > 0.1, "synth must survive its def being freed");

    handle.send(Cmd::FreeNode { id: 1000 }).ok().unwrap();
    let left = render_channel(&mut engine, 100, 0);
    assert!(rms(&left) < 1e-9, "bus must be clean after the free");
    // The instance comes back as garbage and is deleted here, off the audio
    // thread; the factory (held only by the synth at this point) dies with
    // it, instance strictly first.
    assert_eq!(handle.collect_garbage(), 1);
}

// ---- OSC round trip: /d_faust → /s_new → /n_set → /n_free ----

mod osc {
    use super::*;
    use std::net::UdpSocket;

    use clausters::osc::server::{OscServer, ServerInfo};
    use clausters::rosc::{OscMessage, OscPacket, OscType, decoder, encoder};

    #[test]
    fn faust_def_lifecycle_over_osc() {
        let (mut engine, engine_handle) = engine_pair(SR, CHANNELS);
        let info = ServerInfo {
            nominal_sample_rate: SR as f64,
            actual_sample_rate: SR as f64,
        };
        let mut server = OscServer::bind(("127.0.0.1", 0), info, engine_handle).unwrap();
        let addr = server.local_addr().unwrap();
        let server_thread = std::thread::spawn(move || server.run());
        let client = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        client.set_read_timeout(Some(COMPILE_DEADLINE)).unwrap();

        let send = |addr_str: &str, args: Vec<OscType>| {
            let packet = OscPacket::Message(OscMessage {
                addr: addr_str.into(),
                args,
            });
            client
                .send_to(&encoder::encode(&packet).unwrap(), addr)
                .unwrap();
        };
        let recv_until = |want: &str| -> OscMessage {
            let mut buf = [0u8; 65536];
            for _ in 0..100 {
                let (len, _) = client.recv_from(&mut buf).expect("reply timed out");
                if let (_, OscPacket::Message(msg)) = decoder::decode_udp(&buf[..len]).unwrap()
                    && msg.addr == want
                {
                    return msg;
                }
            }
            panic!("never received {want}");
        };

        send(
            "/d_faust",
            vec![
                OscType::String("fsine".into()),
                OscType::String(SINE_SRC.into()),
            ],
        );
        let done = recv_until("/done");
        assert_eq!(done.args[1], OscType::String("fsine".into()));

        // Instantiate with a named control, tick the engine, listen.
        send(
            "/s_new",
            vec![
                OscType::String("fsine".into()),
                OscType::Int(1000),
                OscType::Int(0),
                OscType::Int(0),
                OscType::String("freq".into()),
                OscType::Float(660.0),
            ],
        );
        let left = render_channel(&mut engine, 750, 0);
        let freq = estimated_freq(&left);
        assert!((freq - 660.0).abs() < 7.0, "estimated freq = {freq}");

        // /n_set by name through the def mirror.
        send(
            "/n_set",
            vec![
                OscType::Int(1000),
                OscType::String("freq".into()),
                OscType::Float(330.0),
            ],
        );
        // The /n_set command needs the network thread to forward it before
        // the engine tick can apply it; poll until the pitch settles.
        let mut freq = 0.0;
        for _ in 0..50 {
            freq = estimated_freq(&render_channel(&mut engine, 150, 0));
            if (freq - 330.0).abs() < 5.0 {
                break;
            }
        }
        assert!((freq - 330.0).abs() < 5.0, "estimated freq = {freq}");

        send("/n_free", vec![OscType::Int(1000)]);
        let mut silent = false;
        for _ in 0..50 {
            silent = rms(&render_channel(&mut engine, 100, 0)) < 1e-9;
            if silent {
                break;
            }
        }
        assert!(silent, "bus must be clean after /n_free");

        send("/quit", vec![]);
        recv_until("/done");
        server_thread.join().unwrap().unwrap();
    }
}
