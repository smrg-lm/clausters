//! The documentation as books: each folder's title, parts and chapters, which
//! the viewer's start page and its contents pages are drawn from.
//!
//! A folder that carries mdBook's table of contents, `SUMMARY.md`, is read as
//! the book it describes: its first top-level heading is the book's title (but
//! for mdBook's own `# Summary`, which names nothing), every later one starts a
//! part, and each `[Title](page.md)` is a chapter, nested by its indentation.
//! A folder without one is a book of a single part listing its pages, each
//! titled by its first heading.

use std::path::Path;

use tauri::AppHandle;

use crate::files::{collect_md, roots, Root, SUMMARY};

#[derive(serde::Serialize, Debug, PartialEq)]
pub struct Chapter {
    title: String,
    /// The document, as `read_doc` names it; `None` for a draft (`[Title]()`).
    path: Option<String>,
    children: Vec<Chapter>,
}

#[derive(serde::Serialize, Debug, PartialEq)]
pub struct Part {
    /// `None` for the chapters ahead of the first part's heading.
    title: Option<String>,
    chapters: Vec<Chapter>,
}

#[derive(serde::Serialize, Debug)]
pub struct Book {
    /// What its documents' paths start with: empty for the editor's own.
    prefix: String,
    title: String,
    parts: Vec<Part>,
}

/// The editor's own book, then each package's, as `list_docs` orders them.
#[tauri::command]
pub fn list_books(app: AppHandle) -> Result<Vec<Book>, String> {
    Ok(roots(&app)?.iter().map(book).collect())
}

