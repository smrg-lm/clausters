//! The manifest and what a resolution produces: the format's own types.
//!
//! Everything here is `serde` shape — what `bundle.json` declares, what a
//! template carries, what the caller allocates, what comes back — plus the one
//! [`Error`] the passes speak. No logic: the passes are in [`super`], the
//! substitution in [`super::resolve`].

use super::*;

/// A control bus's rate. A server bus is one of exactly these two; the third
/// rate a def knows (`ir`) is never a bus.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BusRate {
    Audio,
    #[default]
    Control,
}

/// A bus symbol's declaration: what the caller must allocate for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BusSpec {
    pub name: String,
    #[serde(default)]
    pub rate: BusRate,
    #[serde(default = "one")]
    pub channels: u32,
}

fn one() -> u32 {
    1
}

/// The symbols a bundle needs allocated — the `@` half of the holes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolTable {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buses: Vec<BusSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buffers: Vec<String>,
}

impl SymbolTable {
    /// Whether nothing at all is declared (a bundle with no symbols at all is
    /// the common case, and its manifest should not carry an empty table).
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.buses.is_empty() && self.buffers.is_empty()
    }

    /// The namespace `name` belongs to, or `None` when it is not declared.
    pub(super) fn namespace(&self, name: &str) -> Option<Namespace> {
        if self.nodes.iter().any(|n| n == name) {
            Some(Namespace::Node)
        } else if self.buses.iter().any(|b| b.name == name) {
            Some(Namespace::Bus)
        } else if self.buffers.iter().any(|b| b == name) {
            Some(Namespace::Buffer)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Namespace {
    Node,
    Bus,
    Buffer,
}

/// A declared parameter's type — the `$` half of the holes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParamType {
    Float,
    Int,
    String,
    Bool,
}

impl fmt::Display for ParamType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ParamType::Float => "float",
            ParamType::Int => "int",
            ParamType::String => "string",
            ParamType::Bool => "bool",
        })
    }
}

/// One declared parameter: its type, its default, and the range it accepts.
///
/// A parameter with no `default` is required — the tag or a preset must supply
/// it, and mounting without it is an error rather than a silent zero.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParamSpec {
    #[serde(rename = "type")]
    pub kind: ParamType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

/// `bundle.json`: what the directory holds and what mounting it needs.
///
/// Every field but `gui` has a default, so a bundle written before the format
/// grew its contract still parses (it simply declares no symbols and no
/// parameters, which is exactly what it has).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// The bundle's name — also the prefix its def names carry, since a def
    /// name is a global namespace on the server.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// The GuiDef record to mount, by file stem under `defs/guidefs/`.
    pub gui: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub synthdefs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub graphdefs: Vec<String>,
    /// The width of the id block one instance needs: the **highest** local
    /// widget id the template uses, the root's included. Not a count — the
    /// template may number sparsely (`1`, `10`, `20`), and what the caller
    /// allocates is a contiguous run wide enough for the numbering.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub widgets: usize,
    #[serde(default, skip_serializing_if = "SymbolTable::is_empty")]
    pub symbols: SymbolTable,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, ParamSpec>,
    /// The preset names served under `presets/<name>.json`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub presets: Vec<String>,
    /// Buffer symbol -> audio file, relative to the bundle root. The symbol is
    /// what `@name` resolves to; the caller allocates the buffer index.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub buffers: BTreeMap<String, String>,
    /// Whether the bundle carries a `boot.json` preset. Declared so a browser
    /// never probes for the optional file (a probe's 404 litters the console).
    #[serde(default, skip_serializing_if = "is_false")]
    pub boot: bool,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// The persisted GuiDef record — `{ "id": <i32>, "gui": <tree> }` — read as the
/// template it is: its widget ids are local `1..N`, and its props may hold
/// placeholders.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Template {
    pub id: i32,
    pub gui: Value,
}

/// What mounting one instance of a bundle needs the caller to allocate.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Requirements {
    /// A contiguous block of this many widget ids.
    pub widgets: usize,
    pub nodes: Vec<String>,
    pub buses: Vec<BusSpec>,
    pub buffers: Vec<String>,
}

