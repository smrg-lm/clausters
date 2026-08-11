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
use std::collections::BTreeMap;

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
    /// The base OSC port (`--port`), default 57110: UDP binds it and TCP
    /// follows it. Moving UDP alone is a CLI matter (`--udp`) — in a config
    /// file, write the base here and give `tcp` a number of its own.
    pub port: Option<u16>,
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
    /// TCP transport (`--tcp`/`--no-tcp`): on by default at the base `port`;
    /// `false` disables it, a number moves it.
    pub tcp: Option<PortSetting>,
    /// WebSocket transport (`--ws`): `true` for the base `port` + 10, or a
    /// number.
    pub ws: Option<PortSetting>,
    /// Largest OSC frame accepted/sent on the stream transports (TCP and
    /// WebSocket), in bytes (`--max-frame`). A DoS guard, not a protocol
    /// limit; UDP keeps the datagram cap regardless.
    pub max_frame: Option<usize>,
    /// Ceiling for concurrent stream clients, TCP + WebSocket combined
    /// (`--max-clients`). UDP is connectionless and unaffected.
    pub max_clients: Option<usize>,
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
    /// The clock timebase a real-time session anchors to: `"sample"` (the
    /// default — the server's own sample clock, sample-accurate and drift-free)
    /// or `"monotonic"` (wall-clock OSC timetags). Read by the client only; a
    /// `"sample"` session falls back to wall-clock gracefully if no master
    /// answers. Both `Session.live()` (UDP tracker) and `Session.embed()`
    /// (direct in-process read) honour it; `render`/`nrt` stay on wall-clock
    /// (a score server has no live clock).
    pub clock: Option<String>,
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
    /// WebSocket leg of the script front (`--ws`): `true` for `host_port` +
    /// [`WS_PORT_OFFSET`], or a number. The browser's carrier into a native
    /// host.
    pub ws: Option<PortSetting>,
    /// Largest OSC frame accepted on the stream legs (TCP and WebSocket), in
    /// bytes (`--max-frame`).
    pub max_frame: Option<usize>,
    /// `host:port` of the audio server to attach the client leg to (`--server`).
    pub server: Option<String>,
    /// Shared-memory segment to map for meters/scopes (`--shm`).
    pub shm: Option<String>,
    /// Data directory for the GuiDef store (`--data-dir`).
    pub data_dir: Option<String>,
    /// Run with no display (`--headless`).
    pub headless: Option<bool>,
    /// Path to the typeface the host draws text with (`--font`). Read only by a
    /// GUI host built with a rasterizer (the `font-atlas` feature); any other
    /// build draws with its embedded bitmap face and ignores this. With the
    /// feature and no path, the host looks for one of the system's own faces.
    pub font: Option<String>,
    /// `[gui.theme]` — color-role overrides for the host's look, each entry
    /// `role = "#rrggbb[aa]"`. A partial table: unlisted roles keep the
    /// default theme. The role names are the GUI host's `Theme` fields
    /// (`accent`, `text`, `field`, ...); unknown names are warned about and
    /// skipped by the host, never fatal.
    pub theme: Option<BTreeMap<String, String>>,
    /// `[gui.metrics]` — size-role overrides for the host's sizing, each entry
    /// `role = <number>` in device pixels (glyph scales for the text roles).
    /// A partial table, like `[gui.theme]`: unlisted roles keep their generated
    /// default, and the role names are the GUI host's `Metrics` fields (`pad`,
    /// `gap`, `control_h`, `text_scale`, ...). The reserved key
    /// `scale = <number>` is the density multiplier: it regenerates the whole
    /// table at that density before the explicit roles apply. Unknown names and
    /// unusable numbers are warned about and skipped by the host, never fatal.
    pub metrics: Option<BTreeMap<String, Number>>,
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

/// How far a WebSocket front sits from its program's base port when it is given
/// no number of its own. It shares the TCP namespace, so it cannot share the
/// TCP number: 57110 → 57120 for the audio server, 57210 → 57220 for the GUI
/// host.
pub const WS_PORT_OFFSET: u16 = 10;

