//! **A native plugin as data**: what discovery reports about one, the catalog
//! of them, and the recipe a plugin def is built from.
//!
//! The information has three layers, each a different thing:
//!
//! - the **catalog** -- what is installed, as the server's discovery reports it
//!   (`/plugin_scan`): its plugins and the binaries that failed;
//! - a **plugin** ([`PluginInfo`]) -- one read-only entry: its identity
//!   (format, canonical binary path, native id), what it is called, its ports,
//!   its parameters, what it supports and what it declares itself to be. Its
//!   identity travels inside it: a program finds a plugin by what it is called
//!   and never types its id;
//! - a **reference** ([`PluginReference`]) -- the user's decision about a
//!   plugin: which buses its ports use and the state it starts from. It is
//!   what a plugin def stores and what the server loads.
//!
//! Everything here is plain data with the rules over it, written once: the
//! server validates with [`PluginReference::check`], and every client binds the
//! same functions to read a catalog ([`catalog_json`]), search it ([`filter`],
//! [`find`]) and build a reference ([`reference`]), so a reference that does
//! not fit its plugin is refused before it is sent.
//!
//! **What a plugin declares itself to be** comes from two vocabularies: CLAP's
//! descriptor features and VST3's class subcategories. Both are kept as
//! declared ([`PluginInfo::features`]) and read into one: a [`Kind`], a set of
//! tags ([`PluginInfo::tags`]) and a channel layout ([`PluginInfo::channels`]).
//! Data a plugin loads from outside (an impulse response, samples) has no
//! standard declaration in either format; it lives in the plugin's opaque
//! state.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};

/// Largest metadata catalogue from one binary.
pub const MAX_PLUGINS: usize = 512;
/// Port rows per direction.
pub const MAX_PORTS: usize = 16;
/// Stable native parameters per instance. A VST3 instrument that maps MIDI
/// controllers to parameters declares one per controller and channel -- 2048
/// before its own -- so the bound has room for that and more.
pub const MAX_PARAMETERS: usize = 8192;
/// The channels a note port has when the format does not say: MIDI's sixteen.
pub const NOTE_CHANNELS: u32 = 16;

fn yes() -> bool {
    true
}
fn note_channels() -> u32 {
    NOTE_CHANNELS
}

/// An audio port's native identity and channel configuration.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AudioPort {
    /// Native port identity, independent of enumeration order.
    pub id: u32,
    /// Human-readable label, never used as an identity.
    pub name: String,
    /// Number of channels in this port.
    pub channels: u32,
    /// Native flags, in the format's own vocabulary (CLAP audio-port flags,
    /// VST3 bus flags); [`main`](Self::main) and [`active`](Self::active) are
    /// what they say in common.
    pub flags: u32,
    /// The main port of its direction (CLAP's main flag, VST3's main bus
    /// type); the others are auxiliary -- a sidechain, separate outputs.
    #[serde(default)]
    pub main: bool,
    /// The declared channel layout (`mono`, `stereo`, `surround`,
    /// `ambisonic`, ...), empty when the format declares none.
    #[serde(default)]
    pub layout: String,
    /// Whether the plugin wants it active by default (VST3's default-active
    /// flag; a CLAP port always is).
    #[serde(default = "yes")]
    pub active: bool,
}

/// An event port's supported native dialects.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NotePort {
    /// Stable native identity.
    pub id: u32,
    /// Display label.
    pub name: String,
    /// CLAP dialect bits: CLAP=1, MIDI=2, MIDI MPE=4, MIDI2=8.
    pub dialects: u32,
    /// The dialect the plugin prefers, one of the same bits; zero when it
    /// states none.
    #[serde(default)]
    pub preferred: u32,
    /// The channels the port has: VST3 declares a count per event bus, a CLAP
    /// port has MIDI's sixteen.
    #[serde(default = "note_channels")]
    pub channels: u32,
}

/// An adapter-native parameter; CLAP uses plain units, VST3 normalized units.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Parameter {
    /// Stable native identity; display name/order does not identify a parameter.
    pub id: u32,
    /// Display label.
    pub name: String,
    /// Hierarchical display grouping: CLAP's module path, VST3's unit path.
    pub module: String,
    /// Common CLAP-compatible flags, including stepped/read-only/automatable.
    pub flags: u32,
    /// Minimum in the adapter-native parameter units.
    pub min: f64,
    /// Maximum in the same units.
    pub max: f64,
    /// Quantization interval in the reported units; zero means continuous.
    #[serde(default)]
    pub step: f64,
    /// Default in the same parameter units.
    pub default: f64,
}

