//! **The multitrack editor's dialogs**: the path a session is saved under --
//! one form of the shared mechanism ([`crate::forms`]), the audio editor's
//! Save as for a session.

use crate::forms::{self, Spec};

/// **One of the forms.**
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// The path the session is written to, from now on.
    SaveAs,
}

impl Spec for Form {
    fn prefix(self) -> &'static str {
        "file"
    }

    fn page(self) -> i32 {
        1
    }

    fn title(self) -> &'static str {
        "Save as"
    }

    fn fields(self) -> Vec<(&'static str, &'static str)> {
        vec![("path", "File")]
    }
}

/// The forms the window holds, in the order of their pages.
const FORMS: [Form; 1] = [Form::SaveAs];

/// **Every widget of the dialogs, by name**, for a caller to number.
#[must_use]
pub fn names() -> Vec<String> {
    forms::names(&FORMS)
}

/// The forms the window holds, in the order of their pages.
#[must_use]
pub fn forms() -> &'static [Form] {
    &FORMS
}

/// **The window's dialogs**, held whole ([`forms::Dialogs`]).
pub type Dialogs = forms::Dialogs<Form>;
