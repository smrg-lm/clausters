//! **What an element shows over the window of its own accord**, as data:
//! the element holds it and the popup layer
//! ([`crate::host::popup`]) places and draws it.

use clausters_gui_draw::rect::Rect;

use crate::host::menu::Entry;

/// **What an element shows over the window of its own accord**: a list its
/// own keys walk (a code editor's completions) and a note (the parameters of
/// a call, what a name is). Neither is opened on the host: the element holds
/// both and is asked for them at every frame
/// ([`Element::own_popups`](super::Element::own_popups)), and the
/// frame places them against the work area as it places every popup, so a
/// list keeps up with the typing that filters it, with no message to open or
/// close it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Own {
    pub list: Option<OwnList>,
    pub note: Option<Note>,
}

/// A list an element shows and walks itself.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnList {
    pub entries: Vec<Entry>,
    /// The row the keys are on.
    pub hover: Option<usize>,
    /// What it hangs under: the text it completes.
    pub anchor: Rect,
    pub text_size: f32,
    /// The most rows it shows.
    pub rows: usize,
    /// **How far a hand scrolled it**, by its bar or the wheel; `None` while
    /// it follows the row the keys are on.
    pub scroll: Option<f32>,
}

/// **A note over the text**: a head line in the code's face with parts of it
/// marked (a call's current parameter), a word beside it (what a name is),
/// and the documentation under it.
///
/// A note the pointer may enter is a **hover card** (`card`): it stays while
/// the pointer is on what it explains or on it, its documentation scrolls
/// past `Metrics::card_doc_h` and its text can be selected and copied
/// (`popup::Card`). One that may not (a call's parameters, which follow the
/// caret) shows its first lines.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    /// What it explains: it stands over it, or under it where there is no
    /// room over it.
    pub anchor: Rect,
    /// The head line, in runs, the marked ones drawn bold and underlined.
    pub head: Vec<(String, bool)>,
    /// The word after the head.
    pub aside: String,
    pub doc: String,
    /// The most lines of the documentation shown, where it is no card.
    pub doc_lines: usize,
    /// The size the head is written at, in the code's face.
    pub code_size: f32,
    /// The size the documentation is written at.
    pub text_size: f32,
    /// Whether the pointer may enter it.
    pub card: bool,
}