/// What a transport was asked to do with its port, before the base port is
/// known.
///
/// Both programs here bind several fronts around one **base** port — UDP or the
/// script-facing front binds it, TCP follows it, WebSocket sits
/// [`WS_PORT_OFFSET`] above — and both accept the same three answers per
/// transport: follow the base, sit at a number, stay off. The answer cannot be
/// turned into a port as it is read, because `--tcp` may come before the
/// `--port` it follows, so it is recorded here and resolved once the whole
/// command line (and the config beneath it) is in.
///
/// This is the shared half of what used to be two copies of the same rule, one
/// per binary, the second of which encoded "follow the base" as a port-zero
/// sentinel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortChoice {
    /// Bind the base port this transport is measured from.
    Follow,
    /// Bind this number, wherever the base ended up.
    At(u16),
    /// Do not bind at all.
    Off,
}

impl From<PortSetting> for PortChoice {
    fn from(setting: PortSetting) -> Self {
        match setting {
            PortSetting::Enabled(true) => PortChoice::Follow,
            PortSetting::Enabled(false) => PortChoice::Off,
            PortSetting::Port(port) => PortChoice::At(port),
        }
    }
}

impl PortChoice {
    /// The port to bind, or `None` when this transport stays off. `base` is
    /// what [`PortChoice::Follow`] means for *this* transport — the program's
    /// base port, already offset for a WebSocket front.
    pub fn resolve(self, base: u16) -> Option<u16> {
        match self {
            PortChoice::Follow => Some(base),
            PortChoice::At(port) => Some(port),
            PortChoice::Off => None,
        }
    }

    /// The choice a transport ends up with, in the precedence every option
    /// here follows: a command-line flag wins, else the config file's setting,
    /// else `default` (what the program does when nobody says anything).
    pub fn pick(
        flag: Option<PortChoice>,
        setting: Option<PortSetting>,
        default: PortChoice,
    ) -> Self {
        flag.or_else(|| setting.map(PortChoice::from))
            .unwrap_or(default)
    }
}

/// A configured number that may be written as an integer or a float — TOML
/// keeps the two apart, while a size role reads either (`pad = 6` and
/// `pad = 6.0` mean the same thing).
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(untagged)]
pub enum Number {
    /// An integer literal (`pad = 6`).
    Int(i64),
    /// A float literal (`scale = 1.25`).
    Float(f64),
}

impl Number {
    /// The value as an `f64`.
    pub fn as_f64(self) -> f64 {
        match self {
            Number::Int(n) => n as f64,
            Number::Float(v) => v,
        }
    }
}

/// Merges two partial role tables per key: the higher layer's entries win, its
/// unlisted keys fall through to the lower one.
fn merge_table<V>(
    lower: Option<BTreeMap<String, V>>,
    higher: Option<BTreeMap<String, V>>,
) -> Option<BTreeMap<String, V>> {
    match (lower, higher) {
        (Some(mut lower), Some(higher)) => {
            lower.extend(higher);
            Some(lower)
        }
        (lower, higher) => higher.or(lower),
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
            port: pick(self.port, h.port),
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
            max_clients: pick(self.max_clients, h.max_clients),
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
            clock: pick(self.clock, h.clock),
        }
    }
}

