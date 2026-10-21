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
//! Three things are kept apart here. **Listing** is a service at the
//! platform's edge: the disk adapter ([`list_dir`]) and the page's
//! (`host::web::files`) answer an [`Ask`] and know nothing of who asked.
//! **What a chooser is doing** -- the directory it shows, the ask it waits on,
//! what came back or why nothing did -- is a [`Chooser`], which draws nothing
//! and touches no filesystem. **What a chosen path means** is the
//! application's: the chooser reports paths, and reading or opening them is
//! done by whoever asked for the choice (the audio editor's Open form names
//! them for its client to read).
//!
//! Every ask carries a **ticket**, unique in the process, and an answer is
//! taken only by the ask it belongs to. So a listing that comes back after
//! the chooser went somewhere else -- the hand climbed up while a slow
//! directory was being read, the dialog was cancelled and opened again, the
//! window was drawn again -- replaces nothing, even when it names the same
//! directory as the ask now waiting.
//!
//! A **path** means what it means to every verb that takes one: a path on the
//! disk natively, a path in the page's storage in a tab, where `/` is the
//! storage's root (`docs/decisions.md`, "A path in a page is its own storage").
//!
//! [`Element::wants_listing`]: super::widget::Element::wants_listing

use std::sync::atomic::{AtomicU64, Ordering};

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

/// **A directory asked to be listed**: which, and the ticket its answer must
/// carry back ([`Chooser::listed`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub ticket: u64,
    pub path: String,
}

/// A ticket no ask has had yet: one counter for the process, so two choosers,
/// or one chooser built again by a redraw, never share one.
fn ticket() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// **Where a chooser's listing stands.**
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// A directory is to be asked for, and has not been yet.
    Asking,
    /// It was asked for, and the answer is not back.
    Waiting,
    /// The answer is in.
    Listed,
    /// It could not be listed, and this is why.
    Failed(String),
}

/// **What a file chooser is doing**, apart from how it is drawn: the
/// directory it shows, the extensions it lets through, the ask it waits on
/// and what the last answer held.
#[derive(Debug, Clone)]
pub struct Chooser {
    /// The directory shown -- as asked until its listing arrives, then as the
    /// listing resolved it.
    path: String,
    /// The extensions a file must have to be shown, lowercase and without
    /// the dot; empty shows every file.
    filter: Vec<String>,
    entries: Vec<DirEntry>,
    status: Status,
    /// The ticket of the ask for `path`; only its answer is taken.
    ticket: u64,
}

impl Chooser {
    /// A chooser over `path`, which it will ask for, showing the files whose
    /// extension is in `filter`.
    pub fn new(path: impl Into<String>, filter: Vec<String>) -> Self {
        Chooser {
            path: path.into(),
            filter: normal_filter(filter),
            entries: Vec::new(),
            status: Status::Asking,
            ticket: ticket(),
        }
    }

    /// The directory shown.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Where its listing stands.
    pub fn status(&self) -> &Status {
        &self.status
    }

    /// Lets through the files whose extension is in `filter`, from now on.
    pub fn set_filter(&mut self, filter: Vec<String>) {
        self.filter = normal_filter(filter);
    }

    /// **Goes to directory `to`**: what the last one held is gone, and `to` is
    /// asked for under a new ticket -- also when it is the directory shown,
    /// which is how a chooser opened again reads it afresh.
    pub fn go(&mut self, to: impl Into<String>) {
        self.path = to.into();
        self.entries.clear();
        self.status = Status::Asking;
        self.ticket = ticket();
    }

    /// **The directory to list**, once: asked, it waits for its answer.
    pub fn ask(&mut self) -> Option<Ask> {
        if self.status != Status::Asking {
            return None;
        }
        self.status = Status::Waiting;
        Some(Ask {
            ticket: self.ticket,
            path: self.path.clone(),
        })
    }

    /// **An answer**, taken only when it is the one the chooser waits on --
    /// whatever directory it names. Answers whether it was taken.
    pub fn listed(&mut self, ticket: u64, listing: Result<Listing, String>) -> bool {
        if self.status != Status::Waiting || ticket != self.ticket {
            return false;
        }
        match listing {
            Ok(Listing { path, entries }) => {
                self.path = path;
                self.entries = entries;
                self.status = Status::Listed;
            }
            Err(why) => {
                self.entries.clear();
                self.status = Status::Failed(why);
            }
        }
        true
    }

    /// The directory above the one shown, where there is one. A path still as
    /// asked (`.`) has none until its listing resolves it.
    pub fn up(&self) -> Option<String> {
        parent(&self.path)
    }

    /// The entries shown: every directory, and the files the filter lets
    /// through.
    pub fn shown(&self) -> Vec<&DirEntry> {
        self.entries
            .iter()
            .filter(|e| {
                e.dir
                    || self.filter.is_empty()
                    || e.name
                        .rsplit_once('.')
                        .is_some_and(|(_, ext)| self.filter.contains(&ext.to_lowercase()))
            })
            .collect()
    }