/// Stable reference and processing surface reported by isolated discovery.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PluginInfo {
    /// Adapter name: `clap` or `vst3`.
    pub format: String,
    /// Canonical binary path, on the server's filesystem.
    pub path: String,
    /// Native plugin identity within that binary.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Vendor label.
    pub vendor: String,
    /// Vendor's plugin version string.
    pub version: String,
    /// The plugin's own one-line description (CLAP); empty when none.
    #[serde(default)]
    pub description: String,
    /// The plugin's or vendor's web address.
    #[serde(default)]
    pub url: String,
    /// The manual's address (CLAP).
    #[serde(default)]
    pub manual_url: String,
    /// The support address (CLAP), or the vendor's e-mail (VST3).
    #[serde(default)]
    pub support_url: String,
    /// What the plugin declares itself to be, as declared: CLAP's descriptor
    /// features, VST3's class subcategories split at `|`.
    #[serde(default)]
    pub features: Vec<String>,
    /// The interface version the plugin was built against.
    #[serde(default)]
    pub sdk_version: String,
    /// Audio inputs in processing-port order.
    pub inputs: Vec<AudioPort>,
    /// Audio outputs in processing-port order.
    pub outputs: Vec<AudioPort>,
    /// Note inputs in processing-port order.
    pub note_inputs: Vec<NotePort>,
    /// Note outputs in processing-port order.
    pub note_outputs: Vec<NotePort>,
    /// Parameters with stable native ids and adapter-native ranges.
    pub parameters: Vec<Parameter>,
    /// Declared intrinsic latency after activation.
    pub latency: u32,
    /// Whether the plugin exposes opaque state save/load.
    pub state: bool,
    /// Whether a native X11 editor is supported.
    pub editor: bool,
}

/// Each scan row is either metadata or a per-binary diagnostic; errors survive caching.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScanEntry {
    /// A compatible plugin in a binary.
    Plugin {
        /// Validated metadata.
        plugin: Box<PluginInfo>,
    },
    /// Failure of one candidate; other binaries continue scanning.
    Error {
        /// Candidate binary/directory path.
        path: String,
        /// Timeout, crash, capacity or compatibility diagnostic.
        error: String,
    },
}

/// **What a plugin is**, one of five: read from its declared category, else
/// from its ports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Makes sound from notes.
    Instrument,
    /// Processes audio.
    Effect,
    /// Processes notes into notes.
    NoteEffect,
    /// Reads audio or notes and reports on them.
    Analyzer,
    /// None of these, or undeclared and undecidable from the ports.
    Other,
}

impl Kind {
    /// The kind's name, as the catalog writes it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Instrument => "instrument",
            Self::Effect => "effect",
            Self::NoteEffect => "note_effect",
            Self::Analyzer => "analyzer",
            Self::Other => "other",
        }
    }
}

/// What one declared word means in the common vocabulary.
enum Feature {
    Kind(Kind),
    Tag(&'static str),
    Channels(&'static str),
}

/// One declared feature or subcategory, read: CLAP's and VST3's words for the
/// same thing land on one. A word neither format standardizes means nothing
/// here and stays only in [`PluginInfo::features`].
fn feature(word: &str) -> Option<Feature> {
    let word = word.trim().to_ascii_lowercase();
    Some(match word.as_str() {
        "instrument" => Feature::Kind(Kind::Instrument),
        "audio-effect" | "fx" => Feature::Kind(Kind::Effect),
        "note-effect" => Feature::Kind(Kind::NoteEffect),
        "analyzer" | "note-detector" => Feature::Kind(Kind::Analyzer),
        "synthesizer" | "synth" => Feature::Tag("synth"),
        "sampler" => Feature::Tag("sampler"),
        "drum" | "drum-machine" => Feature::Tag("drum"),
        "piano" => Feature::Tag("piano"),
        "external" => Feature::Tag("external"),
        "generator" => Feature::Tag("generator"),
        "filter" => Feature::Tag("filter"),
        "phaser" | "flanger" | "chorus" | "tremolo" | "modulation" => Feature::Tag("modulation"),
        "equalizer" | "eq" => Feature::Tag("eq"),
        "compressor" | "expander" | "gate" | "limiter" | "de-esser" | "transient-shaper"
        | "dynamics" => Feature::Tag("dynamics"),
        "distortion" => Feature::Tag("distortion"),
        "delay" => Feature::Tag("delay"),
        "reverb" => Feature::Tag("reverb"),
        "pitch-shifter" | "frequency-shifter" | "pitch-correction" | "pitch shift"
        | "phase-vocoder" => Feature::Tag("pitch"),
        "granular" => Feature::Tag("granular"),
        "glitch" => Feature::Tag("glitch"),
        "utility" | "tools" => Feature::Tag("utility"),
        "restoration" => Feature::Tag("restoration"),
        "mastering" => Feature::Tag("mastering"),
        "mixing" => Feature::Tag("mixing"),
        "multi-effects" => Feature::Tag("multi_effects"),
        "spatial" | "up-downmix" => Feature::Tag("spatial"),
        "network" => Feature::Tag("network"),
        "mono" => Feature::Channels("mono"),
        "stereo" => Feature::Channels("stereo"),
        "surround" => Feature::Channels("surround"),
        "ambisonic" | "ambisonics" => Feature::Channels("ambisonic"),
        _ => return None,
    })
}

impl PluginInfo {
    /// The identity a catalog keys a plugin by: `format:path#id`.
    #[must_use]
    pub fn key(&self) -> String {
        format!("{}:{}#{}", self.format, self.path, self.id)
    }

    /// **What the plugin is.** Its declared category decides, an instrument
    /// before a note effect before an analyzer before an effect (a synthesizer
    /// with an audio input declares both); with none declared, the ports do: a
    /// note input and no main audio input is an instrument, an audio input an
    /// effect.
    #[must_use]
    pub fn kind(&self) -> Kind {
        let declared: Vec<Kind> = self
            .features
            .iter()
            .filter_map(|w| match feature(w) {
                Some(Feature::Kind(kind)) => Some(kind),
                _ => None,
            })
            .collect();
        for kind in [
            Kind::Instrument,
            Kind::NoteEffect,
            Kind::Analyzer,
            Kind::Effect,
        ] {
            if declared.contains(&kind) {
                return kind;
            }
        }
        if !self.note_inputs.is_empty() && self.main_input().is_none() {
            Kind::Instrument
        } else if !self.inputs.is_empty() {
            Kind::Effect
        } else if !self.note_inputs.is_empty() {
            Kind::Instrument
        } else {
            Kind::Other
        }
    }

    /// **What the plugin declares it does**, in the common vocabulary, once
    /// each, in declared order.
    #[must_use]
    pub fn tags(&self) -> Vec<&'static str> {
        let mut tags = Vec::new();
        for word in &self.features {
            if let Some(Feature::Tag(tag)) = feature(word)
                && !tags.contains(&tag)
            {
                tags.push(tag);
            }
        }
        tags
    }

    /// **Its channel layout**: declared (`mono`, `stereo`, `surround`,
    /// `ambisonic`), else read from the main output's channel count (one is
    /// mono, two stereo, more surround); `None` with no audio output.
    #[must_use]
    pub fn channels(&self) -> Option<&'static str> {
        for word in &self.features {
            if let Some(Feature::Channels(layout)) = feature(word) {
                return Some(layout);
            }
        }
        match self.main_output()?.channels {
            0 => None,
            1 => Some("mono"),
            2 => Some("stereo"),
            _ => Some("surround"),
        }
    }

