//! **The storage a file dialog shows, managed from it**: a new folder, a
//! rename, a delete, and where the storage is a page's own, the copies to
//! and from the disk (an upload, a download).
//!
//! Every file dialog has them, an application's Open and Save As and the
//! code editor's alike, since the dialog is where the storage's files are
//! seen -- and in a page, where nothing else shows that storage, it is the
//! one place to free its room. What a tool does is decided here, once: which
//! path it acts on (the file selected, the folder shown, the name typed),
//! and that a delete asks before it goes on. The change itself is the host's
//! file chooser's to carry out (its `change` prop), natively on the disk and
//! in a page on its storage, which then says how it went (`"changed"`) and
//! lists its folder again.

use std::path::Path;

use serde_json::{Value, json};

/// **One tool of a file dialog.**
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Upload,
    Download,
    NewFolder,
    Rename,
    Delete,
}

impl Tool {
    /// Every tool, in the order the dialog shows them.
    pub const ALL: [Tool; 5] = [
        Tool::Upload,
        Tool::Download,
        Tool::NewFolder,
        Tool::Rename,
        Tool::Delete,
    ];

    /// The name its widget is numbered under, after the form's prefix.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Tool::Upload => "upload",
            Tool::Download => "download",
            Tool::NewFolder => "new_folder",
            Tool::Rename => "rename",
            Tool::Delete => "delete",
        }
    }

    /// What its button says.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Tool::Upload => "Upload...",
            Tool::Download => "Download",
            Tool::NewFolder => "New folder",
            Tool::Rename => "Rename",
            Tool::Delete => "Delete",
        }
    }

    /// **Whether it is shown**: a copy to or from the disk only where the
    /// storage is not the disk, which is a page's.
    #[must_use]
    pub fn shown(self) -> bool {
        !matches!(self, Tool::Upload | Tool::Download) || !STORAGE_IS_THE_DISK
    }

    /// The tool a widget's name names, after the form's prefix.
    #[must_use]
    pub fn named(name: &str) -> Option<Tool> {
        Tool::ALL.into_iter().find(|tool| tool.name() == name)
    }
}

/// Whether the storage a dialog shows is the disk: a program's is, a page's
/// is its own.
pub const STORAGE_IS_THE_DISK: bool = !cfg!(target_arch = "wasm32");

/// The name the line a dialog's tools speak in is numbered under, after the
/// form's prefix.
pub const SAYS: &str = "says";

/// **The row of a dialog's tools**, each numbered by `id`, the ones the
/// storage has no use for left out.
#[must_use]
pub fn tools_row(id: impl Fn(Tool) -> Option<i32>) -> Value {
    let tools: Vec<Value> = Tool::ALL
        .into_iter()
        .filter(|tool| tool.shown())
        .map(|tool| {
            json!({"id": id(tool), "type": "button", "label": tool.label(),
                "variant": "ghost", "size": "sm"})
        })
        .collect();
    json!({"type": "layout", "flow": "row", "pack": 1, "hug": 1, "margin": 0, "gap": 4,
        "children": tools})
}

/// **The line the tools speak in**, numbered `id`: hidden while it has
/// nothing to say.
#[must_use]
pub fn says_line(id: Option<i32>, says: &str) -> Value {
    json!({"type": "label", "id": id, "text": says, "wrap": 1, "muted": 1,
        "shown": !says.is_empty()})
}

/// What the line is corrected with to say `says`.
#[must_use]
pub fn says_props(says: &str) -> Value {
    json!({"text": says, "shown": !says.is_empty()})
}

/// **What a tool pressed does.**
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pressed {
    /// The change the chooser is asked for (its `change` prop), if any.
    pub change: Option<Value>,
    /// The folder the chooser goes to first: the one above a folder deleted.
    pub go: Option<String>,
    /// Whether the name typed was used up (a new folder's), and is cleared.
    pub clear_name: bool,
}

/// **The tools of one dialog**: what they last had to say, and a delete
/// asked once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Storage {
    /// A delete asked once, which a second press goes on with.
    armed: Option<String>,
    /// What the tools last had to say: why they did nothing, a question, or
    /// why the storage refused.
    pub says: String,
}

impl Storage {
    /// **`tool` pressed**, over the folder `dir` shown, the file
    /// `selected` and the path the name field `typed` names.
    pub fn press(
        &mut self,
        tool: Tool,
        dir: &str,
        selected: Option<&str>,
        typed: Option<&str>,
    ) -> Pressed {
        self.says.clear();
        if tool != Tool::Delete {
            self.armed = None;
        }
        let change = |op: &str, path: &str| Some(json!({"op": op, "path": path}));
        match tool {
            Tool::Upload => Pressed {
                change: change("upload", dir),
                ..Pressed::default()
            },
            Tool::Download => match selected {
                Some(path) => Pressed {
                    change: change("download", path),
                    ..Pressed::default()
                },
                None => self.say("Select the file to download."),
            },
            Tool::NewFolder => match typed.filter(|t| Some(*t) != selected) {
                Some(path) => Pressed {
                    change: change("make_folder", path),
                    clear_name: true,
                    ..Pressed::default()
                },
                None => self.say("Type the new folder's name first."),
            },
            Tool::Rename => match (selected, typed) {
                (Some(from), Some(to)) if from != to => Pressed {
                    change: Some(json!({"op": "rename", "path": from, "to": to})),
                    ..Pressed::default()
                },
                _ => self.say("Select a file, then type its new name."),
            },
            Tool::Delete => self.delete(dir, selected),
        }
    }