    /// What is drawn where no entry is: the wait, the reason, or that the
    /// directory holds nothing to show.
    pub fn placeholder(&self) -> Option<&str> {
        match &self.status {
            Status::Asking | Status::Waiting => Some("listing..."),
            Status::Failed(why) => Some(why),
            Status::Listed if self.shown().is_empty() => Some("(empty)"),
            Status::Listed => None,
        }
    }
}

/// Extensions as a filter compares them: lowercase, without the dot, and no
/// empty one.
fn normal_filter(filter: Vec<String>) -> Vec<String> {
    filter
        .into_iter()
        .map(|e| e.trim_start_matches('.').to_lowercase())
        .filter(|e| !e.is_empty())
        .collect()
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
    /// as `(window, widget, ask)` -- asked once each: the element clears its
    /// ask as it answers.
    pub fn pending_listings(&mut self) -> Vec<(i32, i32, Ask)> {
        let mut out = Vec::new();
        for (&def, tree) in self.window_defs.iter_mut() {
            tree.walk_mut(&mut |w| {
                if let (Some(id), Some(el)) = (w.id, w.kind.as_element_mut())
                    && let Some(ask) = el.wants_listing()
                {
                    out.push((def, id, ask));
                }
            });
        }
        out
    }

    /// Hands widget `widget` of window `def` the answer to the ask `ticket`,
    /// answering whether it took it -- an element that asked again since, or
    /// was freed or drawn again meanwhile, does not, and the window has
    /// nothing new to draw.
    pub fn deliver_listing(
        &mut self,
        def: i32,
        widget: i32,
        ticket: u64,
        listing: Result<Listing, String>,
    ) -> bool {
        self.window_def_mut(def)
            .and_then(|tree| tree.find_mut(widget))
            .and_then(|w| w.kind.as_element_mut())
            .is_some_and(|el| el.listed(ticket, listing))
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

    fn takes() -> Listing {
        Listing::ordered(
            "/takes".into(),
            vec![
                entry("old", true),
                entry("kick.wav", false),
                entry("notes.txt", false),
            ],
        )
    }

    #[test]
    fn a_chooser_asks_once_and_takes_only_the_answer_to_its_ask() {
        let mut c = Chooser::new("takes", vec![".WAV".into()]);
        assert_eq!(c.placeholder(), Some("listing..."));
        let ask = c.ask().expect("asked");
        assert_eq!(ask.path, "takes");
        assert_eq!(c.ask(), None, "asked once");
        assert!(
            !c.listed(ask.ticket + 1, Ok(takes())),
            "another ask's answer"
        );
        assert_eq!(c.status(), &Status::Waiting);
        assert!(c.listed(ask.ticket, Ok(takes())));
        assert_eq!(c.path(), "/takes", "as the listing resolved it");
        let names: Vec<&str> = c.shown().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["old", "kick.wav"],
            "the filter lets wav through"
        );
        assert!(!c.listed(ask.ticket, Ok(takes())), "taken once");
    }

    /// **Navigation while loading**: the hand leaves a directory before its
    /// listing is back, and comes back to it; the first answer is stale even
    /// though it names the directory now asked for.
    #[test]
    fn a_listing_that_comes_back_late_replaces_nothing() {
        let mut c = Chooser::new("/takes", Vec::new());
        let first = c.ask().unwrap();
        c.go(c.up().unwrap());
        let up = c.ask().unwrap();
        c.go("/takes");
        let again = c.ask().unwrap();
        assert_eq!(first.path, again.path);
        assert!(
            !c.listed(first.ticket, Ok(takes())),
            "late, for the same path"
        );
        assert!(!c.listed(up.ticket, Ok(Listing::ordered("/".into(), Vec::new()))));
        assert_eq!(c.status(), &Status::Waiting);
        assert!(c.listed(again.ticket, Ok(takes())));
        assert_eq!(c.shown().len(), 3);
    }

    #[test]
    fn a_chooser_that_cannot_list_says_why_and_keeps_its_way_up() {
        let mut c = Chooser::new("/nowhere/deep", Vec::new());
        let ask = c.ask().unwrap();
        assert!(c.listed(ask.ticket, Err("/nowhere/deep: not found".into())));
        assert_eq!(c.placeholder(), Some("/nowhere/deep: not found"));
        assert_eq!(c.up().as_deref(), Some("/nowhere"));
        c.go("/takes");
        assert_eq!(
            c.placeholder(),
            Some("listing..."),
            "the error went with it"
        );
    }

    #[test]
    fn going_to_the_directory_shown_reads_it_again() {
        let mut c = Chooser::new("/takes", Vec::new());
        let ask = c.ask().unwrap();
        c.listed(ask.ticket, Ok(takes()));
        c.go("/takes");
        assert!(c.shown().is_empty(), "what it held is gone");
        let fresh = c.ask().unwrap();
        assert_ne!(fresh.ticket, ask.ticket);
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
