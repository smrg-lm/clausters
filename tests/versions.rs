//! **The package version lives in one place, and this is what says so.**
//!
//! It used to live in ten: the root manifest, seven crates, the GUI client's
//! own workspace, the wheel's `pyproject.toml`, the npm `package.json` and two
//! lockfiles — and exactly one pair of them was checked, so `clients/gui` had
//! drifted before and `package-lock.json` was found two minors behind. The
//! eight Cargo crates inherit `[workspace.package].version` now; the rest
//! cannot inherit anything and are written by `scripts/set-version.sh`, which
//! is what this contrasts.
//!
//! It answers *where*, never *which*: the pre-1.0 breaking tier and the two ABI
//! counters are the `release-versioning` skill's, and `versions.sh` there is
//! the tool that answers them.

use std::fs;
use std::path::{Path, PathBuf};

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn read(rel: &str) -> String {
    fs::read_to_string(repo(rel)).unwrap_or_else(|e| panic!("cannot read {rel}: {e}"))
}

/// The first `version = "…"` (or `"version": "…"`) after `section`, which is
/// how every one of these files spells it.
fn version_after(text: &str, section: &str, rel: &str) -> String {
    let from = text
        .find(section)
        .unwrap_or_else(|| panic!("{rel}: no {section:?} section"));
    for line in text[from..].lines().skip(1) {
        let line = line.trim();
        // A TOML table header or the end of a JSON object closes the section.
        if line.starts_with('[') && line.ends_with(']') {
            break;
        }
        let value = line
            .strip_prefix("version = \"")
            .or_else(|| line.strip_prefix("\"version\": \""));
        if let Some(rest) = value {
            return rest[..rest.find('"').expect("an unterminated version")].to_string();
        }
    }
    panic!("{rel}: no version line under {section:?}");
}

#[test]
fn every_manifest_carries_the_one_version() {
    let workspace = read("Cargo.toml");
    let want = version_after(&workspace, "[workspace.package]", "Cargo.toml");
    assert!(
        want.split('.').count() == 3,
        "the workspace version {want:?} is not x.y.z"
    );

    let elsewhere = [
        ("clients/gui/Cargo.toml", "[package]"),
        ("clients/python/pyproject.toml", "[project]"),
        ("clients/web/package.json", "{"),
        ("clients/web/package-lock.json", "{"),
    ];
    for (rel, section) in elsewhere {
        let text = read(rel);
        let got = version_after(&text, section, rel);
        assert_eq!(
            got, want,
            "{rel} says {got}, the workspace says {want} — one command writes \
             both: scripts/set-version.sh {want}"
        );
    }
}

#[test]
fn every_cargo_crate_inherits_it_rather_than_repeating_it() {
    // A crate that spells its own number is a place the next release will
    // forget, which is the whole reason the workspace has the field.
    let mut checked = 0;
    let mut manifests = vec![repo("Cargo.toml")];
    for entry in fs::read_dir(repo("crates")).expect("crates/") {
        let dir = entry.expect("a crate directory").path();
        if dir.join("Cargo.toml").is_file() {
            manifests.push(dir.join("Cargo.toml"));
        }
    }
    for path in manifests {
        let text = fs::read_to_string(&path).expect("a manifest");
        let package = text
            .find("[package]")
            .unwrap_or_else(|| panic!("{path:?}: no [package] section"));
        let body: String = text[package..]
            .lines()
            .skip(1)
            .take_while(|l| !(l.trim().starts_with('[') && l.trim().ends_with(']')))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            body.contains("version.workspace = true"),
            "{}: writes its own version instead of inheriting \
             [workspace.package].version",
            path.display()
        );
        checked += 1;
    }
    assert!(checked >= 8, "only checked {checked} manifests");
}
