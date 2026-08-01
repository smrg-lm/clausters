//! F3 tests: `FaustSynth` in the node tree. Gated behind the `faust`
//! feature: `cargo test --features faust --test faust_synth`.
//!
//! Engine-level tests drive the same command FIFO the network thread uses
//! and listen with signal asserts; the OSC test at the end covers the whole
//! `/def_send faust` → `/synth_new` → `/node_set` → `/node_free` round trip with a manually
//! ticked engine.

#![cfg(feature = "faust")]

#[path = "common/signal.rs"]
mod signal;

use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "synth")]
use clausters::clausters_core::rng::SEED_STRIDE;
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
            client: Some(clausters::osc::ClientId::Udp(
                "127.0.0.1:1".parse().unwrap(),
            )),
            cache: None,
        })
        .ok()
        .unwrap();
    let result = compiler
        .recv_result_timeout(COMPILE_DEADLINE)
        .expect("compilation must finish");
    Arc::new(result.outcome.expect("def must compile"))
}

/// Builds the instance on this (network-style) thread, with named controls
/// resolved exactly like `/synth_new` does.
fn add_faust(id: i32, def: &Arc<FaustDef>, controls: &[(&str, f32)]) -> Cmd {
    let mut synth = Box::new(
        FaustSynth::new(Arc::clone(def), SR, &clausters::dsp::buffer::empty_pool())
            .expect("instantiation"),
    );
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
    signal::zero_crossing_freq(buf, SR)
}

#[test]
fn probed_def_exposes_params_and_reserved_bus_controls() {
    let def = compile_def("fsine", SINE_SRC);
    assert_eq!(def.num_inputs, 0);
    assert_eq!(def.num_outputs, 1);
    let names: Vec<_> = def.params.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names.len(), 2);
    assert!(
        names.contains(&"freq") && names.contains(&"amp"),
        "{names:?}"
    );
    assert_eq!(def.control_index("out"), Some(def.params.len() as u32));
    assert_eq!(def.control_index("in"), Some(def.params.len() as u32 + 1));
    assert_eq!(def.control_index("nope"), None);
    let freq = &def.params[def.control_index("freq").unwrap() as usize];
    assert_eq!(freq.init, 440.0);
    assert_eq!((freq.min, freq.max), (20.0, 20_000.0));
}