fn book(root: &Root) -> Book {
    // Without a title of its own, a package's book is named after the package
    // and the editor's after its folder.
    let fallback = match root.prefix.strip_suffix('/') {
        Some(name) => name.to_string(),
        None => root
            .dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    let (title, parts) = match std::fs::read_to_string(root.dir.join(SUMMARY)) {
        Ok(text) => parse_summary(&text, &root.prefix),
        Err(_) => (None, vec![listing(&root.dir, &root.prefix)]),
    };
    Book {
        prefix: root.prefix.clone(),
        title: title.unwrap_or(fallback),
        parts,
    }
}

/// A folder without a table of contents: its pages, in `list_docs`' order.
fn listing(dir: &Path, prefix: &str) -> Part {
    let mut docs = Vec::new();
    collect_md(dir, dir, &mut docs);
    docs.sort_by_key(|d| (d.to_lowercase() != "index.md", d.clone()));
    let chapters = docs
        .into_iter()
        .map(|rel| {
            let title = std::fs::read_to_string(dir.join(&rel))
                .ok()
                .and_then(|text| {
                    text.lines()
                        .find_map(|l| l.strip_prefix("# ").map(|t| t.trim().to_string()))
                })
                .unwrap_or_else(|| rel.trim_end_matches(".md").to_string());
            Chapter {
                title,
                path: Some(format!("{prefix}{rel}")),
                children: Vec::new(),
            }
        })
        .collect();
    Part {
        title: None,
        chapters,
    }
}

/// A chapter line of the table of contents, by its indentation.
type Entry = (usize, Chapter);

/// The book's title, if it names one, and its parts.
fn parse_summary(text: &str, prefix: &str) -> (Option<String>, Vec<Part>) {
    let mut title = None;
    let mut seen_heading = false;
    let mut parts = Vec::new();
    let mut part_title = None;
    let mut entries: Vec<Entry> = Vec::new();
    let close = |part_title: Option<String>, entries: &mut Vec<Entry>, parts: &mut Vec<Part>| {
        let chapters = nest(entries, &mut 0);
        entries.clear();
        if part_title.is_some() || !chapters.is_empty() {
            parts.push(Part {
                title: part_title,
                chapters,
            });
        }
    };
    for line in text.lines() {
        if let Some(heading) = line.strip_prefix("# ") {
            let heading = heading.trim().to_string();
            if !seen_heading {
                seen_heading = true;
                if !heading.eq_ignore_ascii_case("summary") {
                    title = Some(heading);
                }
            } else {
                close(part_title.take(), &mut entries, &mut parts);
                part_title = Some(heading);
            }
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let item = line.trim_start();
        let item = item
            .strip_prefix("- ")
            .or_else(|| item.strip_prefix("* "))
            .unwrap_or(item);
        if let Some(chapter) = link(item, prefix) {
            entries.push((indent, chapter));
        }
    }
    close(part_title, &mut entries, &mut parts);
    (title, parts)
}

/// `[Title](page.md)` as a chapter; anything else is not one.
fn link(item: &str, prefix: &str) -> Option<Chapter> {
    let rest = item.strip_prefix('[')?;
    let (title, rest) = rest.split_once("](")?;
    let (target, _) = rest.split_once(')')?;
    let target = target.trim().trim_start_matches("./");
    Some(Chapter {
        title: title.to_string(),
        path: (!target.is_empty()).then(|| format!("{prefix}{target}")),
        children: Vec::new(),
    })
}

/// The entries from `at` on that sit at its indentation, each holding the
/// deeper ones that follow it.
fn nest(entries: &mut [Entry], at: &mut usize) -> Vec<Chapter> {
    let mut out: Vec<Chapter> = Vec::new();
    let Some(level) = entries.get(*at).map(|e| e.0) else {
        return out;
    };
    while let Some(&(indent, _)) = entries.get(*at) {
        if indent < level {
            break;
        }
        if indent > level {
            let children = nest(entries, at);
            match out.last_mut() {
                Some(last) => last.children.extend(children),
                None => out.extend(children),
            }
            continue;
        }
        let chapter = std::mem::replace(
            &mut entries[*at].1,
            Chapter {
                title: String::new(),
                path: None,
                children: Vec::new(),
            },
        );
        out.push(chapter);
        *at += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUMMARY_TEXT: &str = "# Summary

[Introduction](introduction.md)

# User Guide

- [Getting started](getting-started.md)
- [Configuration](configuration.md)
  - [The server](configuration/server.md)
    - [Deeper](configuration/deeper.md)
  - [The host](./configuration/gui.md)
- [Soon]()

---

# Reference

- [Command line](cli.md)
";

    fn titles(chapters: &[Chapter]) -> Vec<&str> {
        chapters.iter().map(|c| c.title.as_str()).collect()
    }

    #[test]
    fn parts_and_nesting() {
        let (title, parts) = parse_summary(SUMMARY_TEXT, "pkg/");
        assert_eq!(title, None);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].title, None);
        assert_eq!(
            parts[0].chapters[0].path.as_deref(),
            Some("pkg/introduction.md")
        );
        assert_eq!(parts[1].title.as_deref(), Some("User Guide"));
        assert_eq!(
            titles(&parts[1].chapters),
            ["Getting started", "Configuration", "Soon"]
        );
        let config = &parts[1].chapters[1];
        assert_eq!(titles(&config.children), ["The server", "The host"]);
        assert_eq!(titles(&config.children[0].children), ["Deeper"]);
        assert_eq!(
            config.children[1].path.as_deref(),
            Some("pkg/configuration/gui.md")
        );
        assert_eq!(parts[1].chapters[2].path, None);
        assert_eq!(titles(&parts[2].chapters), ["Command line"]);
    }

    #[test]
    fn a_titled_summary_names_the_book() {
        let (title, parts) = parse_summary("# The editor\n\n- [Start](index.md)\n", "");
        assert_eq!(title.as_deref(), Some("The editor"));
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].chapters[0].path.as_deref(), Some("index.md"));
    }

    #[test]
    fn a_folder_without_one_lists_its_pages() {
        let dir =
            std::env::temp_dir().join(format!("clausters-editor-books-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("b.md"), "Intro text.\n# Second page\n").unwrap();
        std::fs::write(dir.join("index.md"), "# Start\n").unwrap();
        let book = book(&Root {
            prefix: "pkg/".into(),
            dir: dir.clone(),
        });
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(book.title, "pkg");
        assert_eq!(titles(&book.parts[0].chapters), ["Start", "Second page"]);
        assert_eq!(book.parts[0].chapters[1].path.as_deref(), Some("pkg/b.md"));
    }
}
