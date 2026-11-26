//! **The editor's file dialogs**: its Open, its Save as and the folder of
//! its documentation, file requests of the mechanism every application's
//! dialogs are ([`clausters_apps::forms`]).
//!
//! The code editor is an application like the others, so its files are
//! chosen the way theirs are: in the host's own file dialog, the
//! `file_dialog` element, over the platform's storage (the disk in a
//! window, the page's storage in a tab), read by [`forms::Dialogs`]. This
//! module says only what is the editor's: which requests it makes, the
//! kinds of file its language's sources are, and where their widgets are
//! numbered.

use clausters_apps::forms::{self, FileRequest, FileType, Ids, Spec};
use clausters_core::osc::OscType;
use serde_json::{Value, json};

/// Where the dialogs' ids start: below the editor's own, above every id a
/// client allocates or a script picks.
const BASE: i32 = (1 << 24) - 100_000;
/// Where their nodes with no role of their own are numbered from: past
/// every widget [`forms::names`] lists.
pub const SPARE: i32 = BASE + 100;
/// Past the dialogs' ids.
const END: i32 = BASE + 1000;

/// **One of the editor's file requests**, over the kinds of file its
/// language's sources are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// A file to open in a tab.
    Open(&'static [FileType]),
    /// Where a tab is written.
    SaveAs(&'static [FileType]),
    /// The folder the documentation is read from.
    Folder,
}

impl Spec for Form {
    fn prefix(self) -> &'static str {
        match self {
            Form::Open(_) => "open",
            Form::SaveAs(_) => "file",
            Form::Folder => "folder",
        }
    }

    fn page(self) -> i32 {
        match self {
            Form::Open(_) => 1,
            Form::SaveAs(_) => 2,
            Form::Folder => 3,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Form::Open(_) => "Open",
            Form::SaveAs(_) => "Save as",
            Form::Folder => "Documentation folder",
        }
    }

    fn fields(self) -> Vec<(&'static str, &'static str)> {
        Vec::new()
    }

    fn file(self) -> Option<FileRequest> {
        Some(match self {
            Form::Open(types) => FileRequest::open(types, false),
            Form::SaveAs(types) => FileRequest::save(types),
            Form::Folder => FileRequest::folder(),
        })
    }

    fn accept(self) -> &'static str {
        match self {
            Form::Open(_) => "Open",
            Form::SaveAs(_) => "Save",
            Form::Folder => "Choose",
        }
    }
}

/// **The forms the dialog place holds**, in the order of their pages, over
/// the kinds of file `types`.
#[must_use]
pub fn forms(types: &'static [FileType]) -> [Form; 3] {
    [Form::Open(types), Form::SaveAs(types), Form::Folder]
}

/// **Every widget of the dialogs, numbered** from the editor's range.
#[must_use]
pub fn ids() -> Ids {
    forms::names(&forms(&[])).into_iter().zip(BASE..).collect()
}

/// **The editor's dialogs**, nothing up, their widgets numbered.
#[must_use]
pub fn dialogs() -> forms::Dialogs<Form> {
    let mut dialogs = forms::Dialogs::default();
    dialogs.ids = ids();
    dialogs
}

/// Whether `id` is one of the dialogs'.
#[must_use]
pub fn owns(id: i32) -> bool {
    (BASE..END).contains(&id)
}

/// **An event's tag and what follows it, as the forms read them**: the
/// arguments as values, and a choice's index, which arrives as a number
/// alone, as its tag.
#[must_use]
pub fn report(tag: &str, args: &[OscType]) -> (String, Vec<Value>) {
    let value = |a: &OscType| match a {
        OscType::String(s) => json!(s),
        OscType::Int(v) => json!(v),
        OscType::Long(v) => json!(v),
        OscType::Float(v) => json!(v),
        OscType::Double(v) => json!(v),
        OscType::Bool(v) => json!(v),
        _ => Value::Null,
    };
    let values: Vec<Value> = args.iter().map(value).collect();
    match values.first() {
        Some(Value::Number(n)) if tag.is_empty() => (n.to_string(), values[1..].to_vec()),
        _ => (tag.to_string(), values),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{JAVASCRIPT, PYTHON};
    use clausters_apps::forms::Said;
    use std::collections::BTreeMap;

    /// Every widget is numbered in the editor's range, before its spare
    /// nodes.
    #[test]
    fn the_dialogs_are_numbered_in_the_editor_s_range() {
        let ids = ids();
        assert!(dialogs().numbered(&forms(PYTHON.types)));
        assert!(ids.values().all(|id| owns(*id) && *id < SPARE));
    }

    /// **Each request is the host's file dialog**, asking for the
    /// language's kinds; a save writes any name too, as an editor of text.
    #[test]
    fn each_request_is_the_host_s_file_dialog() {
        let d = dialogs();
        let stack = d.stack(&forms(JAVASCRIPT.types)).unwrap();
        let files = stack["children"][0]["children"].as_array().unwrap();
        assert_eq!(files.len(), 3);
        let of = |id: i32| files.iter().find(|f| f["id"] == id).unwrap();
        let save = of(d.ids["file"]);
        assert_eq!(save["type"], "file_dialog");
        assert_eq!(save["mode"], "save");
        assert_eq!(save["kinds"], json!([["JavaScript", ["mjs", "js"]]]));
        assert_eq!(save["all_files"], true);
        assert_eq!(of(d.ids["folder"])["mode"], "folder");
    }

    /// What the dialog chose is the form's `OK`, its path the one written.
    #[test]
    fn a_choice_is_the_path() {
        let mut d = dialogs();
        let start = BTreeMap::from([(forms::PATH.to_string(), "/sketches/a.mjs".to_string())]);
        let shown = d.open(Form::SaveAs(JAVASCRIPT.types), start);
        let (_, asked) = shown.iter().find(|(w, _)| *w == d.ids["file"]).unwrap();
        assert_eq!(asked["path"], "/sketches/a.mjs");
        assert_eq!(asked["open"], 1);
        let (tag, values) = report(
            "chosen",
            &[
                OscType::String("JavaScript".into()),
                OscType::String("/sketches/b.mjs".into()),
            ],
        );
        assert_eq!(d.read("file", &tag, &values), Some(Said::Accept));
        assert_eq!(d.up().unwrap().path().as_deref(), Some("/sketches/b.mjs"));
    }

    /// A report's tag stands, and a number alone is a choice's index.
    #[test]
    fn a_report_reads_as_the_forms_read_it() {
        assert_eq!(report("a.py", &[]), ("a.py".into(), vec![]));
        assert_eq!(
            report("path", &[OscType::String("/lib".into())]),
            ("path".into(), vec![json!("/lib")])
        );
        assert_eq!(report("", &[OscType::Int(2)]), ("2".into(), vec![]));
        assert_eq!(
            report("focus", &[OscType::Int(1)]),
            ("focus".into(), vec![json!(1)])
        );
    }
}