/// What the caller allocated, handed back to [`resolve`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Allocation {
    /// The first widget id of the block; the template's widget `1` becomes it.
    pub widget_base: i32,
    #[serde(default)]
    pub nodes: BTreeMap<String, i32>,
    #[serde(default)]
    pub buses: BTreeMap<String, i32>,
    #[serde(default)]
    pub buffers: BTreeMap<String, i32>,
}

/// One mounted instance: what to open and what to send.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Resolved {
    /// The id to open the GuiDef under (`/gui_def <def_id> …`).
    pub def_id: i32,
    /// The tree, holes filled and widget ids offset. The root's `boot` list is
    /// **not** in it: it left in [`boot`](Self::boot), so no caller can send it
    /// twice.
    pub tree: Value,
    /// The root's `boot` messages, resolved — each `[addr, args…]`, with the
    /// int/float distinction JSON already carries.
    pub boot: Vec<Vec<Value>>,
    /// The merged parameter values that produced the tree, typed as declared.
    /// Returned because a caller often needs them again (a `/node_set` it sends
    /// itself, a value it displays).
    pub params: BTreeMap<String, Value>,
}

/// What can go wrong resolving or validating a bundle.
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// A `@name` the manifest never declared.
    UnknownSymbol(String),
    /// A declared symbol the caller did not allocate.
    UnallocatedSymbol(String),
    /// A `$name` the manifest never declared.
    UnknownParam(String),
    /// A declared parameter with no default that nothing supplied.
    MissingParam(String),
    /// A supplied value that is not the declared type.
    TypeMismatch {
        name: String,
        expected: ParamType,
        got: String,
    },
    /// A supplied value outside the declared range.
    OutOfRange {
        name: String,
        value: f64,
        min: Option<f64>,
        max: Option<f64>,
    },
    /// The manifest declares a narrower id block than the template numbers
    /// into — two instances offset by it would overlap.
    WidgetBlock { declared: usize, needed: usize },
    /// One widget id used twice in a template (the root's id counts): the two
    /// would resolve to the same widget.
    DuplicateWidgetId(i64),
    /// One name declared in two symbol namespaces — `@name` would be ambiguous.
    DuplicateSymbol(String),
    /// A placeholder found where none may live (a def payload).
    PlaceholderInDef(String),
    /// The template is not a GuiDef record.
    BadTemplate(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::UnknownSymbol(name) => {
                write!(
                    f,
                    "unknown symbol \"@{name}\": the manifest declares no such node, bus or buffer"
                )
            }
            Error::UnallocatedSymbol(name) => {
                write!(f, "symbol \"@{name}\" was declared but not allocated")
            }
            Error::UnknownParam(name) => {
                write!(
                    f,
                    "unknown parameter \"${name}\": the manifest declares no such parameter"
                )
            }
            Error::MissingParam(name) => {
                write!(
                    f,
                    "parameter \"{name}\" has no default and no value was supplied"
                )
            }
            Error::TypeMismatch {
                name,
                expected,
                got,
            } => write!(f, "parameter \"{name}\" wants a {expected}, got {got}"),
            Error::OutOfRange {
                name,
                value,
                min,
                max,
            } => {
                write!(f, "parameter \"{name}\" is {value}, outside")?;
                match (min, max) {
                    (Some(lo), Some(hi)) => write!(f, " [{lo}, {hi}]"),
                    (Some(lo), None) => write!(f, " [{lo}, ..)"),
                    (None, Some(hi)) => write!(f, " (.., {hi}]"),
                    (None, None) => Ok(()),
                }
            }
            Error::WidgetBlock { declared, needed } => write!(
                f,
                "the manifest declares a {declared}-wide widget id block, but the \
                 template numbers up to {needed}"
            ),
            Error::DuplicateWidgetId(id) => write!(
                f,
                "widget id {id} is used twice in the template (the root's id counts)"
            ),
            Error::DuplicateSymbol(name) => write!(
                f,
                "symbol \"{name}\" is declared in two namespaces, so \"@{name}\" is ambiguous"
            ),
            Error::PlaceholderInDef(text) => write!(
                f,
                "the def payload holds the placeholder \"{text}\"; a bus, a node or a \
                 buffer reaches a def as a control, never as a baked constant"
            ),
            Error::BadTemplate(why) => write!(f, "malformed GuiDef template: {why}"),
        }
    }
}

impl std::error::Error for Error {}