    /// The main audio input: the one flagged main, else the first.
    #[must_use]
    pub fn main_input(&self) -> Option<&AudioPort> {
        main_of(&self.inputs)
    }

    /// The main audio output: the one flagged main, else the first.
    #[must_use]
    pub fn main_output(&self) -> Option<&AudioPort> {
        main_of(&self.outputs)
    }

    /// The note input notes go to when none is named: the first.
    #[must_use]
    pub fn main_note_input(&self) -> Option<&NotePort> {
        self.note_inputs.first()
    }

    /// Validate metadata capacities, finite ranges and unique native identities.
    pub fn validate(&self) -> Result<(), String> {
        if self.format.is_empty()
            || self.id.is_empty()
            || self.path.is_empty()
            || self.parameters.len() > MAX_PARAMETERS
        {
            return Err("invalid plugin metadata".into());
        }
        for ports in [&self.inputs, &self.outputs] {
            let mut identities = HashSet::new();
            let mut channels = 0u64;
            if ports.len() > MAX_PORTS {
                return Err("plugin audio port capacity exceeded".into());
            }
            for port in ports {
                channels += u64::from(port.channels);
                if port.channels == 0 || port.id == u32::MAX || !identities.insert(port.id) {
                    return Err("invalid plugin audio port".into());
                }
            }
            if channels > u64::from(crate::plugin_shm::CHANNELS) {
                return Err("plugin audio channel capacity exceeded".into());
            }
        }
        if self.outputs.is_empty() {
            return Err("plugin has no audio output".into());
        }
        for ports in [&self.note_inputs, &self.note_outputs] {
            let mut identities = HashSet::new();
            if ports.len() > MAX_PORTS {
                return Err("plugin note port capacity exceeded".into());
            }
            for port in ports {
                if port.dialects == 0 || port.id == u32::MAX || !identities.insert(port.id) {
                    return Err("invalid plugin note port".into());
                }
            }
        }
        let mut identities = HashSet::new();
        for p in &self.parameters {
            if p.id == u32::MAX
                || !identities.insert(p.id)
                || !p.min.is_finite()
                || !p.max.is_finite()
                || !p.default.is_finite()
                || !p.step.is_finite()
                || p.step < 0.0
                || p.min > p.max
                || !(p.min..=p.max).contains(&p.default)
            {
                return Err("invalid plugin parameter".into());
            }
        }
        Ok(())
    }

    /// **The node's control names, in control order**: parameters sorted by
    /// native id (`param:<id>`), then input ports (`in:<id>`), then output
    /// ports (`out:<id>`). Native ids, never display labels, identify them.
    #[must_use]
    pub fn control_names(&self) -> Vec<String> {
        let mut ids: Vec<u32> = self.parameters.iter().map(|p| p.id).collect();
        ids.sort_unstable();
        let mut names: Vec<String> = ids.iter().map(|id| format!("param:{id}")).collect();
        names.extend(self.inputs.iter().map(|p| format!("in:{}", p.id)));
        names.extend(self.outputs.iter().map(|p| format!("out:{}", p.id)));
        names
    }

