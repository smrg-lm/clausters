use clausters::server::render::{RenderConfig, Score, render_to_wav};

const USAGE: &str = "\
usage:
  clausters                                    real-time server (OSC on UDP 57110)
  clausters --nrt <score.osc> <out.wav> [opts] offline render of a binary score
      --rate <hz>          sample rate (default 48000)
      --channels <n>       output channels (default 2)
      --format <fmt>       int16 | int24 | float (default float)

A score is the scsynth binary format: length-prefixed OSC bundles whose
timetags count seconds from the start; the render ends at the last bundle.";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--nrt") => {
            if let Err(e) = nrt_main(&args[1..]) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Some("--help" | "-h") => println!("{USAGE}"),
        Some(other) => {
            eprintln!("unknown argument: {other}\n{USAGE}");
            std::process::exit(2);
        }
        None => {
            if let Err(e) = realtime_main() {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    }
}

/// Offline render; works with or without the `realtime` feature (no cpal).
fn nrt_main(args: &[String]) -> Result<(), String> {
    let mut cfg = RenderConfig::default();
    let mut format = "float".to_string();
    let mut paths = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value\n{USAGE}"))
        };
        match arg.as_str() {
            "--rate" => {
                cfg.sample_rate = value("--rate")?
                    .parse()
                    .map_err(|e| format!("--rate: {e}"))?;
            }
            "--channels" => {
                cfg.channels = value("--channels")?
                    .parse()
                    .map_err(|e| format!("--channels: {e}"))?;
            }
            "--format" => format = value("--format")?,
            other => paths.push(other.to_string()),
        }
    }
    let [score_path, out_path] = paths.as_slice() else {
        return Err(format!("expected a score file and an output file\n{USAGE}"));
    };

    let score = Score::load(score_path)?;
    let stats = render_to_wav(&score, &cfg, out_path, &format)?;
    println!(
        "rendered {} events into {out_path}: {} frames ({:.3} s) at {} Hz, {} channel(s), {format}",
        stats.events,
        stats.frames,
        stats.frames as f64 / cfg.sample_rate,
        cfg.sample_rate,
        cfg.channels,
    );
    Ok(())
}

#[cfg(feature = "realtime")]
fn realtime_main() -> Result<(), Box<dyn std::error::Error>> {
    use clausters::osc::server::{DEFAULT_PORT, OscServer, ServerInfo};

    let (backend, handle) = clausters::server::backend::start()?;
    let info = ServerInfo {
        nominal_sample_rate: backend.sample_rate as f64,
        actual_sample_rate: backend.sample_rate as f64,
    };
    let mut osc = OscServer::bind(("127.0.0.1", DEFAULT_PORT), info, handle)?;
    println!(
        "clausters M8 — silent until /s_new | {} Hz, {} channels | OSC on {} | /quit or Ctrl-C to stop",
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
fn realtime_main() -> Result<(), String> {
    Err("built without the `realtime` feature: no audio backend (try --nrt)".into())
}
