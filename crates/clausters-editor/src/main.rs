//! **The code editor, drawn by the GUI host**: one program holding the host,
//! its dock and the editor's application (`clausters_editor::app`), which is
//! a client of the host in its process.
//!
//! The host draws every panel -- the code, the post window, the
//! documentation, the preferences -- as a pane of one dock, and the
//! application (`app`) decides what they show and what a verb does, through
//! the protocol any script speaks. The Python session is a process of its
//! own, and so are the environments' operations.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use clausters_gui::host::Host;
use clausters_gui::host::gui::LocalReplies;

use clausters_editor::app::{App, Msg};
use clausters_editor::native::Native;
use clausters_editor::settings::{Settings, ThemePref};
use clausters_editor::theme;
use clausters_editor::wire::{Link, Said};

/// A Python editor with an interactive session.
#[derive(Parser, Debug)]
#[command(name = "clausters-editor", version)]
struct Cli {
    /// Files to open, each in a tab of its own. A file that does not exist
    /// yet opens empty and is made when it is saved.
    #[arg(value_name = "FILE", value_parser = file_arg)]
    files: Vec<PathBuf>,
    /// Draw nothing on the screen: the dock is drawn offscreen, and a script
    /// drives and captures it over UDP on `--port`. For checking the editor
    /// with nobody at the screen.
    #[arg(long, hide = true)]
    headless: bool,
    /// The UDP port a script reaches the host on (any free one by default).
    #[arg(long, hide = true)]
    port: Option<u16>,
    /// The device pixels to a logical one the headless dock is drawn at
    /// (`2` draws what a display at twice the density shows).
    #[arg(long, hide = true, default_value_t = 1.0)]
    scale: f32,
}

/// A `FILE` argument, made absolute against the directory the command runs
/// in. A directory is refused: there is nothing to open in a tab.
fn file_arg(arg: &str) -> Result<PathBuf, String> {
    let path = std::path::absolute(Path::new(arg)).map_err(|e| e.to_string())?;
    if path.is_dir() {
        return Err("is a directory".into());
    }
    Ok(path)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("clausters-editor: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    // the webview application's folders, under the program's name now
    clausters_editor::native::paths::migrate();
    let settings = Settings::load();
    let mut host = Host::new();
    host.dock = true;
    let size = settings.window.unwrap_or((1280, 800));
    host.dock_opens = Some(("clausters-editor".into(), size));
    host.dock_at = settings.position;
    // the look the editor opens in, so the first frame is not the host's
    let table = match settings.theme {
        ThemePref::Dark => theme::dark(),
        _ => theme::light(),
    };
    let pairs: Vec<(String, String)> = table
        .as_object()
        .map(|o| {
            o.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                .collect()
        })
        .unwrap_or_default();
    for w in host
        .theme
        .overlay(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
    {
        tracing::warn!("{w}");
    }
    host.load_default_face();
    host.headless_scale = cli.scale;
    // a window taken out of the dock opens at the size it was left at
    if !cli.headless
        && let Some(path) = clausters_gui::host::sizes::default_path()
    {
        host.sizes = clausters_gui::host::sizes::Sizes::at(path);
    }
    let bind = SocketAddr::from((Ipv4Addr::LOCALHOST, cli.port.unwrap_or(0)));
    let socket = UdpSocket::bind(bind).map_err(|e| format!("cannot bind {bind}: {e}"))?;
    if cli.headless {
        eprintln!(
            "clausters-editor: headless, udp://{}",
            socket.local_addr().map_err(|e| e.to_string())?
        );
    }
    let files = cli.files;
    // a session's client reaches this host over TCP as it reaches any, on
    // the same port, so a large tree is not bounded by a datagram
    let frame = clausters_core::osc::DEFAULT_MAX_FRAME;
    let tcp = socket.local_addr().ok().map(|addr| (addr, frame));
    // and a client that speaks only WebSocket, on a port of its own
    let ws = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|l| l.local_addr())
        .ok()
        .map(|addr| (addr, frame));
    // the sessions are told where, in each carrier
    let mut named = Vec::new();
    if let Some((addr, _)) = tcp {
        named.push(format!("udp://{addr}"));
    }
    if let Some((addr, _)) = ws {
        named.push(format!("ws://{addr}"));
    }
    clausters_editor::native::session::set_gui_host(named.join(" "));
    clausters_gui::host::gui::run_local(
        host,
        Arc::new(socket),
        cli.headless,
        tcp,
        ws,
        move |local| {
            let (tx, rx) = std::sync::mpsc::channel::<Msg>();
            let replies_tx = tx.clone();
            let replies: LocalReplies = Box::new(move |packet| {
                for said in Said::read(packet) {
                    let _ = replies_tx.send(Msg::Host(said));
                }
            });
            std::thread::Builder::new()
                .name("clausters-editor".into())
                .spawn(move || {
                    let local = std::sync::Arc::new(local);
                    let ends = local.clone();
                    let link = Link::new(
                        Box::new(move |msg| local.send(msg)),
                        Box::new(move || {
                            ends.quit();
                        }),
                    );
                    App::new(link, Box::new(Native::new(tx)), settings).run(rx, files)
                })
                .expect("the application's thread starts");
            replies
        },
    )
}
