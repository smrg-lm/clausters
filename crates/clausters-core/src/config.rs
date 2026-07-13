//! The shared configuration model: one TOML schema for the server and every
//! client.
//!
//! Configuration is **read-only** to the programs (the user edits the files;
//! machine-written state lives elsewhere — the def store, `boot.json`,
//! `midi.json`). It comes from two layers, the lower overridden by the higher:
//!
//! 1. **user** — `$CLAUSTERS_CONFIG`, else `$XDG_CONFIG_HOME/clausters/config.toml`,
//!    else (Windows) `%APPDATA%\clausters\config.toml`, else
//!    `~/.config/clausters/config.toml`.
//! 2. **project** — the nearest `clausters.toml` walking up from the working
//!    directory (like Cargo finding `Cargo.toml`).
//!
//! A program then applies its own CLI flags on top, so the full precedence is
//! **CLI flag > project file > user file > compiled default**. Every field is an
//! [`Option`]: `None` means "not set at this layer", so [`Config::merge`] is a
//! plain field-by-field "higher layer wins if present". The compiled defaults
//! are not encoded here — each program keeps its own, applied last when a field
//! is still `None`.
//!
//! The structs are platform-agnostic and compile on `wasm32`; only the path
//! resolution and file reading (which a browser host never does) are gated to
//! native targets, like the rest of the platform seam.

use serde::Deserialize;

/// The whole configuration tree: one section per audience. Unknown keys are
/// ignored (forward compatibility), and every field is optional.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The audio server (`[server]`).
    pub server: ServerConfig,
    /// The Python (and future) client (`[client]`).
    pub client: ClientConfig,
    /// The GUI host (`[gui]`).
    pub gui: GuiConfig,
    /// The standalone app launch (`[standalone]`).
    pub standalone: StandaloneConfig,
}

/// `[server]` — defaults for the audio server's CLI options.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// DSP worker threads (`--workers`); 0 lets the server choose.
    pub workers: Option<usize>,
    /// Imposed output rate in Hz (`--sample-rate`); 0 follows the device.
    pub sample_rate: Option<u32>,
    /// Audio bus count (`--audio-buses`).
    pub audio_buses: Option<usize>,
    /// Control bus count (`--control-buses`).
    pub control_buses: Option<usize>,
    /// Audio-tap ring count (`--taps`); 0 disables the tap region.
    pub taps: Option<usize>,
    /// Per-tap ring capacity in samples (`--tap-frames`), a power of two.
    pub tap_frames: Option<usize>,
    /// Hardware output channels (`--outputs`); unset follows the device default.
    pub outputs: Option<usize>,
    /// Hardware input channels (`--inputs`); unset/0 opens no input device.
    pub inputs: Option<usize>,
    /// Node slab capacity, root included (`--max-nodes`).
    pub max_nodes: Option<usize>,
    /// Buffer pool size (`--max-buffers`).
    pub max_buffers: Option<usize>,
    /// Per-group child capacity (`--max-graph-children`).
    pub max_graph_children: Option<usize>,
    /// Accepted inputs per UGen when compiling a def (`--max-ugen-inputs`).
    pub max_ugen_inputs: Option<usize>,
    /// Whether to persist/reload defs; `false` is the `--no-persist` default.
    pub persist: Option<bool>,
    /// Data directory for the def store (`--data-dir`).
    pub data_dir: Option<String>,
    /// Shared-memory segment path (`--shm`).
    pub shm: Option<String>,
    /// TCP transport (`--tcp`/`--no-tcp`): on by default at the OSC port;
    /// `false` disables it, a number moves it.
    pub tcp: Option<PortSetting>,
    /// WebSocket transport (`--ws`): `true` for the default port, or a number.
    pub ws: Option<PortSetting>,
    /// Largest OSC frame accepted/sent on the stream transports (TCP and
    /// WebSocket), in bytes (`--max-frame`). A DoS guard, not a protocol
    /// limit; UDP keeps the datagram cap regardless.
    pub max_frame: Option<usize>,
    /// Virtual MIDI input (`--midi`): `true` for the default name, or a name.
    pub midi: Option<MidiSetting>,
}

