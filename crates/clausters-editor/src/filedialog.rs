//! **The editor's own file dialog**, over the platform's storage: what a
//! platform with no dialog of its own (a page, whose files are its storage)
//! opens and saves with.
//!
//! It is the protocol's file dialog: the host's `files` chooser, which
//! lists the disk in a window and the page's storage in a tab, a field
//! bound to what it selects, and the buttons. Since it is where the
//! storage's files are seen, a new folder, a rename, a delete, an upload
//! from the disk and a download to it are here too, which the platform
//! carries out ([`Platform::change`]).
//!
//! This is the dialog's state and its tree; the application shows it in its
//! dialog place and hands it what the hand did.
//!
//! [`Platform::change`]: crate::platform::Platform::change

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::platform::{Change, Purpose};

/// Where the dialog's ids start: below the editor's own, above every id a
/// client allocates or a script picks.
const BASE: i32 = (1 << 24) - 100_000;
const DIALOG: i32 = BASE;
const FILES: i32 = BASE + 1;
const NAME: i32 = BASE + 2;
const UPLOAD: i32 = BASE + 3;
const NEW_FOLDER: i32 = BASE + 4;
const RENAME: i32 = BASE + 5;
const DOWNLOAD: i32 = BASE + 6;
const DELETE: i32 = BASE + 7;
const CANCEL: i32 = BASE + 8;
const ACCEPT: i32 = BASE + 9;
/// Where its nodes with no role of their own are numbered from.
pub const SPARE: i32 = BASE + 20;
/// Past the dialog's ids.
const END: i32 = BASE + 100;

/// The chooser's size, in logical pixels.
const W: f64 = 520.0;
const H: f64 = 300.0;

/// What the hand did in the dialog, for the application to carry out.
#[derive(Debug, PartialEq)]
pub enum Does {
    /// Nothing the application has to do.
    Nothing,
    /// The dialog is drawn again as it is now.
    Redraw,
    /// A change to the storage, after which the folder is listed again.
    Change(Change),
    /// The path was chosen, or nothing (`None`): the dialog goes.
    Chosen(Option<PathBuf>),
}

/// **The dialog that is up**: what it was opened for, the folder its
/// chooser shows, the file it selects, the path typed, and what it last
/// has to say.
pub struct FileDialog {
    pub purpose: Purpose,
    dir: PathBuf,
    selected: Option<PathBuf>,
    typed: String,
    says: String,
    /// A delete asked once, which a second asks to go on with.
    armed: Option<PathBuf>,
}

