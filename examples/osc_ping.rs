//! Minimal OSC client for manual testing against a running server.
//!
//! Usage: cargo run --example osc_ping -- [status] [info] [beep] [vibrato] [map] [quit]
//! Default (no args): status. `beep` plays the default synth for a moment,
//! re-tunes it with /n_set, then frees it. `map` demos /n_map and /n_mapa
//! (controls driven live by buses).

use std::net::UdpSocket;
use std::time::Duration;

use clausters::osc::server::DEFAULT_PORT;
use clausters::rosc::{OscMessage, OscPacket, OscType, decoder, encoder};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server = ("127.0.0.1", DEFAULT_PORT);
    let socket = UdpSocket::bind(("127.0.0.1", 0))?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;

    let send = |addr: &str, args: Vec<OscType>| -> Result<(), Box<dyn std::error::Error>> {
        let packet = OscPacket::Message(OscMessage {
            addr: addr.into(),
            args,
        });
        socket.send_to(&encoder::encode(&packet)?, server)?;
        Ok(())
    };
    let recv = |for_addr: &str| -> Result<(), Box<dyn std::error::Error>> {
        let mut buf = [0u8; 65536];
        let (len, _) = socket.recv_from(&mut buf)?;
        if let (_, OscPacket::Message(reply)) = decoder::decode_udp(&buf[..len])? {
            println!("{} -> {} {:?}", for_addr, reply.addr, reply.args);
        }
        Ok(())
    };

    let args: Vec<String> = std::env::args().skip(1).collect();
    let commands = if args.is_empty() {
        vec!["status".to_string()]
    } else {
        args
    };

    for command in &commands {
        match command.as_str() {
            "status" => {
                send("/status", vec![])?;
                recv("/status")?;
            }
            "info" => {
                // Static server config: [audio_buses, control_buses, channels,
                // block_size, nominal_sr, actual_sr]. A client sizes its own
                // bus allocators from this instead of hardcoding the counts.
                send("/server_info", vec![])?;
                recv("/server_info")?;
            }
            "beep" => {
                println!("/s_new default 1000 (440 Hz)");
                send(
                    "/s_new",
                    vec![
                        OscType::String("default".into()),
                        OscType::Int(1000),
                        OscType::Int(1),
                        OscType::Int(0),
                    ],
                )?;
                std::thread::sleep(Duration::from_millis(600));
                println!("/n_set 1000 freq 660");
                send(
                    "/n_set",
                    vec![
                        OscType::Int(1000),
                        OscType::String("freq".into()),
                        OscType::Float(660.0),
                    ],
                )?;
                std::thread::sleep(Duration::from_millis(600));
                println!("/n_free 1000");
                send("/n_free", vec![OscType::Int(1000)])?;
            }
            "vibrato" => {
                // M3 demo: define a synth as JSON, then play it
                let json = r#"{
                    "name": "vibrato",
                    "controls": [{"name": "freq", "default": 440.0}],
                    "ugens": [
                        {"kind": "SinOsc", "inputs": [{"const": 5.0}]},
                        {"kind": "Mul",    "inputs": [{"ugen": 0}, {"const": 25.0}]},
                        {"kind": "Add",    "inputs": [{"ugen": 1}, {"control": 0}]},
                        {"kind": "SinOsc", "inputs": [{"ugen": 2}]},
                        {"kind": "Mul",    "inputs": [{"ugen": 3}, {"const": 0.2}]},
                        {"kind": "Out",    "inputs": [{"const": 0.0}, {"ugen": 4}]},
                        {"kind": "Out",    "inputs": [{"const": 1.0}, {"ugen": 4}]}
                    ]
                }"#;
                send("/d_recv", vec![OscType::Blob(json.as_bytes().to_vec())])?;
                recv("/d_recv")?;
                println!("/s_new vibrato 1001");
                send(
                    "/s_new",
                    vec![
                        OscType::String("vibrato".into()),
                        OscType::Int(1001),
                        OscType::Int(1),
                        OscType::Int(0),
                    ],
                )?;
                std::thread::sleep(Duration::from_millis(1200));
                println!("/n_free 1001");
                send("/n_free", vec![OscType::Int(1001)])?;
            }
            "map" => {
                // /n_map: bind a control to a *control bus*. The synth re-reads
                // the bus every block, so writing the bus retunes it live — no
                // /n_set per change.
                println!("/s_new default 1002, then /n_map freq -> control bus 5");
                send(
                    "/s_new",
                    vec![
                        OscType::String("default".into()),
                        OscType::Int(1002),
                        OscType::Int(1),
                        OscType::Int(0),
                    ],
                )?;
                send(
                    "/n_map",
                    vec![
                        OscType::Int(1002),
                        OscType::String("freq".into()),
                        OscType::Int(5),
                    ],
                )?;
                for hz in [330.0_f32, 495.0, 660.0] {
                    println!("/c_set 5 {hz}  (pitch follows the bus)");
                    send("/c_set", vec![OscType::Int(5), OscType::Float(hz)])?;
                    std::thread::sleep(Duration::from_millis(500));
                }
                send("/n_free", vec![OscType::Int(1002)])?;

                // /n_mapa: bind a control to an *audio bus*, sampled once per
                // block (control-rate). An LFO synth writes a slow sine into a
                // non-output bus; the target's freq tracks it → vibrato.
                let lfo = r#"{
                    "name": "lfo",
                    "ugens": [
                        {"kind": "SinOsc", "inputs": [{"const": 3.0}]},
                        {"kind": "Mul",    "inputs": [{"ugen": 0}, {"const": 120.0}]},
                        {"kind": "Add",    "inputs": [{"ugen": 1}, {"const": 480.0}]},
                        {"kind": "Out",    "inputs": [{"const": 20.0}, {"ugen": 2}]}
                    ]
                }"#;
                send("/d_recv", vec![OscType::Blob(lfo.as_bytes().to_vec())])?;
                recv("/d_recv")?;
                println!("/s_new lfo 1003 (writes bus 20), /s_new default 1004");
                send(
                    "/s_new",
                    vec![
                        OscType::String("lfo".into()),
                        OscType::Int(1003),
                        OscType::Int(1),
                        OscType::Int(0),
                    ],
                )?;
                send(
                    "/s_new",
                    vec![
                        OscType::String("default".into()),
                        OscType::Int(1004),
                        OscType::Int(1),
                        OscType::Int(0),
                    ],
                )?;
                println!("/n_mapa 1004 freq -> audio bus 20 (vibrato)");
                send(
                    "/n_mapa",
                    vec![
                        OscType::Int(1004),
                        OscType::String("freq".into()),
                        OscType::Int(20),
                    ],
                )?;
                std::thread::sleep(Duration::from_millis(1500));
                send("/n_free", vec![OscType::Int(1003), OscType::Int(1004)])?;
            }
            "quit" => {
                send("/quit", vec![])?;
                recv("/quit")?;
            }
            other => {
                eprintln!("unknown command: {other} (use status, info, beep, vibrato, map, quit)")
            }
        }
    }
    Ok(())
}