/// `[client]` — connection defaults for the Python (and future) client.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ClientConfig {
    /// The server host the client talks to.
    pub host: Option<String>,
    /// The server's port (one number serves UDP and TCP alike).
    pub port: Option<u16>,
    /// Seconds added to each event's timetag (scheduling latency).
    pub latency: Option<f64>,
    /// The command carrier: `"tcp"` (the default), `"udp"` or `"ws"`. The
    /// boot-or-attach probe always rides UDP.
    pub transport: Option<String>,
}

/// `[gui]` — defaults for the GUI host's CLI options.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct GuiConfig {
    /// UDP port for the host's script-facing server front (`--port`).
    pub host_port: Option<u16>,
    /// TCP leg of the script front (`--tcp`/`--no-tcp`): on by default at the
    /// host port; `false` disables it, a number moves it.
    pub tcp: Option<PortSetting>,
    /// Largest OSC frame accepted on the TCP leg, in bytes (`--max-frame`).
    pub max_frame: Option<usize>,
    /// `host:port` of the audio server to attach the client leg to (`--server`).
    pub server: Option<String>,
    /// Shared-memory segment to map for meters/scopes (`--shm`).
    pub shm: Option<String>,
    /// Data directory for the GuiDef store (`--data-dir`).
    pub data_dir: Option<String>,
    /// Run with no display (`--headless`).
    pub headless: Option<bool>,
}

/// `[standalone]` — the self-contained app launch (GUI + embedded server).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct StandaloneConfig {
    /// The saved GuiDef name to open when `--standalone` is given no name.
    pub gui: Option<String>,
    /// Whether to run the GuiDef's `boot` messages and the `boot.json` preset.
    pub boot: Option<bool>,
    /// Data directory for the bundle (`--data-dir`).
    pub data_dir: Option<String>,
}

/// A transport toggle that may also carry a port: `tcp = true` (default port),
/// `tcp = false` (off), or `tcp = 57110` (a specific port).
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(untagged)]
pub enum PortSetting {
    /// `true`/`false`: on at the program's default port, or off.
    Enabled(bool),
    /// A concrete port number (implies on).
    Port(u16),
}

impl PortSetting {
    /// Resolves to the port to bind, or `None` when disabled. `default_port` is
    /// the program's own default for this transport.
    pub fn resolve(self, default_port: u16) -> Option<u16> {
        match self {
            PortSetting::Enabled(true) => Some(default_port),
            PortSetting::Enabled(false) => None,
            PortSetting::Port(p) => Some(p),
        }
    }
}

/// A MIDI toggle that may also carry a port name: `midi = true` (default name),
/// `midi = false` (off), or `midi = "name"`.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum MidiSetting {
    /// `true`/`false`: on with the program's default name, or off.
    Enabled(bool),
    /// A concrete virtual-port name (implies on).
    Name(String),
}

impl MidiSetting {
    /// Resolves to the virtual-port name to open, or `None` when disabled.
    /// `default_name` is the program's own default.
    pub fn resolve(&self, default_name: &str) -> Option<String> {
        match self {
            MidiSetting::Enabled(true) => Some(default_name.to_string()),
            MidiSetting::Enabled(false) => None,
            MidiSetting::Name(name) => Some(name.clone()),
        }
    }
}

/// Picks `b` over `a` whenever `b` is set — the per-field merge rule.
fn pick<T>(a: Option<T>, b: Option<T>) -> Option<T> {
    b.or(a)
}

impl Config {
    /// Merges `higher` over `self`, field by field: a value set in `higher`
    /// wins, otherwise `self`'s is kept. Used to layer the project file over the
    /// user file.
    #[must_use]
    pub fn merge(self, higher: Config) -> Config {
        Config {
            server: self.server.merge(higher.server),
            client: self.client.merge(higher.client),
            gui: self.gui.merge(higher.gui),
            standalone: self.standalone.merge(higher.standalone),
        }
    }
}

