//! Every binding symbol is declared in `docs/bindings.md`.
//!
//! `clausters-core` reaches the world through three bindings and cargo checks
//! only that each one agrees with *core* — never that they agree with each
//! other. So a function can be added to the C ABI and never reach the browser,
//! or grown on the wasm side alone, and every build stays green.
//!
//! The Python leg is checked by comparison, because it owes the C ABI total
//! coverage (`clients/python/tests/test_native_parity.py`). The wasm leg does
//! not: a browser has WebSocket already, libverovio is not built for wasm,
//! JavaScript has no `u64`, and wasm frees by `Drop` where C needs an explicit
//! `_free`. Their surfaces differ on purpose, so equality is the wrong test and
//! the right one is **whether each difference was decided**. `docs/bindings.md`
//! is where that decision is written and this is what makes writing it
//! unavoidable: a symbol missing from the table fails here.
//!
//! It reads source text rather than linking anything, so it holds under every
//! feature configuration — including the ones where the symbols it names are
//! not compiled at all.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The `extern "C"` items of `clausters-ffi`, in declaration order.
fn c_abi_symbols() -> BTreeSet<String> {
    let dir = root().join("crates/clausters-ffi/src");
    let mut out = BTreeSet::new();
    for entry in std::fs::read_dir(&dir).expect("clausters-ffi/src") {
        let path = entry.expect("dir entry").path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read");
        for line in text.lines() {
            let line = line.trim_start();
            if let Some(rest) = line
                .strip_prefix("pub extern \"C\" fn ")
                .or_else(|| line.strip_prefix("pub unsafe extern \"C\" fn "))
            {
                out.insert(ident(rest).to_string());
            }
        }
    }
    out
}

/// The `wasm_bindgen` surface of `clausters-core-web`: free functions carrying
/// the attribute, and every method of an `impl` block that carries it.
fn wasm_symbols() -> BTreeSet<String> {
    let text = std::fs::read_to_string(root().join("crates/clausters-core-web/src/lib.rs"))
        .expect("clausters-core-web/src/lib.rs");
    let lines: Vec<&str> = text.lines().collect();
    let mut out = BTreeSet::new();
    let mut class: Option<&str> = None;
    let mut class_exported = false;
    for (i, raw) in lines.iter().enumerate() {
        if let Some(rest) = raw.strip_prefix("impl ") {
            class = Some(ident(rest));
            class_exported = attributes_above(&lines, i).any(|a| a.contains("wasm_bindgen"));
        } else if let Some(rest) = raw.strip_prefix("pub fn ") {
            class = None;
            if attributes_above(&lines, i).any(|a| a.contains("wasm_bindgen")) {
                out.insert(ident(rest).to_string());
            }
        } else if let Some(rest) = raw.strip_prefix("    pub fn ")
            && class_exported
        {
            out.insert(format!(
                "{}.{}",
                class.expect("method outside impl"),
                ident(rest)
            ));
        }
    }
    out
}

/// The attribute lines directly above item `i`, stopping at the first blank
/// line — so an item's own attributes are read and the previous item's are not.
fn attributes_above<'a>(lines: &'a [&'a str], i: usize) -> impl Iterator<Item = &'a str> {
    lines[..i]
        .iter()
        .rev()
        .take_while(|l| {
            let t = l.trim();
            t.starts_with("#[") || t.starts_with("///") || t.starts_with("//")
        })
        .copied()
}

/// The leading identifier of `s` (up to the first non-identifier character).
fn ident(s: &str) -> &str {
    let end = s
        .find(|c: char| !c.is_alphanumeric() && c != '_')
        .unwrap_or(s.len());
    &s[..end]
}

/// Every `` `symbol` `` in the first two columns of the manifest's tables.
fn manifest_symbols(path: &Path) -> BTreeSet<String> {
    let text = std::fs::read_to_string(path).expect("docs/bindings.md");
    let mut out = BTreeSet::new();
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with('|') {
            continue;
        }
        // Columns three onward are prose, and prose cites symbols it does not
        // declare -- only the binding columns count as a declaration.
        for cell in line.split('|').skip(1).take(2) {
            let cell = cell.trim();
            if let Some(sym) = cell.strip_prefix('`').and_then(|c| c.strip_suffix('`')) {
                // A binding symbol is an identifier, optionally `Class.method`.
                // The page's own prose tables name crates and paths in
                // backticks too (`clausters-ffi`, `clausters/_native.py`), and
                // those are not declarations of anything.
                if !sym.is_empty()
                    && sym
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
                {
                    out.insert(sym.to_string());
                }
            }
        }
    }
    out
}

fn manifest() -> BTreeSet<String> {
    manifest_symbols(&root().join("docs/bindings.md"))
}

#[test]
fn the_parsers_found_both_surfaces() {
    // Everything below compares against sets this file extracts from source, so
    // a parser that stops matching turns the real checks into vacuous passes.
    let c = c_abi_symbols();
    let w = wasm_symbols();
    assert!(c.len() > 50, "parsed only {} C ABI symbols", c.len());
    assert!(w.len() > 40, "parsed only {} wasm symbols", w.len());
    assert!(c.contains("clausters_core_abi_version"), "{c:?}");
    assert!(
        w.contains("beats_to_secs") && w.contains("JsRegistry.alloc"),
        "{w:?}"
    );
}

#[test]
fn every_c_abi_symbol_is_declared() {
    let declared = manifest();
    let missing: Vec<_> = c_abi_symbols()
        .into_iter()
        .filter(|s| !declared.contains(s))
        .collect();
    assert!(
        missing.is_empty(),
        "exported by clausters-ffi, absent from docs/bindings.md: {}\n\
         Add a row saying what the wasm side does with it (a symbol, or one of \
         idiom / n/a / gap with the reason).",
        missing.join(", ")
    );
}

#[test]
fn every_wasm_symbol_is_declared() {
    let declared = manifest();
    let missing: Vec<_> = wasm_symbols()
        .into_iter()
        .filter(|s| !declared.contains(s))
        .collect();
    assert!(
        missing.is_empty(),
        "exposed by clausters-core-web, absent from docs/bindings.md: {}\n\
         Add a row saying what the C ABI does with it (a symbol, or one of \
         idiom / n/a / gap with the reason).",
        missing.join(", ")
    );
}

#[test]
fn the_manifest_names_no_symbol_that_is_gone() {
    // The direction that rots quietly: a binding drops a function and its row
    // stays, so the table documents a surface nobody offers any more.
    let live: BTreeSet<String> = c_abi_symbols().union(&wasm_symbols()).cloned().collect();
    let stale: Vec<_> = manifest()
        .into_iter()
        .filter(|s| !live.contains(s))
        .collect();
    assert!(
        stale.is_empty(),
        "docs/bindings.md declares symbols neither binding exports: {}",
        stale.join(", ")
    );
}
