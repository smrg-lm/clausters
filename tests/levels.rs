//! A crate depends only on crates of a lower level.
//!
//! The levels are written once, in `docs/architecture.md` ("The crates and
//! their levels"), and this reads them from there. Cargo refuses a cycle among
//! ordinary dependencies and nothing else: a dev-dependency may point at any
//! crate, which is how a crate's tests come to exercise the layer above it and
//! pull that layer into its own build. So the rule is checked here, over every
//! kind of dependency, and a crate the table does not name fails as well.
//!
//! It reads manifests as text rather than asking cargo, so it holds under every
//! feature configuration and needs nothing resolved or built.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The edges that break the rule today, as `(crate, dependency, kind)`.
///
/// **This list only shrinks.** An entry no manifest matches any more fails the
/// test, so an exception cannot outlive the edge it excuses; a new one is not
/// added, the code moves to the crate that joins the two layers instead.
const EXCEPTIONS: &[(&str, &str, &str)] = &[
    // The server's integration tests of the applications' playback. They
    // belong to `clausters-app-engine`, the crate that joins the two.
    ("clausters", "clausters-apps", "dev-dependencies"),
    ("clausters", "clausters-editing", "dev-dependencies"),
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// Each crate's level, from the table under the architecture page's heading.
fn levels() -> BTreeMap<String, u32> {
    let text = read(&root().join("docs/architecture.md"));
    let section = text
        .split("\n## The crates and their levels\n")
        .nth(1)
        .expect("docs/architecture.md has no \"The crates and their levels\" section");
    let section = section.split("\n## ").next().unwrap_or(section);
    let mut out = BTreeMap::new();
    for line in section.lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        // `| 0 | `a`, `b` | ... |` splits into an empty cell, the level, the crates.
        let Some(level) = cells.get(1).and_then(|c| c.parse::<u32>().ok()) else {
            continue;
        };
        for name in cells[2].split(',') {
            let name = name.trim().trim_matches('`');
            assert!(
                out.insert(name.to_string(), level).is_none(),
                "the levels table names {name} twice"
            );
        }
    }
    assert!(!out.is_empty(), "the levels table has no rows");
    out
}

/// Every manifest the rule covers: the root package and the crates under
/// `crates/`.
fn manifests() -> Vec<PathBuf> {
    let mut out = vec![root().join("Cargo.toml")];
    for entry in std::fs::read_dir(root().join("crates")).expect("crates/") {
        let manifest = entry.expect("a directory entry").path().join("Cargo.toml");
        if manifest.is_file() {
            out.push(manifest);
        }
    }
    out.sort();
    out
}

/// A manifest's package name and its dependencies on crates of the project, as
/// `(dependency, kind)` where the kind is the section's own last word
/// (`dependencies`, `dev-dependencies`, `build-dependencies`).
fn package(manifest: &Path) -> (String, BTreeSet<(String, String)>) {
    let text = read(manifest);
    let mut name = None;
    let mut section = String::new();
    let mut edges = BTreeSet::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = header.to_string();
            // `[dependencies.clausters-core]`, the table form of one entry.
            if let Some((kind, dependency)) = table_entry(header) {
                edges.insert((dependency, kind));
            }
            continue;
        }
        if section == "package" && name.is_none() {
            name = line
                .strip_prefix("name = \"")
                .and_then(|l| l.strip_suffix('"'))
                .map(str::to_string);
        }
        let Some(kind) = dependency_kind(&section) else {
            continue;
        };
        let key = line.split(['=', ' ', '.']).next().unwrap_or("");
        if is_ours(key) && line[key.len()..].trim_start().starts_with(['=', '.']) {
            edges.insert((key.to_string(), kind.to_string()));
        }
    }
    let name = name.unwrap_or_else(|| panic!("{}: no package name", manifest.display()));
    (name, edges)
}

/// The dependency kind a section header declares, `None` for any other section.
/// A target's own (`target.'cfg(unix)'.dependencies`) counts as its kind.
fn dependency_kind(section: &str) -> Option<&'static str> {
    ["dev-dependencies", "build-dependencies", "dependencies"]
        .into_iter()
        .find(|kind| section == *kind || section.ends_with(&format!(".{kind}")))
}

