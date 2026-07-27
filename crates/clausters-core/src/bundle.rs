//! The component bundle: its manifest, its two kinds of hole, and the resolver.
//!
//! A **bundle** is the directory of persisted data that one component mounts —
//! the def payloads, the GuiDef record, the presets, the samples — and the
//! manifest (`bundle.json`) that declares what mounting it needs. One directory
//! runs on three legs: a browser tab, `clausters-gui --standalone`, and a
//! loopback host against a running server. This module is the part that has to
//! agree between them, so it lives here rather than in any one client.
//!
//! # The two kinds of hole
//!
//! Mounting the *same* bundle twice on one page must not collide, so the GuiDef
//! record on disk is a **template** with placeholders, told apart by sigil:
//!
//! ```text
//! "@lfo", "@graph"   a symbol    — an id the caller allocates when mounting
//! "$freq", "$title"  a parameter — a value the tag supplies, or a preset's,
//!                                  or the declared default
//! ```
//!
//! Widget ids are deliberately **not** symbols: the template numbers its widgets
//! `1..N` and [`resolve`] offsets them by a base the caller allocates, so twelve
//! widgets do not mean twelve placeholders.
//!
//! **Placeholders live only in the GuiDef record** (and in its `boot` list).
//! That is the invariant the format is built on: the def payloads under `defs/`
//! contain no holes, so they are byte-identical between two mounted instances
//! and are sent to the server once. It forces one authoring rule, which is the
//! right rule anyway — *a bus, a node or a buffer reaches a def as a control,
//! never as a baked constant.* [`check_def_payload`] is how a writer enforces
//! it.
//!
//! A doubled sigil escapes it: `"$$5"` resolves to the literal `"$5"`, so prose
//! that wants a `$` or an `@` can still be written.
//!
//! # Two pure steps, with the allocation in between
//!
//! ```text
//! requirements(manifest)  ->  { widgets, nodes, buses, buffers }
//!         ... the caller allocates from its own allocators ...
//! resolve(manifest, template, allocation, params)  ->  { def_id, tree, boot }
//! ```
//!
//! Nothing here allocates an id, keeps state or knows a transport: what comes
//! out is the same `/gui_def` tree and `boot` messages as a hand-written bundle,
//! so nothing is added to the `/gui_*` protocol.
//!
//! [`validate`] is the same machinery pointed the other way, for the writers: a
//! dry run over the declared defaults, so a bundle that would fail to mount
//! fails to be written.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

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
    fn namespace(&self, name: &str) -> Option<Namespace> {
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
enum Namespace {
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
    /// Returned because a caller often needs them again (a `/n_set` it sends
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

/// What one instance of `manifest` needs allocated.
///
/// Pure bookkeeping: it reads the declaration and hands it back in the shape a
/// caller allocates from. The caller then builds an [`Allocation`] and calls
/// [`resolve`].
pub fn requirements(manifest: &Manifest) -> Requirements {
    Requirements {
        widgets: manifest.widgets,
        nodes: manifest.symbols.nodes.clone(),
        buses: manifest.symbols.buses.clone(),
        buffers: manifest.symbols.buffers.clone(),
    }
}

/// [`requirements`], with the template on hand to measure what the manifest
/// does not declare.
///
/// A written bundle numbers its widgets `1..N` and declares `widgets`, so the
/// two agree. A bundle written **before** the contract declares nothing, and
/// its saved ids are whatever the author picked — `1`, `10`, `20`. Offsetting
/// those by a one-wide block would make two instances overlap, so the block is
/// sized to the ids actually used ([`widget_span`]) instead.
pub fn requirements_for(manifest: &Manifest, template: Option<&Template>) -> Requirements {
    let mut out = requirements(manifest);
    if let Some(template) = template {
        out.widgets = out.widgets.max(widget_span(template));
    }
    out
}

/// The width of the id block a template needs: its highest local widget id,
/// the root's included. (Ids are offset by `widget_base - 1`, so a template
/// using `1..=20` needs twenty ids however few widgets it holds.)
pub fn widget_span(template: &Template) -> usize {
    fn walk(node: &Value, high: &mut i64) {
        let Some(map) = node.as_object() else { return };
        if let Some(id) = map.get("id").and_then(Value::as_i64) {
            *high = (*high).max(id);
        }
        if let Some(children) = map.get("children").and_then(Value::as_array) {
            for child in children {
                walk(child, high);
            }
        }
    }
    let mut high = template.id as i64;
    walk(&template.gui, &mut high);
    high.max(1) as usize
}

/// The parameter values a mount supplies, in the order they win.
///
/// Resolution is **attribute → preset → declared default**, so a preset is a
/// named bundle of values and an attribute is a local override. Both maps may
/// hold names the manifest does not declare (a tag carries `class` and `style`
/// like any element); those are ignored rather than refused.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ParamInput {
    #[serde(default)]
    pub attributes: Map<String, Value>,
    #[serde(default)]
    pub preset: Map<String, Value>,
}

impl ParamInput {
    /// Just the attributes — the common mount, with no preset named.
    pub fn from_attributes(attributes: Map<String, Value>) -> ParamInput {
        ParamInput {
            attributes,
            preset: Map::new(),
        }
    }
}

/// Fills one instance of a template: offsets its widget ids, substitutes every
/// hole, and lifts its `boot` list out.
///
/// `allocation` is what the caller allocated for the [`requirements`]; `params`
/// is what the tag and its preset supplied. Every declared parameter is typed
/// and range-checked here, so a bad value is an error at mount rather than a
/// surprise on a bus.
pub fn resolve(
    manifest: &Manifest,
    template: &Template,
    allocation: &Allocation,
    params: &ParamInput,
) -> Result<Resolved, Error> {
    check_symbol_namespaces(&manifest.symbols)?;
    let values = merge_params(manifest, params)?;
    let ctx = Ctx {
        symbols: &manifest.symbols,
        allocation,
        params: &values,
    };

    let Value::Object(root) = &template.gui else {
        return Err(Error::BadTemplate("the root is not an object".into()));
    };
    check_widget_ids(template)?;
    let needed = widget_span(template);
    if manifest.widgets != 0 && manifest.widgets < needed {
        return Err(Error::WidgetBlock {
            declared: manifest.widgets,
            needed,
        });
    }

    let offset = allocation.widget_base - 1;
    let mut root = root.clone();
    let boot = match root.remove("boot") {
        Some(Value::Array(list)) => resolve_boot(&list, &ctx)?,
        _ => Vec::new(),
    };
    let tree = resolve_node(&root, offset, &ctx)?;

    Ok(Resolved {
        def_id: template.id + offset,
        tree,
        boot,
        params: values,
    })
}

/// Checks a manifest against its own template, the way a writer needs it
/// checked: the same resolution a mount performs, run over a synthetic
/// allocation and the declared defaults.
///
/// So a bundle whose template names an undeclared symbol, whose parameter
/// defaults do not type-check, or whose widget count is wrong, fails here —
/// at the point it would be written, not at the point someone mounts it.
pub fn validate(manifest: &Manifest, template: &Template) -> Result<(), Error> {
    check_symbol_namespaces(&manifest.symbols)?;
    let mut allocation = Allocation {
        widget_base: 1,
        ..Allocation::default()
    };
    // Distinct dummy ids, so a substitution that swaps two namespaces still
    // shows up as a wrong value rather than passing by coincidence.
    for (i, name) in manifest.symbols.nodes.iter().enumerate() {
        allocation.nodes.insert(name.clone(), 1000 + i as i32);
    }
    for (i, bus) in manifest.symbols.buses.iter().enumerate() {
        allocation.buses.insert(bus.name.clone(), 2000 + i as i32);
    }
    for (i, buffer) in manifest.symbols.buffers.iter().enumerate() {
        allocation.buffers.insert(buffer.clone(), 3000 + i as i32);
    }
    resolve(manifest, template, &allocation, &ParamInput::default())?;
    Ok(())
}

/// Refuses a placeholder in a def payload — the invariant that keeps def
/// payloads shareable between two mounted instances. A writer calls this on
/// every `/d_recv` and `/d_graph` spec before emitting it.
pub fn check_def_payload(payload: &Value) -> Result<(), Error> {
    match payload {
        Value::String(s) => match placeholder(s) {
            Some(Hole::Symbol(_)) | Some(Hole::Param(_)) => Err(Error::PlaceholderInDef(s.clone())),
            _ => Ok(()),
        },
        Value::Array(items) => items.iter().try_for_each(check_def_payload),
        Value::Object(map) => map.values().try_for_each(check_def_payload),
        _ => Ok(()),
    }
}

// --- the shape the bindings carry ---------------------------------------
//
// [`resolve`] and [`validate`] take three and two arguments; a C or wasm door
// carrying each as its own pointer pair would be a wide surface that the two
// doors could then drift apart on. So the envelope is declared here, once, and
// both doors are the same two lines over it.

/// One [`requirements_for`] call: the manifest, and the template when the
/// caller has it (a pre-contract bundle's id block is measured from it).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RequirementsRequest {
    pub manifest: Manifest,
    #[serde(default)]
    pub template: Option<Template>,
}

