//! **The documentation the editor shows**: the folders it reads (its own,
//! then the book of every package of the session's environment that
//! registers one), each read as a book -- its title, parts and chapters --
//! and every document's Markdown, which the `docs` element is given whole and
//! searches itself.
//!
//! A folder that carries mdBook's table of contents, `SUMMARY.md`, is read as
//! the book it describes: its first top-level heading is the book's title (but
//! for mdBook's own `# Summary`, which names nothing), every later one starts a
//! part, and each `[Title](page.md)` is a chapter, nested by its indentation.
//! A folder without one is a book of a single part listing its pages, each
//! titled by its first heading.
//!
//! What a book is built from is a [`Folder`], read: the desktop reads one
//! from the disk (`read_all`), a page from where it is served. The book
//! is built here for both.

use std::collections::BTreeMap;
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};

/// The name mdBook gives a book's table of contents: read as the book's
/// structure, never listed as a page of it.
pub const SUMMARY: &str = "SUMMARY.md";

/// The editor's own documentation: the configured folder, else
/// `CLAUSTERS_EDITOR_DOCS`, else where an installed binary has it -- a
/// `docs` folder beside it, or `../share/clausters-editor/docs` from it, as
/// `install.sh` and the wheel lay it out -- else the crate's own, which a
/// binary run from the repository reads.
#[cfg(not(target_arch = "wasm32"))]
fn docs_dir() -> Result<PathBuf, String> {
    let dir = if let Some(custom) = crate::config::get().docs_dir {
        PathBuf::from(custom)
    } else if let Some(env) = std::env::var_os("CLAUSTERS_EDITOR_DOCS") {
        PathBuf::from(env)
    } else {
        let installed = std::env::current_exe().ok().and_then(|exe| {
            let bin = exe.parent()?;
            [bin.join("docs"), bin.join("../share/clausters-editor/docs")]
                .into_iter()
                .find(|d| d.is_dir())
        });
        installed.unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("docs"))
    };
    dir.canonicalize()
        .map_err(|e| format!("Documentation folder not found: {e}"))
}

#[cfg(not(target_arch = "wasm32"))]
pub fn collect_md(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_md(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "md")
            && entry.file_name() != SUMMARY
            && let Ok(rel) = path.strip_prefix(root)
        {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// A folder of documentation and the prefix its documents are listed under:
/// none for the editor's own, the package's name and a slash for a package's.
#[cfg(not(target_arch = "wasm32"))]
pub struct Root {
    pub prefix: String,
    pub dir: PathBuf,
}

/// The editor's documentation, then the book of every package of the
/// session's environment that registers one.
#[cfg(not(target_arch = "wasm32"))]
pub fn roots() -> Result<Vec<Root>, String> {
    let own = docs_dir();
    let mut roots: Vec<Root> = own
        .as_ref()
        .ok()
        .map(|dir| Root {
            prefix: String::new(),
            dir: dir.clone(),
        })
        .into_iter()
        .collect();
    for (name, dir) in crate::native::pkgdocs::books() {
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

/// **Every book and every document**, read now from the disk: the books
/// as the `docs` element's `books`, the documents by path as its
/// `documents`.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_all() -> Result<crate::app::Docs, String> {
    let folders = roots()?
        .into_iter()
        .map(|root| {
            let mut docs = Vec::new();
            collect_md(&root.dir, &root.dir, &mut docs);
            let documents = docs
                .into_iter()
                .filter_map(|rel| {
                    let text = std::fs::read_to_string(root.dir.join(&rel)).ok()?;
                    Some((rel, text))
                })
                .collect();
            Folder {
                name: root
                    .dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                summary: std::fs::read_to_string(root.dir.join(SUMMARY)).ok(),
                documents,
                at: root.dir.to_string_lossy().into_owned(),
                prefix: root.prefix,
            }
        })
        .collect();
    Ok(read(folders))
}

/// **A folder of documentation, read**: what its book and its documents
/// are made of, wherever it was read from.
pub struct Folder {
    /// What its documents' paths start with: empty for the editor's own,
    /// a package's name and a slash for a package's.
    pub prefix: String,
    /// Its own name, which titles a book of the editor's that names none.
    pub name: String,
    /// Its table of contents (`SUMMARY.md`), if it has one.
    pub summary: Option<String>,
    /// Every document, by its path inside the folder, `/` separated.
    pub documents: Vec<(String, String)>,
    /// Where it is, which a picture a page names is read from: a folder on
    /// the disk, or where a page serves it.
    pub at: String,
}

/// **Every book and every document** of `folders`, in their order.
pub fn read(folders: Vec<Folder>) -> crate::app::Docs {
    let books = folders.iter().map(book).collect();
    let mut documents = BTreeMap::new();
    let mut roots = BTreeMap::new();
    for folder in folders {
        roots.insert(folder.prefix.clone(), folder.at);
        for (rel, text) in folder.documents {
            documents.insert(format!("{}{rel}", folder.prefix), text);
        }
    }
    (books, documents, roots)
}

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

fn book(folder: &Folder) -> Book {
    // Without a title of its own, a package's book is named after the package
    // and the editor's after its folder.
    let fallback = match folder.prefix.strip_suffix('/') {
        Some(name) => name.to_string(),
        None => folder.name.clone(),
    };
    let (title, parts) = match &folder.summary {
        Some(text) => parse_summary(text, &folder.prefix),
        None => (None, vec![listing(folder)]),
    };
    Book {
        prefix: folder.prefix.clone(),
        title: title.unwrap_or(fallback),
        parts,
    }
}

/// A folder without a table of contents: its pages, `index.md` first and
/// the others by their path.
fn listing(folder: &Folder) -> Part {
    let mut docs: Vec<&(String, String)> = folder
        .documents
        .iter()
        .filter(|(rel, _)| rel.ends_with(".md") && rel.rsplit('/').next() != Some(SUMMARY))
        .collect();
    docs.sort_by_key(|(rel, _)| (rel.to_lowercase() != "index.md", rel.clone()));
    let chapters = docs
        .into_iter()
        .map(|(rel, text)| {
            let title = text
                .lines()
                .find_map(|l| l.strip_prefix("# ").map(|t| t.trim().to_string()))
                .unwrap_or_else(|| rel.trim_end_matches(".md").to_string());
            Chapter {
                title,
                path: Some(format!("{}{rel}", folder.prefix)),
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
        let book = book(&Folder {
            prefix: "pkg/".into(),
            name: "docs".into(),
            summary: None,
            documents: vec![
                ("b.md".into(), "Intro text.\n# Second page\n".into()),
                ("index.md".into(), "# Start\n".into()),
            ],
            at: String::new(),
        });
        assert_eq!(book.title, "pkg");
        assert_eq!(titles(&book.parts[0].chapters), ["Start", "Second page"]);
        assert_eq!(book.parts[0].chapters[1].path.as_deref(), Some("pkg/b.md"));
    }
}
