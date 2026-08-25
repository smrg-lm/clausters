//! The `clausters-gui` host binary.
//!
//! Starts the GUI host's server front (UDP + TCP, one port) and, optionally,
//! its client leg to the audio server, then runs the `/gui_*` protocol. By default it opens windows
//! (winit + wgpu): a `window`-rooted GuiDef instantiates an OS window hosting the
//! renderers. With `--headless` it runs the protocol with no display — for
//! tests, automation and machines with no GPU. Drive it from a language client
//! over OSC — see `clients/python/examples/views/window.py` (windowed) and
//! `skeleton.py` (protocol only).

use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use clausters_core::config::{Config, PortChoice, WS_PORT_OFFSET};
use clausters_gui::host::metrics::Metrics;
use clausters_gui::host::store::{self, GuiStore};
use clausters_gui::host::theme::Theme;
use clausters_gui::host::transport::{self, DEFAULT_PORT};
use clausters_gui::host::{Host, ServerLeg, gui};

// Feeding a window a GuiDef this binary built: both the standalone boot and
// `--session` do it, and only the first needs the server crate.
use clausters_core::osc::{OscMessage, OscPacket, OscType};
use clausters_gui::host::ClientId;
use std::net::Ipv4Addr;

// The standalone boot links the server crate directly (the `standalone`
// feature); these are only needed on that path.
#[cfg(feature = "standalone")]
use clausters_core::osc::encode;
#[cfg(feature = "standalone")]
use clausters_gui::host::ServerLink;
#[cfg(feature = "standalone")]
use clausters_gui::host::bundle;
#[cfg(feature = "standalone")]
use clausters_gui::host::embed::{EmbedServer, EmbedSession};
#[cfg(unix)]
use clausters_gui::host::shm::HeadClock;

const USAGE: &str = "\
usage:
  clausters-gui [--port <n>] [--server <host:port>] [--shm <path>] [--headless]
                [--udp [addr:]port] [--tcp [[addr:]port] | --no-tcp]
                [--ws [[addr:]port]] [--max-frame <bytes>]
                [--data-dir <dir>] [--standalone [name]] [--config <path>]
                [--session <file> [--save-to <file>]]
                [--theme <path>] [--font <path>] [--msaa <n>]
                [--follow-block <seconds>]
      --port <n>            port for the GUI host's server front
                            (script -> host, UDP and TCP); default 57210
      --udp [addr:]port     move the UDP leg alone, off the host port. UDP is
                            always on: it is the door a script finds this host on
      --tcp [[addr:]port]   length-prefixed OSC over TCP — on by default at the
                            host port; the flag only moves it
      --no-tcp              disable the TCP leg (UDP-only front)
      --ws [[addr:]port]    also accept /gui_* over WebSocket, reachable from a
                            browser (default the host port + 10, so 57220;
                            ws://host:port/) — the same flag the audio server
                            takes
                            Every leg above binds **loopback** unless its flag
                            names an interface: `--ws 0.0.0.0:57220` opens it to
                            the network. Choosing a carrier is not consenting to
                            the network, so the widening is written down
      --max-frame <bytes>   largest OSC frame on the stream legs, TCP and
                            WebSocket alike (default 16 MiB). A DoS ceiling,
                            not a protocol limit; UDP keeps the ~64 KB
                            datagram cap
      --server <host:port>  also attach the client leg to a running audio
                            server (host -> audio server); default off.
                            Needed for waveform widgets that reference a
                            server buffer number, and for bound widgets
                            (/gui_bind) to forward their value to the server.
      --shm <path>          the shared-memory segment: zero-message
                            meters/scopes, and the **samples** — a take is
                            drawn by mapping it and edited by storing into it,
                            with nothing sent either way. Point it at the audio
                            server's own --shm path. With --session it is where
                            this editor's own samples go, and what a player
                            is started against (`clausters --shm <path>`);
                            without one a path is picked and logged. Unix only
      --data-dir <dir>      data directory for the GuiDef store (named GuiDefs
                            persist there; /gui_load reads from it). Defaults to
                            the same place the server uses ($CLAUSTERS_DATA_DIR,
                            $XDG_DATA_HOME/clausters, ~/.local/share/clausters).
      --session <file>      open a session file (the format the Python client
                            writes) and draw its document as a multitrack, with
                            **this host as its owner**: gestures are applied
                            here, undone here and saved here, with no language
                            client anywhere. The third writer.
                            It opens an on-demand server in this process to own
                            the samples, and plays through the server --server
                            points at -- which is a separate process holding
                            the devices, and the only one that can record.
      --save-to <file>      write the session back here when the window closes.
                            Without it nothing is written: overwriting the file
                            you opened is a decision, not a default
      --standalone [name]   boot the saved GuiDef <name> against an embedded
                            audio server (no separate server or language client):
                            the embedded server loads the data directory's
                            SynthDefs/FaustDefs/GraphDefs and boot.json, the
                            GuiDef's `boot` messages run, and its window opens.
                            A self-contained app. With no name, [standalone].gui
                            from the config is used.
      --config <path>       read configuration from this TOML file instead of
                            the user+project chain (see below).
      --theme <path>        read the host's color theme from this TOML file: a
                            flat table of role = \"#rrggbb[aa]\" entries, laid
                            over [gui.theme] from the config. A partial table
                            is fine — unlisted roles keep the default look.
      --font <path>         draw text with this typeface (TrueType/OpenType)
                            instead of the embedded bitmap face. Only a host
                            built with `--features font-atlas` reads it; any
                            other build warns and keeps its bitmap face. With
                            the feature and no path, one of the system's own
                            faces is used when there is one.
      --follow-block <s>    how much recorded audio a picture waits for
                            before it re-reads its summary, in seconds
                            (default 0, every frame). A take being recorded
                            grows with nothing announcing it, so the host
                            follows the buffer's write frontier and redraws
                            what appeared; larger is cheaper and choppier, and
                            neither the sound nor a playhead over it is
                            affected.
      --msaa <n>            antialias every window with n-sample multisampling
                            (1 = off, the default; 4 is the usual smoothing).
                            One multisampled attachment per window and nothing
                            per widget; a count this GPU does not offer for the
                            surface format falls back to 1 with a warning.
      --headless            run the protocol with no display (tests / no GPU);
                            the default opens windows (winit + wgpu)
  -v, -vv, -vvv             log verbosity: warn (default) -> info -> debug ->
                            trace; -q for errors only. RUST_LOG overrides it.

The options above default to the config file: the `[gui]` and `[standalone]`
sections of $CLAUSTERS_CONFIG / $XDG_CONFIG_HOME/clausters/config.toml, overridden
by a project clausters.toml; a command-line flag wins over both.

The host speaks the /gui_* widget protocol as JSON-in-OSC, the same encoding the
audio server uses. A window-rooted /gui_def opens an actual window; /gui_set,
/gui_free and /gui_query (replying /gui_info) drive and read the tree.";

/// **The host's look**, as the config file and the command line resolved it:
/// the color roles, the size roles and the antialiasing every window is drawn
/// with. The three travel together because they are resolved together and
/// applied together, on both launch paths.
struct Look {
    theme: Theme,
    metrics: Metrics,
    msaa: u32,
    /// Seconds of recorded audio a picture waits for before re-reading its
    /// summary (`--follow-block`). Not a *look*, strictly — it is here because
    /// it is resolved and applied with the rest, on both launch paths.
    follow_block: f64,
}

impl Look {
    fn apply(self, host: &mut Host) {
        host.theme = self.theme;
        host.metrics = self.metrics;
        host.msaa = self.msaa;
        host.follow_block = self.follow_block;
    }
}

fn main() -> ExitCode {
    let mut verbosity: i8 = 0;
    let mut rest: Vec<String> = Vec::new();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-v" | "--verbose" => verbosity += 1,
            "-vv" => verbosity += 2,
            "-vvv" => verbosity += 3,
            "-q" | "--quiet" => verbosity -= 1,
            _ => rest.push(arg),
        }
    }
    init_logging(verbosity);

    match run(&rest) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Reads a carrier flag's optional `[addr:]port` argument: the next token,
