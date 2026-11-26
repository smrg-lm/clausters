//! **The editor's file dialogs**: its Open and its Save as, two forms of
//! the mechanism every application's dialogs are ([`clausters_apps::forms`]).
//!
//! The code editor is an application like the others, so its files are
//! chosen the way theirs are: the host's `files` chooser over the platform's
//! storage (the disk in a window, the page's storage in a tab), the
//! storage's tools under it, the name it fills, the file type and the
//! buttons, read by [`forms::Dialogs`]. This module says only what is the
//! editor's: which forms it has, the kinds of file its language's sources
//! are, and where its widgets are numbered. The application shows the
//! dialogs in its dialog place and carries out what they answer.

use clausters_apps::forms::{self, Chooser, FileType, Ids, Spec};
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

/// **One of the editor's file forms**, over the kinds of file its
/// language's sources are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// A file to open in a tab.
    Open(&'static [FileType]),
    /// Where a tab is written.
    SaveAs(&'static [FileType]),
}

/// The field each form chooses its file in.
pub const PATH: &str = "path";

impl Spec for Form {
    fn prefix(self) -> &'static str {
        match self {
            Form::Open(_) => "open",
            Form::SaveAs(_) => "file",
        }
    }

    fn page(self) -> i32 {
        match self {
            Form::Open(_) => 1,
            Form::SaveAs(_) => 2,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Form::Open(_) => "Open",
            Form::SaveAs(_) => "Save as",
        }
    }

    fn fields(self) -> Vec<(&'static str, &'static str)> {
        vec![(PATH, ""), (forms::NAME, "Name")]
    }

    fn chooser(self, name: &str) -> Option<Chooser> {
        match (self, name) {
            (Form::Open(types), PATH) => Some(Chooser::open(types, false)),
            (Form::SaveAs(types), PATH) => Some(Chooser::save(types)),
            _ => None,
        }
    }

    fn accept(self) -> &'static str {
        match self {
            Form::Open(_) => "Open",
            Form::SaveAs(_) => "Save",
        }
    }
}

/// **The forms the dialog place holds**, in the order of their pages, over
/// the kinds of file `types`.
#[must_use]
pub fn forms(types: &'static [FileType]) -> [Form; 2] {
    [Form::Open(types), Form::SaveAs(types)]
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
    use clausters_apps::forms::{Dialogs, Said, TYPE};
    use std::collections::BTreeMap;

    /// Every widget is numbered in the editor's range, before its spare
    /// nodes.
    #[test]
    fn the_dialogs_are_numbered_in_the_editor_s_range() {
        let ids = ids();
        assert!(dialogs().numbered(&forms(PYTHON.types)));
        assert!(ids.values().all(|id| owns(*id) && *id < SPARE));
    }

    /// **A save writes the kind chosen, or any name**: a name with none of
    /// the language's extensions is given its first, and with every file
    /// chosen it is written as typed.
    #[test]
    fn a_save_writes_the_kind_chosen_or_any_name() {
        let mut d = dialogs();
        let form = Form::SaveAs(JAVASCRIPT.types);
        let start = BTreeMap::from([(PATH.to_string(), "/sketches/a.mjs".to_string())]);
        d.open(form, start);
        let open = d.up().unwrap();
        assert_eq!(open.value(PATH), "/sketches");
        assert_eq!(open.save_path(PATH).as_deref(), Some("/sketches/a.mjs"));
        let saved = |d: &mut Dialogs<Form>, name: &str| {
            d.read("file:name", name, &[]);
            d.up().unwrap().save_path(PATH)
        };
        assert_eq!(saved(&mut d, "c").as_deref(), Some("/sketches/c.mjs"));
        assert_eq!(saved(&mut d, "c.JS").as_deref(), Some("/sketches/c.JS"));
        assert_eq!(
            saved(&mut d, "c.txt").as_deref(),
            Some("/sketches/c.txt.mjs")
        );
        let (tag, values) = report("", &[OscType::Int(1)]);
        d.read("file:type", &tag, &values);
        assert_eq!(d.up().unwrap().value(TYPE), "All files (*.*)");
        assert_eq!(saved(&mut d, "c.txt").as_deref(), Some("/sketches/c.txt"));
        assert_eq!(
            saved(&mut d, "Makefile").as_deref(),
            Some("/sketches/Makefile")
        );
    }

    /// **An open lists the language's files, and every file on asking**,
    /// every file once.
    #[test]
    fn an_open_lists_its_kinds_and_every_file() {
        let mut d = dialogs();
        let shown = d.open(Form::Open(PYTHON.types), BTreeMap::new());
        assert!(shown.contains(&(
            d.ids["open:type"],
            json!({"options": ["Python (*.py, *.pyw)", "All files (*.*)"], "index": 0})
        )));
        let (tag, values) = report("", &[OscType::Float(1.0)]);
        assert_eq!(
            d.read("open:type", &tag, &values),
            Some(Said::Corrected(vec![(
                d.ids["open:path"],
                json!({"filter": []})
            )]))
        );
    }

    /// A save's kinds are the language's, every file last.
    #[test]
    fn a_save_offers_the_language_s_kinds_and_every_file() {
        let mut d = dialogs();
        let shown = d.open(Form::SaveAs(PYTHON.types), BTreeMap::new());
        assert!(shown.contains(&(
            d.ids["file:type"],
            json!({"options": ["Python (*.py, *.pyw)", "All files (*.*)"], "index": 0})
        )));
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
