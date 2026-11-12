//! A module of the GUI host refers only to modules of its own layer or a
//! lower one.
//!
//! The host is one crate, so cargo checks nothing between its modules; the
//! layers are written once, in `docs/architecture.md` ("The host's modules and
//! their layers"), and this reads them from there. A reference is a path the
//! source spells -- `crate::host::paint::Mesh`, `super::font`, `use
//! super::{layout, widget}` -- resolved from the module it is written in,
//! inline modules (a file's `mod tests`) included, so a test that reaches up
//! counts as the code beside it does. Comments, strings and character literals
//! are not read.
//!
//! It reads the source as text rather than asking the compiler, so it holds
//! under every feature configuration and needs nothing built.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use clausters_checks::modules::{clean, declared, files, module_of, read, references};

/// The references that break the rule today, as `(module, module it refers
/// to)`.
///
/// **This list only shrinks.** An entry no reference matches any more fails
/// the test, so an exception cannot outlive the reference it excuses; a new
/// one is not added, the code moves to the layer that joins the two instead.
const EXCEPTIONS: &[(&str, &str)] = &[
    ("gpu", "host::diag"),
    ("host::actions", "host"),
    ("host::activity", "host"),
    ("host::activity", "host::frame"),
    ("host::bind", "host"),
    ("host::bind", "host::wire"),
    ("host::bundle", "host"),
    ("host::columns", "host::widget"),
    ("host::diag", "host"),
    ("host::diag", "host::status"),
    ("host::disk", "host"),
    ("host::disk", "host::takes"),
    ("host::elements", "host"),
    ("host::files", "host"),
    ("host::frame", "host"),
    ("host::gestures", "host"),
    ("host::gestures", "host::native"),
    ("host::gestures", "host::wire"),
    ("host::graphics", "host::diag"),
    ("host::graphics", "host::elements"),
    ("host::graphics", "host::frame"),
    ("host::graphics", "host::guidef"),
    ("host::graphics", "host::live"),
    ("host::graphics", "host::widget"),
    ("host::ids", "host"),
    ("host::instance", "host"),
    ("host::interact", "host"),
    ("host::layers", "host::widget"),
    ("host::layout", "host"),
    ("host::layout", "host::frame"),
    ("host::live", "host"),
    ("host::midi", "host"),
    ("host::node_trees", "host"),
    ("host::panel", "host"),
    ("host::play", "host"),
    ("host::popup", "host"),
    ("host::readback", "host"),
    ("host::replies", "host"),
    ("host::replies", "host::elements"),
    ("host::replies", "host::frame"),
    ("host::replies", "host::takes"),
    ("host::ruler", "host::widget"),
    ("host::status", "host"),
    ("host::status", "host::wire"),
    ("host::timeline", "host"),
    ("host::timeline", "host::frame"),
    ("host::timeline", "host::guidef"),
    ("host::timeline", "host::layout"),
    ("host::timeline", "host::live"),
    ("host::timeline", "host::widget"),
    ("host::voices", "host"),
    ("host::widget", "host"),
    ("host::widget", "host::elements"),
    ("host::widget", "host::menu"),
    ("host::world", "host"),
];

fn src() -> PathBuf {
    clausters_checks::root().join("crates/clausters-gui/src")
}

/// Each module's layer, from the table under the architecture page's heading.
fn layers() -> BTreeMap<String, u32> {
    let text = read(&clausters_checks::root().join("docs/architecture.md"));
    let section = text
        .split("\n### The host's modules and their layers\n")
        .nth(1)
        .expect("docs/architecture.md has no \"The host's modules and their layers\" section");
    let section = section.split("\n#").next().unwrap_or(section);
    let mut out = BTreeMap::new();
    for line in section.lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        let Some(layer) = cells.get(1).and_then(|c| c.parse::<u32>().ok()) else {
            continue;
        };
        for name in cells[3].split(',') {
            let name = name.trim().trim_matches('`');
            assert!(
                out.insert(name.to_string(), layer).is_none(),
                "the layers table names {name} twice"
            );
        }
    }
    assert!(!out.is_empty(), "the layers table has no rows");
    out
}

