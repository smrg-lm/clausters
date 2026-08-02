//! The substitution machinery: reading a placeholder, and filling one.
//!
//! Private to the module. The passes in [`super`] decide *what* is resolved and
//! against which allocation; this is the walk that rewrites a template's values,
//! its node ids and its boot list, plus the checks that refuse a template or a
//! parameter before any of it runs.

use super::*;

/// What a template string is, once its first characters are read.
pub(super) enum Hole<'a> {
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
pub(super) fn placeholder(s: &str) -> Option<Hole<'_>> {
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

pub(super) struct Ctx<'a> {
    pub(super) symbols: &'a SymbolTable,
    pub(super) allocation: &'a Allocation,
    pub(super) params: &'a BTreeMap<String, Value>,
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
pub(super) fn resolve_node(
    node: &Map<String, Value>,
    offset: i32,
    ctx: &Ctx,
) -> Result<Value, Error> {
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
pub(super) fn resolve_boot(list: &[Value], ctx: &Ctx) -> Result<Vec<Vec<Value>>, Error> {
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
pub(super) fn check_widget_ids(template: &Template) -> Result<(), Error> {
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
pub(super) fn check_symbol_namespaces(symbols: &SymbolTable) -> Result<(), Error> {
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
pub(super) fn merge_params(
    manifest: &Manifest,
    input: &ParamInput,
) -> Result<BTreeMap<String, Value>, Error> {
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