    /// **The controls as their ports and parameters read**, each alias with
    /// the native-id name it stands for: the main audio input and output are
    /// `in` and `out`, another port `in_<name>`/`out_<name>`, a parameter its
    /// name, all made identifiers ([`identifier`]). The native-id names stay
    /// the unambiguous spelling, and an alias two controls would share is
    /// nobody's: a port's alias wins over a parameter's, and an alias claimed
    /// twice within ports or within parameters is dropped for all claimants.
    #[must_use]
    pub fn control_aliases(&self) -> Vec<(String, String)> {
        let mut ports: Vec<(String, String)> = Vec::new();
        for (prefix, list, main) in [
            ("in", &self.inputs, self.main_input()),
            ("out", &self.outputs, self.main_output()),
        ] {
            for port in list {
                let alias = if main.is_some_and(|m| m.id == port.id) {
                    prefix.to_owned()
                } else {
                    match identifier(&port.name) {
                        Some(name) => format!("{prefix}_{name}"),
                        None => continue,
                    }
                };
                ports.push((alias, format!("{prefix}:{}", port.id)));
            }
        }
        let ports = unique(ports);
        let taken: HashSet<String> = ports.iter().map(|(a, _)| a.clone()).collect();
        let mut sorted: Vec<&Parameter> = self.parameters.iter().collect();
        sorted.sort_by_key(|p| p.id);
        let parameters = unique(
            sorted
                .into_iter()
                .filter_map(|p| Some((identifier(&p.name)?, format!("param:{}", p.id))))
                .filter(|(alias, _)| !taken.contains(alias))
                .collect(),
        );
        let mut aliases = ports;
        aliases.extend(parameters);
        aliases
    }

    /// **The catalog's entry for this plugin**: its metadata as reported, and
    /// what it is read as -- `key`, `kind`, `tags`, `channels` and `controls`
    /// (alias -> native-id name).
    #[must_use]
    pub fn to_entry(&self) -> Value {
        let mut entry = match serde_json::to_value(self) {
            Ok(Value::Object(map)) => map,
            _ => Map::new(),
        };
        let controls: Map<String, Value> = self
            .control_aliases()
            .into_iter()
            .map(|(alias, name)| (alias, Value::String(name)))
            .collect();
        entry.insert("key".into(), json!(self.key()));
        entry.insert("kind".into(), json!(self.kind().as_str()));
        entry.insert("tags".into(), json!(self.tags()));
        entry.insert("channels".into(), json!(self.channels()));
        entry.insert("controls".into(), Value::Object(controls));
        Value::Object(entry)
    }
}

fn main_of(ports: &[AudioPort]) -> Option<&AudioPort> {
    ports.iter().find(|p| p.main).or_else(|| ports.first())
}

/// The pairs whose alias nobody else claims.
fn unique(pairs: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut count: HashMap<&str, usize> = HashMap::new();
    for (alias, _) in &pairs {
        *count.entry(alias.as_str()).or_default() += 1;
    }
    let keep: HashSet<String> = count
        .into_iter()
        .filter(|&(_, n)| n == 1)
        .map(|(a, _)| a.to_owned())
        .collect();
    pairs
        .into_iter()
        .filter(|(a, _)| keep.contains(a))
        .collect()
}

/// **A display name as an identifier** a program can write as a keyword:
/// lowercase ASCII letters and digits, every other run of characters one `_`,
/// none at either end, and a `p` before a leading digit. `None` when nothing
/// is left.
#[must_use]
pub fn identifier(name: &str) -> Option<String> {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        return None;
    }
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, 'p');
    }
    Some(out)
}

/// **A catalog**: the plugins discovery found, each once by its identity, and
/// the binaries that failed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Catalog {
    pub plugins: Vec<PluginInfo>,
    /// `(path, diagnostic)` per failed candidate.
    pub errors: Vec<(String, String)>,
}

impl Catalog {
    /// **The catalog from scan rows**, as `/plugin_scan` reassembles them
    /// (`{"kind": "plugin", "plugin": {...}}` or `{"kind": "error", "path",
    /// "error"}`). A plugin listed twice keeps its first row.
    pub fn from_rows(rows: &[Value]) -> Result<Self, String> {
        let mut catalog = Self::default();
        let mut seen = HashSet::new();
        for row in rows {
            match serde_json::from_value::<ScanEntry>(row.clone())
                .map_err(|e| format!("invalid plugin scan row: {e}"))?
            {
                ScanEntry::Plugin { plugin } => {
                    if seen.insert(plugin.key()) {
                        catalog.plugins.push(*plugin);
                    }
                }
                ScanEntry::Error { path, error } => catalog.errors.push((path, error)),
            }
        }
        Ok(catalog)
    }
}

/// **The catalog as the clients hold it**: `{"plugins": [entry, ...],
/// "errors": [{"path", "error"}, ...]}`, each entry [`PluginInfo::to_entry`].
pub fn catalog_json(rows: &[Value]) -> Result<Value, String> {
    let catalog = Catalog::from_rows(rows)?;
    Ok(json!({
        "plugins": catalog.plugins.iter().map(PluginInfo::to_entry).collect::<Vec<_>>(),
        "errors": catalog.errors.iter()
            .map(|(path, error)| json!({"path": path, "error": error}))
            .collect::<Vec<_>>(),
    }))
}

/// **What a search asks for**; every field is optional and they all must hold.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Query {
    /// Part of the name, any case.
    #[serde(default)]
    pub name: Option<String>,
    /// The vendor, any case.
    #[serde(default)]
    pub vendor: Option<String>,
    /// `clap` or `vst3`.
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub kind: Option<Kind>,
    /// One tag of the common vocabulary.
    #[serde(default)]
    pub tag: Option<String>,
}

