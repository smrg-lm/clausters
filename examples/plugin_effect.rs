//! Process a native CLAP effect over OSC and record its paced live output.
//!
//! `cargo build -p clausters-plugin-host`, then run:
//! `cargo run --example plugin_effect --features plugin-clap -- /usr/lib/clap/lsp-plugins.clap in.lsp-plug.comp_delay_mono`
//! The example boots its own device-free server and drives its engine at live
//! block cadence. It writes `examples/out/plugin-effect.wav` for listening.
//! This is a recording of the live bridge, not a deterministic offline render.
//! Try the repository fixture's `org.clausters.fixture.gain` for exact gain.
#[cfg(all(feature = "plugin-clap", target_os = "linux"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use clausters::dsp::{ProcessCtx, StageMask};
    use clausters::node::{AddAction, SynthNode};
    use clausters::osc::server::{OscServer, ServerInfo};
    use clausters::rosc::{OscMessage, OscPacket, OscType, encoder};
    use clausters::server::engine::{Cmd, engine_pair};
    use clausters_core::plugin_catalog::{Route, reference};
    use std::net::UdpSocket;
    use std::time::{Duration, Instant};
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
    // Inputs read from bus 4, where the source writes; outputs write from bus 0.
    let reference = reference(&info, &Route::From(4), &Route::From(0), Vec::new())?;
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
    let start = Instant::now();
    for block in 0..750 {
        let mut output = [0.0; 128];
        engine.process_block(&mut output);
        samples.extend_from_slice(&output);
        let deadline = start + Duration::from_secs_f64((block + 1) as f64 * 64.0 / 48000.0);
        if let Some(wait) = deadline.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
    }
    send("/server_quit", Vec::new())?;
    recv()?;
    thread.join().unwrap()?;
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/out");
    std::fs::create_dir_all(&directory)?;
    let path = directory.join("plugin-effect.wav");
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
#[cfg(not(all(feature = "plugin-clap", target_os = "linux")))]
fn main() {
    eprintln!("Build on Linux with --features plugin-clap");
}
