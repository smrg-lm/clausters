//! The `clausters-gui` host binary.
//!
//! Starts the GUI host's server front (UDP + TCP, one port) and, optionally,
//! its client leg to the audio server, then runs the `/gui_*` protocol. By default it opens windows
//! (winit + wgpu): a `window`-rooted GuiDef instantiates an OS window hosting the
//! renderers. With `--headless` it runs the protocol with no display — for
//! tests, automation and machines with no GPU. Drive it from a language client
//! over OSC — see `clients/python/examples/gui_window.py` (windowed) and
//! `gui_skeleton.py` (protocol only).

use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use clausters_core::config::Config;
use clausters_gui::host::store::{self, GuiStore};
use clausters_gui::host::theme::Theme;
use clausters_gui::host::transport::{self, DEFAULT_PORT};
use clausters_gui::host::{Host, ServerLeg, gui};

// The standalone boot links the server crate directly (the `standalone`
// feature); these are only needed on that path.
#[cfg(feature = "standalone")]
use clausters_core::osc::{OscMessage, OscPacket, OscType, encode};
#[cfg(feature = "standalone")]
use clausters_gui::host::bundle;
#[cfg(feature = "standalone")]
use clausters_gui::host::embed::EmbedServer;
#[cfg(feature = "standalone")]
use clausters_gui::host::{ClientId, ServerLink};
#[cfg(feature = "standalone")]
use std::net::Ipv4Addr;

const USAGE: &str = "\
usage:
  clausters-gui [--port <n>] [--server <host:port>] [--shm <path>] [--headless]
                [--tcp [port] | --no-tcp] [--ws [port]] [--max-frame <bytes>]
                [--data-dir <dir>] [--standalone [name]] [--config <path>]
                [--theme <path>]
      --port <n>            port for the GUI host's server front
                            (script -> host, UDP and TCP); default 57210
      --tcp [port]          length-prefixed OSC over TCP — on by default at the
                            host port; the flag only moves it
      --no-tcp              disable the TCP leg (UDP-only front)
      --ws [port]           also accept /gui_* over WebSocket, reachable from a
                            browser (default port 57220; ws://host:port/) — the
                            same flag the audio server takes
      --max-frame <bytes>   largest OSC frame on the stream legs, TCP and
                            WebSocket alike (default 16 MiB). A DoS ceiling,
                            not a protocol limit; UDP keeps the ~64 KB
                            datagram cap
      --server <host:port>  also attach the client leg to a running audio
                            server (host -> audio server); default off.
                            Needed for waveform widgets that reference a
                            server buffer number, and for bound widgets
                            (/gui_bind) to forward their value to the server.
      --shm <path>          map the audio server's shared-memory segment (its
                            own --shm path) for zero-message meters/scopes;
                            Unix only, default off
      --data-dir <dir>      data directory for the GuiDef store (named GuiDefs
                            persist there; /gui_load reads from it). Defaults to
                            the same place the server uses ($CLAUSTERS_DATA_DIR,
                            $XDG_DATA_HOME/clausters, ~/.local/share/clausters).
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