impl Query {
    fn holds(&self, p: &PluginInfo) -> bool {
        let lower = |s: &str| s.to_lowercase();
        self.name
            .as_ref()
            .is_none_or(|n| lower(&p.name).contains(&lower(n)))
            && self
                .vendor
                .as_ref()
                .is_none_or(|v| lower(&p.vendor) == lower(v))
            && self
                .format
                .as_ref()
                .is_none_or(|f| lower(&p.format) == lower(f))
            && self.kind.is_none_or(|k| p.kind() == k)
            && self
                .tag
                .as_ref()
                .is_none_or(|t| p.tags().contains(&lower(t).as_str()))
    }

    fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(n) = &self.name {
            parts.push(format!("name {n:?}"));
        }
        if let Some(v) = &self.vendor {
            parts.push(format!("vendor {v:?}"));
        }
        if let Some(f) = &self.format {
            parts.push(format!("format {f:?}"));
        }
        if let Some(k) = self.kind {
            parts.push(format!("kind {:?}", k.as_str()));
        }
        if let Some(t) = &self.tag {
            parts.push(format!("tag {t:?}"));
        }
        if parts.is_empty() {
            "any plugin".into()
        } else {
            parts.join(", ")
        }
    }
}

/// **The plugins a query holds for**, as indices into `plugins`, in order.
#[must_use]
pub fn filter(plugins: &[PluginInfo], query: &Query) -> Vec<usize> {
    (0..plugins.len())
        .filter(|&i| query.holds(&plugins[i]))
        .collect()
}

/// **The one plugin a query names**, as an index into `plugins`. Of several
/// matches, the ones whose whole name is the query's win; none, or still
/// several, is an error that names the candidates, so the caller can say
/// which (a format, a vendor).
pub fn find(plugins: &[PluginInfo], query: &Query) -> Result<usize, String> {
    let mut matches = filter(plugins, query);
    if matches.len() > 1
        && let Some(name) = &query.name
    {
        let exact: Vec<usize> = matches
            .iter()
            .copied()
            .filter(|&i| plugins[i].name.to_lowercase() == name.to_lowercase())
            .collect();
        if !exact.is_empty() {
            matches = exact;
        }
    }
    match matches.as_slice() {
        [one] => Ok(*one),
        [] => Err(format!("no plugin matches {}", query.describe())),
        several => {
            let listed: Vec<String> = several
                .iter()
                .take(10)
                .map(|&i| {
                    let p = &plugins[i];
                    format!("{} {:?} by {:?} ({})", p.format, p.name, p.vendor, p.path)
                })
                .collect();
            let more = if several.len() > 10 {
                format!(" and {} more", several.len() - 10)
            } else {
                String::new()
            };
            Err(format!(
                "{} plugins match {}: {}{more}; say which with a format or a vendor",
                several.len(),
                query.describe(),
                listed.join("; ")
            ))
        }
    }
}

/// One native port routed to consecutive server audio buses. A negative bus
/// leaves the port unconnected: an input reads silence and an output writes
/// nowhere.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortReference {
    pub id: u32,
    pub channels: u32,
    #[serde(default)]
    pub bus: i32,
}

/// The bridge's buffering depth a reference asks for when it says none.
pub const DEFAULT_DEPTH: u32 = crate::plugin_shm::DEFAULT_DEPTH;

fn default_depth() -> u32 {
    DEFAULT_DEPTH
}

/// Persistent recipe; state bytes are an opaque initial-state snapshot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginReference {
    pub format: String,
    pub path: String,
    pub id: String,
    pub inputs: Vec<PortReference>,
    pub outputs: Vec<PortReference>,
    #[serde(default)]
    pub state: Vec<u8>,
    #[serde(default = "default_depth")]
    pub depth: u32,
}

impl PluginReference {
    /// **Whether this reference fits `info`**: the same format, and every
    /// native audio port routed exactly once with its channel count -- the
    /// rule the server checks when it loads a def, and a client before it
    /// sends one.
    pub fn check(&self, info: &PluginInfo) -> Result<(), String> {
        if info.format != self.format {
            return Err("plugin format does not match binary".into());
        }
        for (ports, routes) in [(&info.inputs, &self.inputs), (&info.outputs, &self.outputs)] {
            if ports.len() != routes.len() {
                return Err("plugin port configuration mismatch".into());
            }
            for port in ports {
                if routes
                    .iter()
                    .filter(|r| r.id == port.id && r.channels == port.channels)
                    .count()
                    != 1
                {
                    return Err("plugin port configuration mismatch".into());
                }
            }
        }
        Ok(())
    }
}

/// **Where one direction's ports go.**
#[derive(Clone, Debug, PartialEq)]
pub enum Route {
    /// Every port unconnected.
    None,
    /// The main port on this bus (and the ones after it, one per channel),
    /// the auxiliary ports unconnected -- an insert, as a track takes one.
    From(i32),
    /// The named ports on their buses, the rest unconnected. A port is named
    /// by its control alias (`in`, `out`, `in_sidechain`) or its native-id
    /// name (`in:10`).
    Ports(Vec<(String, i32)>),
}

