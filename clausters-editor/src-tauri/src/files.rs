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

fn collect_md(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_md(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}

/// Relative paths of every `.md` in the documentation, sorted.
#[tauri::command]
pub fn list_docs(app: AppHandle) -> Result<Vec<String>, String> {
    let root = docs_dir(&app)?;
    let mut docs = Vec::new();
    collect_md(&root, &root, &mut docs);
    docs.sort();
    Ok(docs)
}

/// A document's contents; rejects paths outside the documentation folder.
#[tauri::command]
pub fn read_doc(app: AppHandle, path: String) -> Result<String, String> {
    let root = docs_dir(&app)?;
    let full = root
        .join(&path)
        .canonicalize()
        .map_err(|_| format!("Document not found: {path}"))?;
    if !full.starts_with(&root) {
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

/// Is the occurrence at `i` inside an `inline code` span?
fn in_code_span(line: &str, i: usize) -> bool {
    line[..i].matches('`').count() % 2 == 1
}

/// Sections of the documentation that talk about `term`, most relevant first:
/// a heading that names it weighs more than code, and code more than prose.
#[tauri::command]
pub fn search_docs(app: AppHandle, term: String) -> Result<Vec<DocHit>, String> {
    Ok(search_in(&docs_dir(&app)?, term.trim()))
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
        let mut sections: Vec<(Option<String>, u32)> = vec![(None, 0)];
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
                let plain = title.replace('`', "");
                let score = if plain == term || plain.ends_with(&format!(".{term}")) {
                    300
                } else if word_matches(&title, term).next().is_some() {
                    100
                } else {
                    0
                };
                sections.push((Some(title), score));
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
        // `rev` so that, on equal scores, the first section wins.
        if let Some((heading, score)) = sections.into_iter().rev().max_by_key(|(_, s)| *s) {
            if score > 0 {
                hits.push(DocHit {
                    path: rel,
                    heading,
                    score,
                });
            }
        }
    }
    hits.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
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