/// unless the line has run out or the next token is another flag — a bare
/// `--tcp` follows the host port on the default interface. A token that is
/// there and is not a bind is an error rather than a bare flag followed by a
/// stray argument, which is how a typo used to read. The audio server's
/// `bind_flag`, over this binary's peekable iterator.
fn bind_flag(
    flag: &str,
    it: &mut std::iter::Peekable<std::slice::Iter<'_, String>>,
) -> Result<PortChoice, String> {
    let Some(next) = it.peek() else {
        return Ok(PortChoice::Follow(None));
    };
    if next.starts_with("--") {
        return Ok(PortChoice::Follow(None));
    }
    let choice = PortChoice::parse(next).map_err(|e| format!("{flag}: {e}\n{USAGE}"))?;
    it.next();
    Ok(choice)
}

fn run(args: &[String]) -> Result<(), String> {
    // CLI overrides are collected as `Option`s, then resolved against the config
    // file (the compiled default is the last fallback). Precedence per option:
    // flag > project clausters.toml > user config.toml > default.
    let mut cli_port: Option<u16> = None;
    // What each leg was asked for, if the line says anything; `None` leaves it
    // to the config and then to the default. The base port may still be moved
    // by a later `--port`, so nothing is resolved until the whole line is read.
    let mut cli_udp: Option<PortChoice> = None;
    let mut cli_tcp: Option<PortChoice> = None;
    let mut cli_ws: Option<PortChoice> = None;
    let mut cli_max_frame: Option<usize> = None;
    let mut cli_server: Option<String> = None;
    let mut cli_shm: Option<String> = None;
    let mut cli_headless = false;
    let mut cli_data_dir: Option<String> = None;
    let mut standalone_flag = false;
    let mut cli_standalone_name: Option<String> = None;
    let mut session_path: Option<String> = None;
    let mut save_to: Option<String> = None;
    let mut config_path: Option<String> = None;
    let mut theme_path: Option<String> = None;
    let mut font_path: Option<String> = None;
    let mut cli_msaa: Option<u32> = None;
    let mut cli_follow_block: Option<f64> = None;
    let mut it = args.iter().peekable();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--port" => {
                let v = it
                    .next()
                    .ok_or_else(|| format!("--port needs a value\n{USAGE}"))?;
                cli_port = Some(v.parse().map_err(|e| format!("--port: {e}"))?);
            }
            "--udp" => {
                // Optional bind; without one, UDP rides the host port.
                cli_udp = Some(bind_flag("--udp", &mut it)?);
            }
            "--tcp" => {
                // Optional bind; without one, the TCP leg rides the host port.
                cli_tcp = Some(bind_flag("--tcp", &mut it)?);
            }
            "--no-tcp" => cli_tcp = Some(PortChoice::Off),
            "--ws" => {
                // Optional bind; without one it follows the host port offset by
                // WS_PORT_OFFSET, since the WS leg binds its own TCP listener
                // and cannot share the TCP number (the audio server's pattern).
                cli_ws = Some(bind_flag("--ws", &mut it)?);
            }
            "--max-frame" => {
                let v = it
                    .next()
                    .ok_or_else(|| format!("--max-frame needs a byte count\n{USAGE}"))?;
                cli_max_frame = Some(v.parse().map_err(|e| format!("--max-frame: {e}"))?);
            }
            "--server" => {
                let v = it
                    .next()
                    .ok_or_else(|| format!("--server needs host:port\n{USAGE}"))?;
                cli_server = Some(v.clone());
            }
            "--shm" => {
                let v = it
                    .next()
                    .ok_or_else(|| format!("--shm needs a path\n{USAGE}"))?;
                cli_shm = Some(v.clone());
            }
            "--data-dir" => {
                let v = it
                    .next()
                    .ok_or_else(|| format!("--data-dir needs a path\n{USAGE}"))?;
                cli_data_dir = Some(v.clone());
            }
            "--theme" => {
                theme_path = Some(
                    it.next()
                        .ok_or_else(|| format!("--theme needs a path\n{USAGE}"))?
                        .clone(),
                );
            }
            "--font" => {
                font_path = Some(
                    it.next()
                        .ok_or_else(|| format!("--font needs a path\n{USAGE}"))?
                        .clone(),
                );
            }
            "--follow-block" => {
                let v = it
                    .next()
                    .ok_or_else(|| format!("--follow-block needs seconds\n{USAGE}"))?;
                cli_follow_block = Some(v.parse().map_err(|e| format!("--follow-block: {e}"))?);
            }
            "--msaa" => {
                let v = it
                    .next()
                    .ok_or_else(|| format!("--msaa needs a sample count\n{USAGE}"))?;
                cli_msaa = Some(v.parse().map_err(|e| format!("--msaa: {e}"))?);
            }
            "--config" => {
                let v = it
                    .next()
                    .ok_or_else(|| format!("--config needs a path\n{USAGE}"))?;
                config_path = Some(v.clone());
            }
            "--standalone" => {
                standalone_flag = true;
                // Optional GuiDef name: consume the next token unless it is a flag.
                if let Some(next) = it.peek()
                    && !next.starts_with("--")
                {
                    cli_standalone_name = Some((*next).clone());
                    it.next();
                }
            }
            "--session" => {
                session_path = Some(
                    it.next()
                        .ok_or_else(|| format!("--session needs a path\n{USAGE}"))?
                        .clone(),
                );
            }
            "--save-to" => {
                save_to = Some(
                    it.next()
                        .ok_or_else(|| format!("--save-to needs a path\n{USAGE}"))?
                        .clone(),
                );
            }
            "--headless" => cli_headless = true,
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => return Err(format!("unknown argument: {other}\n{USAGE}")),
        }
    }

    let cfg = match &config_path {
        Some(p) => Config::from_path(Path::new(p))?,
        None => Config::load(),
    };
    let port = cli_port.or(cfg.gui.host_port).unwrap_or(DEFAULT_PORT);
    // Each leg answers where it listens, interface included, and an unnamed
    // interface is loopback in all three — the audio server's rule, and the
    // reason `--ws` no longer opens the host to the LAN by picking a carrier.
    let udp_bind = PortChoice::pick(cli_udp, None, PortChoice::Follow(None))?
        .resolve(port)
        .expect("--udp never turns the leg off");
    // The TCP leg is on by default at the host port; `--no-tcp` (or
    // `tcp = false` in the config) turns it off, a bind moves it.
    let tcp_bind = PortChoice::pick(cli_tcp, cfg.gui.tcp, PortChoice::Follow(None))
        .map_err(|e| format!("[gui].tcp: {e}"))?
        .resolve(port);
    // The WebSocket leg is opt-in (`--ws`, or `ws = true`/a port in the
    // config), the audio server's own semantics; following the host port means
    // a host moved off 57210 takes its WS leg along instead of leaving it on
    // the default's neighbour.
    let ws_bind = PortChoice::pick(cli_ws, cfg.gui.ws, PortChoice::Off)
        .map_err(|e| format!("[gui].ws: {e}"))?
        .resolve(port.saturating_add(WS_PORT_OFFSET));
    let max_frame = cli_max_frame
        .or(cfg.gui.max_frame)
        .unwrap_or(clausters_core::osc::DEFAULT_MAX_FRAME)
        .max(65536);
    let server = cli_server.or_else(|| cfg.gui.server.clone());
    let shm = cli_shm.or_else(|| cfg.gui.shm.clone());
    let headless = cli_headless || cfg.gui.headless == Some(true);
    // The windows' antialiasing: a sample count the GPU is asked for and clamps
    // (see `Gpu::new`). 1 is no multisampling, which is what an oscilloscope
    // trace wants and what every build drew before this was a flag.
    let msaa = cli_msaa.or(cfg.gui.msaa).unwrap_or(1).max(1);
    // The host's look: the default theme, overlaid by [gui.theme] from the
    // config, then by a --theme file. Unknown roles or bad colors warn and
    // fall through, so a stale style file degrades to the default look.
    let mut theme = Theme::default();
    if let Some(table) = &cfg.gui.theme {
        for w in theme.overlay(table.iter().map(|(k, v)| (k.as_str(), v.as_str()))) {
            tracing::warn!("{w} (config [gui.theme])");
        }
    }
    if let Some(path) = &theme_path {
        let table = clausters_core::config::read_theme_file(Path::new(path))?;
        for w in theme.overlay(table.iter().map(|(k, v)| (k.as_str(), v.as_str()))) {
            tracing::warn!("{w} ({path})");
        }
    }
    // The host's sizing: the generated metrics table, overlaid by
    // [gui.metrics] from the config (whose reserved `scale` key regenerates it
    // at another density). Unknown roles or unusable numbers warn and fall
    // through, exactly as the theme's do.
    let mut metrics = Metrics::default();
    if let Some(table) = &cfg.gui.metrics {
        for w in metrics.overlay(table.iter().map(|(k, v)| (k.as_str(), v.as_f64()))) {
            tracing::warn!("{w} (config [gui.metrics])");
        }
    }
    // How much recorded audio a picture waits for before it re-reads its
    // summary. Zero or less means every tick, which is what it did before the
    // block existed and what a measurement wants.
    let follow_block = cli_follow_block.or(cfg.gui.follow_block).unwrap_or(0.0);
    let look = Look {
        theme,
        metrics,
        msaa,
        follow_block,
    };
    // The data directory: an explicit flag wins; otherwise the standalone
    // section (when booting one) then the gui section provide it; finally the
    // XDG fallback resolves a default.
    let data_dir = cli_data_dir
        .or_else(|| {
            standalone_flag
                .then(|| cfg.standalone.data_dir.clone())
                .flatten()
        })
        .or_else(|| cfg.gui.data_dir.clone());
    let resolved_dir = store::resolve_data_dir(data_dir.as_deref());

    // A session: the host opens a document and owns it. No store, no embedded
    // server and no script -- the piece is not played yet, which is what
    // separates this from `--standalone` and is named in the plan rather than
    // implied here.
    if let Some(path) = session_path {
        return run_session(&path, save_to.as_deref(), udp_bind, look, shm, server);
    }

    // Standalone: boot a saved GuiDef against an embedded server, no separate
    // server process and no language client. Built only with the `standalone`
    // feature (it links the server crate); otherwise it is a friendly error.
    if standalone_flag {
        let name = cli_standalone_name
            .or_else(|| cfg.standalone.gui.clone())
            .ok_or_else(|| {
                "--standalone needs a GuiDef name: give it on the command line or set \
                 [standalone].gui in the config"
                    .to_string()
            })?;
        // `boot = false` in the config suppresses the GuiDef's own boot messages.
        let run_boot = cfg.standalone.boot != Some(false);
        #[cfg(feature = "standalone")]
        {
            let dir = resolved_dir.ok_or_else(|| {
                "--standalone needs a data directory (--data-dir, [standalone].data_dir, or \
                 $CLAUSTERS_DATA_DIR / $XDG_DATA_HOME / $HOME); none could be resolved"
                    .to_string()
            })?;
            let store = GuiStore::open(&dir).map_err(|e| {
                format!(
                    "--standalone: cannot open the GuiDef store at {}: {e}",
                    dir.display()
                )
            })?;
            return run_standalone(&name, store, &dir, udp_bind, run_boot, look);
        }
        #[cfg(not(feature = "standalone"))]
        {
            let _ = (&name, &resolved_dir, udp_bind, run_boot, look);
            return Err("this clausters-gui was built without standalone support; \
                        rebuild with `--features standalone` (it links the embedded server)"
                .to_string());
        }
    }

    let store = resolved_dir.as_deref().and_then(open_store);

    let socket =
        UdpSocket::bind(udp_bind).map_err(|e| format!("failed to bind UDP {udp_bind}: {e}"))?;
    let local = socket.local_addr().map_err(|e| e.to_string())?;

    let mut host = Host::new();
    look.apply(&mut host);
    load_face(
        &mut host,
        font_path.or_else(|| cfg.gui.font.clone()),
        headless,
    );
    if let Some(store) = store {
        host = host.with_store(store);
    }
    if let Some(spec) = server {
        let target = resolve(&spec)?;
        let leg = ServerLeg::connect(target).map_err(|e| format!("client leg: {e}"))?;
        tracing::info!("client leg ready: host -> audio server at {}", leg.target());
        host = host.with_server(leg);
    }

    let mode = if headless { "headless" } else { "windowed" };
    tracing::info!("clausters-gui host listening on udp://{local} ({mode}; script -> host)");
    if headless {
        if shm.is_some() {
            tracing::warn!("--shm has no effect headless (meters need a window)");
        }
        // The TCP/WS legs' readers wake the serve loop through its own UDP
        // socket.
        let hub = match tcp_bind {
            Some(bind) => {
                let hub =
                    transport::bind_tcp(&socket, bind, max_frame).map_err(|e| e.to_string())?;
                tracing::info!(
                    "clausters-gui host listening on tcp://{} (script -> host)",
                    hub.local_addr()
                );
                Some(hub)
            }
            None => None,
        };
        let ws_hub = match ws_bind {
            Some(bind) => {
                let hub =
                    transport::bind_ws(&socket, bind, max_frame).map_err(|e| e.to_string())?;
                tracing::info!(
                    "clausters-gui host listening on ws://{} (script -> host, browser-reachable)",
                    hub.local_addr()
                );
                Some(hub)
            }
            None => None,
        };
        transport::serve(host, socket, hub, ws_hub).map_err(|e| e.to_string())
    } else {
        // The segment carries two things this host wants and they come from
        // one path: the buses its meters read, and the **samples** it draws
        // and edits in place rather than fetching and sending back.
        #[cfg(unix)]
        let bus = {
            let (bus, buffers) = gui::open_shm_buffers(shm, HeadClock::Device);
            if let Some(buffers) = buffers {
                host.set_shared_buffers(buffers);
            }
            bus
        };
        #[cfg(not(unix))]
        let bus = gui::open_shm(shm);
        gui::run(
            host,
            Arc::new(socket),
            bus,
            tcp_bind.map(|bind| (bind, max_frame)),
            ws_bind.map(|bind| (bind, max_frame)),
        )
    }
}

