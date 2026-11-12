//! **A crate's modules read as text**: every path a source file spells,
//! resolved to the module it lands in.
//!
//! What the checks on a crate's inner order share (`host_layers`,
//! `apps_layers`). A reference is a path the source spells --
//! `crate::host::paint::Mesh`, `super::font`, `use super::{layout, widget}`
//! -- resolved from the module it is written in, inline modules (a file's
//! `mod tests`) included. Comments, strings and character literals are not
//! read. It reads the source rather than asking the compiler, so it holds
//! under every feature configuration and needs nothing built.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// A file's text, or a panic that names it.
pub fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// The modules a file declares with `mod name;`, whatever its `cfg`.
pub fn declared(file: &Path) -> BTreeSet<String> {
    let text = clean(&read(file));
    let mut out = BTreeSet::new();
    let words: Vec<&str> = text.split_whitespace().collect();
    for pair in words.windows(2) {
        if pair[0] == "mod"
            && let Some(name) = pair[1].strip_suffix(';')
        {
            out.insert(name.to_string());
        }
    }
    out
}

/// **The source without what is not code**: comments dropped, and the inside
/// of every string and character literal emptied, so neither a path in a
/// message nor a brace in a format string is read.
pub fn clean(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let mut depth = 0;
                while i < b.len() {
                    if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                        depth += 1;
                        i += 2;
                    } else if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
                out.push(' ');
            }
            b'r' if raw_string(b, i).is_some() => {
                let (hashes, open) = raw_string(b, i).unwrap();
                let close: Vec<u8> = std::iter::once(b'"')
                    .chain(std::iter::repeat_n(b'#', hashes))
                    .collect();
                i = open + 1;
                while i < b.len() && !b[i..].starts_with(&close) {
                    i += 1;
                }
                i += close.len();
                out.push_str("\"\"");
            }
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
                out.push_str("\"\"");
            }
            b'\'' => match char_literal(b, i) {
                Some(end) => {
                    i = end;
                    out.push_str("' '");
                }
                None => {
                    out.push('\'');
                    i += 1;
                }
            },
            _ => {
                // copy one whole character
                let c = text[i..].chars().next().unwrap();
                out.push(c);
                i += c.len_utf8();
            }
        }
    }
    out
}

/// At `i`, a raw string's opening (`r"`, `r#"`, ...) not inside a word: the
/// number of hashes and where its quote is.
fn raw_string(b: &[u8], i: usize) -> Option<(usize, usize)> {
    if i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_') && b[i - 1] != b'b' {
        return None;
    }
    let mut j = i + 1;
    while b.get(j) == Some(&b'#') {
        j += 1;
    }
    (b.get(j) == Some(&b'"')).then_some((j - i - 1, j))
}

/// At `i`, the end of a character literal (`'a'`, `'\n'`, `'\u{7f}'`), or
/// `None` for a lifetime.
fn char_literal(b: &[u8], i: usize) -> Option<usize> {
    if b.get(i + 1) == Some(&b'\\') {
        let mut j = i + 2;
        while j < b.len() && b[j] != b'\'' && j < i + 12 {
            j += 1;
        }
        return (b.get(j) == Some(&b'\'')).then_some(j + 1);
    }
    let rest = std::str::from_utf8(&b[i + 1..]).ok()?;
    let c = rest.chars().next()?;
    (b.get(i + 1 + c.len_utf8()) == Some(&b'\'')).then_some(i + 2 + c.len_utf8())
}

/// A file's module path from the crate root: `host/paint.rs` is
/// `["host", "paint"]`, `host/font/mod.rs` is `["host", "font"]`.
pub fn module_of(src: &Path, file: &Path) -> Vec<String> {
    let rel = file.strip_prefix(src).unwrap().with_extension("");
    let mut path: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if matches!(path.last().map(String::as_str), Some("mod" | "lib")) {
        path.pop();
    }
    path
}

