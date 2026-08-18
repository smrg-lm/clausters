use clausters::server::render::{RenderConfig, Score, render_to_wav};

const USAGE: &str = "\
usage:
  clausters [--port <n>] [--workers <n>] [--shm <path>] [--data-dir <dir>] [--no-persist] [--prune-defs] [--udp [port]] [--tcp [port] | --no-tcp] [--ws [port]] [--midi [name]] [--sample-rate <hz>]
                                               real-time server (OSC on UDP + TCP 57110)
      --port <n>           the base OSC port, default 57110: UDP binds it and
                           TCP follows it, so one flag moves the whole server
                           and several can run side by side
      --sample-rate <hz>   imposed output rate, default 48000; 0 follows the
                           device (PipeWire honors it per-app; other hosts fall
                           back to the device rate if unsupported)
      --audio-buses <n>    audio buses (default 128, the hard maximum)
      --control-buses <n>  control buses (default 16384)
      --taps <n>           audio-tap rings for oscilloscopes (default 8;
                           0 disables): /bus_tap routes an audio bus into one,
                           read from the shared segment or via /bus_tapStream
      --tap-frames <n>     per-tap ring capacity in samples (default 16384;
                           rounded up to a power of two)
      --outputs <n>        hardware output channels (default: the device's);
                           audio buses 0..outputs are the hardware outs
      --inputs <n>         hardware input channels (default 0 = no input); opens
                           the input device, readable via In on audio
                           buses outputs..outputs+inputs
      --host <name>        audio host/backend to use (jack, alsa, pipewire,
                           coreaudio, wasapi -- whatever this build has);
                           default: the platform's
      --device <name>      output device by name (exact, or a substring of one);
                           default: the host's default. Under JACK this is also
                           the client name its ports carry
      --input-device <n>   input device by name; default: the host's default.
                           Capture belongs to whoever holds this device
      --client-name <name> what this server calls itself to the audio graph, so
                           its ports come back under the same name after a
                           restart and a patchbay can reconnect them (PipeWire;
                           under JACK use --device)
      --list-devices       print every host and device this build can see, with
                           the names the three flags above take, and exit
      --max-nodes <n>          node slab capacity, root included (default 8192)
      --max-buffers <n>        buffer pool size (default 4096)
      --max-graph-children <n> per-group child capacity (default 512)
      --max-ugen-inputs <n>    accepted inputs per UGen (default 32, the max)
      --udp [port]         move the UDP front alone, off the base port. UDP is
                           always on: it is the door a client boots against
      --tcp [port]         length-prefixed OSC over TCP — on by default at the
                           base port; the flag only moves it (RT only)
      --no-tcp             disable the TCP transport (UDP-only server)
      --max-frame <bytes>  largest OSC frame on the stream transports (TCP and
                           WebSocket; default 16 MiB). A DoS ceiling, not a
                           protocol limit; UDP keeps the ~64 KB datagram cap
      --max-clients <n>    concurrent stream clients, TCP + WebSocket combined
                           (default 64); a connection past the ceiling is
                           dropped at accept. UDP is connectionless, unaffected
      --ws [port]          also accept OSC over WebSocket, reachable from a
                           browser (RT only; default the base port + 10, so
                           57120; ws://host:port/)
      --midi [name]        open a virtual MIDI input port (RT only; default
                           name \"clausters\"; connect with aconnect/qpwgraph)
      --pin <cpu[,cpu..]>  CPU affinity (Linux, experimental; needs a build
                           with the `rtprio` feature): first CPU for the audio
                           callback thread, the rest round-robin over the DSP
                           workers
  clausters --nrt <score.osc> <out.wav> [opts] offline render of a binary score
      --rate <hz>          sample rate (default 48000)
      --channels <n>       output channels (default 2)
      --format <fmt>       int16 | int24 | float (default float)
      --seed <n>           starting seed for noise UGens (default: a fresh one
                           each run; the seed used is reported, pass it back
                           here to replay that exact take)
      --stats              print the render's stats as one JSON line instead
                           of the human summary (for a client driving --nrt)
      --workers <n>        DSP threads for /group_parallel groups (default 0)
      --shm <path>         shared-memory segment for local clients (RT only;
                           put it on /dev/shm — see docs/ipc.md). One that
                           already exists is attached to, never truncated: the
                           first server on a segment owns its command plane and
                           its material, a later one plays what the owner
                           published
      --data-dir <dir>     where defs are persisted/reloaded (RT only;
                           default $CLAUSTERS_DATA_DIR or the XDG data dir)
      --no-persist         disable def persistence for this run (RT only)
      --prune-defs         drop the persisted defs that no longer load, instead
                           of warning about them (they warn at every boot, and
                           a UGen that grew an input makes every def written
                           against it unloadable). Only the families this build
                           has are pruned
  -v, -vv, -vvv            log verbosity: warn (default) -> info -> debug ->
                           trace; -q for errors only. RUST_LOG overrides it
                           (e.g. RUST_LOG=clausters::osc=trace); a client can
                           retune it live with /server_verbosity and /server_dumpOsc. Logs go
                           to stderr.

A score is the scsynth binary format: length-prefixed OSC bundles whose
timetags count seconds from the start; the render ends at the last bundle.

The flags above default to the `[server]` section of the config file
($CLAUSTERS_CONFIG / $XDG_CONFIG_HOME/clausters/config.toml, overridden by a
project clausters.toml); a flag on the command line wins over both.";

fn main() {
    // Verbosity flags are consumed here (anywhere on the line) and removed
    // before dispatch, so the subcommand parsers never see them. `RUST_LOG`
    // overrides the level; the `/server_verbosity` OSC command retunes it live.
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

/// The virtual MIDI port's default name. A server off the default OSC port
/// carries the port in the name, so two servers on one machine open two
/// distinguishable ports instead of two both called "clausters".
#[cfg(feature = "realtime")]
fn default_midi_name(port: u16) -> String {
    if port == clausters::osc::server::DEFAULT_PORT {
        "clausters".to_string()
    } else {
        format!("clausters:{port}")
    }
}

/// Offline render; works with or without the `realtime` feature (no cpal).
fn nrt_main(args: &[String]) -> Result<(), String> {
    let mut cfg = RenderConfig::default();
    let mut format = "float".to_string();
    let mut stats_json = false;
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
            "--seed" => {
                cfg.seed = Some(
                    value("--seed")?
                        .parse()
                        .map_err(|e| format!("--seed: {e}"))?,
                );
            }
            "--workers" => cfg.workers = parse_workers(&value("--workers")?)?,
            "--stats" => stats_json = true,
            other => paths.push(other.to_string()),
        }
    }
    let [score_path, out_path] = paths.as_slice() else {
        return Err(format!("expected a score file and an output file\n{USAGE}"));
    };

    let score = Score::load(score_path)?;
    let stats = render_to_wav(&score, &cfg, out_path, &format)?;
    if stats_json {
        // One machine-readable line, for a client driving `--nrt` as a
        // subprocess: it gets the render's stats without reading the file it
        // just asked the server to write.
        // Widened to f64 before printing: `{}` on an f32 gives the shortest
        // string that round-trips *as f32*, which a JSON reader parsing into
        // a double turns into a different number.
        let list = |v: &[f32]| {
            v.iter()
                .map(|x| format!("{}", *x as f64))
                .collect::<Vec<_>>()
                .join(",")
        };
        println!(
            "{{\"frames\":{},\"events\":{},\"channels\":{},\"sampleRate\":{},\
             \"seed\":{},\"peak\":[{}],\"rms\":[{}]}}",
            stats.frames,
            stats.events,
            cfg.channels,
            cfg.sample_rate,
            stats.seed,
            list(&stats.peak),
            list(&stats.rms),
        );
        return Ok(());
    }
    // The seed is on the human line too: without `--seed` this render was a
    // fresh take, and this is the only way back to it.
    println!(
        "rendered {} events into {out_path}: {} frames ({:.3} s) at {} Hz, {} channel(s), \
         {format}, seed {}",
        stats.events,
        stats.frames,
        stats.frames as f64 / cfg.sample_rate,
        cfg.sample_rate,
        cfg.channels,
        stats.seed,
    );
    Ok(())
}

#[cfg(feature = "realtime")]
fn realtime_main(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use clausters::osc::server::{DEFAULT_PORT, OscServer, ServerInfo};
    use clausters_core::config::{MidiSetting, PortChoice, WS_PORT_OFFSET};

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
    let mut prune_defs = false;
    // The base OSC port, which UDP binds and the other transports follow. Every
    // transport's port is settled in two steps — the config and the flags record
    // *what was asked for* (follow the base, sit at a number, stay off) and the
    // base is only known once the whole line is read, since `--tcp` may come
    // before `--port` on it.
    let mut base_port: u16 = cfg.port.unwrap_or(DEFAULT_PORT);
    // `--udp <port>`: the UDP front alone, off the base. There is no way to turn
    // it off — it is the door `/server_status` answers on, so a client can find
    // this server at all.
    let mut udp_at: Option<u16> = None;
    // What the command line asks of each stream front, if it asks anything;
    // `None` leaves the answer to the config and then to the default, which
    // `PortChoice::pick` settles below in that order.
    let mut cli_tcp: Option<PortChoice> = None;
    let mut max_frame: usize = cfg.max_frame.unwrap_or(clausters::osc::DEFAULT_MAX_FRAME);
    let mut max_clients: usize = cfg
        .max_clients
        .unwrap_or(clausters::osc::DEFAULT_MAX_CLIENTS);
    let mut cli_ws: Option<PortChoice> = None;
    // Enabled-ness now, the name later: the default name carries the port (see
    // `default_midi_name`), which the loop below can still move.
    let mut midi_setting = cfg.midi.clone();
    // The server imposes 48 kHz by default (PipeWire honors it per-app); `0`
    // means "follow the device's default rate". `None` => follow the device.
    let mut sample_rate: Option<u32> = match cfg.sample_rate {
        Some(0) => None,
        Some(hz) => Some(hz),
        None => Some(48_000),
    };
    let mut audio_buses = cfg.audio_buses.unwrap_or(DEFAULT_AUDIO_BUSES);
    let mut control_buses = cfg.control_buses.unwrap_or(DEFAULT_CONTROL_BUSES);
    let mut taps = cfg.taps.unwrap_or(clausters::server::ipc::DEFAULT_TAPS);
    let mut tap_frames = cfg
        .tap_frames
        .unwrap_or(clausters::server::ipc::DEFAULT_TAP_FRAMES);
    // Hardware I/O channel counts (scsynth `-o`/`-i`). `outputs = None` follows
    // the device default; `inputs = 0` opens no input device.
    let mut outputs: Option<usize> = cfg.outputs;
    let mut inputs: usize = cfg.inputs.unwrap_or(0);
    // **Which devices this server holds and what it calls itself.** An audio
    // application is routed by hand and expected to come back under the same
    // name; see `backend::Devices`.
    let mut devices = clausters::server::backend::Devices {
        host: cfg.host.clone(),
        output: cfg.device.clone(),
        input: cfg.input_device.clone(),
        client_name: cfg.client_name.clone(),
    };
    let mut list_devices = false;
    // `--pin`: CPU affinity list — first CPU for the audio callback thread,
    // the rest round-robin over the DSP workers. Experimental, Linux only,
    // and only in `rtprio` builds (see `server::rt`).
    #[cfg(feature = "rtprio")]
    let mut pin: Vec<usize> = Vec::new();
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
            "--port" => {
                let value = it.next().ok_or(format!("--port needs a port\n{USAGE}"))?;
                base_port = value.parse().map_err(|e| format!("--port: {e}"))?;
            }
            "--udp" => {
                // Optional port, like --tcp; bare, UDP stays on the base port.
                if let Some(next) = it.clone().next()
                    && let Ok(p) = next.parse::<u16>()
                {
                    udp_at = Some(p);
                    it.next();
                }
            }
            "--tcp" => {
                // Optional port; bare, it follows the base port (a separate
                // namespace from UDP's, so sharing the number is fine).
                let mut choice = PortChoice::Follow;
                if let Some(next) = it.clone().next()
                    && let Ok(p) = next.parse::<u16>()
                {
                    choice = PortChoice::At(p);
                    it.next();
                }
                cli_tcp = Some(choice);
            }
            "--no-tcp" => cli_tcp = Some(PortChoice::Off),
            "--max-frame" => {
                let value = it
                    .next()
                    .ok_or(format!("--max-frame needs a byte count\n{USAGE}"))?;
                max_frame = value.parse().map_err(|e| format!("--max-frame: {e}"))?;
            }
            "--max-clients" => {
                let value = it
                    .next()
                    .ok_or(format!("--max-clients needs a count\n{USAGE}"))?;
                max_clients = value.parse().map_err(|e| format!("--max-clients: {e}"))?;
            }
            "--ws" => {
                // Optional port; bare, it follows the base port offset by
                // WS_PORT_OFFSET, since both it and --tcp bind a TCP listener
                // and would collide on the same number.
                let mut choice = PortChoice::Follow;
                if let Some(next) = it.clone().next()
                    && let Ok(p) = next.parse::<u16>()
                {
                    choice = PortChoice::At(p);
                    it.next();
                }
                cli_ws = Some(choice);
            }
            "--midi" => {
                // Optional virtual-port name; the next token unless it's a flag.
                // Bare, the name is filled in once the port is known.
                let mut setting = MidiSetting::Enabled(true);
                if let Some(next) = it.clone().next()
                    && !next.starts_with("--")
                {
                    setting = MidiSetting::Name(next.clone());
                    it.next();
                }
                midi_setting = Some(setting);
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
            "--prune-defs" => prune_defs = true,
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
            "--taps" => {
                let value = it.next().ok_or(format!("--taps needs a value\n{USAGE}"))?;
                taps = value.parse().map_err(|e| format!("--taps: {e}"))?;
            }
            "--tap-frames" => {
                let value = it
                    .next()
                    .ok_or(format!("--tap-frames needs a value\n{USAGE}"))?;
                tap_frames = value.parse().map_err(|e| format!("--tap-frames: {e}"))?;
            }
            "--host" => {
                devices.host = Some(
                    it.next()
                        .ok_or(format!("--host needs a name\n{USAGE}"))?
                        .clone(),
                );
            }
            "--device" => {
                devices.output = Some(
                    it.next()
                        .ok_or(format!("--device needs a name\n{USAGE}"))?
                        .clone(),
                );
            }
            "--input-device" => {
                devices.input = Some(
                    it.next()
                        .ok_or(format!("--input-device needs a name\n{USAGE}"))?
                        .clone(),
                );
            }
            "--client-name" => {
                devices.client_name = Some(
                    it.next()
                        .ok_or(format!("--client-name needs a name\n{USAGE}"))?
                        .clone(),
                );
            }
            "--list-devices" => list_devices = true,
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
            #[cfg(feature = "rtprio")]
            "--pin" => {
                let value = it.next().ok_or(format!("--pin needs a value\n{USAGE}"))?;
                let cpus: Result<Vec<usize>, _> =
                    value.split(',').map(|c| c.trim().parse()).collect();
                pin = cpus.map_err(|e| format!("--pin: {e}"))?;
                if pin.is_empty() {
                    return Err(format!("--pin needs at least one CPU\n{USAGE}").into());
                }
            }
            #[cfg(not(feature = "rtprio"))]
            "--pin" => {
                return Err("--pin needs a build with the `rtprio` feature (see BUILD.md)".into());
            }
            other => return Err(format!("unknown argument: {other}\n{USAGE}").into()),
        }
    }

    // Every flag is in: settle the ports against the base the line ended with.
    let udp_port = udp_at.unwrap_or(base_port);
    // TCP is on by default (the command plane for large payloads); the config's
    // `tcp = false` — or `--no-tcp` — turns it off. WebSocket is opt-in.
    let tcp_port = PortChoice::pick(cli_tcp, cfg.tcp, PortChoice::Follow).resolve(base_port);
    let ws_port = PortChoice::pick(cli_ws, cfg.ws, PortChoice::Off)
        .resolve(base_port.saturating_add(WS_PORT_OFFSET));
    let midi_port = midi_setting.and_then(|m| m.resolve(&default_midi_name(udp_port)));

    // `--list-devices` answers and stops: it is a question about the machine,
    // not a way to start a server.
    if list_devices {
        devices.arm();
        for line in clausters::server::backend::Devices::list() {
            println!("{line}");
        }
        return Ok(());
    }
    // The ring must be a power of two of at least one block; round up quietly.
    let tap_frames = tap_frames.max(clausters::server::engine::BLOCK_SIZE);
    let tap_frames = tap_frames.next_power_of_two();
    // Taps live in the segment. With `--shm` the mapped file carries them; a
    // server without `--shm` but with taps gets an in-memory segment so
    // `/bus_tapStream` still works (nothing else changes: the control buses just
    // live inside it, exactly as in the embed case).
    // The segment kept past the wiring below, so the control-plane claim can be
    // given back on the way out.
    let mut shared: Option<std::sync::Arc<Segment>> = None;
    // `--shm` **attaches** to a segment that is already there and creates one
    // only when it is not: the segment indexes the material now, so a server
    // that truncated it on the way in would take somebody's take with it.
    let mut segment_created = false;
    let segment = match &shm_path {
        Some(path) => {
            let (seg, created) = Segment::open_or_create_full(
                std::path::Path::new(path),
                control_buses,
                taps,
                tap_frames,
            )?;
            if !created {
                // The shape of a segment belongs to whoever created it: a
                // server that attaches adopts the header's counts rather than
                // running the engine against sizes the memory does not have.
                let (adopted, was) = (seg.control_bus_count(), control_buses);
                if adopted != was {
                    tracing::info!(
                        "attached segment carries {adopted} control bus(es), not {was}: using the \
                         segment's"
                    );
                }
                control_buses = adopted;
            }
            segment_created = created;
            Some(seg)
        }
        None if taps > 0 => Some(Segment::in_memory_full(control_buses, taps, tap_frames)),
        None => None,
    };
    // `rtprio` builds promote the audio callback to real-time scheduling,
    // which puts it under RTKit's RLIMIT_RTTIME watchdog: arm the SIGXCPU
    // guard *before* the stream exists so a sustained overload degrades the
    // audio (demotion to SCHED_OTHER) instead of killing the server, and
    // hand the callback thread its --pin CPU (it pins itself on its first
    // callback — the thread is spawned deep inside cpal).
    #[cfg(feature = "rtprio")]
    {
        clausters::server::rt::install_sigxcpu_guard();
        if let Some(&cpu) = pin.first() {
            clausters::server::rt::request_audio_pin(cpu);
        }
    }
    let (backend, handle) = clausters::server::backend::start(
        workers,
        segment.clone(),
        sample_rate,
        audio_buses,
        control_buses,
        limits,
        outputs,
        inputs,
        &devices,
    )?;
    // The workers exist now (spawned by the engine); pin them to the CPUs
    // after the audio thread's, and log the scheduling the callback actually
    // got once it has run.
    #[cfg(feature = "rtprio")]
    {
        if pin.len() > 1 {
            clausters::server::rt::pin_workers(&pin[1..]);
        }
        clausters::server::rt::spawn_diag_report();
    }
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
    let mut osc = OscServer::bind(("127.0.0.1", udp_port), info, handle)?;
    // Before the listeners: the TCP/WS hubs capture the ceiling when they bind.
    osc.set_max_frame(max_frame);
    osc.set_max_clients(max_clients);
    if !no_persist && let Some(dir) = resolve_data_dir(data_dir.as_deref()) {
        match DefStore::open(&dir) {
            Ok(store) => {
                osc.prune_dead_defs(prune_defs);
                osc.attach_store(store);
                tracing::info!("persisting defs in {}", dir.display());
            }
            Err(e) => tracing::warn!(
                "def persistence disabled: cannot open {}: {e}",
                dir.display()
            ),
        }
    }
    // The command-plane ring peer only exists for a *mapped* segment (a local
    // client can reach it); the in-memory tap fallback has no ring client, so
    // attaching would only tighten the run-loop poll for nothing.
    if let (Some(segment), Some(path)) = (segment, shm_path.as_deref()) {
        let abi = clausters::server::ipc::ABI_VERSION;
        // **Two roles, and the claim decides which one this server has.** The
        // rings are SPSC and there is one pair, so the first server on a
        // segment serves the command plane and owns the material; a second one
        // — the RT server in the editor's arrangement — attaches to the data
        // plane, maps what the owner published, and serves its own clients
        // over its sockets.
        if segment.claim_control() {
            osc.attach_ipc(IpcPeer::new(std::sync::Arc::clone(&segment), Role::Server))?;
            osc.share_buffers_at(std::path::PathBuf::from(path));
            let verb = if segment_created {
                "created"
            } else {
                "adopted"
            };
            tracing::info!("shared segment {verb} at {path} (ABI v{abi}); this server owns it");
        } else {
            osc.attach_segment(std::sync::Arc::clone(&segment));
            let found = osc.attach_material_at(std::path::PathBuf::from(path));
            tracing::info!(
                "attached to the shared segment at {path} (ABI v{abi}); pid {} owns it, {found} \
                 buffer(s) mapped — commands over the sockets, /buffer_attach for later ones",
                segment.control_owner().unwrap_or(0),
            );
        }
        shared = Some(segment);
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
        return Err(format!(
            "--midi \"{name}\": built without the `midi` feature: rebuild with --features midi"
        )
        .into());
    }
    println!(
        "clausters — silent until /synth_new | {} Hz, {} out / {} in ch | {} DSP worker(s) | OSC on {} | /server_quit or Ctrl-C to stop",
        backend.sample_rate,
        backend.channels,
        backend.input_channels,
        workers,
        osc.local_addr()?
    );
    // The OSC server runs on the main thread; the audio runs in cpal's
    // callback thread until `backend` is dropped.
    osc.run()?;
    tracing::info!("received /server_quit, shutting down");
    // Give the command plane back, so the next server on this segment adopts
    // it rather than taking it over from a pid that is gone. An unclean exit
    // is covered too — a claim nothing answers to is stale — but a clean one
    // should not need that path.
    if let Some(segment) = &shared {
        segment.release_control();
    }
    Ok(())
}

#[cfg(not(feature = "realtime"))]
fn realtime_main(_args: &[String]) -> Result<(), String> {
    Err("built without the `realtime` feature: no audio backend (try --nrt)".into())
}
