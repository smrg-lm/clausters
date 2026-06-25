//! The `clausters-gui` host binary (headless skeleton).
//!
//! Starts the GUI host's UDP server front and, optionally, its client leg to
//! the audio server, then runs the `/gui_*` command loop. No window yet (G3
//! brings the first pixels): this validates the protocol and the host's
//! dual-role topology. Drive it from a language client over OSC — see
//! `clients/python/examples/gui_skeleton.py`.

use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::process::ExitCode;

use clausters_gui::host::transport::{self, DEFAULT_PORT};
use clausters_gui::host::{Host, ServerLeg};

const USAGE: &str = "\
usage:
  clausters-gui [--port <n>] [--server <host:port>]
      --port <n>            UDP port for the GUI host's server front
                            (script -> host); default 57210
      --server <host:port>  also attach the client leg to a running audio
                            server (host -> audio server); default off
  -v, -vv, -vvv             log verbosity: warn (default) -> info -> debug ->
                            trace; -q for errors only. RUST_LOG overrides it.

The host speaks the /gui_* widget protocol as JSON-in-OSC, the same encoding the
audio server uses. This headless skeleton interprets /gui_def, /gui_set,
/gui_free and /gui_query (replying /gui_info) and logs them; there is no GPU
yet.";

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
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => return Err(format!("unknown argument: {other}\n{USAGE}")),
        }
    }

    let socket = UdpSocket::bind(("127.0.0.1", port))
        .map_err(|e| format!("failed to bind UDP port {port}: {e}"))?;
    let local = socket.local_addr().map_err(|e| e.to_string())?;

    let mut host = Host::new();
    if let Some(spec) = server {
        let target = resolve(&spec)?;
        let leg = ServerLeg::connect(target).map_err(|e| format!("client leg: {e}"))?;
        tracing::info!("client leg ready: host -> audio server at {}", leg.target());
        host = host.with_server(leg);
    }

    tracing::info!("clausters-gui host listening on udp://{local} (script -> host)");
    transport::serve(host, socket).map_err(|e| e.to_string())
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
