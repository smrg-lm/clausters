use clausters::server::render::{RenderConfig, Score, render_to_wav};

const USAGE: &str = "\
usage:
  clausters [--workers <n>] [--shm <path>] [--data-dir <dir>] [--no-persist] [--tcp [port]] [--ws [port]] [--midi [name]] [--sample-rate <hz>]
                                               real-time server (OSC on UDP 57110)
      --sample-rate <hz>   imposed output rate, default 48000; 0 follows the
                           device (PipeWire honors it per-app; other hosts fall
                           back to the device rate if unsupported)
      --audio-buses <n>    audio buses (default 128, the hard maximum)
      --control-buses <n>  control buses (default 1024)
      --outputs <n>        hardware output channels (default: the device's);
                           audio buses 0..outputs are the hardware outs
      --inputs <n>         hardware input channels (default 0 = no input); opens
                           the default input device, readable via In on audio
                           buses outputs..outputs+inputs
      --max-nodes <n>          node slab capacity, root included (default 1024)
      --max-buffers <n>        buffer pool size (default 1024)
      --max-graph-children <n> per-group child capacity (default 256)
      --max-ugen-inputs <n>    accepted inputs per UGen (default 32, the max)
      --tcp [port]         also accept length-prefixed OSC over TCP (RT only;
                           default port 57110)
      --ws [port]          also accept OSC over WebSocket, reachable from a
                           browser (RT only; default port 57120; ws://host:port/)
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
  -v, -vv, -vvv            log verbosity: warn (default) -> info -> debug ->
                           trace; -q for errors only. RUST_LOG overrides it
                           (e.g. RUST_LOG=clausters::osc=trace); a client can
                           retune it live with /verbosity and /dumpOSC. Logs go
                           to stderr.

A score is the scsynth binary format: length-prefixed OSC bundles whose
timetags count seconds from the start; the render ends at the last bundle.

The flags above default to the `[server]` section of the config file
($CLAUSTERS_CONFIG / $XDG_CONFIG_HOME/clausters/config.toml, overridden by a
project clausters.toml); a flag on the command line wins over both.";

fn main() {
    // Verbosity flags are consumed here (anywhere on the line) and removed
    // before dispatch, so the subcommand parsers never see them. `RUST_LOG`
    // overrides the level; the `/verbosity` OSC command retunes it live.
    let mut verbosity: i8 = 0;
    let mut args: Vec<String> = Vec::new();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-v" | "--verbose" => verbosity += 1,
            "-vv" => verbosity += 2,
            "-vvv" => verbosity += 3,
            "-q" | "--quiet" => verbosity -= 1,
            _ => args.push(arg),
        }
    }
    clausters::logging::init(verbosity);

    match args.first().map(String::as_str) {
        Some("--nrt") => {
            if let Err(e) = nrt_main(&args[1..]) {
                tracing::error!("{e}");
                std::process::exit(1);
            }
        }
        Some("--help" | "-h") => println!("{USAGE}"),
        _ => {
            if let Err(e) = realtime_main(&args) {
                tracing::error!("{e}");
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

    use clausters::dsp::Limits;
    use clausters::server::defstore::{DefStore, resolve_data_dir};
    use clausters::server::engine::{DEFAULT_AUDIO_BUSES, DEFAULT_CONTROL_BUSES};
    use clausters::server::ipc::{IpcPeer, Role, Segment};

    // The config file (`[server]` of the user and project layers) supplies the
    // defaults; the CLI flags below override them, and a still-unset field falls
    // back to the compiled default. Precedence: flag > project > user > default.
    let cfg = clausters_core::config::Config::load().server;
    let mut workers = cfg.workers.unwrap_or(0);
    let mut shm_path: Option<String> = cfg.shm.clone();
    let mut data_dir: Option<String> = cfg.data_dir.clone();
    // `persist = false` in config is the same as `--no-persist`; the flag can
    // still force it off, there is no flag to force it back on.
    let mut no_persist = cfg.persist == Some(false);
    let mut tcp_port: Option<u16> = cfg.tcp.and_then(|t| t.resolve(DEFAULT_PORT));
    let mut ws_port: Option<u16> = cfg.ws.and_then(|w| w.resolve(DEFAULT_PORT + 10));
    let mut midi_port: Option<String> = cfg.midi.as_ref().and_then(|m| m.resolve("clausters"));
    // The server imposes 48 kHz by default (PipeWire honors it per-app); `0`
    // means "follow the device's default rate". `None` => follow the device.
    let mut sample_rate: Option<u32> = match cfg.sample_rate {
        Some(0) => None,
        Some(hz) => Some(hz),
        None => Some(48_000),
    };
    let mut audio_buses = cfg.audio_buses.unwrap_or(DEFAULT_AUDIO_BUSES);
    let mut control_buses = cfg.control_buses.unwrap_or(DEFAULT_CONTROL_BUSES);
    // Hardware I/O channel counts (scsynth `-o`/`-i`). `outputs = None` follows
    // the device default; `inputs = 0` opens no input device.
    let mut outputs: Option<usize> = cfg.outputs;
    let mut inputs: usize = cfg.inputs.unwrap_or(0);
    // Boot-time pool sizes, config over the compiled defaults (clamped later in
    // the engine). Each is a slab built once at startup.
    let mut limits = Limits::default();
    if let Some(n) = cfg.max_nodes {
        limits.max_nodes = n;
    }
    if let Some(n) = cfg.max_buffers {
        limits.max_buffers = n;
    }
    if let Some(n) = cfg.max_graph_children {
        limits.max_group_children = n;
    }
    if let Some(n) = cfg.max_ugen_inputs {
        limits.max_ugen_inputs = n;
    }
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
            "--ws" => {
                // Optional port; defaults away from --tcp's, since both bind a
                // TCP listener and would collide on the same port.
                let mut port = DEFAULT_PORT + 10;
                if let Some(next) = it.clone().next()
                    && let Ok(p) = next.parse::<u16>()
                {
                    port = p;
                    it.next();
                }
                ws_port = Some(port);
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
            "--sample-rate" => {
                let value = it
                    .next()
                    .ok_or(format!("--sample-rate needs a value\n{USAGE}"))?;
                let hz: u32 = value.parse().map_err(|e| format!("--sample-rate: {e}"))?;
                // 0 = follow the device default; otherwise impose the rate.
                sample_rate = (hz != 0).then_some(hz);
            }
            "--audio-buses" => {
                let value = it
                    .next()
                    .ok_or(format!("--audio-buses needs a value\n{USAGE}"))?;
                audio_buses = value.parse().map_err(|e| format!("--audio-buses: {e}"))?;
            }
            "--control-buses" => {
                let value = it
                    .next()
                    .ok_or(format!("--control-buses needs a value\n{USAGE}"))?;
                control_buses = value.parse().map_err(|e| format!("--control-buses: {e}"))?;
            }
            "--outputs" => {
                let value = it
                    .next()
                    .ok_or(format!("--outputs needs a value\n{USAGE}"))?;
                outputs = Some(value.parse().map_err(|e| format!("--outputs: {e}"))?);
            }
            "--inputs" => {
                let value = it
                    .next()
                    .ok_or(format!("--inputs needs a value\n{USAGE}"))?;
                inputs = value.parse().map_err(|e| format!("--inputs: {e}"))?;
            }
            "--max-nodes" => {
                let value = it
                    .next()
                    .ok_or(format!("--max-nodes needs a value\n{USAGE}"))?;
                limits.max_nodes = value.parse().map_err(|e| format!("--max-nodes: {e}"))?;
            }
            "--max-buffers" => {
                let value = it
                    .next()
                    .ok_or(format!("--max-buffers needs a value\n{USAGE}"))?;
                limits.max_buffers = value.parse().map_err(|e| format!("--max-buffers: {e}"))?;
            }
            "--max-graph-children" => {
                let value = it
                    .next()
                    .ok_or(format!("--max-graph-children needs a value\n{USAGE}"))?;
                limits.max_group_children = value
                    .parse()
                    .map_err(|e| format!("--max-graph-children: {e}"))?;
            }
            "--max-ugen-inputs" => {
                let value = it
                    .next()
                    .ok_or(format!("--max-ugen-inputs needs a value\n{USAGE}"))?;
                limits.max_ugen_inputs = value
                    .parse()
                    .map_err(|e| format!("--max-ugen-inputs: {e}"))?;
            }
            other => return Err(format!("unknown argument: {other}\n{USAGE}").into()),
        }
    }

    let segment = match &shm_path {
        Some(path) => Some(Segment::create_with(
            std::path::Path::new(path),
            control_buses,
        )?),
        None => None,
    };
    let (backend, handle) = clausters::server::backend::start(
        workers,
        segment.clone(),
        sample_rate,
        audio_buses,
        control_buses,
        limits,
        outputs,
        inputs,
    )?;
    // Nominal = what we asked for; actual = what the device gave us. They differ
    // only when the host could not honor the requested rate (see backend.rs).
    let nominal = sample_rate.map_or(backend.sample_rate as f64, f64::from);
    let info = ServerInfo {
        nominal_sample_rate: nominal,
        actual_sample_rate: backend.sample_rate as f64,
    };
    if nominal != backend.sample_rate as f64 {
        tracing::warn!(
            "requested {nominal} Hz but the device runs at {} Hz",
            backend.sample_rate
        );
    }
    let mut osc = OscServer::bind(("127.0.0.1", DEFAULT_PORT), info, handle)?;
    if !no_persist && let Some(dir) = resolve_data_dir(data_dir.as_deref()) {
        match DefStore::open(&dir) {
            Ok(store) => {
                osc.attach_store(store);
                tracing::info!("persisting defs in {}", dir.display());
            }
            Err(e) => tracing::warn!(
                "def persistence disabled: cannot open {}: {e}",
                dir.display()
            ),
        }
    }
    if let Some(segment) = segment {
        osc.attach_ipc(IpcPeer::new(segment, Role::Server))?;
        tracing::info!(
            "shared segment at {} (ABI v{})",
            shm_path.as_deref().unwrap_or(""),
            clausters::server::ipc::ABI_VERSION
        );
    }
    if let Some(port) = tcp_port {
        let bound = osc.listen_tcp(("0.0.0.0", port))?;
        tracing::info!("OSC on TCP {bound} (length-prefixed)");
    }
    if let Some(port) = ws_port {
        let bound = osc.listen_ws(("0.0.0.0", port))?;
        tracing::info!("OSC on WebSocket {bound} (ws://, browser-reachable)");
    }
    if let Some(name) = &midi_port {
        #[cfg(feature = "midi")]
        {
            osc.listen_midi(name)?;
            #[cfg(feature = "midi-jack")]
            tracing::info!("MIDI input on virtual JACK port \"{name}\" (connect with qpwgraph)");
            #[cfg(not(feature = "midi-jack"))]
            tracing::info!("MIDI input on virtual ALSA port \"{name}\" (connect with aconnect)");
        }
        #[cfg(not(feature = "midi"))]
        return Err("built without the `midi` feature: rebuild with --features midi".into());
    }
    println!(
        "clausters — silent until /s_new | {} Hz, {} out / {} in ch | {} DSP worker(s) | OSC on {} | /quit or Ctrl-C to stop",
        backend.sample_rate,
        backend.channels,
        backend.input_channels,
        workers,
        osc.local_addr()?
    );
    // The OSC server runs on the main thread; the audio runs in cpal's
    // callback thread until `backend` is dropped.
    osc.run()?;
    tracing::info!("received /quit, shutting down");
    Ok(())
}

#[cfg(not(feature = "realtime"))]
fn realtime_main(_args: &[String]) -> Result<(), String> {
    Err("built without the `realtime` feature: no audio backend (try --nrt)".into())
}
