//! **The UGen builders both clients ship, generated from the registry.**
//!
//! The server's registry (`dsp::registry`) is the catalog: each kind's name,
//! its inputs in wire order with their defaults, and the rate it runs at when a
//! def does not choose one. A builder that only mirrors that row -- its
//! parameters are the wire's inputs, in order, and it returns one node of the
//! kind -- is not written by hand in either client. It is generated, here,
//! into a private module of each client, which the client's family module
//! re-exports:
//!
//! - `clients/python/clausters/defs/ugens/_generated/<module>.py`
//! - `clients/web/src/defs/ugens/_generated/<module>.ts`
//!
//! The generated files are committed, so a diff shows what a change to the
//! catalog does to the clients, and this test fails when one of them is not
//! what the registry and the descriptions say it should be. After changing
//! either, regenerate them:
//!
//! ```text
//! CLAUSTERS_BLESS=1 cargo test -p clausters --test ugen_builders
//! ```
//!
//! **What a builder says that the registry does not** is in
//! `src/dsp/registry/builders/<module>.toml`, one file per family module of
//! the clients, in TOML:
//!
//! - `[[ugen]]`, a generated builder: `kind` (the registry's name), `builder`
//!   (the Python name, snake case), `ts` (the TypeScript name, only where it is
//!   not the camel case of `builder`), `required` (the leading inputs a caller
//!   must give, which have no default in the client), `rate` (the rate the
//!   builder gives its node, where the client chooses one; a node without one
//!   runs at the kind's default) and `doc`, the builder's documentation in
//!   Markdown. The documentation is written once, for both
//!   clients, in Python's spelling: an inline code span is read as code, so in
//!   TypeScript a builder's or parameter's snake-case name becomes its camel
//!   case and `name=value` becomes `name: value`. A fenced block tagged
//!   `python` or `ts` reaches that client alone.
//! - `[[hand]]`, a kind whose builder each client writes by hand, because it is
//!   more than the wire's row (a value list that leads, a static field, a
//!   channel per output), and `why`.
//! - `[[operator]]`, a kind no builder makes, because an operator on a graph
//!   node builds it, and `why`.
//!
//! Every kind of the registry is in exactly one of those lists, so a kind the
//! server grows fails here until it has a place in both clients. The clients'
//! catalog contrasts, which still check the hand-written signatures, take the
//! lists from here too: the Python one reads the TOML files, and the web one,
//! which has no TOML reader, `clients/web/tests/ugen-builders.json`, written
//! and checked with the generated builders.

#![cfg(feature = "synth")]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use clausters::dsp::Rate;
use clausters::dsp::registry::{self, Arity, UGenDescriptor};
use serde::Deserialize;

/// One family module's descriptions: `builders/<module>.toml`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Module {
    #[serde(default)]
    ugen: Vec<Builder>,
    #[serde(default)]
    hand: Vec<Excused>,
    #[serde(default)]
    operator: Vec<Excused>,
}

/// A generated builder.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Builder {
    kind: String,
    builder: String,
    ts: Option<String>,
    #[serde(default)]
    required: Vec<String>,
    rate: Option<String>,
    doc: String,
}

/// A kind with no generated builder, and why.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Excused {
    kind: String,
    why: String,
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn builders_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/dsp/registry/builders")
}

/// Every `builders/*.toml`, by module name.
fn modules() -> BTreeMap<String, Module> {
    let mut out = BTreeMap::new();
    for entry in std::fs::read_dir(builders_dir()).expect("the builders' descriptions") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let name = path.file_stem().unwrap().to_str().unwrap().to_string();
        let text = std::fs::read_to_string(&path).unwrap();
        let module: Module =
            toml::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        out.insert(name, module);
    }
    out
}