/// Points the host at the typeface it draws with: the path the command line or
/// the config named, or one of the system's faces. Only a build with a
/// rasterizer can use one — without the feature a named path is a warning, not
/// an error, since the bitmap face draws either way.
fn load_face(host: &mut Host, path: Option<String>, headless: bool) {
    #[cfg(feature = "font-atlas")]
    {
        if headless {
            return; // nothing draws; a face would be read for nobody
        }
        let source = match &path {
            Some(p) => Some(clausters_gui::host::fontfile::FontFile::at(p)),
            None => clausters_gui::host::fontfile::FontFile::system(),
        };
        match source {
            Some(face) => {
                let at = face.path().display().to_string();
                if host.load_face(&face) {
                    tracing::info!("drawing text with the typeface at {at}");
                } else {
                    tracing::warn!(
                        "{at} is not a typeface this host can read; \
                                    drawing with the embedded bitmap face"
                    );
                }
            }
            None => tracing::info!("no typeface found; drawing with the embedded bitmap face"),
        }
    }
    #[cfg(not(feature = "font-atlas"))]
    {
        let _ = (host, headless);
        if path.is_some() {
            tracing::warn!(
                "--font needs a host built with `--features font-atlas`; \
                 drawing with the embedded bitmap face"
            );
        }
    }
}

