//! Reading and writing the user's files and the documentation folder.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

/// The configured folder; otherwise the project's `docs/` in development, or the
/// resource bundled with the packaged app.
fn docs_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = if let Some(custom) = crate::config::get(app).docs_dir {
        PathBuf::from(custom)
    } else if cfg!(debug_assertions) {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs")
    } else {
        app.path()
            .resource_dir()
            .map_err(|e| e.to_string())?
            .join("docs")
    };
    dir.canonicalize()
        .map_err(|e| format!("Documentation folder not found: {e}"))
}

/// The name mdBook gives a book's table of contents: read as the book's
/// structure (`books`), never listed as a page of it.
pub(crate) const SUMMARY: &str = "SUMMARY.md";

pub(crate) fn collect_md(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_md(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "md") && entry.file_name() != SUMMARY {
            if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}

/// A folder of documentation and the prefix its documents are listed under: none
/// for the editor's own, the package's name and a slash for a package's book.
pub(crate) struct Root {
    pub(crate) prefix: String,
    pub(crate) dir: PathBuf,
}

/// The editor's documentation, then the book of every package of the session's
/// environment that registers one (see `pkgdocs`).
pub(crate) fn roots(app: &AppHandle) -> Result<Vec<Root>, String> {
    let own = docs_dir(app);
    let mut roots: Vec<Root> = own
        .as_ref()
        .ok()
        .map(|dir| Root {
            prefix: String::new(),
            dir: dir.clone(),
        })
        .into_iter()
        .collect();
    for (name, dir) in crate::pkgdocs::books(app) {
        if let Ok(dir) = dir.canonicalize() {
            roots.push(Root {
                prefix: format!("{name}/"),
                dir,
            });
        }
    }
    match own {
        Err(e) if roots.is_empty() => Err(e),
        _ => Ok(roots),
    }
}

/// Relative paths of every `.md` in the documentation: the editor's own first, then
/// each package's book under its name, each folder's `index.md` ahead of the rest.
#[tauri::command]
pub fn list_docs(app: AppHandle) -> Result<Vec<String>, String> {
    let mut all = Vec::new();
    for root in roots(&app)? {
        let mut docs = Vec::new();
        collect_md(&root.dir, &root.dir, &mut docs);
        // Each folder's index first: it is where the folder starts.
        docs.sort_by_key(|d| (d.to_lowercase() != "index.md", d.clone()));
        all.extend(docs.into_iter().map(|d| format!("{}{d}", root.prefix)));
    }
    Ok(all)
}

/// A document's contents; rejects paths outside the documentation folders.
#[tauri::command]
pub fn read_doc(app: AppHandle, path: String) -> Result<String, String> {
    let roots = roots(&app)?;
    // A package's prefix wins over a folder of the same name in the editor's own.
    let (root, rel) = roots
        .iter()
        .filter(|r| !r.prefix.is_empty())
        .find_map(|r| path.strip_prefix(&r.prefix).map(|rel| (r, rel)))
        .or_else(|| {
            roots
                .iter()
                .find(|r| r.prefix.is_empty())
                .map(|r| (r, path.as_str()))
        })
        .ok_or_else(|| format!("Document not found: {path}"))?;
    let full = root
        .dir
        .join(rel)
        .canonicalize()
        .map_err(|_| format!("Document not found: {path}"))?;
    if !full.starts_with(&root.dir) {
        return Err("Path outside the documentation".into());
    }
    std::fs::read_to_string(full).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn read_file(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| format!("Could not read {path}: {e}"))
}

#[tauri::command]
pub fn write_file(path: String, contents: String) -> Result<(), String> {
    std::fs::write(&path, contents).map_err(|e| format!("Could not save {path}: {e}"))
}

#[derive(serde::Serialize)]
pub struct DocHit {
    path: String,
    /// Heading of the section where the term appears most (None = before the first one).
    heading: Option<String>,
    score: u32,
    /// Whether that heading *is* the name (`## Server`, `#### Server.boot`): an API
    /// reference's entry for it, which help prefers to any page that only talks about it.
    named: bool,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Occurrences of `term` as a whole word in `text`, by start position.
fn word_matches<'a>(text: &'a str, term: &'a str) -> impl Iterator<Item = usize> + 'a {
    text.match_indices(term).filter_map(move |(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + term.len()..].chars().next();
        (!before.is_some_and(is_word) && !after.is_some_and(is_word)).then_some(i)
    })
}

/// `text` without Markdown's backslash escapes (`free\_all` -> `free_all`).
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match chars.peek() {
            Some(&next) if c == '\\' && next.is_ascii_punctuation() => {
                out.push(next);
                chars.next();
            }
            _ => out.push(c),
        }
    }
    out
}