/// `dependencies.clausters-core` as `("dependencies", "clausters-core")`.
fn table_entry(header: &str) -> Option<(String, String)> {
    let (section, dependency) = header.rsplit_once('.')?;
    let kind = dependency_kind(section)?;
    is_ours(dependency).then(|| (kind.to_string(), dependency.to_string()))
}

fn is_ours(name: &str) -> bool {
    name == "clausters" || name.starts_with("clausters-")
}

#[test]
fn a_crate_depends_only_on_lower_levels() {
    let levels = levels();
    let mut seen = BTreeSet::new();
    let mut used = BTreeSet::new();
    let mut wrong = Vec::new();

    for manifest in manifests() {
        let (name, edges) = package(&manifest);
        let Some(&level) = levels.get(&name) else {
            wrong.push(format!(
                "{name} ({}) has no row in the levels table",
                manifest.display()
            ));
            continue;
        };
        seen.insert(name.clone());
        for (dependency, kind) in edges {
            let Some(&below) = levels.get(&dependency) else {
                wrong.push(format!("{name} depends on {dependency}, which has no row"));
                continue;
            };
            if below < level {
                continue;
            }
            let edge = (name.as_str(), dependency.as_str(), kind.as_str());
            match EXCEPTIONS.iter().find(|e| **e == edge) {
                Some(excused) => {
                    used.insert(*excused);
                }
                None => wrong.push(format!(
                    "{name} (level {level}) has {dependency} (level {below}) in its {kind}"
                )),
            }
        }
    }

    for name in levels.keys().filter(|name| !seen.contains(*name)) {
        wrong.push(format!(
            "the levels table names {name}, which no manifest is"
        ));
    }
    for stale in EXCEPTIONS.iter().filter(|e| !used.contains(*e)) {
        wrong.push(format!(
            "the exception {stale:?} matches no manifest any more: remove it"
        ));
    }

    assert!(
        wrong.is_empty(),
        "the crates' levels (docs/architecture.md, \"The crates and their levels\") \
         do not hold:\n  {}",
        wrong.join("\n  ")
    );
}

#[test]
fn the_reader_sees_the_edges_it_is_there_for() {
    // A reader that silently found nothing would pass the rule over an empty
    // graph, so the edges this tree is known to have are asked for by name.
    let (name, edges) = package(&root().join("Cargo.toml"));
    assert_eq!(name, "clausters");
    for (dependency, kind) in [
        ("clausters-core", "dependencies"),
        ("clausters-apps", "dev-dependencies"),
        // Declared under `[target.'cfg(target_os = "linux")'.dependencies]`.
        ("clausters-plugin-host", "dependencies"),
    ] {
        assert!(
            edges.contains(&(dependency.to_string(), kind.to_string())),
            "the reader missed {dependency} in the server's {kind}: {edges:?}"
        );
    }
    assert_eq!(
        table_entry("dev-dependencies.clausters-core"),
        Some(("dev-dependencies".into(), "clausters-core".into()))
    );
    assert_eq!(table_entry("package.metadata"), None);
}

/// The GUI host lived under the clients' directory until it became a crate
/// among the others. Nothing tracked may still name that path: a reference to
/// it compiles, lints and tests clean, and is found only by whoever follows
/// it. The frozen build log is history and keeps what it said.
#[test]
fn nothing_names_the_host_where_it_used_to_live() {
    // Spelled in two parts so that this file is not its own finding.
    let old = ["clients", "gui"].join("/");
    let out = std::process::Command::new("git")
        .args([
            "grep",
            "-l",
            "--fixed-strings",
            &old,
            "--",
            ".",
            ":!docs/history",
        ])
        .current_dir(root())
        .output()
        .expect("git grep");
    let found = String::from_utf8_lossy(&out.stdout);
    assert!(
        found.trim().is_empty(),
        "these files still name {old}, which is crates/clausters-gui now:\n{found}"
    );
}
