//! **What is in a directory**, for an element that shows one -- the file
//! chooser.
//!
//! The listing is the host's, not a client's: the same chooser lists the disk
//! in a window and the page's own storage (OPFS) in a tab, so neither client
//! implements it. It is asked the way a bulk resource is: an element says which
//! directory it wants listed ([`Element::wants_listing`]), the front lists it --
//! at once natively ([`list_dir`]), asynchronously in a page, where OPFS answers
//! with promises (`host::web::files`) -- and hands the answer back
//! ([`Host::deliver_listing`]). The element never touches a filesystem, so the
//! agnostic core stays free of one.
//!
//! A **path** means what it means to every verb that takes one: a path on the
//! disk natively, a path in the page's storage in a tab, where `/` is the
//! storage's root (`docs/decisions.md`, "A path in a page is its own storage").
//!
//! [`Element::wants_listing`]: super::widget::Element::wants_listing

use super::Host;

/// One entry of a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub dir: bool,
    /// The size in bytes (`0` for a directory).
    pub size: u64,
}

/// A directory's listing: the path it was resolved to -- absolute on the disk,
/// rooted at `/` in a page -- and what is in it, directories first, then by
/// name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub path: String,
    pub entries: Vec<DirEntry>,
}

impl Listing {
    /// Puts the entries in the order a chooser shows them: directories first,
    /// then by name, ignoring case; and leaves out the hidden ones (a name
    /// starting with a dot).
    pub fn ordered(path: String, mut entries: Vec<DirEntry>) -> Listing {
        entries.retain(|e| !e.name.starts_with('.'));
        entries.sort_by(|a, b| {
            b.dir
                .cmp(&a.dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Listing { path, entries }
    }
}

/// The directory `path` is in, lexically: `None` at a root.
pub fn parent(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        return None; // "/" is a root
    }
    let cut = trimmed.rfind(['/', '\\'])?;
    Some(if cut == 0 {
        "/".to_string()
    } else {
        trimmed[..cut].to_string()
    })
}

/// `name` inside directory `path`.
pub fn join(path: &str, name: &str) -> String {
    if path.ends_with('/') || path.ends_with('\\') {
        format!("{path}{name}")
    } else {
        format!("{path}/{name}")
    }
}

/// Lists `path` on the disk, resolving it to an absolute path first.
#[cfg(not(target_arch = "wasm32"))]
pub fn list_dir(path: &str) -> Result<Listing, String> {
    let resolved = std::fs::canonicalize(path).map_err(|e| format!("{path}: {e}"))?;
    let read = std::fs::read_dir(&resolved).map_err(|e| format!("{path}: {e}"))?;
    let entries = read
        .filter_map(Result::ok)
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            Some(DirEntry {
                name: e.file_name().to_string_lossy().into_owned(),
                dir: meta.is_dir(),
                size: if meta.is_dir() { 0 } else { meta.len() },
            })
        })
        .collect();
    Ok(Listing::ordered(
        resolved.to_string_lossy().into_owned(),
        entries,
    ))
}

impl Host {
    /// Every directory an element of an open window is waiting to have listed,
    /// as `(window, widget, path)` -- asked once each: the element clears its
    /// ask as it answers.
    pub fn pending_listings(&mut self) -> Vec<(i32, i32, String)> {
        let mut out = Vec::new();
        for (&def, tree) in self.window_defs.iter_mut() {
            tree.walk_mut(&mut |w| {
                if let (Some(id), Some(el)) = (w.id, w.kind.as_element_mut())
                    && let Some(path) = el.wants_listing()
                {
                    out.push((def, id, path));
                }
            });
        }
        out
    }

    /// Hands widget `widget` of window `def` the listing of `asked`, answering
    /// whether it took it -- an element that moved on to another directory
    /// meanwhile does not, and the window has nothing new to draw.
    pub fn deliver_listing(
        &mut self,
        def: i32,
        widget: i32,
        asked: &str,
        listing: Result<Listing, String>,
    ) -> bool {
        self.window_def_mut(def)
            .and_then(|tree| tree.find_mut(widget))
            .and_then(|w| w.kind.as_element_mut())
            .is_some_and(|el| el.listed(asked, listing))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, dir: bool) -> DirEntry {
        DirEntry {
            name: name.into(),
            dir,
            size: 0,
        }
    }

    #[test]
    fn a_listing_puts_directories_first_and_leaves_hidden_entries_out() {
        let l = Listing::ordered(
            "/x".into(),
            vec![
                entry("b.wav", false),
                entry("Zeta", true),
                entry(".git", true),
                entry("a.wav", false),
                entry("alpha", true),
            ],
        );
        let names: Vec<&str> = l.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "Zeta", "a.wav", "b.wav"]);
    }

    #[test]
    fn a_path_walks_up_to_its_root_and_down_by_a_name() {
        assert_eq!(parent("/home/user").as_deref(), Some("/home"));
        assert_eq!(parent("/home").as_deref(), Some("/"));
        assert_eq!(parent("/"), None);
        assert_eq!(join("/", "a"), "/a");
        assert_eq!(join("/a", "b.wav"), "/a/b.wav");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn the_disk_is_listed_from_an_absolute_path() {
        let dir = std::env::temp_dir().join(format!("clausters-files-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("take.wav"), [0u8; 12]).unwrap();
        let l = list_dir(dir.to_str().unwrap()).unwrap();
        assert!(l.path.starts_with('/') || l.path.contains(':'));
        assert_eq!(l.entries[0], entry("sub", true));
        assert_eq!(l.entries[1].size, 12);
        assert!(list_dir(dir.join("nothing").to_str().unwrap()).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