#[test]
fn soundfile_reads_a_server_buffer() {
    use clausters::dsp::buffer::{Buffer, empty_pool};

    // `soundfile("0", 1)` binds to server buffer 0. The primitive's outputs
    // are [length, sampleRate, channel0]; we route one of them to bus 0 per
    // def and read it back. A self-incrementing index streams the channel.
    let len_def = compile_def(
        "sflen",
        r#"process = (0, 0) : soundfile("0", 1) : (_, !, !);"#,
    );
    let sr_def = compile_def(
        "sfsr",
        r#"process = (0, 0) : soundfile("0", 1) : (!, _, !);"#,
    );
    let read_def = compile_def(
        "sfread",
        r#"
counter = (+(1) ~ _) - 1;          // 0, 1, 2, ... per sample
process = (0, int(counter)) : soundfile("0", 1) : (!, !, _);
"#,
    );

    // Buffer 0: four known mono samples at the engine's own sample rate.
    let samples = [0.1f32, 0.2, 0.3, 0.4];
    let mut pool = empty_pool();
    pool[0] = Some(Arc::new(Buffer::new(samples.to_vec(), 1, 4, SR as f64)));

    let build = |def: &Arc<FaustDef>| -> Cmd {
        let synth = Box::new(FaustSynth::new(Arc::clone(def), SR, &pool).expect("instantiation"));
        Cmd::AddSynth {
            id: 1000,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            synth,
            usage: Default::default(),
        }
    };

    // Length and sample rate come straight from the buffer's shape.
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle.send(build(&len_def)).ok().unwrap();
    assert_eq!(
        render_channel(&mut engine, 1, 0)[0],
        4.0,
        "soundfile length"
    );

    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle.send(build(&sr_def)).ok().unwrap();
    assert_eq!(
        render_channel(&mut engine, 1, 0)[0],
        SR,
        "soundfile sample rate"
    );

    // Streaming the channel reproduces the buffer, then clamps at the last
    // frame (the read index saturates at the part length).
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle.send(build(&read_def)).ok().unwrap();
    let out = render_channel(&mut engine, 1, 0);
    for (i, &expected) in samples.iter().enumerate() {
        assert!(
            (out[i] - expected).abs() < 1e-6,
            "frame {i}: {} vs {expected}",
            out[i]
        );
    }
    assert!(
        (out[4] - 0.4).abs() < 1e-6,
        "index past the end clamps to the last frame: {}",
        out[4]
    );
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
fn n_map_drives_a_faust_zone_from_a_control_bus() {
    let def = compile_def("fsine", SINE_SRC);
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle.send(add_faust(1000, &def, &[])).ok().unwrap();

    // Map the `freq` zone to control bus 5: M11 unifies UGen and Faust
    // parameters under the same bus mapping.
    let freq = def.control_index("freq").unwrap();
    handle
        .send(Cmd::MapControl {
            id: 1000,
            index: freq,
            bus: 5,
            audio: false,
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::SetControlBus {
            index: 5,
            value: 660.0,
        })
        .ok()
        .unwrap();

    let left = render_channel(&mut engine, 750, 0);
    let f = estimated_freq(&left);
    assert!((f - 660.0).abs() < 8.0, "estimated freq = {f}");

    // The zone tracks the bus live, with no further /node_set.
    handle
        .send(Cmd::SetControlBus {
            index: 5,
            value: 330.0,
        })
        .ok()
        .unwrap();
    let left = render_channel(&mut engine, 750, 0);
    let f = estimated_freq(&left);
    assert!((f - 330.0).abs() < 8.0, "estimated freq = {f}");
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

#[cfg(feature = "synth")]
#[test]
fn ugen_and_faust_synths_mix_on_the_same_bus() {
    use clausters::synthdef::instance::UGenSynth;
    use clausters::synthdef::{compile, default_spec};

    let fdef = compile_def("fsine", SINE_SRC);
    let udef = Arc::new(compile(default_spec()).unwrap());
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);

    // Same freq and phase, both summing into bus 0: amplitudes add.
    let mut usynth = Box::new(UGenSynth::new(Arc::clone(&udef), SR, SEED_STRIDE));
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

    // `/def_free` semantics: the table's Arc goes away while the instance is
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

// ---- OSC round trip: /def_send faust → /synth_new → /node_set → /node_free ----

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
            "/def_send",
            vec![
                OscType::String("faust".into()),
                OscType::String("fsine".into()),
                OscType::String(SINE_SRC.into()),
            ],
        );
        let done = recv_until("/done");
        assert_eq!(done.args[1], OscType::String("faust".into()));
        assert_eq!(done.args[2], OscType::String("fsine".into()));

        // M30: /def_query reports the compiled def's parameter surface, which for
        // a FaustDef carries its declared range (init/min/max/step) after the
        // shared (name, default, rate) triple. The reserved out/in bus controls
        // are engine plumbing and stay out of it.
        send("/def_query", vec![OscType::String("fsine".into())]);
        let info = recv_until("/def_query.reply");
        assert_eq!(info.args[0], OscType::String("fsine".into()));
        assert_eq!(info.args[1], OscType::String("faust".into()));
        assert_eq!(info.args[2], OscType::Int(2), "freq and amp, not out/in");
        assert_eq!(info.args[3], OscType::String("amp".into()));
        assert_eq!(info.args[4], OscType::Float(0.2), "its init");
        assert_eq!(info.args[5], OscType::String("kr".into()));
        assert_eq!(info.args[6], OscType::Float(0.0), "min");
        assert_eq!(info.args[7], OscType::Float(1.0), "max");
        assert_eq!(info.args[9], OscType::String("freq".into()));
        assert_eq!(info.args[10], OscType::Float(440.0));
        recv_until("/done");

        // Instantiate with a named control, tick the engine, listen.
        send(
            "/synth_new",
            vec![
                OscType::String("fsine".into()),
                OscType::Int(1000),
                OscType::Int(0),
                OscType::Int(0),
                OscType::String("freq".into()),
                OscType::Float(660.0),
            ],
        );
        // /synth_new also crosses the network thread before the engine sees it;
        // poll until the voice settles at its initial pitch.
        let mut freq = 0.0;
        for _ in 0..50 {
            freq = estimated_freq(&render_channel(&mut engine, 150, 0));
            if (freq - 660.0).abs() < 5.0 {
                break;
            }
        }
        assert!((freq - 660.0).abs() < 5.0, "estimated freq = {freq}");

        // /node_set by name through the def mirror.
        send(
            "/node_set",
            vec![
                OscType::Int(1000),
                OscType::String("freq".into()),
                OscType::Float(330.0),
            ],
        );
        // The /node_set command needs the network thread to forward it before
        // the engine tick can apply it; poll until the pitch settles.
        let mut freq = 0.0;
        for _ in 0..50 {
            freq = estimated_freq(&render_channel(&mut engine, 150, 0));
            if (freq - 330.0).abs() < 5.0 {
                break;
            }
        }
        assert!((freq - 330.0).abs() < 5.0, "estimated freq = {freq}");

        send("/node_free", vec![OscType::Int(1000)]);
        let mut silent = false;
        for _ in 0..50 {
            silent = rms(&render_channel(&mut engine, 100, 0)) < 1e-9;
            if silent {
                break;
            }
        }
        assert!(silent, "bus must be clean after /node_free");

        send("/server_quit", vec![]);
        recv_until("/done");
        server_thread.join().unwrap().unwrap();
    }
}
