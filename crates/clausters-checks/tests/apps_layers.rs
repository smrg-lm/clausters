//! A module of the applications' crate refers only to modules of its own
//! layer or a lower one.
//!
//! `clausters-apps` is one crate, so cargo checks nothing between its
//! modules; the layers are written once, in `docs/architecture.md` ("The
//! applications' modules and their layers"), and this reads them from there.
//! A module is one the crate's root declares, with everything under it. The
//! source is read as `host_layers` reads the host's
//! (`clausters_checks::modules`), with one difference: **a test is not
//! read**, neither a `tests.rs` nor an inline `mod tests`, since a test that
//! drives an editor through the editing context is how an editor is tested.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use clausters_checks::modules::{declared, files, module_of, read, references};

/// The references that break the rule today, as `(module, module it refers
/// to)`.
///
/// **This list only shrinks.** An entry no reference matches any more fails
/// the test, so an exception cannot outlive the reference it excuses; a new
/// one is not added, the code moves to the layer that joins the two instead.
const EXCEPTIONS: &[(&str, &str)] = &[
    // A roll writes a note's marks with the symbols the score's palettes
    // offer (`score::palettes::symbol`), so the page and the roll write one.
    ("notes", "score"),
];

fn src() -> PathBuf {
    clausters_checks::root().join("crates/clausters-apps/src")
}

/// Each module's layer, from the table under the architecture page's heading.
fn layers() -> BTreeMap<String, u32> {
    let text = read(&clausters_checks::root().join("docs/architecture.md"));
    let section = text
        .split("\n### The applications' modules and their layers\n")
        .nth(1)
        .expect("docs/architecture.md has no \"The applications' modules and their layers\"");
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

#[test]
fn an_application_module_refers_only_to_its_layer_or_below() {
    let layers = layers();
    let src = src();
    let modules = declared(&src.join("lib.rs"));
    let is_test = |path: &[String]| path.iter().any(|seg| seg == "tests");
    let mut wrong = Vec::new();
    let mut used = BTreeSet::new();

    for name in &modules {
        if !layers.contains_key(name) {
            wrong.push(format!("{name} has no row in the layers table"));
        }
    }
    for name in layers.keys() {
        if !modules.contains(name) {
            wrong.push(format!(
                "the layers table names {name}, which has no source"
            ));
        }
    }

    for file in files(&src) {
        let home = module_of(&src, &file);
        let Some(from) = home.first().filter(|m| modules.contains(*m)) else {
            continue;
        };
        if is_test(&home) {
            continue;
        }
        let Some(&layer) = layers.get(from) else {
            continue;
        };
        let text = read(&file);
        for (path, line) in references(&src, &file) {
            let Some(to) = path.first().filter(|m| modules.contains(*m)) else {
                continue;
            };
            let Some(&above) = layers.get(to) else {
                continue;
            };
            if to == from || above <= layer {
                continue;
            }
            // Inside an inline `mod tests`: the lines from its opening on.
            let before: String = text.lines().take(line).collect::<Vec<_>>().join("\n");
            if before.contains("\nmod tests {") {
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
