//! **Quitting an application: one act, asked once over all its unsaved
//! work.**
//!
//! A window is closed by the rule of [`crate::closing`], each with its own
//! form. Quitting is not a run of closes: the application's window (a dock's,
//! or a host's last) goes with everything in it, so what would be lost is
//! everything unsaved at once, and it is asked about **once**: one question
//! naming every document, answered Don't save, Cancel or Save all.
//!
//! - **Only the work's only holder is asked about.** A document whose window
//!   a client holds loses nothing by a quit (the client keeps it), so only
//!   windows that say they ask before closing
//!   ([`Converse::asks_to_close`](crate::turn::Converse::asks_to_close)) and
//!   hold work their file does not are gathered ([`Editing::unsaved_work`]).
//! - **Save all goes through each document's own Save** and the application
//!   ends once nothing is left to lose. A document with no file yet asks where
//!   in its own window, and the quit waits for it.
//! - **Cancel keeps everything**, the windows and the work in them.
//!
//! Whoever holds the application's other documents (an editor's own files,
//! beside the host's applications) gathers them into the same question with
//! [`says`] and asks in [`dialog`].
//!
//! [`Editing::unsaved_work`]: crate::editing::Editing::unsaved_work

use serde_json::Value;

use crate::closing;

/// **One document a quit would lose work in**: its window and the title it
/// is named by in the question, without the unsaved mark.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unsaved {
    /// The window that holds it.
    pub window: i32,
    /// What it is called.
    pub title: String,
}

/// **What the question asks**: its title.
pub const TITLE: &str = "Save changes before quitting?";

/// **What the question says of the work**: the documents by name, each in
/// quotes, and that their changes are not saved. Empty for none.
#[must_use]
pub fn says<S: AsRef<str>>(titles: &[S]) -> String {
    let quoted: Vec<String> = titles
        .iter()
        .map(|t| format!("\"{}\"", t.as_ref()))
        .collect();
    match quoted.len() {
        0 => String::new(),
        1 => format!("{} has changes that are not saved.", quoted[0]),
        _ => format!("{} have changes that are not saved.", quoted.join(", ")),
    }
}

/// **The question, as an alert dialog**: [`closing::asking`] titled
/// [`TITLE`], saying `says`, its saving button `Save` for one document and
/// `Save all` for several. `id` numbers a button by its name
/// ([`closing::SAVE`], [`closing::DISCARD`], [`closing::CANCEL`]), and the
/// answer is read as the close form's ([`closing::choice`]).
#[must_use]
pub fn dialog(
    dialog_id: Option<i32>,
    says: &str,
    documents: usize,
    id: impl Fn(&str) -> Option<i32>,
) -> Value {
    let save = if documents > 1 { "Save all" } else { "Save" };
    closing::asking(dialog_id, TITLE, says, save, id)
}

/// **The question held as a window holds its close form**
/// ([`closing::stack`]), numbered with the same ids, so the window that
/// asks to close asks to quit in the same place.
#[must_use]
pub fn stack(ids: &closing::Ids, says: &str, documents: usize) -> Value {
    let form = dialog(Some(ids.dialog), says, documents, |name| ids.button(name));
    closing::holding(ids, form)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One document is named alone; several are listed, in their order.
    #[test]
    fn the_question_names_every_document() {
        assert_eq!(says::<&str>(&[]), "");
        assert_eq!(says(&["a.py"]), "\"a.py\" has changes that are not saved.");
        assert_eq!(
            says(&["a.py", "Session"]),
            "\"a.py\", \"Session\" have changes that are not saved."
        );
    }

    /// The question is the close form's dialog, titled for a quit, whose
    /// saving button says how many it saves.
    #[test]
    fn the_question_is_the_close_forms_dialog() {
        let id = |name: &str| match name {
            closing::SAVE => Some(3),
            closing::DISCARD => Some(4),
            closing::CANCEL => Some(5),
            _ => None,
        };
        let one = dialog(Some(2), "x", 1, id);
        assert_eq!(one["title"], TITLE);
        assert_eq!(one["alert"], true);
        let buttons = &one["children"][0]["children"];
        assert_eq!(buttons[3]["label"], "Save");
        assert_eq!(buttons[3]["id"], 3);
        let two = dialog(Some(2), "x", 2, id);
        assert_eq!(two["children"][0]["children"][3]["label"], "Save all");
    }
}