/// One [`resolve`] call as a single JSON object — what the wasm and C doors
/// carry.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ResolveRequest {
    pub manifest: Manifest,
    pub template: Template,
    pub allocation: Allocation,
    #[serde(default)]
    pub params: ParamInput,
}

/// One [`validate`] call, plus the def payloads to check for holes — the whole
/// pre-flight a writer runs before emitting a directory.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ValidateRequest {
    pub manifest: Manifest,
    pub template: Template,
    /// The `/d_recv` and `/d_graph` payloads, each parsed. Checked by
    /// [`check_def_payload`].
    #[serde(default)]
    pub defs: Vec<Value>,
}

/// [`requirements_for`], from the envelope the bindings carry.
pub fn requirements_request(request: &RequirementsRequest) -> Requirements {
    requirements_for(&request.manifest, request.template.as_ref())
}

/// [`resolve`], from the envelope the bindings carry.
pub fn resolve_request(request: &ResolveRequest) -> Result<Resolved, Error> {
    resolve(
        &request.manifest,
        &request.template,
        &request.allocation,
        &request.params,
    )
}

/// [`validate`] plus [`check_def_payload`] over every payload, from the
/// envelope the bindings carry.
pub fn validate_request(request: &ValidateRequest) -> Result<(), Error> {
    validate(&request.manifest, &request.template)?;
    request.defs.iter().try_for_each(check_def_payload)
}

