//! Minimal OSC client for manual testing against a running server.
//!
//! Usage: cargo run --example osc_ping -- [status] [beep] [quit]
//! Default (no args): status. `beep` plays the default synth for a moment,
//! re-tunes it with /n_set, then frees it.

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
            "quit" => {
                send("/quit", vec![])?;
                recv("/quit")?;
            }
            other => eprintln!("unknown command: {other} (use status, beep, vibrato, quit)"),
        }
    }
    Ok(())
}