impl ServerConfig {
    fn merge(self, h: ServerConfig) -> ServerConfig {
        ServerConfig {
            workers: pick(self.workers, h.workers),
            sample_rate: pick(self.sample_rate, h.sample_rate),
            audio_buses: pick(self.audio_buses, h.audio_buses),
            control_buses: pick(self.control_buses, h.control_buses),
            taps: pick(self.taps, h.taps),
            tap_frames: pick(self.tap_frames, h.tap_frames),
            outputs: pick(self.outputs, h.outputs),
            inputs: pick(self.inputs, h.inputs),
            max_nodes: pick(self.max_nodes, h.max_nodes),
            max_buffers: pick(self.max_buffers, h.max_buffers),
            max_graph_children: pick(self.max_graph_children, h.max_graph_children),
            max_ugen_inputs: pick(self.max_ugen_inputs, h.max_ugen_inputs),
            persist: pick(self.persist, h.persist),
            data_dir: pick(self.data_dir, h.data_dir),
            shm: pick(self.shm, h.shm),
            tcp: pick(self.tcp, h.tcp),
            ws: pick(self.ws, h.ws),
            max_frame: pick(self.max_frame, h.max_frame),
            midi: pick(self.midi, h.midi),
        }
    }
}

impl ClientConfig {
    fn merge(self, h: ClientConfig) -> ClientConfig {
        ClientConfig {
            host: pick(self.host, h.host),
            port: pick(self.port, h.port),
            latency: pick(self.latency, h.latency),
            transport: pick(self.transport, h.transport),
        }
    }
}

impl GuiConfig {
    fn merge(self, h: GuiConfig) -> GuiConfig {
        GuiConfig {
            host_port: pick(self.host_port, h.host_port),
            tcp: pick(self.tcp, h.tcp),
            max_frame: pick(self.max_frame, h.max_frame),
            server: pick(self.server, h.server),
            shm: pick(self.shm, h.shm),
            data_dir: pick(self.data_dir, h.data_dir),
            headless: pick(self.headless, h.headless),
        }
    }
}

impl StandaloneConfig {
    fn merge(self, h: StandaloneConfig) -> StandaloneConfig {
        StandaloneConfig {
            gui: pick(self.gui, h.gui),
            boot: pick(self.boot, h.boot),
            data_dir: pick(self.data_dir, h.data_dir),
        }
    }
}

// ---- Native file resolution and loading (a browser host never reads files) ----

#[cfg(not(target_arch = "wasm32"))]
mod load {
    use super::Config;
    use std::path::{Path, PathBuf};

    /// Env var pointing at the user config file directly (highest priority for
    /// the user layer).
    const CONFIG_ENV: &str = "CLAUSTERS_CONFIG";
    /// The file name searched for the project layer, walking up from the CWD.
    const PROJECT_FILE: &str = "clausters.toml";

    /// The user config path: `$CLAUSTERS_CONFIG`, then `$XDG_CONFIG_HOME`, then
    /// (Windows) `%APPDATA%`, then `~/.config`. `None` if no home can be found.
    pub fn user_config_path() -> Option<PathBuf> {
        if let Ok(p) = std::env::var(CONFIG_ENV)
            && !p.is_empty()
        {
            return Some(PathBuf::from(p));
        }
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
            && !xdg.is_empty()
        {
            return Some(PathBuf::from(xdg).join("clausters/config.toml"));
        }
        #[cfg(windows)]
        if let Ok(appdata) = std::env::var("APPDATA")
            && !appdata.is_empty()
        {
            return Some(PathBuf::from(appdata).join("clausters/config.toml"));
        }
        std::env::var("HOME")
            .ok()
            .filter(|h| !h.is_empty())
            .map(|home| PathBuf::from(home).join(".config/clausters/config.toml"))
    }

    /// The nearest `clausters.toml` at or above `start`, or `None` if none.
    pub fn find_project_config(start: &Path) -> Option<PathBuf> {
        let mut dir = Some(start);
        while let Some(d) = dir {
            let candidate = d.join(PROJECT_FILE);
            if candidate.is_file() {
                return Some(candidate);
            }
            dir = d.parent();
        }
        None
    }

    /// Parses one TOML config file. An absent file yields `None`; a malformed
    /// one yields `None` and a warning on stderr (so the user notices the typo
    /// rather than silently getting defaults).
    pub fn read_config_file(path: &Path) -> Option<Config> {
        let text = std::fs::read_to_string(path).ok()?;
        match toml::from_str::<Config>(&text) {
            Ok(cfg) => Some(cfg),
            Err(e) => {
                eprintln!(
                    "clausters: ignoring malformed config {}: {e}",
                    path.display()
                );
                None
            }
        }
    }