// --- the resolution machinery -------------------------------------------

/// What a template string is, once its first characters are read.
enum Hole<'a> {
    Symbol(&'a str),
    Param(&'a str),
    /// A doubled sigil: the literal text with one sigil dropped.
    Escaped(String),
}

/// Reads a template string as a hole, or `None` when it is ordinary text.
///
/// Only a **whole** string is a placeholder — `"@lfo"` is the bus, `"bus @lfo"`
/// is prose. Substituting inside text would make every label a minefield and
/// would have to invent a type for the result.
fn placeholder(s: &str) -> Option<Hole<'_>> {
    let mut chars = s.chars();
    let sigil = chars.next()?;
    if sigil != '@' && sigil != '$' {
        return None;
    }
    let rest = &s[sigil.len_utf8()..];
    if rest.starts_with(sigil) {
        return Some(Hole::Escaped(rest.to_string()));
    }
    if rest.is_empty() {
        return None;
    }
    Some(match sigil {
        '@' => Hole::Symbol(rest),
        _ => Hole::Param(rest),
    })
}

struct Ctx<'a> {
    symbols: &'a SymbolTable,
    allocation: &'a Allocation,
    params: &'a BTreeMap<String, Value>,
}

impl Ctx<'_> {
    /// One `@name` as the id the caller allocated for it.
    fn symbol(&self, name: &str) -> Result<Value, Error> {
        let table = match self.symbols.namespace(name) {
            Some(Namespace::Node) => &self.allocation.nodes,
            Some(Namespace::Bus) => &self.allocation.buses,
            Some(Namespace::Buffer) => &self.allocation.buffers,
            None => return Err(Error::UnknownSymbol(name.to_string())),
        };
        table
            .get(name)
            .map(|id| Value::Number((*id).into()))
            .ok_or_else(|| Error::UnallocatedSymbol(name.to_string()))
    }

    /// One `$name` as its merged, typed value.
    fn param(&self, name: &str) -> Result<Value, Error> {
        self.params
            .get(name)
            .cloned()
            .ok_or_else(|| Error::UnknownParam(name.to_string()))
    }
}

