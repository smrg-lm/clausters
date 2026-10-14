//! Capture, load and recover one live native effect's opaque state over OSC.
//!
//! Build the helper with `cargo build -p clausters-plugin-host`, then build the
//! repository fixture:
//! `cc -std=c11 -O2 -fPIC -shared -Itools/plugin-bridge/vendor crates/clausters-plugin-host/tests/fixtures/extended.c -o /tmp/clausters-state.clap`
//! Run `cargo run --example plugin_state --features plugin-clap -- /tmp/clausters-state.clap org.clausters.fixture.state`.
//! For VST3, build the helper with `--features vst3`, then build the fixture:
//! `cargo build --offline --manifest-path crates/clausters-plugin-host/tests/fixtures/vst3/Cargo.toml --target-dir /tmp/clausters-vst3-fixture`
//! Copy `/tmp/clausters-vst3-fixture/debug/libclausters_vst3_fixture.so` to
//! `/tmp/clausters-state.vst3`, then run with `--features plugin-vst3` and the
//! arguments `/tmp/clausters-state.vst3 6E33225254224A00AA69301AF318797D`.
//! This effect has a gain at native parameter ID 0 and opaque state support.
//! The example boots a device-free server and records a sine through the live
//! bridge to `examples/out/plugin-state.wav`. Listen to its four levels:
//! initial 0.5, captured 0.75, edited 0.2, restored 0.75. Capture/load have a
//! silent preparation interval and clear held notes. The snapshot is also the
//! recovery image; restart keeps current controls. No audio device is needed.
#[cfg(all(
    any(feature = "plugin-clap", feature = "plugin-vst3"),
    target_os = "linux"
))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use clausters::dsp::{ProcessCtx, StageMask};
    use clausters::node::{AddAction, SynthNode};
    use clausters::osc::server::{OscServer, ServerInfo};
    use clausters::plugin::{PluginReference, PortReference};
    use clausters::rosc::{OscMessage, OscPacket, OscType, encoder};
    use clausters::server::engine::{Cmd, engine_pair};
    use std::net::UdpSocket;
    use std::time::Duration;
    struct Source {
        phase: f32,
    }
    impl SynthNode for Source {
        fn process(&mut self, ctx: &mut ProcessCtx) {
            for i in ctx.offset..ctx.offset + ctx.frames {
                let value = self.phase.sin() * 0.05;
                self.phase =
                    (self.phase + std::f32::consts::TAU * 220.0 / 48000.0) % std::f32::consts::TAU;
                // SAFETY: source is processed sequentially before the effect.
                for bus in 4..36 {
                    (unsafe { ctx.buses.audio_mut(bus) })[i] = value;
                }
            }
        }
        fn set_control(&mut self, _: u32, _: f32) {}
        fn map_control(&mut self, _: u32, _: i32, _: bool) {}
        fn ugen_count(&self) -> usize {
            1
        }
    }
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        return Err("expected binary path and native plugin id".into());
    }
    let helper = clausters_plugin_host::runtime::helper_path()?;
    let info = clausters_plugin_host::runtime::probe(
        &helper,
        std::path::Path::new(&args[1]),
        &args[2],
        Duration::from_secs(2),
    )?;
    let mut bus = 4;
    let inputs = info
        .inputs
        .iter()
        .map(|p| {
            let row = PortReference {
                id: p.id,
                channels: p.channels,
                bus,
            };
            bus += p.channels;
            row
        })
        .collect();
    let mut bus = 0;
    let outputs = info
        .outputs
        .iter()
        .map(|p| {
            let row = PortReference {
                id: p.id,
                channels: p.channels,
                bus,
            };
            bus += p.channels;
            row
        })
        .collect();
    let reference = PluginReference {
        format: info.format.clone(),
        path: args[1].clone(),
        id: args[2].clone(),
        inputs,
        outputs,
        state: Vec::new(),
        depth: 4,
    };
    let (mut engine, mut handle) = engine_pair(48000.0, 2);
    handle
        .send(Cmd::AddSynth {
            id: 10,
            target: 0,
            action: AddAction::Head,
            synth: Box::new(Source { phase: 0.0 }),
            usage: StageMask::default(),
        })
        .map_err(|_| "FIFO full")?;
    let mut server = OscServer::bind(
        ("127.0.0.1", 0),
        ServerInfo {
            nominal_sample_rate: 48000.0,
            actual_sample_rate: 48000.0,
        },
        handle,
    )?;
    let address = server.local_addr()?;
    let thread = std::thread::spawn(move || server.run());
    let client = UdpSocket::bind(("127.0.0.1", 0))?;
    client.set_read_timeout(Some(Duration::from_secs(5)))?;
    let send = |addr: &str, args| -> Result<(), Box<dyn std::error::Error>> {
        client.send_to(
            &encoder::encode(&OscPacket::Message(OscMessage {
                addr: addr.into(),
                args,
            }))?,
            address,
        )?;
        Ok(())
    };
    let recv = || -> Result<OscMessage, Box<dyn std::error::Error>> {
        let mut bytes = [0; 65536];
        let n = client.recv(&mut bytes)?;
        let OscPacket::Message(msg) = clausters::osc::decode_packet(&bytes[..n])? else {
            return Err("expected message".into());
        };
        if msg.addr == "/fail" {
            return Err(format!("{:?}", msg.args).into());
        }
        Ok(msg)
    };
    send(
        "/def_send",
        vec![
            OscType::String("plugin".into()),
            OscType::String("fx".into()),
            OscType::String(serde_json::to_string(&reference)?),
        ],
    )?;
    recv()?;
    send(
        "/synth_new",
        vec![
            OscType::String("fx".into()),
            OscType::Int(11),
            OscType::Int(1),
            OscType::Int(0),
        ],
    )?;
    send("/server_status", Vec::new())?;
    recv()?;
    let mut samples = Vec::with_capacity(48000 * 2);
    let mut snapshot = Vec::new();
    for block in 0..1200 {
        if block == 200 || block == 600 {
            send(
                "/node_set",
                vec![
                    OscType::Int(11),
                    OscType::String("param:0".into()),
                    OscType::Float(if block == 200 { 0.75 } else { 0.2 }),
                ],
            )?;
        }
        if block == 400 {
            send("/plugin_saveState", vec![OscType::Int(11)])?;
        }
        if block == 800 {
            send(
                "/plugin_loadState",
                vec![OscType::Int(11), OscType::Blob(snapshot.clone())],
            )?;
        }
        if block == 1000 {
            send("/plugin_restart", vec![OscType::Int(11)])?;
        }
        // A status round trip ensures the command precedes this engine block.
        if [200, 400, 600, 800, 1000].contains(&block) {
            send("/server_status", Vec::new())?;
            recv()?;
        }
        let mut output = [0.0; 128];
        engine.process_block(&mut output);
        samples.extend_from_slice(&output);
        if block == 400 {
            loop {
                let message = recv()?;
                if message.addr == "/done" {
                    break;
                }
                if message.addr != "/plugin_saveState.reply" {
                    return Err("expected state fragment".into());
                }
                let OscType::Blob(bytes) = &message.args[3] else {
                    return Err("expected state blob".into());
                };
                snapshot.extend_from_slice(bytes);
            }
            println!("Captured {} opaque bytes", snapshot.len());
        } else if block == 800 || block == 1000 {
            recv()?;
        }
        // Administrative work takes wall time: do not submit a catch-up burst.
        std::thread::sleep(Duration::from_secs_f64(64.0 / 48000.0));
    }
    send("/server_quit", Vec::new())?;
    recv()?;
    thread.join().unwrap()?;
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/out");
    std::fs::create_dir_all(&directory)?;
    let path = directory.join("plugin-state.wav");
    let mut writer = hound::WavWriter::create(
        &path,
        hound::WavSpec {
            channels: 2,
            sample_rate: 48000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )?;
    for sample in &samples {
        writer.write_sample(*sample)?;
    }
    writer.finalize()?;
    let peak = samples.iter().map(|v| v.abs()).fold(0.0, f32::max);
    println!("Recorded {}: peak={peak}", path.display());
    if peak == 0.0 {
        return Err("plugin emitted only silence".into());
    }
    Ok(())
}
#[cfg(not(all(
    any(feature = "plugin-clap", feature = "plugin-vst3"),
    target_os = "linux"
)))]
fn main() {
    eprintln!("Build on Linux with --features plugin-clap or plugin-vst3");
}
