use clausters::server::render::{RenderConfig, Score, render_to_wav};

const USAGE: &str = "\
usage:
  clausters [--workers <n>] [--shm <path>]     real-time server (OSC on UDP 57110)
  clausters --nrt <score.osc> <out.wav> [opts] offline render of a binary score
      --rate <hz>          sample rate (default 48000)
      --channels <n>       output channels (default 2)
      --format <fmt>       int16 | int24 | float (default float)
      --workers <n>        DSP threads for /g_parallel groups (default 0)
      --shm <path>         shared-memory segment for local clients (RT only;
                           put it on /dev/shm — see docs/ipc.md)

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
        _ => {
            if let Err(e) = realtime_main(&args) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    }
}

fn parse_workers(value: &str) -> Result<usize, String> {
    value.parse().map_err(|e| format!("--workers: {e}"))
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
            "--workers" => cfg.workers = parse_workers(&value("--workers")?)?,
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
fn realtime_main(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use clausters::osc::server::{DEFAULT_PORT, OscServer, ServerInfo};

    use clausters::server::ipc::{IpcPeer, Role, Segment};

    let mut workers = 0usize;
    let mut shm_path: Option<String> = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--workers" => {
                let value = it.next().ok_or(format!("--workers needs a value\n{USAGE}"))?;
                workers = parse_workers(value)?;
            }
            "--shm" => {
                let value = it.next().ok_or(format!("--shm needs a path\n{USAGE}"))?;
                shm_path = Some(value.clone());
            }
            other => return Err(format!("unknown argument: {other}\n{USAGE}").into()),
        }
    }

    let segment = match &shm_path {
        Some(path) => Some(Segment::create(std::path::Path::new(path))?),
        None => None,
    };
    let (backend, handle) = clausters::server::backend::start(workers, segment.clone())?;
    let info = ServerInfo {
        nominal_sample_rate: backend.sample_rate as f64,
        actual_sample_rate: backend.sample_rate as f64,
    };
    let mut osc = OscServer::bind(("127.0.0.1", DEFAULT_PORT), info, handle)?;
    if let Some(segment) = segment {
        osc.attach_ipc(IpcPeer::new(segment, Role::Server))?;
        println!(
            "shared segment at {} (ABI v{})",
            shm_path.as_deref().unwrap_or(""),
            clausters::server::ipc::ABI_VERSION
        );
    }
    println!(
        "clausters M14 — silent until /s_new | {} Hz, {} channels | {} DSP worker(s) | OSC on {} | /quit or Ctrl-C to stop",
        backend.sample_rate,
        backend.channels,
        workers,
        osc.local_addr()?
    );
    // The OSC server runs on the main thread; the audio runs in cpal's
    // callback thread until `backend` is dropped.
    osc.run()?;
    println!("received /quit, shutting down");
    Ok(())
}

#[cfg(not(feature = "realtime"))]
fn realtime_main(_args: &[String]) -> Result<(), String> {
    Err("built without the `realtime` feature: no audio backend (try --nrt)".into())
}