/// Substitutes holes through any prop value, however nested.
fn substitute(value: &Value, ctx: &Ctx) -> Result<Value, Error> {
    Ok(match value {
        Value::String(s) => match placeholder(s) {
            Some(Hole::Symbol(name)) => ctx.symbol(name)?,
            Some(Hole::Param(name)) => ctx.param(name)?,
            Some(Hole::Escaped(text)) => Value::String(text),
            None => value.clone(),
        },
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| substitute(item, ctx))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| Ok((k.clone(), substitute(v, ctx)?)))
                .collect::<Result<Map<_, _>, Error>>()?,
        ),
        _ => value.clone(),
    })
}

/// One widget node: its own id offset, its props substituted, its children
/// walked. Ids are offset **structurally** — only the `id` of a node reached
/// through `children` — so a prop that happens to be called `id` is left alone.
fn resolve_node(node: &Map<String, Value>, offset: i32, ctx: &Ctx) -> Result<Value, Error> {
    let mut out = Map::new();
    for (key, value) in node {
        match key.as_str() {
            "id" => {
                let Some(id) = value.as_i64() else {
                    return Err(Error::BadTemplate(format!(
                        "widget id {value} is not an int"
                    )));
                };
                out.insert("id".into(), Value::from(id as i32 + offset));
            }
            "children" => {
                let Value::Array(children) = value else {
                    return Err(Error::BadTemplate("\"children\" is not an array".into()));
                };
                let resolved: Vec<Value> = children
                    .iter()
                    .map(|child| match child {
                        Value::Object(map) => resolve_node(map, offset, ctx),
                        other => Err(Error::BadTemplate(format!(
                            "a child widget is not an object: {other}"
                        ))),
                    })
                    .collect::<Result<_, _>>()?;
                out.insert("children".into(), Value::Array(resolved));
            }
            _ => {
                out.insert(key.clone(), substitute(value, ctx)?);
            }
        }
    }
    Ok(Value::Object(out))
}

/// The root's `boot` list, each entry `[addr, args…]` with its holes filled.
fn resolve_boot(list: &[Value], ctx: &Ctx) -> Result<Vec<Vec<Value>>, Error> {
    let mut out = Vec::with_capacity(list.len());
    for entry in list {
        let Value::Array(items) = entry else {
            return Err(Error::BadTemplate(format!(
                "a boot entry is not an array: {entry}"
            )));
        };
        out.push(
            items
                .iter()
                .map(|item| substitute(item, ctx))
                .collect::<Result<_, _>>()?,
        );
    }
    Ok(out)
}

/// Refuses a widget id used twice — the root's id counts, since it is offset
/// with the rest and a child numbered like the root would resolve onto it.
fn check_widget_ids(template: &Template) -> Result<(), Error> {
    fn walk(node: &Value, seen: &mut Vec<i64>) -> Result<(), Error> {
        let Some(map) = node.as_object() else {
            return Ok(());
        };
        if let Some(id) = map.get("id").and_then(Value::as_i64) {
            if seen.contains(&id) {
                return Err(Error::DuplicateWidgetId(id));
            }
            seen.push(id);
        }
        if let Some(children) = map.get("children").and_then(Value::as_array) {
            for child in children {
                walk(child, seen)?;
            }
        }
        Ok(())
    }
    let mut seen = vec![template.id as i64];
    walk(&template.gui, &mut seen)
}

/// Refuses one name declared in two namespaces: `@name` would not say which.
fn check_symbol_namespaces(symbols: &SymbolTable) -> Result<(), Error> {
    let mut seen: Vec<&str> = Vec::new();
    let names = symbols
        .nodes
        .iter()
        .map(String::as_str)
        .chain(symbols.buses.iter().map(|b| b.name.as_str()))
        .chain(symbols.buffers.iter().map(String::as_str));
    for name in names {
        if seen.contains(&name) {
            return Err(Error::DuplicateSymbol(name.to_string()));
        }
        seen.push(name);
    }
    Ok(())
}

// --- parameters ----------------------------------------------------------

/// Merges **attribute → preset → default** and types every declared parameter.
fn merge_params(manifest: &Manifest, input: &ParamInput) -> Result<BTreeMap<String, Value>, Error> {
    let mut out = BTreeMap::new();
    for (name, spec) in &manifest.params {
        let supplied = input
            .attributes
            .get(name)
            .or_else(|| input.preset.get(name))
            .or(spec.default.as_ref());
        let Some(value) = supplied else {
            return Err(Error::MissingParam(name.clone()));
        };
        out.insert(name.clone(), coerce(name, spec, value)?);
    }
    Ok(out)
}