fn run(args: &[String]) -> Result<(), String> {
    // CLI overrides are collected as `Option`s, then resolved against the config
    // file (the compiled default is the last fallback). Precedence per option:
    // flag > project clausters.toml > user config.toml > default.
    let mut cli_port: Option<u16> = None;
    let mut cli_tcp: Option<Option<u16>> = None; // None = unset, Some(None) = off
    let mut cli_ws: Option<u16> = None;
    let mut cli_max_frame: Option<usize> = None;
    let mut cli_server: Option<String> = None;
    let mut cli_shm: Option<String> = None;
    let mut cli_headless = false;
    let mut cli_data_dir: Option<String> = None;
    let mut standalone_flag = false;
    let mut cli_standalone_name: Option<String> = None;
    let mut config_path: Option<String> = None;
    let mut theme_path: Option<String> = None;
    let mut it = args.iter().peekable();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--port" => {
                let v = it
                    .next()
                    .ok_or_else(|| format!("--port needs a value\n{USAGE}"))?;
                cli_port = Some(v.parse().map_err(|e| format!("--port: {e}"))?);
            }
            "--tcp" => {
                // Optional port; without one, the TCP leg rides the host port.
                let mut port = None;
                if let Some(next) = it.peek()
                    && let Ok(p) = next.parse::<u16>()
                {
                    port = Some(p);
                    it.next();
                }
                cli_tcp = Some(port.or(Some(0))); // 0 = "the host port", resolved below
            }
            "--no-tcp" => cli_tcp = Some(None),
            "--ws" => {
                // Optional port; defaults away from the host port, since the
                // WS leg binds its own TCP listener (the server's pattern).
                let mut port = DEFAULT_PORT + 10;
                if let Some(next) = it.peek()
                    && let Ok(p) = next.parse::<u16>()
                {
                    port = p;
                    it.next();
                }
                cli_ws = Some(port);
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
    // The TCP leg is on by default at the host port; `--no-tcp` (or
    // `tcp = false` in the config) turns it off, a port moves it. The CLI's
    // port-0 sentinel means "--tcp with no port": ride the host port.
    let tcp_port: Option<u16> = match cli_tcp {
        Some(Some(0)) => Some(port),
        Some(Some(p)) => Some(p),
        Some(None) => None,
        None => match cfg.gui.tcp {
            Some(setting) => setting.resolve(port),
            None => Some(port),
        },
    };
    // The WebSocket leg is opt-in (`--ws`, or `ws = true`/a port in the
    // config), the audio server's own semantics; its default port sits clear
    // of the host port since both bind a TCP listener.
    let ws_port: Option<u16> =
        cli_ws.or_else(|| cfg.gui.ws.and_then(|w| w.resolve(DEFAULT_PORT + 10)));
    let max_frame = cli_max_frame
        .or(cfg.gui.max_frame)
        .unwrap_or(clausters_core::osc::DEFAULT_MAX_FRAME)
        .max(65536);
    let server = cli_server.or_else(|| cfg.gui.server.clone());
    let shm = cli_shm.or_else(|| cfg.gui.shm.clone());
    let headless = cli_headless || cfg.gui.headless == Some(true);
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
            return run_standalone(&name, store, &dir, port, run_boot, theme);
        }
        #[cfg(not(feature = "standalone"))]
        {
            let _ = (&name, &resolved_dir, port, run_boot);
            return Err("this clausters-gui was built without standalone support; \
                        rebuild with `--features standalone` (it links the embedded server)"
                .to_string());
        }
    }

    let store = resolved_dir.as_deref().and_then(open_store);

    let socket = UdpSocket::bind(("127.0.0.1", port))
        .map_err(|e| format!("failed to bind UDP port {port}: {e}"))?;
    let local = socket.local_addr().map_err(|e| e.to_string())?;

    let mut host = Host::new();
    host.theme = theme;
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
        let hub = match tcp_port {
            Some(p) => {
                let hub = transport::bind_tcp(&socket, p, max_frame).map_err(|e| e.to_string())?;
                tracing::info!(
                    "clausters-gui host listening on tcp://{} (script -> host)",
                    hub.local_addr()
                );
                Some(hub)
            }
            None => None,
        };
        let ws_hub = match ws_port {
            Some(p) => {
                let hub = transport::bind_ws(&socket, p, max_frame).map_err(|e| e.to_string())?;
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
        gui::run(
            host,
            Arc::new(socket),
            shm,
            tcp_port.map(|p| (p, max_frame)),
            ws_port.map(|p| (p, max_frame)),
        )
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

/// Boots a saved GuiDef as a self-contained app: an embedded audio server, the
/// bundle's defs loaded into it, the GuiDef's `boot` messages run, then its
/// window opened. No separate server process and no language client.
#[cfg(feature = "standalone")]
fn run_standalone(
    name: &str,
    store: GuiStore,
    data_dir: &Path,
    port: u16,
    run_boot: bool,
    theme: Theme,
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

    // Bring the instrument up: the GuiDef's own `boot` messages (e.g. an /s_new),
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
    host.theme = theme;
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
    let socket = UdpSocket::bind(("127.0.0.1", port))
        .map_err(|e| format!("failed to bind UDP port {port}: {e}"))?;
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