impl FileDialog {
    /// The dialog for `purpose`, in the folder of `start` with its name
    /// typed when it names a file, or at the storage's root.
    pub fn new(purpose: Purpose, start: Option<&Path>) -> Self {
        let file = start.filter(|p| p.extension().is_some());
        let dir = match (file, start) {
            (Some(f), _) => f.parent().map(Path::to_path_buf),
            (None, dir) => dir.map(Path::to_path_buf),
        }
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("/"));
        Self {
            purpose,
            dir,
            selected: None,
            typed: file
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default(),
            says: String::new(),
            armed: None,
        }
    }

    /// Whether widget `id` reports a value of text: the field, and the
    /// chooser's file selected.
    pub fn valued(id: i32) -> bool {
        id == NAME || id == FILES
    }

    /// Whether `id` is one of the dialog's.
    pub fn owns(id: i32) -> bool {
        (BASE..END).contains(&id)
    }

    /// A change that did not happen says why; one that did says nothing.
    pub fn changed(&mut self, done: Result<(), String>) {
        self.says = done.err().unwrap_or_default();
    }

    fn saving(&self) -> bool {
        matches!(self.purpose, Purpose::SaveAs { .. })
    }

    /// The path the field names: from the root when it starts with one,
    /// else in the folder shown.
    fn path(&self) -> Option<PathBuf> {
        let typed = self.typed.trim();
        if typed.is_empty() {
            return None;
        }
        Some(if typed.starts_with('/') {
            PathBuf::from(typed)
        } else {
            self.dir.join(typed)
        })
    }

    /// The file selected, or the folder shown with none.
    fn target(&self) -> PathBuf {
        self.selected.clone().unwrap_or_else(|| self.dir.clone())
    }

    /// **What an event in the dialog does**: its tag, and its first
    /// argument as text -- or, for a value (a field's text, the file the
    /// chooser selects), no tag and the value.
    pub fn on(&mut self, id: i32, tag: &str, text: String) -> Does {
        match (id, tag) {
            (NAME, "") => {
                self.typed = text;
                Does::Nothing
            }
            // the chooser's value: the file selected
            (FILES, "") => {
                self.selected = (!text.is_empty()).then(|| PathBuf::from(&text));
                self.typed = text;
                Does::Nothing
            }
            (FILES, "path") => {
                self.dir = PathBuf::from(text);
                self.selected = None;
                Does::Nothing
            }
            (FILES, "pick") => Does::Chosen(Some(PathBuf::from(text))),
            (DIALOG, "cancel") | (CANCEL, "click") => Does::Chosen(None),
            (ACCEPT, "click") => match self.path() {
                Some(path) => Does::Chosen(Some(path)),
                None => self.say("Type a file's name first."),
            },
            (UPLOAD, "click") => Does::Change(Change::Upload(self.dir.clone())),
            (NEW_FOLDER, "click") => match self.path() {
                Some(path) => {
                    self.typed.clear();
                    Does::Change(Change::MakeFolder(path))
                }
                None => self.say("Type the new folder's name first."),
            },
            (RENAME, "click") => match (self.selected.clone(), self.path()) {
                (Some(from), Some(to)) if from != to => {
                    self.selected = None;
                    Does::Change(Change::Rename(from, to))
                }
                _ => self.say("Select a file, then type its new name."),
            },
            (DOWNLOAD, "click") => match self.selected.clone() {
                Some(path) => Does::Change(Change::Download(path)),
                None => self.say("Select the file to download."),
            },
            (DELETE, "click") => self.delete(),
            _ => Does::Nothing,
        }
    }

    fn say(&mut self, says: &str) -> Does {
        self.says = says.into();
        Does::Redraw
    }

    /// **A delete asks once**: the file selected, or the folder shown and
    /// everything in it; its second press goes on.
    fn delete(&mut self) -> Does {
        let target = self.target();
        if self.armed.take().as_ref() == Some(&target) {
            if self.selected.take().is_none() {
                self.dir = self
                    .dir
                    .parent()
                    .map_or_else(|| "/".into(), Path::to_path_buf);
            }
            return Does::Change(Change::Remove(target));
        }
        if target.parent().is_none() {
            return self.say("The storage's root is not deleted.");
        }
        let what = if self.selected.is_some() {
            ""
        } else {
            " and everything in it"
        };
        self.says = format!("Delete {}{what}? Delete again to go on.", target.display());
        self.armed = Some(target);
        Does::Redraw
    }

    /// **The dialog's tree**, as it is now.
    pub fn tree(&self) -> Value {
        let (title, accept) = if self.saving() {
            ("Save", "Save")
        } else {
            ("Open", "Open")
        };
        let tool = |id: i32, label: &str| json!({"id": id, "type": "button", "label": label, "variant": "ghost", "size": "sm"});
        json!({
            "id": DIALOG, "type": "layout", "modal": 1, "hug": 1, "title": title, "flow": "col",
            "children": [
                {"id": FILES, "type": "files", "path": self.dir.to_string_lossy(),
                    "w": W, "h": H, "bind": ["widget", NAME, "value"]},
                {"type": "layout", "flow": "row", "pack": 1, "hug": 1, "margin": 0, "gap": 4,
                    "children": [
                        tool(UPLOAD, "Upload..."),
                        tool(DOWNLOAD, "Download"),
                        tool(NEW_FOLDER, "New folder"),
                        tool(RENAME, "Rename"),
                        tool(DELETE, "Delete"),
                    ]},
                {"id": NAME, "type": "text", "value": self.typed},
                {"type": "label", "text": self.says, "wrap": 1, "muted": 1,
                    "shown": !self.says.is_empty()},
                {"type": "layout", "flow": "row", "pack": 1, "hug": 1, "margin": 0, "gap": 8,
                    "children": [
                        {"type": "separator", "weight": 1, "line": 0},
                        {"id": CANCEL, "type": "button", "label": "Cancel", "variant": "outline"},
                        {"id": ACCEPT, "type": "button", "label": accept, "default": 1,
                            "variant": "default"},
                    ]},
            ]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A save starts in its file's folder with its path typed, and the path
    /// typed is where it saves, from the folder shown when it is a name.
    #[test]
    fn a_save_is_where_its_field_says() {
        let mut d = FileDialog::new(
            Purpose::SaveAs { win: 1 },
            Some(Path::new("/sketches/a.mjs")),
        );
        assert_eq!(d.tree()["children"][0]["path"], "/sketches");
        assert_eq!(
            d.on(ACCEPT, "click", String::new()),
            Does::Chosen(Some("/sketches/a.mjs".into()))
        );
        d.on(NAME, "", "b.mjs".into());
        assert_eq!(
            d.on(ACCEPT, "click", String::new()),
            Does::Chosen(Some("/sketches/b.mjs".into()))
        );
    }

    /// What the chooser selects is the field's, and a pick is chosen.
    #[test]
    fn the_chooser_chooses() {
        let mut d = FileDialog::new(Purpose::Open, None);
        d.on(FILES, "path", "/lib".into());
        d.on(FILES, "", "/lib/a.mjs".into());
        assert_eq!(
            d.on(ACCEPT, "click", String::new()),
            Does::Chosen(Some("/lib/a.mjs".into()))
        );
        assert_eq!(
            d.on(FILES, "pick", "/lib/b.mjs".into()),
            Does::Chosen(Some("/lib/b.mjs".into()))
        );
    }

    /// A delete asks once before it goes on, and a rename moves the file
    /// selected to the path typed.
    #[test]
    fn a_delete_asks_and_a_rename_moves() {
        let mut d = FileDialog::new(Purpose::Open, None);
        d.on(FILES, "", "/a.mjs".into());
        assert_eq!(d.on(DELETE, "click", String::new()), Does::Redraw);
        assert_eq!(
            d.on(DELETE, "click", String::new()),
            Does::Change(Change::Remove("/a.mjs".into()))
        );
        d.on(FILES, "", "/a.mjs".into());
        d.on(NAME, "", "b.mjs".into());
        assert_eq!(
            d.on(RENAME, "click", String::new()),
            Does::Change(Change::Rename("/a.mjs".into(), "/b.mjs".into()))
        );
        // the folder shown, with nothing selected, and never the root
        assert_eq!(d.on(DELETE, "click", String::new()), Does::Redraw);
        assert!(d.says.contains("root"));
    }
}