/// Opens the GuiDef store at the resolved data directory, logging and disabling
/// persistence on failure (rather than refusing to start).
fn open_store(dir: &Path) -> Option<GuiStore> {
    match GuiStore::open(dir) {
        Ok(store) => {
            tracing::info!("GuiDef store at {}", dir.join("defs/guidefs").display());
            Some(store)
        }
        Err(e) => {
            tracing::warn!("cannot open the GuiDef store at {}: {e}", dir.display());
            None
        }
    }
}

/// Opens a session and draws it, with this host as its **owner**.
///
/// The third writer, and the shortest statement of what that means: a document
/// is read with the crate every writer reads it with, drawn as an ordinary
/// GuiDef, and edited by gestures this host applies to itself. Nothing here
/// parses the format, decides what an edit means or remembers an inverse —
/// those are the crate's, which is the whole reason a session survives being
/// passed between writers.
fn run_session(
    path: &str,
    save_to: Option<&str>,
    udp_bind: SocketAddr,
    look: Look,
    #[cfg_attr(not(feature = "standalone"), allow(unused_variables))] shm: Option<String>,
    #[cfg_attr(not(feature = "standalone"), allow(unused_variables))] player: Option<String>,
) -> Result<(), String> {
    use clausters_gui::host::document::{Owner, sources, tree};

    let mut owner = Owner::open(path)?;

    // **The samples, before the picture.** A document says what plays when and
    // never where its samples are; the session's table says that, and a host
    // that can read it draws takes instead of empty rectangles. The buffers are
    // read first and waited for, because a clip's fetch starts the moment the
    // tree is handed over and would ask the server for a buffer it has not
    // filled yet.
    let beside = Path::new(path).parent().unwrap_or(Path::new("."));
    let load = owner
        .session
        .as_ref()
        .map(|session| sources::plan(session, beside, 0))
        .unwrap_or_default();
    for (id, why) in &load.unresolved {
        tracing::warn!("session: source {} is not loadable: {why}", id.0);
    }

    let mut host = Host::new();
    #[cfg(feature = "standalone")]
    // The player is held, not used: it is a process this editor may own, and
    // dropping it is what stops it when the window closes.
    let (bus, _player) = attach_server(&mut host, &load, shm, player)?;
    #[cfg(not(feature = "standalone"))]
    let bus: Option<Arc<dyn clausters_gui::host::BusSource>> = None;
    #[cfg(not(feature = "standalone"))]
    if !load.messages.is_empty() {
        tracing::warn!(
            "session: {} take(s) will draw empty — this clausters-gui was built without \
             standalone support, so there is no server to read them into (rebuild with \
             `--features standalone`)",
            load.messages.len()
        );
    }

    let name = |p: &str| {
        Path::new(p)
            .file_name()
            .map_or_else(|| p.to_string(), |n| n.to_string_lossy().into_owned())
    };
    // **The title says where a save goes**, because nothing else on screen can:
    // a window that edits a file and cannot say which one leaves the reader to
    // guess, and "read-only" is worth saying outright rather than discovering
    // by pressing the key and watching nothing happen.
    let title = match save_to {
        Some(out) => format!("{} → {}", name(path), name(out)),
        None => format!("{} (read-only: no --save-to)", name(path)),
    };
    let def_id = 1;
    let drawn = tree::draw(
        &owner.document,
        &tree::Look {
            // Past the window's own id: a GuiDef's id *is* its root widget's,
            // so a tree numbering from 1 beside a def 1 collides and the
            // registry drops the whole subtree -- which is an empty window and
            // one line in the log.
            first_id: def_id + 1,
            takes: Some(&load.takes),
            ..tree::Look::default()
        },
        &title,
    );
    for bound in &drawn.bindings {
        owner.bind(bound.widget, bound.node);
    }

    // Saving is **Ctrl+S**, a user's action rather than an exit's side effect —
    // and it writes only where `--save-to` named a file, since overwriting what
    // you opened is a decision.
    if let Some(out) = save_to {
        owner = owner.saving_to(out);
    }
    look.apply(&mut host);
    host.owner = Some(
        owner
            .with_units_per_beat(tree::Look::default().units_per_beat)
            .with_takes(load.takes.clone()),
    );
    let origin = ClientId::Udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)));
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: "/gui_def".into(),
            args: vec![OscType::Int(def_id), OscType::String(drawn.def.to_string())],
        }),
        origin,
    );
    // Counted by what each child *is* rather than by arithmetic on the list:
    // the window holds lanes, one ruler and an editor per take, and a count
    // that subtracts a constant goes quietly wrong the day another pane joins.
    let editors = load.takes.len();
    let lanes = drawn.def["children"]
        .as_array()
        .map_or(0, |c| c.len())
        .saturating_sub(1 + editors);
    tracing::info!(
        "session: opened {path} — {} clip(s), {lanes} lane(s) + a ruler, {editors} take editor(s)",
        drawn.bindings.len(),
    );
    match save_to {
        Some(out) => tracing::info!("session: Ctrl+S writes {out}"),
        None => tracing::info!("session: read-only — pass --save-to <file> for Ctrl+S to write"),
    }

    let socket =
        UdpSocket::bind(udp_bind).map_err(|e| format!("failed to bind UDP {udp_bind}: {e}"))?;
    gui::run(host, Arc::new(socket), bus, None, None)
}

