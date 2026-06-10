#[cfg(feature = "realtime")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let backend = claudesufa::server::backend::start()?;
    println!(
        "claudesufa M0 — 440 Hz sine | {} Hz, {} channels | Ctrl-C to quit",
        backend.sample_rate, backend.channels
    );
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

#[cfg(not(feature = "realtime"))]
fn main() {
    eprintln!("built without the `realtime` feature: no audio backend");
    std::process::exit(1);
}
