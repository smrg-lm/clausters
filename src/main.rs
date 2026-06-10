#[cfg(feature = "realtime")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use claudesufa::osc::server::{DEFAULT_PORT, OscServer, ServerInfo};

    let (backend, handle) = claudesufa::server::backend::start()?;
    let info = ServerInfo {
        nominal_sample_rate: backend.sample_rate as f64,
        actual_sample_rate: backend.sample_rate as f64,
    };
    let mut osc = OscServer::bind(("127.0.0.1", DEFAULT_PORT), info, handle)?;
    println!(
        "claudesufa M3 — silent until /s_new | {} Hz, {} channels | OSC on {} | /quit or Ctrl-C to stop",
        backend.sample_rate,
        backend.channels,
        osc.local_addr()?
    );
    // The OSC server runs on the main thread; the audio runs in cpal's
    // callback thread until `backend` is dropped.
    osc.run()?;
    println!("received /quit, shutting down");
    Ok(())
}

#[cfg(not(feature = "realtime"))]
fn main() {
    eprintln!("built without the `realtime` feature: no audio backend");
    std::process::exit(1);
}
