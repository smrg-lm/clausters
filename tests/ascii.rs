//! **A source file is ASCII, and this is what says so.**
//!
//! Comments, doc comments, docstrings and the strings an application prints
//! carried 13 000 characters a terminal, a diff, a grep pattern or an editor's
//! font may not render or match: em dashes, arrows, ellipses, middle dots and
//! the Greek a formula reached for. They are spelled out now (`--`, `->`,
//! `...`, `*`, `pi`) and this keeps them spelled out, because a convention that
//! nothing checks comes back one paste at a time.
//!
//! The rule is about *incidental* typography. Where the character **is** the
//! data -- a glyph table keyed by the character it draws, a dead-key
//! composition doc, a test whose point is that a letter takes two bytes -- it
//! stays, and `DATA` below names every such file with the reason. A new entry
//! there is a claim that the character is the subject of the file, not a
//! flourish in its prose.
//!
//! The books are deliberately not covered: they are read rendered, where the
//! typography is the point.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Files whose content is characters, so the characters stay.
const DATA: &[(&str, &str)] = &[
    (
        "clients/gui/src/host/font/mod.rs",
        "the glyph table: every arm is keyed by the character it draws",
    ),
    (
        "clients/gui/src/host/graphics/textedit.rs",
        "a caret test over a string whose letters are two bytes each",
    ),
    (
        "clients/gui/src/host/gui/app.rs",
        "the doc naming the accented capital a keyboard used to swallow",
    ),
    (
        "clients/gui/src/host/web/compose.rs",
        "dead-key composition: the docs name the marks they compose",
    ),
    (
        "clients/python/examples/panels/text.py",
        "the example that shows accented text being drawn and edited",
    ),
    (
        "clients/web/examples/panels/text.html",
        "the same example, in the page",
    ),
    (
        "clients/web/tests/gen-osc-vectors.py",
        "the OSC utf8 string vector, which exists to carry non-ASCII",
    ),
    (
        "clients/web/tests/osc-vectors.json",
        "generated from it, so it carries the same string",
    ),
];

/// The extensions this covers: source, configuration and the pages that are
/// programs. Markdown is the books' and is left to them.
const EXT: &[&str] = &[
    "rs", "py", "ts", "tsx", "js", "mjs", "sh", "toml", "html", "wgsl", "css", "yml", "yaml",
    "json", "dsp",
];

fn repo() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Every tracked file, asked of git so that generated and ignored trees (the
/// `target/`s, `node_modules`, `dist/`) are never walked.
fn tracked() -> Vec<PathBuf> {
    let out = Command::new("git")
        .arg("ls-files")
        .arg("-z")
        .current_dir(repo())
        .output()
        .expect("git ls-files");
    assert!(out.status.success(), "git ls-files failed");
    String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect()
}

#[test]
fn no_source_file_carries_a_non_ascii_character() {
    let mut found: Vec<String> = Vec::new();
    for rel in tracked() {
        let ext = rel.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !EXT.contains(&ext) {
            continue;
        }
        let path = rel.to_string_lossy().replace('\\', "/");
        if DATA.iter().any(|(data, _)| *data == path) {
            continue;
        }
        let Ok(text) = fs::read_to_string(repo().join(&rel)) else {
            continue; // not UTF-8 text at all: not this test's business
        };
        for (i, line) in text.lines().enumerate() {
            if let Some(ch) = line.chars().find(|c| !c.is_ascii()) {
                found.push(format!(
                    "{path}:{}: U+{:04X} {ch:?} -- {}",
                    i + 1,
                    ch as u32,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        found.is_empty(),
        "non-ASCII in {} source line(s). Spell it out (-- -> ... * pi), or, if the \
         character is the file's subject rather than its typography, add the file to \
         DATA in this test with the reason:\n{}",
        found.len(),
        found.join("\n")
    );
}

#[test]
fn every_data_file_still_exists_and_still_carries_one() {
    for (rel, why) in DATA {
        let text = fs::read_to_string(repo().join(rel))
            .unwrap_or_else(|e| panic!("{rel} is listed as carrying character data: {e}"));
        assert!(
            !text.is_ascii(),
            "{rel} is listed as {why}, but it is all ASCII now -- drop the entry"
        );
    }
}
