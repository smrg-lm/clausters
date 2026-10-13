//! Scan native CLAP plugins over OSC without opening an audio device.
//!
//! Build the matching helper first: `cargo build -p clausters-plugin-host`.
//! Then run `cargo run --example plugin_scan --features plugin-clap -- <dirs...>`.
//! `bash tools/plugin-bridge/run.sh` builds a small catalogue at
//! `target/plugin-bridge/` for a deterministic manual check. The example starts
//! its own server, scans via a real UDP client, verifies the async barrier and
//! prints metadata/error rows. It caches metadata in `examples/out/plugin-scan`.

#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use clausters::osc::server::{OscServer, ServerInfo};
    use clausters::rosc::{OscMessage, OscPacket, OscType, encoder};
    use clausters::server::defstore::DefStore;
    use clausters::server::engine::engine_pair;
    use std::net::UdpSocket;
    use std::time::Duration;
    let (_engine, handle) = engine_pair(48000.0, 2);
    let mut server = OscServer::bind(
        ("127.0.0.1", 0),
        ServerInfo {
            nominal_sample_rate: 48000.0,
            actual_sample_rate: 48000.0,
        },
        handle,
    )?;
    server.attach_store(DefStore::open(std::path::Path::new(
        "examples/out/plugin-scan",
    ))?);
    let address = server.local_addr()?;
    let thread = std::thread::spawn(move || server.run());
    let client = UdpSocket::bind(("127.0.0.1", 0))?;
    client.set_read_timeout(Some(Duration::from_secs(30)))?;
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
    send(
        "/plugin_scan",
        std::env::args().skip(1).map(OscType::String).collect(),
    )?;
    send("/server_sync", vec![OscType::Int(29)])?;
    let mut fragments =
        std::collections::BTreeMap::<i32, std::collections::BTreeMap<i32, Vec<u8>>>::new();
    let mut done = false;
    let mut buffer = [0; 65536];
    let mut failure = None;
    loop {
        let n = client.recv(&mut buffer)?;
        let OscPacket::Message(msg) = clausters::osc::decode_packet(&buffer[..n])? else {
            continue;
        };
        if msg.addr == "/plugin_scan.reply" {
            if let [
                OscType::Int(index),
                _,
                OscType::Int(chunk),
                _,
                OscType::Blob(data),
            ] = msg.args.as_slice()
                && *index >= 0
            {
                fragments
                    .entry(*index)
                    .or_default()
                    .insert(*chunk, data.clone());
            }
        } else if msg.addr == "/done"
            && msg.args.first() == Some(&OscType::String("/plugin_scan".into()))
        {
            done = true;
        } else if msg.addr == "/fail" {
            failure = Some(format!("scan failed: {:?}", msg.args));
        } else if msg.addr == "/server_sync.reply" {
            break;
        }
    }
    send("/server_quit", Vec::new())?;
    thread.join().map_err(|_| "server thread panicked")??;
    if let Some(error) = failure {
        return Err(error.into());
    }
    if !done {
        return Err("barrier arrived before scan completion".into());
    }
    for chunks in fragments.into_values() {
        let data: Vec<u8> = chunks.into_values().flatten().collect();
        let row: serde_json::Value = serde_json::from_slice(&data)?;
        println!("{}", serde_json::to_string_pretty(&row)?);
    }
    Ok(())
}
#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("native plugin scanning currently requires Linux");
}