impl Route {
    /// A route as JSON writes it: `null`, a first bus, or `{port: bus}`.
    pub fn from_json(value: &Value) -> Result<Self, String> {
        match value {
            Value::Null => Ok(Self::None),
            Value::Number(n) => n
                .as_i64()
                .and_then(|b| i32::try_from(b).ok())
                .map(Self::From)
                .ok_or_else(|| "a plugin route's bus must be an integer".into()),
            Value::Object(map) => map
                .iter()
                .map(|(port, bus)| {
                    bus.as_i64()
                        .and_then(|b| i32::try_from(b).ok())
                        .map(|b| (port.clone(), b))
                        .ok_or_else(|| format!("port {port:?}: its bus must be an integer"))
                })
                .collect::<Result<_, _>>()
                .map(Self::Ports),
            _ => Err("a plugin route is null, a first bus or {port: bus}".into()),
        }
    }
}

/// **A reference to `info` with its ports routed**: `inputs` and `outputs`
/// say where each direction's ports go ([`Route`]), and `state` is the
/// initial state (empty for the plugin's own). A port named that the plugin
/// does not have is refused.
pub fn reference(
    info: &PluginInfo,
    inputs: &Route,
    outputs: &Route,
    state: Vec<u8>,
) -> Result<PluginReference, String> {
    let aliases = info.control_aliases();
    let route = |prefix: &str, ports: &[AudioPort], route: &Route| {
        let mut rows: Vec<PortReference> = ports
            .iter()
            .map(|p| PortReference {
                id: p.id,
                channels: p.channels,
                bus: -1,
            })
            .collect();
        match route {
            Route::None => {}
            Route::From(first) => {
                if let Some(main) = main_of(ports) {
                    for row in rows.iter_mut().filter(|r| r.id == main.id) {
                        row.bus = *first;
                    }
                }
            }
            Route::Ports(named) => {
                for (name, bus) in named {
                    let native = aliases
                        .iter()
                        .find(|(alias, _)| alias == name)
                        .map_or(name.as_str(), |(_, native)| native.as_str());
                    let row = native
                        .strip_prefix(prefix)
                        .and_then(|rest| rest.strip_prefix(':'))
                        .and_then(|id| id.parse::<u32>().ok())
                        .and_then(|id| rows.iter_mut().find(|r| r.id == id))
                        .ok_or_else(|| {
                            let known: Vec<String> = aliases
                                .iter()
                                .filter(|(_, n)| n.starts_with(&format!("{prefix}:")))
                                .map(|(a, _)| a.clone())
                                .collect();
                            format!(
                                "{:?} has no {} port {name:?}; its ports are {}",
                                info.name,
                                if prefix == "in" { "input" } else { "output" },
                                if known.is_empty() {
                                    "none".into()
                                } else {
                                    known.join(", ")
                                }
                            )
                        })?;
                    row.bus = *bus;
                }
            }
        }
        Ok::<_, String>(rows)
    };
    let reference = PluginReference {
        format: info.format.clone(),
        path: info.path.clone(),
        id: info.id.clone(),
        inputs: route("in", &info.inputs, inputs)?,
        outputs: route("out", &info.outputs, outputs)?,
        state,
        depth: DEFAULT_DEPTH,
    };
    reference.check(info)?;
    Ok(reference)
}

/// **The client door to the catalog**, one JSON request and one JSON answer
/// for every binding: `{"rows": [scan row, ...]}` answers [`catalog_json`];
/// an error is `{"error": message}`.
#[must_use]
pub fn catalog_request(request: &Value) -> Value {
    match request.get("rows").and_then(Value::as_array) {
        Some(rows) => catalog_json(rows).unwrap_or_else(|e| json!({ "error": e })),
        None => json!({"error": "a catalog request carries its scan rows"}),
    }
}

fn plugins_of(request: &Value) -> Result<Vec<PluginInfo>, String> {
    request
        .get("plugins")
        .and_then(Value::as_array)
        .ok_or("a search carries the catalog's plugins")?
        .iter()
        .map(|p| serde_json::from_value(p.clone()).map_err(|e| format!("invalid plugin: {e}")))
        .collect()
}

/// **The client door to a search**: `{"plugins": [entry, ...], "query":
/// {...}, "one": bool}` answers `{"index": i}` for [`find`] (`one`) or
/// `{"indices": [...]}` for [`filter`].
#[must_use]
pub fn search_request(request: &Value) -> Value {
    let answer = || -> Result<Value, String> {
        let plugins = plugins_of(request)?;
        let query: Query =
            serde_json::from_value(request.get("query").cloned().unwrap_or_else(|| json!({})))
                .map_err(|e| format!("invalid plugin query: {e}"))?;
        if request.get("one").and_then(Value::as_bool).unwrap_or(false) {
            Ok(json!({"index": find(&plugins, &query)?}))
        } else {
            Ok(json!({"indices": filter(&plugins, &query)}))
        }
    };
    answer().unwrap_or_else(|e| json!({ "error": e }))
}

