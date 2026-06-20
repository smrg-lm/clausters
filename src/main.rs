use clausters::server::render::{RenderConfig, Score, render_to_wav};

const USAGE: &str = "\
usage:
  clausters [--workers <n>] [--shm <path>] [--data-dir <dir>] [--no-persist] [--tcp [port]] [--midi [name]]
                                               real-time server (OSC on UDP 57110)
      --tcp [port]         also accept length-prefixed OSC over TCP (RT only;
                           default port 57110)
      --midi [name]        open a virtual MIDI input port (RT only; default
                           name \"clausters\"; connect with aconnect/qpwgraph)
  clausters --nrt <score.osc> <out.wav> [opts] offline render of a binary score
      --rate <hz>          sample rate (default 48000)
      --channels <n>       output channels (default 2)
      --format <fmt>       int16 | int24 | float (default float)
      --workers <n>        DSP threads for /g_parallel groups (default 0)
      --shm <path>         shared-memory segment for local clients (RT only;
                           put it on /dev/shm — see docs/ipc.md)
      --data-dir <dir>     where defs are persisted/reloaded (RT only;
                           default $CLAUSTERS_DATA_DIR or the XDG data dir)
      --no-persist         disable def persistence for this run (RT only)

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

    use clausters::server::defstore::{DefStore, resolve_data_dir};
    use clausters::server::ipc::{IpcPeer, Role, Segment};

    let mut workers = 0usize;
    let mut shm_path: Option<String> = None;
    let mut data_dir: Option<String> = None;
    let mut no_persist = false;
    let mut tcp_port: Option<u16> = None;
    let mut midi_port: Option<String> = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--tcp" => {
                // Optional port; defaults to the UDP port (separate namespace).
                let mut port = DEFAULT_PORT;
                if let Some(next) = it.clone().next()
                    && let Ok(p) = next.parse::<u16>()
                {
                    port = p;
                    it.next();
                }
                tcp_port = Some(port);
            }
            "--midi" => {
                // Optional virtual-port name; the next token unless it's a flag.
                let mut name = "clausters".to_string();
                if let Some(next) = it.clone().next()
                    && !next.starts_with("--")
                {
                    name = next.clone();
                    it.next();
                }
                midi_port = Some(name);
            }
            "--workers" => {
                let value = it
                    .next()
                    .ok_or(format!("--workers needs a value\n{USAGE}"))?;
                workers = parse_workers(value)?;
            }
            "--shm" => {
                let value = it.next().ok_or(format!("--shm needs a path\n{USAGE}"))?;
                shm_path = Some(value.clone());
            }
            "--data-dir" => {
                let value = it
                    .next()
                    .ok_or(format!("--data-dir needs a path\n{USAGE}"))?;
                data_dir = Some(value.clone());
            }
            "--no-persist" => no_persist = true,
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
    if !no_persist && let Some(dir) = resolve_data_dir(data_dir.as_deref()) {
        match DefStore::open(&dir) {
            Ok(store) => {
                osc.attach_store(store);
                println!("persisting defs in {}", dir.display());
            }
            Err(e) => eprintln!(
                "def persistence disabled: cannot open {}: {e}",
                dir.display()
            ),
        }
    }
    if let Some(segment) = segment {
        osc.attach_ipc(IpcPeer::new(segment, Role::Server))?;
        println!(
            "shared segment at {} (ABI v{})",
            shm_path.as_deref().unwrap_or(""),
            clausters::server::ipc::ABI_VERSION
        );
    }
    if let Some(port) = tcp_port {
        let bound = osc.listen_tcp(("0.0.0.0", port))?;
        println!("OSC on TCP {bound} (length-prefixed)");
    }
    if let Some(name) = &midi_port {
        #[cfg(feature = "midi")]
        {
            osc.listen_midi(name)?;
            #[cfg(feature = "midi-jack")]
            println!("MIDI input on virtual JACK port \"{name}\" (connect with qpwgraph)");
            #[cfg(not(feature = "midi-jack"))]
            println!("MIDI input on virtual ALSA port \"{name}\" (connect with aconnect)");
        }
        #[cfg(not(feature = "midi"))]
        return Err("built without the `midi` feature: rebuild with --features midi".into());
    }
    println!(
        "clausters — silent until /s_new | {} Hz, {} channels | {} DSP worker(s) | OSC on {} | /quit or Ctrl-C to stop",
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