/// Is the occurrence at `i` inside an `inline code` span?
fn in_code_span(line: &str, i: usize) -> bool {
    line[..i].matches('`').count() % 2 == 1
}

/// Sections of the documentation that talk about `term`, most relevant first:
/// a heading that is the name first, then by score -- a heading that names it
/// weighs more than code, and code more than prose.
#[tauri::command]
pub fn search_docs(app: AppHandle, term: String) -> Result<Vec<DocHit>, String> {
    let mut hits = Vec::new();
    for root in roots(&app)? {
        hits.extend(
            search_in(&root.dir, term.trim())
                .into_iter()
                .map(|mut hit| {
                    hit.path = format!("{}{}", root.prefix, hit.path);
                    hit
                }),
        );
    }
    hits.sort_by(|a, b| rank(b).cmp(&rank(a)).then_with(|| a.path.cmp(&b.path)));
    hits.truncate(10);
    Ok(hits)
}

fn rank(hit: &DocHit) -> (bool, u32) {
    (hit.named, hit.score)
}

fn search_in(root: &Path, term: &str) -> Vec<DocHit> {
    if term.is_empty() {
        return Vec::new();
    }
    let mut files = Vec::new();
    collect_md(root, root, &mut files);

    let mut hits = Vec::new();
    for rel in files {
        let Ok(text) = std::fs::read_to_string(root.join(&rel)) else {
            continue;
        };
        // Score per section (heading -> points).
        let mut sections: Vec<(Option<String>, u32, bool)> = vec![(None, 0, false)];
        let mut in_fence = false;
        for line in text.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                in_fence = !in_fence;
                continue;
            }
            if !in_fence && trimmed.starts_with('#') {
                let title = trimmed.trim_start_matches('#').trim().to_string();
                // A heading that *is* the name (`## Routine`, `# clausters.play`) weighs
                // more than one that only contains it (`#### Routine.run`).
                // As it reads: `Server.free\_all` is the name `Server.free_all`.
                let plain = unescape(&title.replace('`', ""));
                let score = if plain == term || plain.ends_with(&format!(".{term}")) {
                    300
                } else if word_matches(&plain, term).next().is_some() {
                    100
                } else {
                    0
                };
                sections.push((Some(title), score, score >= 300));
                continue;
            }
            let section = sections.last_mut().unwrap();
            for i in word_matches(line, term) {
                section.1 += if in_fence || in_code_span(line, i) {
                    10
                } else {
                    1
                };
            }
        }
        // `rev` so that, on equal ranks, the first section wins.
        if let Some((heading, score, named)) = sections
            .into_iter()
            .rev()
            .max_by_key(|(_, score, named)| (*named, *score))
        {
            if score > 0 {
                hits.push(DocHit {
                    path: rel,
                    heading,
                    score,
                    named,
                });
            }
        }
    }
    hits.sort_by(|a, b| rank(b).cmp(&rank(a)).then_with(|| a.path.cmp(&b.path)));
    hits.truncate(10);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_word_and_code() {
        let dir =
            std::env::temp_dir().join(format!("clausters-editor-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("a.md"),
            "# Intro\nUse `ServerOptions`.\n\n## The `Server`\nText.\n",
        )
        .unwrap();
        std::fs::write(dir.join("b.md"), "# Other\nThe Server is mentioned once.\n").unwrap();
        let hits = search_in(&dir, "Server");
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].path, "a.md");
        assert_eq!(hits[0].heading.as_deref(), Some("The `Server`"));
        assert_eq!(hits[1].score, 1); // `ServerOptions` does not count
    }

    #[test]
    fn escaped_heading_is_the_name() {
        let dir = std::env::temp_dir().join(format!(
            "clausters-editor-test-escaped-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("server.md"),
            "# server\n* [free\\_all](#server.Server.free_all)\n\n#### Server.free\\_all\nFrees.\n",
        )
        .unwrap();
        let hits = search_in(&dir, "Server.free_all");
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(hits[0].named);
        assert_eq!(hits[0].heading.as_deref(), Some("Server.free\\_all"));
    }

    /// Prints results over a real folder: DOCS=/path cargo test -- --ignored --nocapture
    #[test]
    #[ignore]
    fn show() {
        let root = PathBuf::from(std::env::var("DOCS").unwrap());
        for term in std::env::var("TERMS")
            .unwrap_or("Server Routine play".into())
            .split(' ')
        {
            let hits = search_in(&root, term);
            println!(
                "{term:>10}: {:?}",
                hits.iter()
                    .take(3)
                    .map(|h| (&h.path, &h.heading, h.score))
                    .collect::<Vec<_>>()
            );
        }
    }
}