/// **The client door to a reference**: `{"plugin": entry, "inputs": route,
/// "outputs": route, "state": [byte, ...]}` (each route as
/// [`Route::from_json`] reads it) answers the [`reference`] as JSON.
#[must_use]
pub fn reference_request(request: &Value) -> Value {
    let answer = || -> Result<Value, String> {
        let info: PluginInfo = serde_json::from_value(
            request
                .get("plugin")
                .cloned()
                .ok_or("a reference names its plugin")?,
        )
        .map_err(|e| format!("invalid plugin: {e}"))?;
        let none = Value::Null;
        let inputs = Route::from_json(request.get("inputs").unwrap_or(&none))?;
        let outputs = Route::from_json(request.get("outputs").unwrap_or(&none))?;
        let state: Vec<u8> = match request.get("state") {
            None | Some(Value::Null) => Vec::new(),
            Some(bytes) => serde_json::from_value(bytes.clone())
                .map_err(|_| "a plugin state is a list of bytes")?,
        };
        let reference = reference(&info, &inputs, &outputs, state)?;
        serde_json::to_value(reference).map_err(|e| e.to_string())
    };
    answer().unwrap_or_else(|e| json!({ "error": e }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(id: u32, name: &str, channels: u32, main: bool) -> AudioPort {
        AudioPort {
            id,
            name: name.into(),
            channels,
            flags: u32::from(main),
            main,
            layout: String::new(),
            active: true,
        }
    }

    fn parameter(id: u32, name: &str) -> Parameter {
        Parameter {
            id,
            name: name.into(),
            module: String::new(),
            flags: 0,
            min: 0.0,
            max: 1.0,
            step: 0.0,
            default: 0.5,
        }
    }

    fn plugin(name: &str, format: &str, features: &[&str]) -> PluginInfo {
        PluginInfo {
            format: format.into(),
            path: format!("/plugins/{name}.{format}"),
            id: format!("org.test.{name}"),
            name: name.into(),
            vendor: "Test".into(),
            version: "1".into(),
            description: String::new(),
            url: String::new(),
            manual_url: String::new(),
            support_url: String::new(),
            features: features.iter().map(|s| (*s).to_owned()).collect(),
            sdk_version: String::new(),
            inputs: vec![port(10, "Main", 2, true), port(11, "Side Chain", 1, false)],
            outputs: vec![port(20, "Main", 2, true)],
            note_inputs: vec![],
            note_outputs: vec![],
            parameters: vec![parameter(5, "Volume"), parameter(2, "Mix (%)")],
            latency: 0,
            state: true,
            editor: false,
        }
    }

    #[test]
    fn both_vocabularies_read_into_one() {
        let clap = plugin("verb", "clap", &["audio-effect", "reverb", "stereo"]);
        assert_eq!(clap.kind(), Kind::Effect);
        assert_eq!(clap.tags(), ["reverb"]);
        assert_eq!(clap.channels(), Some("stereo"));
        let vst3 = plugin("keys", "vst3", &["Instrument", "Piano"]);
        assert_eq!(vst3.kind(), Kind::Instrument);
        assert_eq!(vst3.tags(), ["piano"]);
        let analyzer = plugin("scope", "vst3", &["Fx", "Analyzer"]);
        assert_eq!(analyzer.kind(), Kind::Analyzer);
        let both = plugin(
            "synth",
            "clap",
            &["audio-effect", "instrument", "synthesizer"],
        );
        assert_eq!(both.kind(), Kind::Instrument);
        assert_eq!(both.tags(), ["synth"]);
    }

    #[test]
    fn an_undeclared_kind_comes_from_the_ports() {
        let mut p = plugin("x", "clap", &[]);
        assert_eq!(p.kind(), Kind::Effect);
        assert_eq!(p.channels(), Some("stereo"));
        p.inputs.clear();
        p.note_inputs.push(NotePort {
            id: 0,
            name: "Notes".into(),
            dialects: 1,
            preferred: 1,
            channels: 16,
        });
        assert_eq!(p.kind(), Kind::Instrument);
    }

    #[test]
    fn controls_read_as_their_ports_and_parameters() {
        let p = plugin("verb", "clap", &[]);
        let aliases = p.control_aliases();
        assert_eq!(
            aliases,
            [
                ("in".to_owned(), "in:10".to_owned()),
                ("in_side_chain".to_owned(), "in:11".to_owned()),
                ("out".to_owned(), "out:20".to_owned()),
                ("mix".to_owned(), "param:2".to_owned()),
                ("volume".to_owned(), "param:5".to_owned()),
            ]
        );
        assert_eq!(
            p.control_names(),
            ["param:2", "param:5", "in:10", "in:11", "out:20"]
        );
    }

    #[test]
    fn a_shared_alias_is_nobodys_and_a_port_wins() {
        let mut p = plugin("x", "clap", &[]);
        p.parameters = vec![
            parameter(1, "Gain"),
            parameter(2, "gain"),
            parameter(3, "In"),
            parameter(4, "1 Voice"),
        ];
        let aliases: Vec<String> = p.control_aliases().into_iter().map(|(a, _)| a).collect();
        assert_eq!(aliases, ["in", "in_side_chain", "out", "p1_voice"]);
        assert_eq!(identifier("  --  "), None);
        assert_eq!(identifier("Cut-Off Freq."), Some("cut_off_freq".into()));
    }

    #[test]
    fn a_reference_routes_its_ports_and_is_checked() {
        let p = plugin("verb", "clap", &[]);
        let r = reference(&p, &Route::From(4), &Route::From(0), vec![]).unwrap();
        assert_eq!(
            r.inputs.iter().map(|x| x.bus).collect::<Vec<_>>(),
            [4, -1],
            "the main port on the bus, the sidechain unconnected"
        );
        assert_eq!(r.outputs[0].bus, 0);
        let r = reference(&p, &Route::None, &Route::From(0), vec![]).unwrap();
        assert!(r.inputs.iter().all(|x| x.bus == -1));
        let named = Route::Ports(vec![("in_side_chain".into(), 8), ("in:10".into(), 2)]);
        let r = reference(&p, &named, &Route::From(0), vec![]).unwrap();
        assert_eq!(r.inputs.iter().map(|x| x.bus).collect::<Vec<_>>(), [2, 8]);
        let wrong = Route::Ports(vec![("in_aux".into(), 8)]);
        let error = reference(&p, &wrong, &Route::From(0), vec![]).unwrap_err();
        assert!(error.contains("in_side_chain"), "{error}");
        let mut bad = r.clone();
        bad.inputs.pop();
        assert_eq!(
            bad.check(&p).unwrap_err(),
            "plugin port configuration mismatch"
        );
    }

    #[test]
    fn a_search_names_one_or_says_which() {
        let plugins = vec![
            plugin("Piano", "clap", &["instrument", "piano"]),
            plugin("Piano", "vst3", &["Instrument", "Piano"]),
            plugin("Piano Hall", "clap", &["audio-effect", "reverb"]),
        ];
        let by_name = |name: &str| Query {
            name: Some(name.into()),
            ..Query::default()
        };
        let error = find(&plugins, &by_name("piano")).unwrap_err();
        assert!(error.starts_with("2 plugins match"), "{error}");
        let q = Query {
            format: Some("VST3".into()),
            ..by_name("piano")
        };
        assert_eq!(find(&plugins, &q), Ok(1));
        assert_eq!(find(&plugins, &by_name("hall")), Ok(2));
        assert!(find(&plugins, &by_name("organ")).is_err());
        let reverbs = Query {
            tag: Some("reverb".into()),
            ..Query::default()
        };
        assert_eq!(filter(&plugins, &reverbs), [2]);
        let instruments = Query {
            kind: Some(Kind::Instrument),
            ..Query::default()
        };
        assert_eq!(filter(&plugins, &instruments), [0, 1]);
    }

    #[test]
    fn the_doors_answer_in_json() {
        let p = plugin("verb", "clap", &["audio-effect", "reverb"]);
        let catalog = catalog_request(&json!({"rows": [{"kind": "plugin", "plugin": p}]}));
        let plugins = catalog["plugins"].clone();
        let found =
            search_request(&json!({"plugins": plugins, "query": {"tag": "reverb"}, "one": true}));
        assert_eq!(found, json!({"index": 0}));
        let reference =
            reference_request(&json!({"plugin": plugins[0], "inputs": {"in": 4}, "outputs": 0}));
        assert_eq!(reference["inputs"][0]["bus"], 4);
        assert_eq!(reference["inputs"][1]["bus"], -1);
        assert_eq!(reference["depth"], DEFAULT_DEPTH);
        assert!(
            reference_request(&json!({"plugin": plugins[0], "outputs": "x"}))["error"].is_string()
        );
        assert!(
            search_request(&json!({"plugins": plugins, "query": {"size": 1}}))["error"].is_string()
        );
    }

    #[test]
    fn a_catalog_reads_rows_once_each_and_keeps_old_ones() {
        let p = plugin("verb", "clap", &["audio-effect"]);
        let row = json!({"kind": "plugin", "plugin": p});
        let mut old = row.clone();
        for key in [
            "description",
            "url",
            "manual_url",
            "support_url",
            "features",
            "sdk_version",
        ] {
            old["plugin"].as_object_mut().unwrap().remove(key);
        }
        for port in old["plugin"]["inputs"].as_array_mut().unwrap() {
            for key in ["main", "layout", "active"] {
                port.as_object_mut().unwrap().remove(key);
            }
        }
        let error = json!({"kind": "error", "path": "/plugins/bad.clap", "error": "timeout"});
        let catalog = catalog_json(&[row.clone(), row, error, old]).unwrap();
        let plugins = catalog["plugins"].as_array().unwrap();
        assert_eq!(plugins.len(), 1, "one identity, one entry");
        assert_eq!(plugins[0]["kind"], "effect");
        assert_eq!(plugins[0]["controls"]["volume"], "param:5");
        assert_eq!(catalog["errors"][0]["error"], "timeout");
        let read = Catalog::from_rows(&[json!({"kind": "plugin", "plugin": {
            "format": "clap", "path": "/p.clap", "id": "x", "name": "X", "vendor": "",
            "version": "", "inputs": [{"id": 0, "name": "In", "channels": 1, "flags": 1}],
            "outputs": [{"id": 0, "name": "Out", "channels": 1, "flags": 1}],
            "note_inputs": [{"id": 0, "name": "N", "dialects": 1}], "note_outputs": [],
            "parameters": [], "latency": 0, "state": false, "editor": false}})])
        .unwrap();
        assert!(read.plugins[0].inputs[0].active);
        assert_eq!(read.plugins[0].note_inputs[0].channels, 16);
    }
}