/// One supplied value as its declared type, then range-checked.
///
/// Strings coerce into the numeric and boolean types, because an HTML attribute
/// is always a string: `freq="440"` on a `float` parameter is the ordinary way
/// a component is written, not a sloppy one.
fn coerce(name: &str, spec: &ParamSpec, value: &Value) -> Result<Value, Error> {
    let mismatch = || Error::TypeMismatch {
        name: name.to_string(),
        expected: spec.kind,
        got: describe(value),
    };
    let typed = match spec.kind {
        ParamType::Float => {
            let n = match value {
                Value::Number(n) => n.as_f64().ok_or_else(mismatch)?,
                Value::String(s) => s.trim().parse::<f64>().map_err(|_| mismatch())?,
                _ => return Err(mismatch()),
            };
            if !n.is_finite() {
                return Err(mismatch());
            }
            range_check(name, spec, n)?;
            Value::from(n)
        }
        ParamType::Int => {
            let n = match value {
                Value::Number(n) => n.as_i64().ok_or_else(mismatch)?,
                Value::String(s) => s.trim().parse::<i64>().map_err(|_| mismatch())?,
                _ => return Err(mismatch()),
            };
            range_check(name, spec, n as f64)?;
            Value::from(n)
        }
        ParamType::String => match value {
            Value::String(s) => Value::String(s.clone()),
            _ => return Err(mismatch()),
        },
        ParamType::Bool => match value {
            Value::Bool(b) => Value::Bool(*b),
            Value::String(s) => match s.trim() {
                "true" | "1" => Value::Bool(true),
                "false" | "0" | "" => Value::Bool(false),
                _ => return Err(mismatch()),
            },
            Value::Number(n) => Value::Bool(n.as_f64().ok_or_else(mismatch)? != 0.0),
            _ => return Err(mismatch()),
        },
    };
    Ok(typed)
}

fn range_check(name: &str, spec: &ParamSpec, n: f64) -> Result<(), Error> {
    let low = spec.min.is_some_and(|min| n < min);
    let high = spec.max.is_some_and(|max| n > max);
    if low || high {
        return Err(Error::OutOfRange {
            name: name.to_string(),
            value: n,
            min: spec.min,
            max: spec.max,
        });
    }
    Ok(())
}