impl GuiConfig {
    fn merge(self, h: GuiConfig) -> GuiConfig {
        GuiConfig {
            host_port: pick(self.host_port, h.host_port),
            tcp: pick(self.tcp, h.tcp),
            ws: pick(self.ws, h.ws),
            max_frame: pick(self.max_frame, h.max_frame),
            server: pick(self.server, h.server),
            shm: pick(self.shm, h.shm),
            data_dir: pick(self.data_dir, h.data_dir),
            headless: pick(self.headless, h.headless),
            font: pick(self.font, h.font),
            // The theme and metrics tables merge per key (the overlay
            // semantics): the higher layer's roles win, its unlisted roles fall
            // through.
            theme: merge_table(self.theme, h.theme),
            metrics: merge_table(self.metrics, h.metrics),
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

    /// Reads a free-standing theme file (`--theme <path>`): a flat TOML table
    /// of `role = "#rrggbb[aa]"` entries, the same shape as `[gui.theme]`.
    /// The error is the file/parse failure verbatim.
    pub fn read_theme_file(path: &Path) -> Result<super::BTreeMap<String, String>, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read theme {}: {e}", path.display()))?;
        toml::from_str(&text).map_err(|e| format!("invalid theme {}: {e}", path.display()))
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use load::{find_project_config, read_config_file, read_theme_file, user_config_path};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transport_follows_the_base_port_it_is_measured_from() {
        // The rule both binaries bind by: follow the base, sit at a number, or
        // stay off -- and the WebSocket front's base is the program's, offset.
        let base = 57130;
        assert_eq!(PortChoice::Follow.resolve(base), Some(57130));
        assert_eq!(PortChoice::At(57145).resolve(base), Some(57145));
        assert_eq!(PortChoice::Off.resolve(base), None);
        assert_eq!(
            PortChoice::Follow.resolve(base + WS_PORT_OFFSET),
            Some(57140)
        );
    }

    #[test]
    fn a_flag_wins_over_the_config_and_the_config_over_the_default() {
        // The precedence every option here follows, in the one place that now
        // states it for a port.
        let off = Some(PortSetting::Enabled(false));
        assert_eq!(
            PortChoice::pick(Some(PortChoice::At(9)), off, PortChoice::Follow),
            PortChoice::At(9)
        );
        assert_eq!(
            PortChoice::pick(None, off, PortChoice::Follow),
            PortChoice::Off
        );
        assert_eq!(
            PortChoice::pick(None, None, PortChoice::Follow),
            PortChoice::Follow
        );
        // A configured number and a configured `true` are the two other shapes
        // the file can take.
        assert_eq!(
            PortChoice::pick(None, Some(PortSetting::Port(57145)), PortChoice::Off),
            PortChoice::At(57145)
        );
        assert_eq!(
            PortChoice::pick(None, Some(PortSetting::Enabled(true)), PortChoice::Off),
            PortChoice::Follow
        );
    }

    #[test]
    fn gui_theme_table_parses_and_merges_per_key() {
        let user: Config = toml::from_str(
            r##"
            [gui.theme]
            accent = "#ff0000"
            text = "#eeeeee"
            "##,
        )
        .unwrap();
        let project: Config = toml::from_str(
            r##"
            [gui.theme]
            accent = "#00ff00"
            field = "#101010"
            "##,
        )
        .unwrap();
        let merged = user.merge(project).gui.theme.unwrap();
        assert_eq!(merged.get("accent").unwrap(), "#00ff00", "higher wins");
        assert_eq!(
            merged.get("text").unwrap(),
            "#eeeeee",
            "lower falls through"
        );
        assert_eq!(merged.get("field").unwrap(), "#101010");
    }

    #[test]
    fn gui_metrics_table_takes_ints_and_floats_and_merges_per_key() {
        let user: Config = toml::from_str(
            r#"
            [gui.metrics]
            pad = 6
            gap = 8
            "#,
        )
        .unwrap();
        let project: Config = toml::from_str(
            r#"
            [gui.metrics]
            scale = 1.25
            pad = 5.5
            "#,
        )
        .unwrap();
        let merged = user.merge(project).gui.metrics.unwrap();
        assert_eq!(merged.get("pad").unwrap().as_f64(), 5.5, "higher wins");
        assert_eq!(
            merged.get("gap").unwrap().as_f64(),
            8.0,
            "an integer entry reads as a number, and falls through"
        );
        assert_eq!(merged.get("scale").unwrap().as_f64(), 1.25);
    }

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
