//! The `clausters-gui` host binary.
//!
//! Starts the GUI host's UDP server front and, optionally, its client leg to the
//! audio server, then runs the `/gui_*` protocol. By default it opens windows
//! (winit + wgpu): a `window`-rooted GuiDef instantiates an OS window hosting the
//! renderers. With `--headless` it runs the protocol with no display — for
//! tests, automation and machines with no GPU. Drive it from a language client
//! over OSC — see `clients/python/examples/gui_window.py` (windowed) and
//! `gui_skeleton.py` (protocol only).

use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::process::ExitCode;
use std::sync::Arc;

use clausters_gui::host::store::{self, GuiStore};
use clausters_gui::host::transport::{self, DEFAULT_PORT};
use clausters_gui::host::{Host, ServerLeg, gui};

// The standalone boot links the server crate directly (the `standalone`
// feature); these are only needed on that path.
#[cfg(feature = "standalone")]
use clausters_core::osc::{OscMessage, OscPacket, OscType, encode};
#[cfg(feature = "standalone")]
use clausters_gui::host::embed::EmbedServer;
#[cfg(feature = "standalone")]
use clausters_gui::host::{ClientId, ServerLink};
#[cfg(feature = "standalone")]
use std::net::Ipv4Addr;

const USAGE: &str = "\
usage:
  clausters-gui [--port <n>] [--server <host:port>] [--shm <path>] [--headless]
                [--data-dir <dir>] [--standalone <name>]
      --port <n>            UDP port for the GUI host's server front
                            (script -> host); default 57210
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
      --standalone <name>   boot the saved GuiDef <name> against an embedded
                            audio server (no separate server or language client):
                            loads the bundle's SynthDefs/GraphDefs, runs the
                            GuiDef's `boot` messages, and opens its window. A
                            self-contained app.
      --headless            run the protocol with no display (tests / no GPU);
                            the default opens windows (winit + wgpu)
  -v, -vv, -vvv             log verbosity: warn (default) -> info -> debug ->
                            trace; -q for errors only. RUST_LOG overrides it.

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
    let mut port = DEFAULT_PORT;
    let mut server: Option<String> = None;
    let mut shm: Option<String> = None;
    let mut headless = false;
    let mut data_dir: Option<String> = None;
    let mut standalone: Option<String> = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--port" => {
                let v = it
                    .next()
                    .ok_or_else(|| format!("--port needs a value\n{USAGE}"))?;
                port = v.parse().map_err(|e| format!("--port: {e}"))?;
            }
            "--server" => {
                let v = it
                    .next()
                    .ok_or_else(|| format!("--server needs host:port\n{USAGE}"))?;
                server = Some(v.clone());
            }
            "--shm" => {
                let v = it
                    .next()
                    .ok_or_else(|| format!("--shm needs a path\n{USAGE}"))?;
                shm = Some(v.clone());
            }
            "--data-dir" => {
                let v = it
                    .next()
                    .ok_or_else(|| format!("--data-dir needs a path\n{USAGE}"))?;
                data_dir = Some(v.clone());
            }
            "--standalone" => {
                let v = it
                    .next()
                    .ok_or_else(|| format!("--standalone needs a GuiDef name\n{USAGE}"))?;
                standalone = Some(v.clone());
            }
            "--headless" => headless = true,
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => return Err(format!("unknown argument: {other}\n{USAGE}")),
        }
    }

    let store = open_store(data_dir.as_deref());

    // Standalone: boot a saved GuiDef against an embedded server, no separate
    // server process and no language client. Built only with the `standalone`
    // feature (it links the server crate); otherwise it is a friendly error.
    if let Some(name) = standalone {
        #[cfg(feature = "standalone")]
        {
            let store = store.ok_or_else(|| {
                "--standalone needs a data directory (--data-dir, or $CLAUSTERS_DATA_DIR / \
                 $XDG_DATA_HOME / $HOME); none could be resolved"
                    .to_string()
            })?;
            return run_standalone(&name, store, port);
        }
        #[cfg(not(feature = "standalone"))]
        {
            let _ = (&name, &store, port);
            return Err("this clausters-gui was built without standalone support; \
                        rebuild with `--features standalone` (it links the embedded server)"
                .to_string());
        }
    }

    let socket = UdpSocket::bind(("127.0.0.1", port))
        .map_err(|e| format!("failed to bind UDP port {port}: {e}"))?;
    let local = socket.local_addr().map_err(|e| e.to_string())?;

    let mut host = Host::new();
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
        transport::serve(host, socket).map_err(|e| e.to_string())
    } else {
        gui::run(host, Arc::new(socket), shm)
    }
}

/// Opens the GuiDef store for the resolved data directory, logging and disabling
/// persistence on failure (rather than refusing to start).
fn open_store(cli_override: Option<&str>) -> Option<GuiStore> {
    let dir = store::resolve_data_dir(cli_override)?;
    match GuiStore::open(&dir) {
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
fn run_standalone(name: &str, store: GuiStore, port: u16) -> Result<(), String> {
    let (id, json) = store
        .load(name)
        .map_err(|e| format!("--standalone: loading GuiDef \"{name}\": {e}"))?;

    let embed = EmbedServer::open()?;
    tracing::info!("standalone: embedded audio server started");

    // Load the bundle's defs (order preserved by the ring, so a `boot` /s_new
    // sees its def). Both ride the same encode door the rest of the host uses.
    let mut defs = 0;
    for spec in store.synthdef_specs() {
        send_spec(&embed, "/d_recv", &spec)?;
        defs += 1;
    }
    for spec in store.graphdef_specs() {
        send_spec(&embed, "/d_graph", &spec)?;
        defs += 1;
    }
    tracing::info!("standalone: loaded {defs} def(s) into the embedded server");

    // Bring the instrument up: the GuiDef's `boot` messages (e.g. an /s_new).
    let boot = store::boot_messages(&json);
    for msg in &boot {
        send_osc(&embed, msg.clone())?;
    }
    tracing::info!("standalone: sent {} boot message(s)", boot.len());

    // Register the GuiDef so the windowed front opens it on resume. The embed is
    // the host's server link, so bound widgets drive it directly.
    let mut host = Host::new()
        .with_server_link(ServerLink::Embed(embed))
        .with_store(store);
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

    // gui::run still wants a script-front socket, unused in standalone.
    let socket = UdpSocket::bind(("127.0.0.1", port))
        .map_err(|e| format!("failed to bind UDP port {port}: {e}"))?;
    tracing::info!("standalone: opening GuiDef \"{name}\" (id {id})");
    gui::run(host, Arc::new(socket), None)
}

/// Sends a def spec (`/d_recv` or `/d_graph` with the JSON as a string) to the
/// embedded server.
#[cfg(feature = "standalone")]
fn send_spec(embed: &EmbedServer, addr: &str, spec: &[u8]) -> Result<(), String> {
    send_osc(
        embed,
        OscMessage {
            addr: addr.into(),
            args: vec![OscType::String(String::from_utf8_lossy(spec).into_owned())],
        },
    )
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
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("clausters_gui={level},warn")));
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