/// A supplied value's kind, for the error message.
fn describe(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(_) => "a bool".into(),
        Value::Number(_) => "a number".into(),
        Value::String(s) => format!("the string {s:?}"),
        Value::Array(_) => "an array".into(),
        Value::Object(_) => "an object".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn manifest() -> Manifest {
        serde_json::from_value(json!({
            "name": "fm-voice",
            "gui": "fm-voice",
            "synthdefs": ["fm-voice.voice"],
            "widgets": 4,
            "symbols": {
                "nodes": ["graph"],
                "buses": [{ "name": "lfo", "rate": "control", "channels": 1 }],
                "buffers": ["hit"]
            },
            "params": {
                "freq": { "type": "float", "default": 220.0, "min": 60.0, "max": 700.0 },
                "title": { "type": "string", "default": "FM voice" }
            }
        }))
        .unwrap()
    }

    fn template() -> Template {
        serde_json::from_value(json!({
            "id": 1,
            "gui": {
                "type": "window",
                "title": "$title",
                "boot": [["/graph_new", "fm-voice.graph", "@graph", 0, 0],
                         ["/n_set", "@graph", "freq", "$freq"]],
                "children": [
                    { "id": 2, "type": "meter", "bus": "@lfo" },
                    { "id": 3, "type": "panel", "children": [
                        { "id": 4, "type": "knob", "value": "$freq",
                          "bind": ["/n_set", "@graph", "freq"] }
                    ]}
                ]
            }
        }))
        .unwrap()
    }

    fn allocation() -> Allocation {
        Allocation {
            widget_base: 100,
            nodes: [("graph".to_string(), 1500)].into(),
            buses: [("lfo".to_string(), 17)].into(),
            buffers: [("hit".to_string(), 4)].into(),
        }
    }

    /// The manifest round-trips, and its optional halves stay out of the JSON.
    #[test]
    fn manifest_round_trips() {
        let m = manifest();
        let back: Manifest = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(m, back);
        let minimal: Manifest = serde_json::from_value(json!({ "gui": "drone" })).unwrap();
        assert_eq!(
            serde_json::to_value(&minimal).unwrap(),
            json!({"gui":"drone"})
        );
    }

    /// A manifest written before the format grew its contract still reads: the
    /// three legs keep running today's bundles.
    #[test]
    fn a_pre_contract_manifest_still_parses() {
        let m: Manifest = serde_json::from_value(json!({
            "gui": "piano", "synthdefs": ["piano_voice"], "graphdefs": []
        }))
        .unwrap();
        assert_eq!(m.gui, "piano");
        assert_eq!(m.widgets, 0);
        assert!(m.symbols.is_empty());
        assert!(m.params.is_empty());
    }

    #[test]
    fn requirements_read_the_declaration() {
        let req = requirements(&manifest());
        assert_eq!(req.widgets, 4);
        assert_eq!(req.nodes, ["graph"]);
        assert_eq!(req.buses[0].rate, BusRate::Control);
        assert_eq!(req.buffers, ["hit"]);
    }

    /// A bundle written before the contract declares no count, and its saved
    /// ids are whatever the author picked. The block is sized to those, or two
    /// instances would overlap on the widget ids they were offset into.
    #[test]
    fn an_undeclared_widget_block_is_measured_from_the_template() {
        let old: Manifest = serde_json::from_value(json!({ "gui": "piano" })).unwrap();
        let sparse: Template = serde_json::from_value(json!({
            "id": 1,
            "gui": { "type": "window", "children": [
                { "id": 10, "type": "piano" },
                { "id": 20, "type": "meter", "bus": 0 }]}
        }))
        .unwrap();
        assert_eq!(requirements(&old).widgets, 0);
        assert_eq!(requirements_for(&old, Some(&sparse)).widgets, 20);

        // Two instances, each offset by its own 20-wide block: no id is shared.
        let ids = |base: i32| {
            let out = resolve(
                &old,
                &sparse,
                &Allocation {
                    widget_base: base,
                    ..Allocation::default()
                },
                &ParamInput::default(),
            )
            .unwrap();
            let children = out.tree["children"].as_array().unwrap().clone();
            let mut ids: Vec<i64> = vec![out.def_id as i64];
            ids.extend(children.iter().map(|c| c["id"].as_i64().unwrap()));
            ids
        };
        let (first, second) = (ids(1000), ids(1020));
        assert_eq!(first, [1000, 1009, 1019]);
        assert_eq!(second, [1020, 1029, 1039]);
        assert!(first.iter().all(|id| !second.contains(id)));
    }

    /// A declared count wins when it is larger; the span never shrinks a block.
    #[test]
    fn a_declared_widget_count_is_kept() {
        let req = requirements_for(&manifest(), Some(&template()));
        assert_eq!(req.widgets, 4);
    }

    /// The whole pass: ids offset through the nesting, symbols and parameters
    /// substituted in props and in boot, the boot list lifted out of the tree.
    #[test]
    fn resolve_offsets_ids_and_fills_both_kinds_of_hole() {
        let out = resolve(
            &manifest(),
            &template(),
            &allocation(),
            &ParamInput::default(),
        )
        .unwrap();
        assert_eq!(out.def_id, 100);
        let tree = out.tree.as_object().unwrap();
        assert_eq!(tree["title"], json!("FM voice"));
        assert!(!tree.contains_key("boot"), "the boot list left the tree");

        let children = tree["children"].as_array().unwrap();
        assert_eq!(children[0]["id"], json!(101));
        assert_eq!(children[0]["bus"], json!(17));
        assert_eq!(children[1]["id"], json!(102));
        let knob = &children[1]["children"][0];
        assert_eq!(knob["id"], json!(103));
        assert_eq!(knob["value"], json!(220.0));
        assert_eq!(knob["bind"], json!(["/n_set", 1500, "freq"]));

        assert_eq!(
            out.boot,
            vec![
                vec![
                    json!("/graph_new"),
                    json!("fm-voice.graph"),
                    json!(1500),
                    json!(0),
                    json!(0)
                ],
                vec![json!("/n_set"), json!(1500), json!("freq"), json!(220.0)],
            ]
        );
    }

    /// Two mounts of one template share nothing: different ids, different
    /// buses, one def payload.
    #[test]
    fn two_instances_do_not_collide() {
        let m = manifest();
        let t = template();
        let first = resolve(&m, &t, &allocation(), &ParamInput::default()).unwrap();
        let second = resolve(
            &m,
            &t,
            &Allocation {
                widget_base: 200,
                nodes: [("graph".to_string(), 1501)].into(),
                buses: [("lfo".to_string(), 18)].into(),
                buffers: [("hit".to_string(), 5)].into(),
            },
            &ParamInput::default(),
        )
        .unwrap();
        assert_ne!(first.def_id, second.def_id);
        assert_eq!(second.tree["children"][0]["bus"], json!(18));
        assert_eq!(second.boot[0][2], json!(1501));
    }

    /// Attribute over preset over default, all three typed on the way in.
    #[test]
    fn parameters_merge_attribute_over_preset_over_default() {
        let params = ParamInput {
            // As they arrive from HTML: every attribute is a string.
            attributes: json!({ "freq": "440" }).as_object().unwrap().clone(),
            preset: json!({ "freq": 660.0, "title": "bright" })
                .as_object()
                .unwrap()
                .clone(),
        };
        let out = resolve(&manifest(), &template(), &allocation(), &params).unwrap();
        assert_eq!(out.params["freq"], json!(440.0));
        assert_eq!(out.params["title"], json!("bright"));
        assert_eq!(out.tree["title"], json!("bright"));
    }

    /// A doubled sigil is the escape: prose keeps its `$` and `@`.
    #[test]
    fn a_doubled_sigil_escapes_it() {
        let mut m = manifest();
        m.widgets = 1;
        let t: Template = serde_json::from_value(json!({
            "id": 1, "gui": { "type": "window", "title": "$$5 and @@home" }
        }))
        .unwrap();
        let out = resolve(&m, &t, &allocation(), &ParamInput::default()).unwrap();
        assert_eq!(out.tree["title"], json!("$5 and @@home"));
    }

    #[test]
    fn an_unknown_symbol_is_an_error() {
        let mut m = manifest();
        m.widgets = 1;
        let t: Template =
            serde_json::from_value(json!({ "id": 1, "gui": { "type": "meter", "bus": "@nope" } }))
                .unwrap();
        assert_eq!(
            resolve(&m, &t, &allocation(), &ParamInput::default()),
            Err(Error::UnknownSymbol("nope".into()))
        );
    }

    #[test]
    fn an_unknown_parameter_is_an_error() {
        let mut m = manifest();
        m.widgets = 1;
        let t: Template = serde_json::from_value(
            json!({ "id": 1, "gui": { "type": "window", "title": "$nope" } }),
        )
        .unwrap();
        assert_eq!(
            resolve(&m, &t, &allocation(), &ParamInput::default()),
            Err(Error::UnknownParam("nope".into()))
        );
    }

    #[test]
    fn a_parameter_with_no_default_must_be_supplied() {
        let mut m = manifest();
        m.params.insert(
            "gain".into(),
            ParamSpec {
                kind: ParamType::Float,
                default: None,
                min: None,
                max: None,
            },
        );
        assert_eq!(
            resolve(&m, &template(), &allocation(), &ParamInput::default()),
            Err(Error::MissingParam("gain".into()))
        );
        let supplied =
            ParamInput::from_attributes(json!({ "gain": "0.5" }).as_object().unwrap().clone());
        let out = resolve(&m, &template(), &allocation(), &supplied).unwrap();
        assert_eq!(out.params["gain"], json!(0.5));
    }

    #[test]
    fn a_type_mismatch_is_an_error() {
        let params =
            ParamInput::from_attributes(json!({ "freq": "loud" }).as_object().unwrap().clone());
        assert_eq!(
            resolve(&manifest(), &template(), &allocation(), &params),
            Err(Error::TypeMismatch {
                name: "freq".into(),
                expected: ParamType::Float,
                got: "the string \"loud\"".into(),
            })
        );
    }

    #[test]
    fn a_value_out_of_range_is_an_error() {
        let params =
            ParamInput::from_attributes(json!({ "freq": 9000.0 }).as_object().unwrap().clone());
        assert_eq!(
            resolve(&manifest(), &template(), &allocation(), &params),
            Err(Error::OutOfRange {
                name: "freq".into(),
                value: 9000.0,
                min: Some(60.0),
                max: Some(700.0),
            })
        );
    }

    #[test]
    fn a_declared_but_unallocated_symbol_is_an_error() {
        let bare = Allocation {
            widget_base: 100,
            ..Allocation::default()
        };
        assert_eq!(
            resolve(&manifest(), &template(), &bare, &ParamInput::default()),
            Err(Error::UnallocatedSymbol("graph".into()))
        );
    }

    /// A block narrower than the numbering is the error worth catching: two
    /// instances offset by it would overlap. A wider one is merely generous.
    #[test]
    fn too_narrow_a_widget_block_is_an_error() {
        let mut m = manifest();
        m.widgets = 2;
        assert_eq!(
            resolve(&m, &template(), &allocation(), &ParamInput::default()),
            Err(Error::WidgetBlock {
                declared: 2,
                needed: 4
            })
        );
        m.widgets = 9;
        assert!(resolve(&m, &template(), &allocation(), &ParamInput::default()).is_ok());
    }

    /// The root's id is offset with the rest, so a child numbered like it would
    /// resolve onto it.
    #[test]
    fn a_widget_id_used_twice_is_an_error() {
        let mut m = manifest();
        m.widgets = 2;
        m.symbols = SymbolTable::default();
        m.params.clear();
        let t: Template = serde_json::from_value(json!({
            "id": 1,
            "gui": { "type": "window", "children": [
                { "id": 1, "type": "knob" },
                { "id": 2, "type": "meter", "bus": 0 }]}
        }))
        .unwrap();
        assert_eq!(
            resolve(&m, &t, &allocation(), &ParamInput::default()),
            Err(Error::DuplicateWidgetId(1))
        );
    }

    #[test]
    fn one_name_in_two_namespaces_is_ambiguous() {
        let mut m = manifest();
        m.symbols.nodes.push("lfo".into());
        assert_eq!(
            resolve(&m, &template(), &allocation(), &ParamInput::default()),
            Err(Error::DuplicateSymbol("lfo".into()))
        );
    }

    /// The invariant a writer enforces: no hole ever reaches a def payload, so
    /// two instances can share the one that was sent.
    #[test]
    fn a_placeholder_in_a_def_payload_is_refused() {
        let clean = json!({ "name": "voice", "ugens": [{ "rate": 2, "inputs": [0, 1] }] });
        assert_eq!(check_def_payload(&clean), Ok(()));
        let baked = json!({ "name": "voice", "ugens": [{ "inputs": ["@lfo"] }] });
        assert_eq!(
            check_def_payload(&baked),
            Err(Error::PlaceholderInDef("@lfo".into()))
        );
    }

    /// `validate` catches at write time what a mount would have hit.
    #[test]
    fn validate_dry_runs_the_mount() {
        assert_eq!(validate(&manifest(), &template()), Ok(()));

        let mut wrong = manifest();
        wrong.symbols.nodes.clear();
        assert_eq!(
            validate(&wrong, &template()),
            Err(Error::UnknownSymbol("graph".into()))
        );

        let mut untyped = manifest();
        untyped.params.get_mut("freq").unwrap().default = Some(json!("loud"));
        assert!(matches!(
            validate(&untyped, &template()),
            Err(Error::TypeMismatch { .. })
        ));
    }

    /// A bundle that declares nothing resolves as itself, holes or not — the
    /// pre-contract bundles keep mounting.
    #[test]
    fn a_bundle_with_no_contract_resolves_unchanged() {
        let m: Manifest = serde_json::from_value(json!({ "gui": "drone" })).unwrap();
        let t: Template = serde_json::from_value(json!({
            "id": 1,
            "gui": { "type": "window", "children": [{ "id": 2, "type": "meter", "bus": 0 }] }
        }))
        .unwrap();
        let out = resolve(&m, &t, &allocation(), &ParamInput::default()).unwrap();
        assert_eq!(out.def_id, 100);
        assert_eq!(out.tree["children"][0]["id"], json!(101));
        assert_eq!(out.tree["children"][0]["bus"], json!(0));
    }
}