/// Every path a file spells, resolved from the crate root, with the line it
/// is on.
pub fn references(src: &Path, file: &Path) -> Vec<(Vec<String>, usize)> {
    let text = clean(&read(file));
    let b = text.as_bytes();
    let home = module_of(src, file);
    let mut out = Vec::new();
    // the inline modules open at this point, with the depth each opened at
    let mut inline: Vec<(String, usize)> = Vec::new();
    let mut depth = 0usize;
    let mut i = 0;
    let word_at = |i: usize| -> &str {
        let end = text[i..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .map_or(text.len(), |n| i + n);
        &text[i..end]
    };
    while i < b.len() {
        match b[i] {
            b'{' => {
                depth += 1;
                i += 1;
                continue;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                if inline.last().is_some_and(|(_, at)| *at == depth) {
                    inline.pop();
                }
                i += 1;
                continue;
            }
            _ => {}
        }
        let starts_word = b[i].is_ascii_alphabetic() || b[i] == b'_';
        let after_word = i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
        if !starts_word || after_word {
            i += 1;
            continue;
        }
        let word = word_at(i);
        let next = i + word.len();
        if word == "mod" {
            let rest = text[next..].trim_start();
            let name = word_at(text.len() - rest.len());
            let after = rest[name.len()..].trim_start();
            if !name.is_empty() && after.starts_with('{') {
                inline.push((name.to_string(), depth));
            }
            i = next;
            continue;
        }
        let preceded_by_path = i >= 2 && &text[i - 2..i] == "::";
        if !matches!(word, "crate" | "super" | "self") || preceded_by_path {
            i = next;
            continue;
        }
        // the module the path is written in
        let mut here = home.clone();
        here.extend(inline.iter().map(|(name, _)| name.clone()));
        let line = text[..i].matches('\n').count() + 1;
        let (paths, end) = path_at(&text, i, &here);
        out.extend(paths.into_iter().map(|p| (p, line)));
        i = end.max(next);
    }
    out
}

/// The path (or the paths of a `{...}` group) starting at `i`, resolved from
/// `here`, and where it ends.
fn path_at(text: &str, i: usize, here: &[String]) -> (Vec<Vec<String>>, usize) {
    let mut path: Vec<String> = Vec::new();
    let mut j = i;
    let mut first = true;
    loop {
        let rest = &text[j..];
        let len = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        let seg = &rest[..len];
        if seg.is_empty() {
            break;
        }
        match (first, seg) {
            (true, "crate") => path.clear(),
            (true, "self") => path = here.to_vec(),
            (true, "super") => {
                path = here.to_vec();
                path.pop();
            }
            (false, "super") => {
                path.pop();
            }
            _ => path.push(seg.to_string()),
        }
        first = false;
        j += len;
        if !text[j..].starts_with("::") {
            return (vec![path], j);
        }
        j += 2;
        if text[j..].starts_with('{') {
            // a group: each top-level item's first segment
            let mut depth = 0;
            let mut item = String::new();
            let mut items = Vec::new();
            for (k, c) in text[j..].char_indices() {
                match c {
                    '{' => {
                        depth += 1;
                        if depth == 1 {
                            continue;
                        }
                    }
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            items.push(std::mem::take(&mut item));
                            let paths = items
                                .iter()
                                .filter_map(|item| {
                                    let name = item
                                        .trim()
                                        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                                        .next()?;
                                    if name.is_empty() {
                                        return None;
                                    }
                                    let mut p = path.clone();
                                    if name != "self" {
                                        p.push(name.to_string());
                                    }
                                    Some(p)
                                })
                                .collect();
                            return (paths, j + k + 1);
                        }
                    }
                    ',' if depth == 1 => {
                        items.push(std::mem::take(&mut item));
                        continue;
                    }
                    _ => {}
                }
                if depth >= 1 {
                    item.push(c);
                }
            }
            return (vec![path], j);
        }
    }
    (vec![path], j)
}

/// Every source file of the library: `src/` but the binaries.
pub fn files(src: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("a source directory") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n != "bin") {
                    walk(&path, out);
                }
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(src, &mut out);
    out.sort();
    out
}