/// **Gives a session host its two servers.** The arrangement this whole mode
/// exists for, and the three roles it splits into.
///
/// - The **on-demand session**, in this process: it owns the samples. Every
///   take is a region beside its segment, so this host draws them by mapping
///   and edits them by storing, with nothing sent either way. It has no audio
///   device and needs none — it computes.
/// - The **player**, another process (`clausters --shm <path>`): it holds the
///   machine's input and output, and it is therefore the only one that can
///   record or make a sound. It attaches to the same segment, so it plays
///   the very samples being edited; killing it takes no take with it, and
///   the next one adopts what is there.
/// - The **editor**, this host: it performs the actions. Which is why it owns
///   the transport, allocates through the session and plays through the player.
///
/// **A failed boot is not a failed session.** The document, its edits, its undo
/// and its save need no server at all; what needs one is the sound and the
/// picture of a take. So a machine with no audio device still opens the file,
/// with the takes drawn as what they are — a warning and an empty clip, which
/// is the same honesty the unresolved sources get.
#[cfg(feature = "standalone")]
fn attach_server(
    host: &mut Host,
    load: &clausters_gui::host::document::sources::Load,
    shm: Option<String>,
    player: Option<String>,
) -> Result<Attached, String> {
    let path = std::path::PathBuf::from(shm.unwrap_or_else(default_segment_path));
    let session = match EmbedSession::open(&path, 48_000.0, 2) {
        Ok(session) => session,
        Err(e) => {
            tracing::warn!(
                "session: no on-demand server ({e}) — the document opens and edits, but takes \
                 will not draw or sound"
            );
            return Ok((None, None));
        }
    };
    tracing::info!(
        "session: samples at {} — an on-demand server owns them, and a player attaches to them",
        path.display()
    );

    // **The samples go to their owner.** A take is read into a buffer of the
    // session, which puts it in a region beside the segment; from there the
    // player maps it and this host draws it.
    for msg in &load.messages {
        send_session(&session, msg.clone())?;
    }
    if !load.messages.is_empty() {
        // **Waited for, not fired and forgotten.** A buffer read is
        // asynchronous, and a clip's fetch starts the moment the tree reaches
        // the host: without this the window would ask for the shape of a buffer
        // that is still empty, once, and draw nothing forever.
        await_reads(&session, load.messages.len());
    }

    // **The picture, from the memory the samples are in.** The same file the
    // player attaches to: the segment for the clocks and the buses, the
    // regions beside it for the samples.
    #[cfg(unix)]
    let bus = {
        let (bus, buffers) =
            gui::open_shm_buffers(Some(path.display().to_string()), HeadClock::Piece);
        match buffers {
            Some(buffers) => host.set_shared_buffers(buffers),
            None => tracing::warn!("session: the buffers could not be mapped; takes will fetch"),
        }
        bus
    };
    #[cfg(not(unix))]
    let bus: Option<Arc<dyn clausters_gui::host::BusSource>> = None;
    if bus.is_none() {
        tracing::warn!("session: no data plane — the playhead will stand still");
    }

    // **The player**, given one: what sounds, records and moves the transport.
    let owned = match attach_player(host, &path, player, &load.takes.bufnums()) {
        Ok(owned) => {
            host.set_owns_transport(true);
            owned
        }
        Err(e) => {
            tracing::warn!(
                "session: no player ({e}) — the document opens, draws and edits, and nothing \
                 sounds. Start one yourself with `clausters --shm {}` and pass --server",
                path.display()
            );
            None
        }
    };
    host.set_server_link(ServerLink::Session(session));
    Ok((bus, owned))
}

