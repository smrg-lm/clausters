//! **The editor's file dialog**, over the platform's storage: what it opens
//! and saves with, natively and in a page alike.
//!
//! It is the protocol's file dialog: the host's `files` chooser, which
//! lists the disk in a window and the page's storage in a tab, a field
//! bound to what it selects, and the buttons. Since it is where the
//! storage's files are seen, the storage's tools are here too -- a new
//! folder, a rename, a delete, and in a page an upload and a download --
//! every application's ([`clausters_apps::forms::storage`]), which the
//! chooser carries out (its `change`) and answers (`"changed"`).
//!
//! This is the dialog's state and its tree; the application shows it in its
//! dialog place and hands it what the hand did.

use std::path::{Path, PathBuf};

use clausters_apps::forms::storage::{self, Storage, Tool};
use serde_json::{Value, json};

use crate::platform::Purpose;

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
const SAYS: i32 = BASE + 10;
/// Where its nodes with no role of their own are numbered from.
pub const SPARE: i32 = BASE + 20;
/// Past the dialog's ids.
const END: i32 = BASE + 100;

/// The id of a tool's button.
fn tool_id(tool: Tool) -> i32 {
    match tool {
        Tool::Upload => UPLOAD,
        Tool::Download => DOWNLOAD,
        Tool::NewFolder => NEW_FOLDER,
        Tool::Rename => RENAME,
        Tool::Delete => DELETE,
    }
}

/// What the hand did in the dialog, for the application to carry out.
#[derive(Debug, PartialEq)]
pub enum Does {
    /// Nothing the application has to do.
    Nothing,
    /// The dialog is drawn again as it is now.
    Redraw,
    /// Widgets of the dialog set, the dialog standing as it is: the chooser
    /// asked for a change of the storage or sent to a folder, the name field
    /// cleared, the line the tools speak in. Drawing the dialog again would
    /// build its chooser afresh, and with it the change it was asked for.
    Set(Vec<(i32, Value)>),
    /// The path was chosen, or nothing (`None`): the dialog goes.
    Chosen(Option<PathBuf>),
}

/// **The dialog that is up**: what it was opened for, the folder its
/// chooser shows, the file it selects, the path typed, and its tools.
pub struct FileDialog {
    pub purpose: Purpose,
    dir: PathBuf,
    selected: Option<PathBuf>,
    typed: String,
    storage: Storage,
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
            storage: Storage::default(),
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
            // what is selected, reported whether or not the field is bound
            (FILES, "select") => {
                self.selected = (!text.is_empty()).then(|| PathBuf::from(&text));
                if !text.is_empty() {
                    self.typed = text;
                }
                Does::Nothing
            }
            (FILES, "pick") => Does::Chosen(Some(PathBuf::from(text))),
            // the chooser carried a change out: what went wrong, if anything
            (FILES, "changed") => {
                self.storage.changed(&text);
                self.selected = None;
                Does::Set(vec![(SAYS, storage::says_props(&self.storage.says))])
            }
            (DIALOG, "cancel") | (CANCEL, "click") => Does::Chosen(None),
            (ACCEPT, "click") => match self.path() {
                Some(path) => Does::Chosen(Some(path)),
                None => {
                    self.storage.says = "Type a file's name first.".into();
                    Does::Set(vec![(SAYS, storage::says_props(&self.storage.says))])
                }
            },
            (id, "click") => match Tool::ALL.into_iter().find(|t| tool_id(*t) == id) {
                Some(tool) => self.tool(tool),
                None => Does::Nothing,
            },
            _ => Does::Nothing,
        }
    }

    /// **A tool pressed**: the change the chooser is asked for, or what the
    /// tools say instead.
    fn tool(&mut self, tool: Tool) -> Does {
        let text = |p: &Path| p.to_string_lossy().into_owned();
        let (dir, selected, typed) = (
            text(&self.dir),
            self.selected.as_deref().map(text),
            self.path().as_deref().map(text),
        );
        let pressed = self
            .storage
            .press(tool, &dir, selected.as_deref(), typed.as_deref());
        let mut set = Vec::new();
        if let Some(to) = pressed.go {
            self.dir = PathBuf::from(&to);
            set.push((FILES, json!({"path": to})));
        }
        if let Some(change) = pressed.change {
            self.selected = None;
            set.push((FILES, json!({"change": change})));
        }
        if pressed.clear_name {
            self.typed.clear();
            set.push((NAME, json!({"value": ""})));
        }
        set.push((SAYS, storage::says_props(&self.storage.says)));
        Does::Set(set)
    }

    /// **The dialog's tree**, as it is now: every application's file dialog
    /// ([`clausters_apps::forms`]) -- the chooser, the storage's tools under
    /// it, the name it fills, the line the tools speak in, the buttons.
    pub fn tree(&self) -> Value {
        use clausters_apps::forms::{Chooser, chooser_widget, field_row, footer};
        let (title, accept) = match self.purpose {
            Purpose::SaveAs { .. } => ("Save as", "Save"),
            Purpose::Open | Purpose::DocsFolder => ("Open", "Open"),
        };
        // a file to open is chosen or typed alike, so the chooser fills the
        // field either way
        let chooser = Chooser::save(&[]);
        let files = chooser_widget(
            Some(FILES),
            &self.dir.to_string_lossy(),
            &chooser,
            Some(NAME),
        );
        let name = json!({"id": NAME, "type": "text", "value": self.typed});
        json!({
            "id": DIALOG, "type": "layout", "modal": 1, "hug": 1, "title": title, "flow": "col",
            "children": [
                field_row("", None, files, Some(chooser.w)),
                storage::tools_row(|tool| Some(tool_id(tool))),
                field_row("Name", None, name, None),
                storage::says_line(Some(SAYS), &self.storage.says),
                footer(Some(CANCEL), Some(ACCEPT), accept),
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
        assert_eq!(d.tree()["children"][0]["children"][0]["path"], "/sketches");
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

    /// **The tools are every dialog's**: a delete asks once and then asks
    /// the chooser to remove the file selected, a rename moves it to the
    /// path typed, and the chooser's answer is said.
    #[test]
    fn the_tools_ask_the_chooser_and_say_its_answer() {
        let says = |text: &str| (SAYS, storage::says_props(text));
        let mut d = FileDialog::new(Purpose::Open, None);
        d.on(FILES, "", "/a.mjs".into());
        let Does::Set(asked) = d.on(DELETE, "click", String::new()) else {
            panic!("a delete asks");
        };
        assert!(asked.iter().all(|(id, _)| *id != FILES), "not yet");
        assert_eq!(
            d.on(DELETE, "click", String::new()),
            Does::Set(vec![
                (FILES, json!({"change": {"op": "remove", "path": "/a.mjs"}})),
                says(""),
            ])
        );
        d.on(FILES, "", "/a.mjs".into());
        d.on(NAME, "", "b.mjs".into());
        assert_eq!(
            d.on(RENAME, "click", String::new()),
            Does::Set(vec![
                (
                    FILES,
                    json!({"change": {"op": "rename", "path": "/a.mjs", "to": "/b.mjs"}})
                ),
                says(""),
            ])
        );
        assert_eq!(
            d.on(FILES, "changed", "/b.mjs: taken".into()),
            Does::Set(vec![says("/b.mjs: taken")])
        );
        assert_eq!(d.tree()["children"][3]["text"], "/b.mjs: taken");
        // the folder shown, with nothing selected, and never the root
        d.on(DELETE, "click", String::new());
        assert!(
            d.tree()["children"][3]["text"]
                .as_str()
                .unwrap()
                .contains("root")
        );
    }
}