/// The modules of the crate, as the layers name them: `host::<module>` for
/// the host's, the bare name for the crate root's, and `host` for the host's
/// own root.
struct Names {
    host: BTreeSet<String>,
    root: BTreeSet<String>,
    /// What the crate root re-exports from the host (`crate::element` is
    /// `host::widget`'s), by the name it is re-exported under.
    aliases: BTreeMap<String, String>,
}

impl Names {
    fn read() -> Names {
        let mut aliases = BTreeMap::new();
        let lib = clean(&read(&src().join("lib.rs")));
        for line in lib.split(';') {
            let line = line.trim();
            let Some(path) = line.strip_prefix("pub use host::") else {
                continue;
            };
            let module = path.split("::").next().unwrap_or("");
            let last = path.trim_end_matches('}');
            let items = match last.split_once('{') {
                Some((head, group)) => {
                    let parent = head
                        .trim_end_matches("::")
                        .rsplit("::")
                        .next()
                        .unwrap_or("");
                    group
                        .split(',')
                        .map(|item| {
                            let item = item.trim();
                            if item == "self" { parent } else { item }.to_string()
                        })
                        .collect()
                }
                None => vec![last.rsplit("::").next().unwrap_or("").to_string()],
            };
            for item in items {
                aliases.insert(item, format!("host::{module}"));
            }
        }
        Names {
            host: declared(&src().join("host/mod.rs")),
            root: declared(&src().join("lib.rs")),
            aliases,
        }
    }

    /// The module a path from the crate root lands in, `None` for the crate
    /// root itself and for what is not ours.
    fn of(&self, path: &[String]) -> Option<String> {
        let first = path.first()?;
        if first == "host" {
            return Some(match path.get(1) {
                Some(module) if self.host.contains(module) => format!("host::{module}"),
                _ => "host".to_string(),
            });
        }
        if self.root.contains(first) {
            return Some(first.clone());
        }
        self.aliases.get(first).cloned()
    }
}

#[test]
fn a_host_module_refers_only_to_its_layer_or_below() {
    let layers = layers();
    let names = Names::read();
    let mut wrong = Vec::new();
    let mut used = BTreeSet::new();
    let mut seen = BTreeSet::new();

    let src = src();
    for file in files(&src) {
        let Some(from) = names.of(&module_of(&src, &file)) else {
            continue;
        };
        let Some(&layer) = layers.get(&from) else {
            wrong.push(format!(
                "{from} ({}) has no row in the layers table",
                file.display()
            ));
            continue;
        };
        seen.insert(from.clone());
        for (path, line) in references(&src, &file) {
            let Some(to) = names.of(&path) else {
                continue;
            };
            if to == from {
                continue;
            }
            let Some(&above) = layers.get(&to) else {
                wrong.push(format!("{from} refers to {to}, which has no row"));
                continue;
            };
            if above <= layer {
                continue;
            }
            if EXCEPTIONS.contains(&(from.as_str(), to.as_str())) {
                used.insert((from.clone(), to.clone()));
                continue;
            }
            let rel = file.strip_prefix(&src).unwrap().display().to_string();
            wrong.push(format!(
                "{from} (layer {layer}) refers to {to} (layer {above}): {rel}:{line}"
            ));
        }
    }

    for name in layers.keys() {
        if !seen.contains(name) {
            wrong.push(format!(
                "the layers table names {name}, which has no source"
            ));
        }
    }
    for &(from, to) in EXCEPTIONS {
        if !used.contains(&(from.to_string(), to.to_string())) {
            wrong.push(format!(
                "the exception {from} -> {to} matches no reference any more: remove it"
            ));
        }
    }
    wrong.sort();
    wrong.dedup();
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}