/// A player process this editor started, killed when the editor goes.
///
/// An application starts its own engine — the Python client boots a server
/// when none answers, and this is the same posture: a person who opens a
/// session wants to hear it, not to arrange two processes by hand. A player
/// the user started themselves (`--server`) is not owned and not killed.
#[cfg(feature = "standalone")]
struct OwnedPlayer(std::process::Child);

/// What a session's servers hand back: the data plane its windows read, and
/// the player process it owns, if it started one.
#[cfg(feature = "standalone")]
type Attached = (
    Option<Arc<dyn clausters_gui::host::BusSource>>,
    Option<OwnedPlayer>,
);

#[cfg(feature = "standalone")]
impl Drop for OwnedPlayer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Starts a player against `segment` and returns where it is listening.
///
/// **The segment exists by now**, which is what fixes the ordering: the
/// editor's session created it and claimed the command plane, so the player
/// attaches to what is there instead of truncating it and taking the samples
/// with it. Started the other way round it would be the owner, and the session
/// would refuse to open.
#[cfg(feature = "standalone")]
fn spawn_player(segment: &Path) -> Result<(OwnedPlayer, String), String> {
    // The binary beside this one, unless an override says otherwise: an
    // editor and its player ship together.
    let here = std::env::current_exe().ok();
    let dir = here.as_ref().and_then(|p| p.parent());
    let exe = std::env::var_os("CLAUSTERS_BIN")
        .map(std::path::PathBuf::from)
        .or_else(|| dir.map(|d| d.join("clausters")).filter(|p| p.is_file()))
        // The development checkout: this crate is its own workspace, so its
        // binaries are under `clients/gui/target/<profile>` while the server's
        // are under the repo's own. Guessed rather than required, because a
        // person running from a build tree should not have to set a variable
        // to hear their session.
        .or_else(|| {
            dir.and_then(|d| {
                d.parent()?
                    .parent()?
                    .parent()?
                    .parent()
                    .map(Path::to_path_buf)
            })
            .map(|root| {
                root.join("target")
                    .join(if cfg!(debug_assertions) {
                        "debug"
                    } else {
                        "release"
                    })
                    .join("clausters")
            })
            .filter(|p| p.is_file())
        })
        .unwrap_or_else(|| std::path::PathBuf::from("clausters"));
    // A port of its own, so an editor never collides with a server somebody
    // else is running on the default one.
    let port = 57310 + (std::process::id() % 200) as u16;
    let child = std::process::Command::new(&exe)
        .arg("--shm")
        .arg(segment)
        .arg("--port")
        .arg(port.to_string())
        .arg("--client-name")
        .arg("clausters-editor")
        .spawn()
        .map_err(|e| format!("cannot start a player ({}): {e}", exe.display()))?;
    Ok((OwnedPlayer(child), format!("127.0.0.1:{port}")))
}