/// `lf_noise0` -> `lfNoise0`: the TypeScript spelling of a snake-case name.
fn camel(name: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for (i, c) in name.chars().enumerate() {
        if c == '_' && i > 0 && i + 1 < name.len() {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

fn ts_name(b: &Builder) -> String {
    b.ts.clone().unwrap_or_else(|| camel(&b.builder))
}

/// A registry default as a literal both languages read back to the same f32:
/// Rust's shortest round-trip spelling, with a `.0` where it has no point.
fn literal(value: f32) -> String {
    assert!(value.is_finite(), "a default must be finite");
    format!("{value:?}")
}

/// A rate's wire spelling.
fn rate_name(rate: Rate) -> &'static str {
    match rate {
        Rate::Ir => "ir",
        Rate::Kr => "kr",
        Rate::Ar => "ar",
        Rate::Dr => "dr",
    }
}

/// The documentation for one client: the fenced blocks tagged for the other
/// language dropped, and blank lines around what is left kept single.
fn doc_for(doc: &str, drop: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut skipping = false;
    for line in doc.trim().lines() {
        let fence = line.trim_start();
        if skipping {
            if fence.starts_with("```") {
                skipping = false;
            }
            continue;
        }
        if fence.starts_with("```") && fence[3..].trim() == drop {
            skipping = true;
            continue;
        }
        lines.push(line.trim_end().to_string());
    }
    let mut out: Vec<String> = Vec::new();
    for line in lines {
        if line.is_empty() && out.last().is_none_or(|l| l.is_empty()) {
            continue;
        }
        out.push(line);
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out
}

/// An inline code span of the shared documentation, spelled for TypeScript:
/// `name=value` pairs become `name: value`, and every snake-case name outside
/// a string literal becomes its camel case -- through `names` where the
/// TypeScript name is not that.
fn ts_span(span: &str, names: &BTreeMap<String, String>) -> String {
    let kwargs = span.split(", ").all(|part| {
        part.split_once('=').is_some_and(|(k, v)| {
            !k.is_empty()
                && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !v.starts_with('=')
        })
    });
    let span = if kwargs && !span.contains("==") {
        span.split(", ")
            .map(|part| {
                let (k, v) = part.split_once('=').unwrap();
                format!("{k}: {v}")
            })
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        span.to_string()
    };
    let mut out = String::new();
    let mut word = String::new();
    let mut quoted = false;
    let flush = |word: &mut String, out: &mut String| {
        if !word.is_empty() {
            let renamed = names.get(word.as_str()).cloned().unwrap_or_else(|| {
                if word.starts_with(|c: char| c.is_ascii_lowercase()) && word.contains('_') {
                    camel(word)
                } else {
                    word.clone()
                }
            });
            out.push_str(&renamed);
            word.clear();
        }
    };
    for c in span.chars() {
        if c == '"' {
            flush(&mut word, &mut out);
            quoted = !quoted;
            out.push(c);
        } else if !quoted && (c.is_ascii_alphanumeric() || c == '_') {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// A line of prose with each inline code span spelled for TypeScript; the
/// lines inside a fenced block are TypeScript already.
fn ts_doc(lines: Vec<String>, names: &BTreeMap<String, String>) -> Vec<String> {
    let mut fenced = false;
    lines
        .into_iter()
        .map(|line| {
            if line.trim_start().starts_with("```") {
                fenced = !fenced;
                return line;
            }
            if fenced {
                return line;
            }
            let mut out = String::new();
            let mut parts = line.split('`');
            out.push_str(parts.next().unwrap_or(""));
            for (i, part) in parts.enumerate() {
                out.push('`');
                if i % 2 == 0 {
                    out.push_str(&ts_span(part, names));
                } else {
                    out.push_str(part);
                }
            }
            out
        })
        .collect()
}

const PY_WIDTH: usize = 79;
const TS_WIDTH: usize = 90;

fn regenerate(command: &str) -> String {
    format!(
        "Generated from the server's UGen registry and\n\
         `crates/clausters-server/src/dsp/registry/builders/{command}.toml` by\n\
         `crates/clausters-server/tests/ugen_builders.rs`. Do not edit: change the\n\
         description and regenerate with\n\
         `CLAUSTERS_BLESS=1 cargo test -p clausters --test ugen_builders`."
    )
}

fn python(module: &str, builders: &[(&Builder, &UGenDescriptor)]) -> String {
    let mut s = String::new();
    writeln!(
        s,
        "\"\"\"The `{module}` builders that mirror the wire, re-exported by `{module}`.\n"
    )
    .unwrap();
    writeln!(s, "{}\n\"\"\"\n", regenerate(module)).unwrap();
    writeln!(s, "from ..graph import Ugen\n").unwrap();
    writeln!(s, "__all__ = [").unwrap();
    for (b, _) in builders {
        writeln!(s, "    \"{}\",", b.builder).unwrap();
    }
    writeln!(s, "]").unwrap();
    for (b, d) in builders {
        let names: Vec<&str> = d.inputs.iter().map(|i| i.name).collect();
        let params: Vec<String> = d
            .inputs
            .iter()
            .map(|i| {
                if b.required.iter().any(|r| r == i.name) {
                    i.name.to_string()
                } else {
                    format!("{}={}", i.name, literal(i.default))
                }
            })
            .collect();
        let head = format!("def {}({}) -> Ugen:", b.builder, params.join(", "));
        s.push_str("\n\n");
        if head.len() <= PY_WIDTH {
            writeln!(s, "{head}").unwrap();
        } else {
            writeln!(s, "def {}(", b.builder).unwrap();
            for p in &params {
                writeln!(s, "    {p},").unwrap();
            }
            writeln!(s, ") -> Ugen:").unwrap();
        }
        let doc = doc_for(&b.doc, "ts");
        if doc.len() == 1 && doc[0].len() + 10 <= PY_WIDTH {
            writeln!(s, "    \"\"\"{}\"\"\"", doc[0]).unwrap();
        } else {
            writeln!(s, "    \"\"\"{}", doc[0]).unwrap();
            for line in &doc[1..] {
                if line.is_empty() {
                    s.push('\n');
                } else {
                    writeln!(s, "    {line}").unwrap();
                }
            }
            writeln!(s, "    \"\"\"").unwrap();
        }
        let rate = b
            .rate
            .as_ref()
            .map(|r| format!(", rate=\"{r}\""))
            .unwrap_or_default();
        let body = format!(
            "    return Ugen(\"{}\", [{}]{rate})",
            d.name,
            names.join(", ")
        );
        if body.len() <= PY_WIDTH {
            writeln!(s, "{body}").unwrap();
        } else {
            writeln!(s, "    return Ugen(").unwrap();
            writeln!(s, "        \"{}\",", d.name).unwrap();
            writeln!(s, "        [{}],", names.join(", ")).unwrap();
            if let Some(r) = &b.rate {
                writeln!(s, "        rate=\"{r}\",").unwrap();
            }
            writeln!(s, "    )").unwrap();
        }
    }
    s
}

fn typescript(
    module: &str,
    builders: &[(&Builder, &UGenDescriptor)],
    names: &BTreeMap<String, String>,
) -> String {
    let mut s = String::new();
    writeln!(
        s,
        "// The `{module}` builders that mirror the wire, re-exported by `{module}.ts`."
    )
    .unwrap();
    writeln!(s, "//").unwrap();
    for line in regenerate(module).lines() {
        writeln!(s, "// {line}").unwrap();
    }
    writeln!(s).unwrap();
    writeln!(s, "import {{ Ugen }} from \"../graph.ts\";").unwrap();
    writeln!(s, "import type {{ Channel }} from \"../graph.ts\";").unwrap();
    for (b, d) in builders {
        let ts = ts_name(b);
        s.push('\n');
        let doc = ts_doc(doc_for(&b.doc, "python"), names);
        if doc.len() == 1 && doc[0].len() + 7 <= TS_WIDTH {
            writeln!(s, "/** {} */", doc[0]).unwrap();
        } else {
            writeln!(s, "/**").unwrap();
            for line in &doc {
                if line.is_empty() {
                    writeln!(s, " *").unwrap();
                } else {
                    writeln!(s, " * {line}").unwrap();
                }
            }
            writeln!(s, " */").unwrap();
        }
        let params: Vec<String> = d
            .inputs
            .iter()
            .map(|i| {
                if b.required.iter().any(|r| r == i.name) {
                    format!("{}: Channel", camel(i.name))
                } else {
                    format!("{}: Channel = {}", camel(i.name), literal(i.default))
                }
            })
            .collect();
        let args: Vec<String> = d.inputs.iter().map(|i| camel(i.name)).collect();
        let rate = b
            .rate
            .as_ref()
            .map(|r| format!(", {{ rate: \"{r}\" }}"))
            .unwrap_or_default();
        let body = format!("new Ugen(\"{}\", [{}]{rate});", d.name, args.join(", "));
        let head = format!("export const {ts} = ({}): Ugen =>", params.join(", "));
        if head.len() + 1 + body.len() <= TS_WIDTH {
            writeln!(s, "{head} {body}").unwrap();
        } else if head.len() <= TS_WIDTH && body.len() + 4 <= TS_WIDTH {
            writeln!(s, "{head}\n    {body}").unwrap();
        } else {
            writeln!(s, "export const {ts} = (").unwrap();
            for p in &params {
                writeln!(s, "    {p},").unwrap();
            }
            writeln!(s, "): Ugen => {body}").unwrap();
        }
    }
    s
}

/// Every file this test writes, by path, with what it should hold.
fn generated() -> BTreeMap<PathBuf, String> {
    let modules = modules();
    let registry: BTreeMap<&str, &UGenDescriptor> = registry::all().map(|d| (d.name, d)).collect();

    // Every kind in exactly one place, and every place naming a kind.
    let mut placed: BTreeMap<&str, String> = BTreeMap::new();
    for (module, m) in &modules {
        let kinds = m
            .ugen
            .iter()
            .map(|b| (b.kind.as_str(), "ugen"))
            .chain(m.hand.iter().map(|e| (e.kind.as_str(), "hand")))
            .chain(m.operator.iter().map(|e| (e.kind.as_str(), "operator")));
        for (kind, list) in kinds {
            assert!(
                registry.contains_key(kind),
                "builders/{module}.toml: `{kind}` ({list}) is not a kind of the registry"
            );
            if let Some(before) = placed.insert(kind, format!("{module}.toml ({list})")) {
                panic!("`{kind}` is placed twice: in {before} and in {module}.toml ({list})");
            }
        }
        for e in m.hand.iter().chain(&m.operator) {
            assert!(
                !e.why.trim().is_empty(),
                "`{}` says nothing in `why`",
                e.kind
            );
        }
    }
    let missing: Vec<&str> = registry
        .keys()
        .filter(|k| !placed.contains_key(*k))
        .copied()
        .collect();
    assert!(
        missing.is_empty(),
        "kinds of the registry no builder description places: {missing:?} -- a kind the \
         server grew needs a builder in both clients (a `[[ugen]]` of builders/<module>.toml), \
         or a `[[hand]]` or `[[operator]]` entry saying how it is built"
    );

    // The TypeScript name of every generated builder, for the documentation's
    // code spans.
    let mut ts_names = BTreeMap::new();
    let mut python_names = BTreeSet::new();
    for m in modules.values() {
        for b in &m.ugen {
            assert!(
                python_names.insert(b.builder.clone()),
                "two builders are named `{}`",
                b.builder
            );
            ts_names.insert(b.builder.clone(), ts_name(b));
        }
    }

    let mut out = BTreeMap::new();
    let py_dir = repo().join("clients/python/clausters/defs/ugens/_generated");
    let ts_dir = repo().join("clients/web/src/defs/ugens/_generated");
    out.insert(
        py_dir.join("__init__.py"),
        format!(
            "\"\"\"The UGen builders that mirror the wire, one module per family, each\n\
             re-exported by the family module of the same name.\n\n{}\n\"\"\"\n",
            regenerate("<module>")
        ),
    );
    for (module, m) in &modules {
        if m.ugen.is_empty() {
            continue;
        }
        let rows: Vec<(&Builder, &UGenDescriptor)> = m
            .ugen
            .iter()
            .map(|b| {
                let d = registry[b.kind.as_str()];
                assert!(
                    d.arity == Arity::Fixed(d.inputs.len()),
                    "`{}`: a generated builder takes the wire's inputs, and the kind's arity \
                     is not its inputs' count",
                    b.kind
                );
                let names: Vec<&str> = d.inputs.iter().map(|i| i.name).collect();
                assert!(
                    names.len() >= b.required.len() && names[..b.required.len()] == b.required[..],
                    "`{}`: `required` must name the leading inputs, which are {names:?}",
                    b.kind
                );
                assert!(
                    !b.doc.trim().is_empty(),
                    "`{}` has no documentation",
                    b.kind
                );
                if let Some(r) = &b.rate {
                    assert!(
                        d.rates.iter().any(|&allowed| rate_name(allowed) == r),
                        "`{}`: rate `{r}` is not one the kind runs at",
                        b.kind
                    );
                }
                (b, d)
            })
            .collect();
        out.insert(py_dir.join(format!("{module}.py")), python(module, &rows));
        out.insert(
            ts_dir.join(format!("{module}.ts")),
            typescript(module, &rows, &ts_names),
        );
    }
    out.insert(
        repo().join("clients/web/tests/ugen-builders.json"),
        lists(&modules),
    );
    out
}

/// The three lists, for the web client's catalog contrast, which has no TOML
/// reader: every generated kind, and every hand-written and operator-built
/// one with its reason. The Python contrast reads the descriptions directly.
fn lists(modules: &BTreeMap<String, Module>) -> String {
    let mut generated = BTreeSet::new();
    let mut hand = BTreeMap::new();
    let mut operator = BTreeMap::new();
    for m in modules.values() {
        generated.extend(m.ugen.iter().map(|b| b.kind.clone()));
        hand.extend(m.hand.iter().map(|e| (e.kind.clone(), e.why.clone())));
        operator.extend(m.operator.iter().map(|e| (e.kind.clone(), e.why.clone())));
    }
    let json = serde_json::json!({
        "_comment": format!(
            "Which UGen kinds have a generated builder, a hand-written one or none. {}",
            regenerate("<module>").replace('\n', " ")
        ),
        "generated": generated,
        "hand": hand,
        "operator": operator,
    });
    serde_json::to_string_pretty(&json).unwrap() + "\n"
}

#[test]
fn the_generated_builders_are_the_registrys() {
    let files = generated();
    let bless = std::env::var_os("CLAUSTERS_BLESS").is_some();
    let mut stale = Vec::new();
    for (path, want) in &files {
        let have = std::fs::read_to_string(path).unwrap_or_default();
        if have == *want {
            continue;
        }
        if bless {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, want).unwrap();
        } else {
            stale.push(
                path.strip_prefix(repo())
                    .unwrap_or(path)
                    .display()
                    .to_string(),
            );
        }
    }
    // A generated file no description writes any more is stale too.
    for dir in [
        "clients/python/clausters/defs/ugens/_generated",
        "clients/web/src/defs/ugens/_generated",
    ] {
        let Ok(entries) = std::fs::read_dir(repo().join(dir)) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_file() && !files.contains_key(&path) {
                if bless {
                    std::fs::remove_file(&path).unwrap();
                } else {
                    stale.push(
                        path.strip_prefix(repo())
                            .unwrap_or(&path)
                            .display()
                            .to_string(),
                    );
                }
            }
        }
    }
    assert!(
        stale.is_empty(),
        "generated builders out of date: {stale:?}\n\
         regenerate: CLAUSTERS_BLESS=1 cargo test -p clausters --test ugen_builders"
    );
}

#[test]
fn a_code_span_is_spelled_for_typescript() {
    let names = BTreeMap::from([("oscn".to_string(), "oscN".to_string())]);
    assert_eq!(
        ts_span("buf_rate_scale(buf) * pitch", &names),
        "bufRateScale(buf) * pitch"
    );
    assert_eq!(ts_span("oscn", &names), "oscN");
    assert_eq!(ts_span("rate=\"control\"", &names), "rate: \"control\"");
    assert_eq!(
        ts_span("stretch=1, shift=0", &names),
        "stretch: 1, shift: 0"
    );
    assert_eq!(ts_span("\"max_delay\"", &names), "\"max_delay\"");
    assert_eq!(ts_span("signal = -1", &names), "signal = -1");
    assert_eq!(ts_span("freeze <= 0", &names), "freeze <= 0");
    assert_eq!(
        ts_span("Server.transport_loop", &names),
        "Server.transportLoop"
    );
    assert_eq!(ts_span("in_", &names), "in_");
}