    fn say(&mut self, says: &str) -> Pressed {
        self.says = says.into();
        Pressed::default()
    }

    /// **A delete asks once**: the file selected, or the folder shown and
    /// everything in it; its second press goes on, and a folder deleted
    /// leaves the chooser in the one above it.
    fn delete(&mut self, dir: &str, selected: Option<&str>) -> Pressed {
        let target = selected.unwrap_or(dir).to_string();
        let parent = Path::new(&target)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .filter(|p| !p.is_empty());
        if self.armed.take().as_ref() == Some(&target) {
            return Pressed {
                change: Some(json!({"op": "remove", "path": target})),
                go: parent.filter(|_| selected.is_none()),
                ..Pressed::default()
            };
        }
        if parent.is_none() || target == "." {
            return self.say("The storage's root is not deleted.");
        }
        let what = if selected.is_some() {
            ""
        } else {
            " and everything in it"
        };
        self.says = format!("Delete {target}{what}? Delete again to go on.");
        self.armed = Some(target);
        Pressed::default()
    }

    /// **The chooser said how a change went**: why it was refused, or
    /// nothing.
    pub fn changed(&mut self, why: &str) {
        self.armed = None;
        self.says = why.to_string();
    }

    /// The dialog opened again: nothing is said and nothing is armed.
    pub fn reset(&mut self) {
        *self = Storage::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A delete asks once before it goes on, and the root is never asked.
    #[test]
    fn a_delete_asks_once_and_never_the_root() {
        let mut s = Storage::default();
        assert_eq!(
            s.press(Tool::Delete, "/a", Some("/a/b.mid"), None),
            Pressed::default()
        );
        assert!(s.says.contains("Delete /a/b.mid?"));
        let gone = s.press(Tool::Delete, "/a", Some("/a/b.mid"), None);
        assert_eq!(
            gone.change,
            Some(json!({"op": "remove", "path": "/a/b.mid"}))
        );
        assert_eq!(gone.go, None);
        // the folder shown, with nothing selected, and the chooser goes up
        s.press(Tool::Delete, "/a", None, None);
        assert!(s.says.contains("and everything in it"));
        let gone = s.press(Tool::Delete, "/a", None, None);
        assert_eq!(gone.change, Some(json!({"op": "remove", "path": "/a"})));
        assert_eq!(gone.go.as_deref(), Some("/"));
        assert_eq!(s.press(Tool::Delete, "/", None, None), Pressed::default());
        assert!(s.says.contains("root"));
    }

    /// A rename moves the file selected to the path typed, and a new folder
    /// takes the name typed, which is then cleared.
    #[test]
    fn a_rename_and_a_new_folder_take_the_name_typed() {
        let mut s = Storage::default();
        let renamed = s.press(Tool::Rename, "/a", Some("/a/b.mid"), Some("/a/c.mid"));
        assert_eq!(
            renamed.change,
            Some(json!({"op": "rename", "path": "/a/b.mid", "to": "/a/c.mid"}))
        );
        // the name still the file's is no new name
        let same = s.press(Tool::Rename, "/a", Some("/a/b.mid"), Some("/a/b.mid"));
        assert_eq!(same.change, None);
        assert!(!s.says.is_empty());
        let made = s.press(Tool::NewFolder, "/a", None, Some("/a/takes"));
        assert_eq!(
            made.change,
            Some(json!({"op": "make_folder", "path": "/a/takes"}))
        );
        assert!(made.clear_name);
        assert_eq!(s.press(Tool::NewFolder, "/a", None, None).change, None);
    }

    /// An upload goes into the folder shown, a download takes the file
    /// selected, and the two are a page's alone.
    #[test]
    fn the_copies_are_a_page_s() {
        let mut s = Storage::default();
        assert_eq!(
            s.press(Tool::Upload, "/a", None, None).change,
            Some(json!({"op": "upload", "path": "/a"}))
        );
        assert_eq!(s.press(Tool::Download, "/a", None, None).change, None);
        assert_eq!(
            s.press(Tool::Download, "/a", Some("/a/b.mid"), None).change,
            Some(json!({"op": "download", "path": "/a/b.mid"}))
        );
        assert_eq!(Tool::Upload.shown(), !STORAGE_IS_THE_DISK);
        assert!(Tool::Delete.shown());
    }
}