/// Attaches the player and hands it what it needs to sound a take: the
/// monitor's def, its transport-bound group, and a `/buffer_attach` per take
/// the session just read — because the player maps the directory once, at
/// startup, and these arrived after it.
///
/// Returns whether one was attached at all.
#[cfg(feature = "standalone")]
fn attach_player(
    host: &mut Host,
    segment: &Path,
    player: Option<String>,
    takes: &[i32],
) -> Result<Option<OwnedPlayer>, String> {
    // A player the user started is used as it is; with none named, the editor
    // starts one of its own — it is an application, and an application does
    // not ask you to arrange its processes by hand.
    let (owned, spec) = match player {
        Some(spec) => (None, spec),
        None => {
            let (owned, spec) = spawn_player(segment)?;
            (Some(owned), spec)
        }
    };
    let target = resolve(&spec)?;
    let leg = ServerLeg::connect(target).map_err(|e| format!("player leg: {e}"))?;
    // A spawned player takes a moment to bind its socket, and every message
    // below would land in nothing. Wait for it to answer before saying it is
    // there — three seconds is a boot, not a hang.
    if owned.is_some() {
        await_player(&leg)?;
    }
    tracing::info!(
        "session: player at {} (it holds the devices; the samples stay at {})",
        leg.target(),
        segment.display()
    );
    // The monitor's def goes with the samples: a take is data, and what sounds
    // it is an instrument. Sent before anything can press the space bar.
    leg.send(clausters_gui::host::play::take_def_message())
        .map_err(|e| e.to_string())?;
    // The monitor's own group, bound to the transport: what `/transport_stop`
    // freezes and `/transport_play` thaws. Created stopped, so a reader added
    // to it stands still until a hand asks for sound.
    for msg in clausters_gui::host::play::take_group_messages() {
        leg.send(msg).map_err(|e| e.to_string())?;
    }
    // **The takes, by number and not by sample.** A player maps the buffer
    // directory when it starts, and these were read into it afterwards — so it
    // is pointed at them, which is the whole message: no blob, no copy, and
    // the very cells this editor is about to draw.
    for &bufnum in takes {
        leg.send(OscMessage {
            addr: "/buffer_attach".into(),
            args: vec![OscType::Int(bufnum)],
        })
        .map_err(|e| e.to_string())?;
    }
    host.set_player_link(ServerLink::Udp(leg));
    Ok(owned)
}

/// Waits for a freshly started player to answer, so nothing is sent into a
/// socket that is not listening yet.
#[cfg(feature = "standalone")]
fn await_player(leg: &ServerLeg) -> Result<(), String> {
    use std::time::{Duration, Instant};

    let socket = leg.socket();
    socket
        .set_read_timeout(Some(Duration::from_millis(200)))
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut buf = [0u8; 4096];
    while Instant::now() < deadline {
        leg.send(OscMessage {
            addr: "/server_status".into(),
            args: vec![],
        })
        .map_err(|e| e.to_string())?;
        if socket.recv_from(&mut buf).is_ok() {
            return Ok(());
        }
    }
    Err("the player did not answer within 3s".into())
}

/// Where an editor puts its segment when nobody said: a memory filesystem if
/// there is one, the temp directory otherwise, named for this process so two
/// editors never share one.
#[cfg(feature = "standalone")]
fn default_segment_path() -> String {
    let dir = if Path::new("/dev/shm").is_dir() {
        std::path::PathBuf::from("/dev/shm")
    } else {
        std::env::temp_dir()
    };
    dir.join(format!("clausters-editor-{}", std::process::id()))
        .display()
        .to_string()
}

/// Sends one message to the session that owns the samples.
#[cfg(feature = "standalone")]
fn send_session(session: &EmbedSession, msg: OscMessage) -> Result<(), String> {
    let addr = msg.addr.clone();
    let bytes = encode(&OscPacket::Message(msg)).map_err(|e| e.to_string())?;
    if !session.send(&bytes) {
        tracing::warn!("session: {addr} dropped (the session's command ring is full)");
    }
    Ok(())
}

/// Waits for `n` buffer reads to answer, reporting each failure by its own
/// words. Bounded: a read that never answers costs a few seconds and a line,
/// not a window that never opens.
#[cfg(feature = "standalone")]
fn await_reads(session: &EmbedSession, n: usize) {
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut answered = 0;
    let mut buf = vec![0u8; 65536];
    while answered < n && Instant::now() < deadline {
        let Some(len) = session.poll_into(&mut buf) else {
            std::thread::sleep(Duration::from_millis(5));
            continue;
        };
        let Ok(OscPacket::Message(msg)) = clausters_core::osc::decode_packet(&buf[..len]) else {
            continue;
        };
        let of = |args: &[OscType]| match args.first() {
            Some(OscType::String(s)) => s.clone(),
            _ => String::new(),
        };
        match msg.addr.as_str() {
            "/done" if of(&msg.args) == "/buffer_allocRead" => answered += 1,
            "/fail" if of(&msg.args) == "/buffer_allocRead" => {
                answered += 1;
                tracing::warn!("session: a take did not load: {:?}", msg.args);
            }
            _ => {}
        }
    }
    if answered < n {
        tracing::warn!(
            "session: {} of {n} take(s) had not loaded after 10s — they will draw empty",
            n - answered
        );
    } else {
        tracing::info!("session: {n} take(s) read into buffers");
    }
}