    impl Config {
        /// Loads and merges the user and project config layers (project over
        /// user), resolving the project file by walking up from the current
        /// working directory. Missing or malformed files fall back to defaults.
        pub fn load() -> Config {
            let cwd = std::env::current_dir().ok();
            Config::load_from(cwd.as_deref())
        }

        /// Like [`Config::load`] but searches for the project file from `cwd`
        /// (testable without changing the process directory). `None` skips the
        /// project layer.
        pub fn load_from(cwd: Option<&Path>) -> Config {
            let user = user_config_path()
                .and_then(|p| read_config_file(&p))
                .unwrap_or_default();
            let project = cwd
                .and_then(find_project_config)
                .and_then(|p| read_config_file(&p))
                .unwrap_or_default();
            user.merge(project)
        }

        /// Loads a single config file, used by a `--config <path>` override. The
        /// error is the file/parse failure verbatim.
        pub fn from_path(path: &Path) -> Result<Config, String> {
            let text = std::fs::read_to_string(path)
                .map_err(|e| format!("cannot read config {}: {e}", path.display()))?;
            toml::from_str(&text).map_err(|e| format!("invalid config {}: {e}", path.display()))
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use load::{find_project_config, read_config_file, user_config_path};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_config() {
        let cfg: Config = toml::from_str(
            r#"
            [server]
            sample_rate = 44100
            audio_buses = 64
            outputs = 1
            inputs = 2
            max_nodes = 2048
            max_ugen_inputs = 16
            persist = false
            tcp = true
            ws = 9000
            midi = "synth"

            [client]
            host = "192.168.1.2"
            port = 57111

            [standalone]
            gui = "drone"
            boot = true
            "#,
        )
        .unwrap();
        assert_eq!(cfg.server.sample_rate, Some(44100));
        assert_eq!(cfg.server.audio_buses, Some(64));
        assert_eq!(cfg.server.outputs, Some(1));
        assert_eq!(cfg.server.inputs, Some(2));
        assert_eq!(cfg.server.max_nodes, Some(2048));
        assert_eq!(cfg.server.max_ugen_inputs, Some(16));
        assert_eq!(cfg.server.persist, Some(false));
        assert_eq!(cfg.server.tcp.unwrap().resolve(57110), Some(57110));
        assert_eq!(cfg.server.ws.unwrap().resolve(57120), Some(9000));
        assert_eq!(
            cfg.server.midi.unwrap().resolve("clausters"),
            Some("synth".into())
        );
        assert_eq!(cfg.client.host.as_deref(), Some("192.168.1.2"));
        assert_eq!(cfg.client.port, Some(57111));
        assert_eq!(cfg.standalone.gui.as_deref(), Some("drone"));
    }

    #[test]
    fn project_overrides_user_field_by_field() {
        let user: Config = toml::from_str(
            r#"
            [server]
            sample_rate = 48000
            audio_buses = 128
            "#,
        )
        .unwrap();
        let project: Config = toml::from_str(
            r#"
            [server]
            sample_rate = 96000
            "#,
        )
        .unwrap();
        let merged = user.merge(project);
        // The project's value wins where set; the user's is kept where absent.
        assert_eq!(merged.server.sample_rate, Some(96000));
        assert_eq!(merged.server.audio_buses, Some(128));
    }

    #[test]
    fn port_and_midi_toggles_resolve() {
        assert_eq!(PortSetting::Enabled(true).resolve(57110), Some(57110));
        assert_eq!(PortSetting::Enabled(false).resolve(57110), None);
        assert_eq!(PortSetting::Port(1234).resolve(57110), Some(1234));
        assert_eq!(MidiSetting::Enabled(false).resolve("clausters"), None);
        assert_eq!(
            MidiSetting::Name("x".into()).resolve("clausters"),
            Some("x".into())
        );
    }

    #[test]
    fn an_empty_config_is_all_none() {
        let cfg = Config::default();
        assert!(cfg.server.sample_rate.is_none());
        assert!(cfg.client.host.is_none());
        assert!(cfg.standalone.gui.is_none());
    }
}
