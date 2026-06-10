//! Minimal OSC client for manual testing: sends /status (and optionally
//! /quit) to a running server and prints the replies.
//!
//! Usage: cargo run --example osc_ping [-- quit]

use std::net::UdpSocket;
use std::time::Duration;

use claudesufa::osc::server::DEFAULT_PORT;
use claudesufa::rosc::{OscMessage, OscPacket, decoder, encoder};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server = ("127.0.0.1", DEFAULT_PORT);
    let socket = UdpSocket::bind(("127.0.0.1", 0))?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;

    let mut commands = vec!["/status"];
    if std::env::args().any(|a| a == "quit") {
        commands.push("/quit");
    }

    let mut buf = [0u8; 65536];
    for addr in commands {
        let packet = OscPacket::Message(OscMessage {
            addr: addr.into(),
            args: vec![],
        });
        socket.send_to(&encoder::encode(&packet)?, server)?;
        let (len, _) = socket.recv_from(&mut buf)?;
        if let (_, OscPacket::Message(reply)) = decoder::decode_udp(&buf[..len])? {
            println!("{} -> {} {:?}", addr, reply.addr, reply.args);
        }
    }
    Ok(())
}