/// Boots a saved GuiDef as a self-contained app: an embedded audio server, the
/// bundle's defs loaded into it, the GuiDef's `boot` messages run, then its
/// window opened. No separate server process and no language client.
#[cfg(feature = "standalone")]
fn run_standalone(
    name: &str,
    store: GuiStore,
    data_dir: &Path,
    udp_bind: SocketAddr,
    run_boot: bool,
    look: Look,
) -> Result<(), String> {
    let (id, json) = store
        .load(name)
        .map_err(|e| format!("--standalone: loading GuiDef \"{name}\": {e}"))?;

    // A manifest that declares the component contract makes this bundle a
    // *template*: its symbols are allocated and its holes resolved, the same
    // pass a browser tab runs, so one directory behaves identically on both
    // legs. Without one — or with a manifest written before the contract — the
    // saved tree is opened verbatim, exactly as it always was.
    let manifest = bundle::read_manifest(data_dir).filter(bundle::is_symbolic);
    let mounted = match &manifest {
        Some(manifest) => {
            // The store hands back the record's two halves — its id and its
            // tree — so the template is put back together here rather than
            // re-parsed off disk.
            let template = bundle::Template {
                id,
                gui: serde_json::from_slice(&json)
                    .map_err(|e| format!("--standalone: GuiDef \"{name}\" is not a tree: {e}"))?,
            };
            let mut alloc = bundle::MountAllocator::default();
            let mount = bundle::mount(
                manifest,
                &template,
                &data_dir.to_string_lossy(),
                &mut alloc,
                &Default::default(),
            )
            .map_err(|e| format!("--standalone: mounting \"{name}\": {e}"))?;
            Some(mount)
        }
        None => None,
    };
    let (id, json) = match &mounted {
        Some(mount) => (mount.def_id, mount.tree.clone()),
        None => (id, json),
    };

    // The embedded server loads the bundle's defs itself from the data directory
    // (SynthDefs, FaustDefs with the `faust` feature, GraphDefs, MIDI bindings
    // and the boot.json preset) — the same startup the standalone server binary
    // performs, so the GUI no longer replays specs by hand.
    let embed = EmbedServer::open_with_data_dir(Some(data_dir))?;
    tracing::info!(
        "standalone: embedded audio server started, defs loaded from {}",
        data_dir.display()
    );

    // Bring the instrument up: the GuiDef's own `boot` messages (e.g. an /synth_new),
    // unless the config disabled it. A mounted bundle's boot list came out of
    // the resolver with its ids already filled in.
    if run_boot {
        let boot = match &mounted {
            Some(mount) => mount.messages.clone(),
            None => store::boot_messages(&json),
        };
        for msg in &boot {
            send_osc(&embed, msg.clone())?;
        }
        tracing::info!("standalone: sent {} boot message(s)", boot.len());
    }

    // Register the GuiDef so the windowed front opens it on resume. The embed is
    // the host's server link, so bound widgets drive it directly.
    let mut host = Host::new()
        .with_server_link(ServerLink::Embed(embed))
        .with_store(store);
    look.apply(&mut host);
    let origin = ClientId::Udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)));
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: "/gui_def".into(),
            args: vec![
                OscType::Int(id),
                OscType::String(String::from_utf8_lossy(&json).into_owned()),
            ],
        }),
        origin,
    );

    // gui::run still wants a script-front socket, unused in standalone (a
    // script could still attach over UDP; the TCP and WS legs stay off here).
    let socket =
        UdpSocket::bind(udp_bind).map_err(|e| format!("failed to bind UDP {udp_bind}: {e}"))?;
    tracing::info!("standalone: opening GuiDef \"{name}\" (id {id})");
    gui::run(host, Arc::new(socket), None, None, None)
}

/// Encodes and sends one OSC message to the embedded server, warning if the ring
/// is momentarily full.
#[cfg(feature = "standalone")]
fn send_osc(embed: &EmbedServer, msg: OscMessage) -> Result<(), String> {
    let addr = msg.addr.clone();
    let bytes = encode(&OscPacket::Message(msg)).map_err(|e| e.to_string())?;
    if !embed.send(&bytes) {
        tracing::warn!("standalone: {addr} dropped (embed command ring full)");
    }
    Ok(())
}

/// Resolves a `host:port` (or bare `:port` / `port`) to a socket address.
fn resolve(spec: &str) -> Result<SocketAddr, String> {
    let spec = if spec.starts_with(':') {
        format!("127.0.0.1{spec}")
    } else if !spec.contains(':') {
        format!("127.0.0.1:{spec}")
    } else {
        spec.to_string()
    };
    spec.to_socket_addrs()
        .map_err(|e| format!("--server {spec}: {e}"))?
        .next()
        .ok_or_else(|| format!("--server {spec}: no address resolved"))
}

fn init_logging(verbosity: i8) {
    use tracing_subscriber::{EnvFilter, fmt};
    let level = match verbosity {
        i if i <= -1 => "error",
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    // RUST_LOG wins if set, like the server; otherwise the -v level applies.
    // sctk-adwaita (winit's Wayland window decorations) logs a benign ERROR
    // when the XDG settings portal misses its 100 ms color-scheme query (it
    // just falls back to the default theme), so it is muted by default.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("clausters_gui={level},sctk_adwaita=off,warn")));
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
