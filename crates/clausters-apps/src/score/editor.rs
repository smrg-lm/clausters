//! **The score editor, one turn at a time**: what a gesture on the page or a
//! verb a client calls does to the score, the entry it leaves, and what the
//! window is corrected with.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use clausters_core::notation::{
    AnyEngraver, Item, NOTE, Op, PAPERS, Page, PageSetup, Pages, Score, Sheet, View, field_of,
    item_id, layout_options, measure_id, pitch_near, sheet_to_mei,
};
use clausters_core::ratio::Ratio;
use clausters_editing::conversation::{self, Answer, Conversation, Correction};

use super::entry::{self, Place};
use super::verbs::{self, Action};
use super::{Chrome, dialogs, icons, menu, palettes, selection, tools};
use super::{Ids, PAGE_GAP, Shared, Window, correction, scale_for, window};
use crate::turn::{self, Converse, Event, Kind, Leg, Record, int, text};

/// The vocabulary the editor's structure is registered under.
pub const DOMAIN: &str = "score";

/// What the status line says when nothing is selected.
pub const HINT: &str = "click a note to select it, or press N to write notes";

/// **What one turn came to.**
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    /// What kind of turn the message was.
    pub turn: Kind,
    /// What to send the host.
    pub answer: Option<Answer>,
    /// The stamp a [`Kind::Step`] is answered with.
    pub seq: i64,
    /// Whether a [`Kind::Step`] walks forward.
    pub redo: bool,
    /// The entry to record, when the turn edited the score.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<Record>,
    /// Whether the score changed.
    pub changed: bool,
    /// The version after the turn.
    pub version: i64,
    /// **What is selected after the turn**, as the page's element ids, when
    /// the turn moved it -- a press, or a note just written. Not an edit: it
    /// enters no history.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected: Option<Vec<String>>,
    /// **The file to write the score to**, when the turn asked for a save. A
    /// file is its holder's to write -- the disk under a native host, the
    /// page's own storage in a tab -- so the editor says where and whoever
    /// drives it writes the score's document there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save: Option<String>,
    /// **The file to open**, when the turn asked for one: its holder reads
    /// the text and hands it back as the `open` verb, which is the edit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open: Option<String>,
    /// **Whether to close the window**: the File menu's Close, once nothing
    /// is left unsaved or the writer said not to save it. A turn that also
    /// names a file to save closes once that file is written.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub close: bool,
    /// **The file to export the score's render to**, and as what:
    /// `{"path", "format"}`, the format `"smf"`, a Standard MIDI File, or
    /// `"clip"`, a MIDI 2.0 Clip File. Its holder renders the score and
    /// writes it, as a sequence writes either.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub export: Option<Value>,
    /// **What a play asks of the playback**: `{"looping", "range", "from"}`
    /// -- the space bar over the window, the toolbar's play or the menu's.
    /// `from` is the beat a pass starts at, where the selection starts or
    /// where the cursor was left; `range` is `[start, end]` in beats when
    /// several items are selected, which a loop repeats, or `null`. Whoever
    /// drives the editor plays or stops: the editor holds no transport.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub play: Option<Value>,
    /// **What the loop switch asks of a pass in progress**: the same pass,
    /// when `L`, the toolbar or the menu turned the loop on or off.
    #[serde(rename = "loop", skip_serializing_if = "Option::is_none")]
    pub relooped: Option<Value>,
    /// Where the position cursor was placed, as a beat of the score -- a
    /// rewind. Not an edit: a stopped playback is cued there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locate: Option<f64>,
}

turn::turned!(Outcome);

/// How many beats a whole note is in a rendered score: a quarter to the beat,
/// the default interpretation's.
const RENDER_BEAT_UNIT: i64 = 4;

/// **A score editor**: a shared score, what is selected on its page, the value
/// a note is written with, and its end of the conversation.
#[derive(Clone)]
pub struct ScoreEditor {
    score: Shared,
    conversation: Conversation,
    window: Option<i32>,
    ids: Option<Ids>,
    /// **Whether the window is the page alone**, with no chrome around it: no
    /// menu bar, no toolbar, no palettes, no status line and no dialogs --
    /// none of it composed, so none of it engraved for, sent or corrected.
    /// The keys stay, since they are the window's and not the menu's. It is
    /// how a holder that edits through its own handle opens the editor; a
    /// host with no holder beside it opens the whole window, which is then
    /// the only way to reach what the editor does.
    bare: bool,
    /// The toolbar's widgets, by the tool each one is ([`tools::TOOLS`]).
    tools: tools::Ids,
    /// The dialogs' widgets, by name ([`dialogs::names`]).
    dialogs: dialogs::Ids,
    /// The palettes' entries, by name ([`palettes::names`]).
    palettes: palettes::Ids,
    /// The form that is up, and what its fields hold as they are typed.
    dialog: Option<dialogs::Open>,
    /// The file the score is saved to: the one it was read from, or the last
    /// one a save named. `None` for a score that has no file yet.
    path: Option<String>,
    /// **The score as its file holds it**: the document when the editor
    /// opened, or when it was last saved, opened or made new. A score that
    /// is not this has changes to lose.
    saved: String,
    /// The file an Open asked for, until its holder hands the document back.
    opening: Option<String>,
    /// Whether a save the close form asked for closes the window once a file
    /// is named for it.
    closing: bool,
    /// Whether a pass loops: the loop switch, the toolbar's and the menu's.
    looping: bool,
    /// Where a pass starts with nothing selected, as a beat of the score.
    cursor: f64,
    /// The engraver's outlines for the symbols the tools and the palettes
    /// are drawn with: asked once, the first time a window has either, and
    /// kept -- `None` until then, and empty when the engraver handed none
    /// out.
    outlines: Option<tools::Outlines>,
    title: String,
    size: (i64, i64),
    /// The selected elements, as the page names them (`n7`, `n7-2`), in the
    /// order they were picked.
    selection: Vec<String>,
    /// Whether the selection is **a stretch of the score** -- a measure's,
    /// one extended from an item to another, or everything -- rather than
    /// the elements a hand picked one by one. A stretch is what a play plays
    /// and a loop repeats, even where it holds one item; one item picked is
    /// where a play starts.
    stretch: bool,
    /// The written value a note entered on the page takes, undotted.
    value: Ratio,
    /// Whether that value is dotted.
    dotted: bool,
    /// Whether a press on empty staff writes a rest rather than a note.
    rest: bool,
    /// The accidental armed for the next note written, in semitones; it is
    /// let go once that note is.
    accidental: Option<i32>,
    /// **Whether the window is in note entry.** In it, a press on a staff
    /// writes at the time it fell at and the keys write at the edit cursor;
    /// outside it, a press on a staff selects the measure it fell in. The
    /// window opens outside it.
    entry: bool,
    /// The edit cursor, while in note entry.
    place: Option<Place>,
    /// Where the edit cursor was when the mode was last left: where it comes
    /// back with nothing selected.
    left: Option<Place>,
    /// The item note entry wrote last, while the cursor stands just past it:
    /// what a chord key adds to and what the arrows up and down move.
    entered: Option<u64>,
    /// The ids the page last drew, so an item is selected as every element it
    /// is drawn as -- the parts a barline split it into, a chord's pitches.
    drawn: Vec<String>,
    /// How the window looks at the score: as pages, or as one system. The
    /// window's, not the document's.
    view: View,
    /// The engraver's options the score is laid out under, so it is laid out
    /// again only when the paper or the view changed.
    laid: String,
}

impl std::fmt::Debug for ScoreEditor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScoreEditor")
            .field("window", &self.window)
            .field("ids", &self.ids)
            .field("selection", &self.selection)
            .field("value", &self.value)
            .finish_non_exhaustive()
    }
}

/// What a score editor is opened with, as the context's door reads it.
#[derive(Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Opened {
    title: String,
    w: i64,
    h: i64,
    value: Option<Ratio>,
    version: i64,
    chrome: bool,
}

impl Default for Opened {
    fn default() -> Self {
        Self {
            title: "Score".into(),
            w: 960,
            h: 640,
            value: None,
            version: 1,
            chrome: true,
        }
    }
}

impl ScoreEditor {
    /// An editor over `score`.
    ///
    /// **The page is written from the model before it is edited.** A score
    /// opened from a document somebody else wrote carries the ids its reader
    /// minted, which name no item of the model, so a press on a note would
    /// select nothing until the first edit wrote the page again. Opening writes
    /// it now -- the same document, read through the model, with the model's
    /// ids -- and records nothing, since nothing was edited.
    pub fn new(score: Shared, version: i64) -> Self {
        let saved = {
            let mut held = score.lock().unwrap_or_else(|e| e.into_inner());
            let written = held.sheet().and_then(|sheet| sheet_to_mei(sheet).ok());
            if let Some(mei) = written
                && mei != held.mei()
            {
                held.load(&mei);
            }
            held.mei()
        };
        Self {
            score,
            conversation: Conversation::new(version),
            window: None,
            ids: None,
            bare: false,
            tools: tools::Ids::new(),
            dialogs: dialogs::Ids::new(),
            palettes: palettes::Ids::new(),
            dialog: None,
            path: None,
            saved,
            opening: None,
            closing: false,
            looping: false,
            stretch: false,
            cursor: 0.0,
            outlines: None,
            title: "Score".into(),
            size: (960, 640),
            selection: Vec::new(),
            value: Ratio::new(1, 4),
            dotted: false,
            rest: false,
            accidental: None,
            entry: false,
            place: None,
            left: None,
            entered: None,
            drawn: Vec::new(),
            view: View::Page,
            laid: String::new(),
        }
    }

    /// The score it edits.
    pub fn score(&self) -> &Shared {
        &self.score
    }

    /// **Whether the window is the page alone**, with no chrome: what a
    /// holder that edits through its own handle asks for. It is read when the
    /// window is composed, so it is said before [`window`](Self::window).
    pub fn set_bare(&mut self, bare: bool) {
        self.bare = bare;
    }

    /// Whether the window is the page alone ([`set_bare`](Self::set_bare)).
    pub fn bare(&self) -> bool {
        self.bare
    }

    fn held(&self) -> std::sync::MutexGuard<'_, Score<AnyEngraver>> {
        // A poisoned lock is a panic elsewhere while the score was held; the
        // document is still the score, and refusing it here would lose it.
        self.score.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// **The window**, with the page under `ids.page` and every widget of the
    /// chrome under the id `chrome` gives it -- which are then the widgets
    /// the editor answers for. Chrome left unnumbered is chrome the window
    /// does not have: no toolbar, no palettes, no dialogs. A bare editor
    /// ([`set_bare`](Self::set_bare)) has none whatever was numbered, and no
    /// menu bar or status line either: its window is the page in its scroll.
    pub fn window(&mut self, ids: Ids, chrome: Chrome) -> Value {
        let (ids, chrome) = if self.bare {
            (
                Ids {
                    status: None,
                    ..ids
                },
                Chrome::default(),
            )
        } else {
            (ids, chrome)
        };
        self.ids = Some(ids);
        self.tools = chrome.tools;
        self.dialogs = chrome.dialogs;
        self.palettes = chrome.palettes;
        // a window drawn again opens with no form up
        self.dialog = None;
        // the tools' symbols are the engraver's, asked for before the page is
        // drawn since asking loads the document again
        let labelled = !self.tools.is_empty() || !self.palettes.is_empty();
        if labelled && self.outlines.is_none() {
            // ...and what its face does not hold is the editor's to draw
            let codes: Vec<&str> = [tools::codes(), palettes::codes(), icons::drawn_from()]
                .concat()
                .into_iter()
                .filter(|code| !icons::is_own(code))
                .collect();
            let mut found = self.held().outlines(&codes);
            icons::complete(&mut found);
            self.outlines = Some(found);
        }
        let none = tools::Outlines::new();
        let outlines = self.outlines.clone().filter(|_| !self.bare).unwrap_or(none);
        let page = self.page();
        self.drawn = page.draw.kinds.keys().cloned().collect();
        window(Window {
            page: &page,
            ids,
            title: &self.title,
            size: self.size,
            status: (!self.bare).then(|| self.describe()),
            entry: self.entry,
            edit_cursor: self.edit_cursor(),
            keys: self.keys(),
            scale: self.scale(),
            menu: (!self.bare).then(|| self.menu()),
            toolbar: tools::toolbar(&self.tools, &self.input(), &outlines),
            dialogs: dialogs::stack(&self.dialogs),
            palettes: palettes::column(&self.palettes, &outlines),
            glyphs: &outlines,
        })
    }

    /// **What the toolbar shows**: the input state, and the voice of what is
    /// selected.
    fn input(&self) -> tools::State {
        let voice = {
            let held = self.held();
            self.items()
                .first()
                .and_then(|&id| held.sheet().and_then(|sheet| verbs::locate(sheet, id)))
                .map_or(0, |located| located.voice)
        };
        tools::State {
            entry: self.entry,
            value: self.value,
            dotted: self.dotted,
            rest: self.rest,
            accidental: self.accidental,
            voice,
            view: self.view,
            looping: self.looping,
        }
    }

    /// **The pass a play asks for**: from where the selection starts -- or
    /// where the cursor was left, with nothing selected -- over the stretch
    /// the selection covers, which is what a loop repeats; and whether it
    /// loops. In beats, a quarter to the beat, as the score is rendered.
    ///
    /// **A selection is a stretch or a place**, as a time selection and the
    /// position cursor are in the other editors: measures, an extended
    /// selection and several items are played from their first note to the
    /// end of the one that ends last, once or over and over; one item picked
    /// is where the pass starts, and it goes on to the end.
    fn pass(&self) -> Value {
        let span = {
            let held = self.held();
            held.sheet().and_then(|sheet| {
                let ids = verbs::in_time(sheet, &self.items());
                let first = verbs::locate(sheet, *ids.first()?)?;
                // the one that ends last, which need not start last
                let last = ids
                    .iter()
                    .filter_map(|id| verbs::locate(sheet, *id))
                    .max_by_key(|at| at.onset + at.item.dur())?;
                // where it is first heard: a repeat is played out before it
                let beats = |whole: Ratio| {
                    let written = whole.to_f64() * RENDER_BEAT_UNIT as f64;
                    clausters_core::notation::heard_beats(sheet, RENDER_BEAT_UNIT, written)
                        .first()
                        .copied()
                        .unwrap_or(written)
                };
                Some((
                    beats(first.onset),
                    beats(last.onset + last.item.dur()),
                    ids.len(),
                ))
            })
        };
        match span {
            Some((start, end, count)) => json!({
                "looping": self.looping,
                "range": (count > 1 || self.stretch).then(|| json!([start, end])),
                "from": start,
            }),
            None => json!({"looping": self.looping, "range": null, "from": self.cursor}),
        }
    }

    /// **The score as the sequence it plays as** (`events::score::render`):
    /// its repeats played out and its tempo marks its tempo map -- the same
    /// reading a page's cursor is drawn over, so the cursor is where the sound
    /// is.
    ///
    /// # Errors
    /// When the document has no model, or a spanner of it names no item.
    pub fn rendered(&self) -> Result<Value, String> {
        let held = self.held();
        let sheet = held
            .sheet()
            .ok_or_else(|| "this document has no model to render".to_string())?;
        let sequence = clausters_document::events::score::render(
            sheet,
            &clausters_core::notation::default_interpretation(),
        )?;
        serde_json::to_value(&sequence).map_err(|why| why.to_string())
    }

    /// The value the next item is written with: the one in hand, and half as
    /// much again when it is dotted.
    fn written(&self) -> Ratio {
        if self.dotted {
            self.value * Ratio::new(3, 2)
        } else {
            self.value
        }
    }

    /// The palette entry the widget `widget` is, by name.
    fn palette_entry(&self, widget: i64) -> Option<&str> {
        self.palettes
            .iter()
            .find(|(_, id)| i64::from(**id) == widget)
            .map(|(name, _)| name.as_str())
    }

    /// The tool the widget `widget` is, when it is one of the toolbar's.
    fn tool_of(&self, widget: i64) -> Option<&str> {
        self.tools
            .iter()
            .find(|(_, id)| i64::from(**id) == widget)
            .map(|(name, _)| name.as_str())
    }

    /// **The chrome, corrected**: the tools and the menu bar told the state
    /// they show, with no page engraved -- what moving the input state takes.
    fn chrome(&self) -> Vec<Correction> {
        let mut out: Vec<Correction> = tools::corrections(&self.tools, &self.input())
            .into_iter()
            .map(|(widget, props)| Correction {
                widget: i64::from(widget),
                props,
            })
            .collect();
        if let Some(window) = self.window {
            // a bare window has no menu bar to correct; its keys still follow
            // the mode
            let props = if self.bare {
                json!({"keys": self.keys()})
            } else {
                json!({"menu": self.menu(), "keys": self.keys()})
            };
            out.push(Correction {
                widget: i64::from(window),
                props,
            });
        }
        out
    }

    /// **The key table's scopes in force in the window**: the score's own,
    /// and note entry's while the window is in it.
    fn keys(&self) -> Value {
        if self.entry {
            json!(["score", "note_entry"])
        } else {
            json!(["score"])
        }
    }

    /// **The page's `edit_cursor`**: the element that shows where note entry
    /// writes next, the staff and whether it stands past the element -- or
    /// the empty string, which is none, outside the mode or where nothing is
    /// drawn to show it by. Not `null`, which no wire has a type for.
    fn edit_cursor(&self) -> Value {
        let none = || json!("");
        let Some(place) = self.place.filter(|_| self.entry) else {
            return none();
        };
        let shown = {
            let held = self.held();
            held.sheet().and_then(|sheet| entry::shown_by(sheet, place))
        };
        let Some((item, end)) = shown else {
            return none();
        };
        let parts = self.elements_of(&[item]);
        let element = if end { parts.last() } else { parts.first() };
        json!({"at": element, "staff": place.staff, "end": end})
    }

    /// **Into note entry, or out of it.** In, the cursor goes on what is
    /// selected -- its staff, its voice, its time -- or, with nothing
    /// selected, where it was last left, or the start of the score. Out, it
    /// goes and its place is kept.
    fn set_entry(&mut self, on: bool) {
        if on == self.entry {
            return;
        }
        self.entry = on;
        self.entered = None;
        if !on {
            self.left = self.place.take();
            return;
        }
        let selected = {
            let held = self.held();
            held.sheet().and_then(|sheet| {
                let first = *verbs::in_time(sheet, &self.items()).first()?;
                let at = verbs::locate(sheet, first)?;
                Some(Place {
                    staff: at.staff,
                    voice: at.voice,
                    at: at.onset,
                })
            })
        };
        self.place = Some(selected.or(self.left).unwrap_or_default());
    }

    /// **A pass is about to play, and playing leaves note entry**: what the
    /// window is corrected with when it was in it, nothing when it was not.
    fn leave_to_play(&mut self) -> Vec<Correction> {
        if !self.entry {
            return Vec::new();
        }
        self.set_entry(false);
        let mut out = self.cursor_shown();
        out.extend(self.chrome());
        out
    }

    /// **The edit cursor and the status line, corrected** -- what moving
    /// the cursor or the mode takes, with no page engraved.
    fn cursor_shown(&self) -> Vec<Correction> {
        let Some(ids) = self.ids else {
            return Vec::new();
        };
        let mut out = vec![Correction {
            widget: i64::from(ids.page),
            props: json!({"entry": self.entry, "edit_cursor": self.edit_cursor()}),
        }];
        if let Some(status) = ids.status {
            out.push(Correction {
                widget: i64::from(status),
                props: json!({"text": self.describe()}),
            });
        }
        out
    }

    /// **The menu bar as the editor now stands**: what is checked in it is the
    /// editor's state -- the layout, entry, the value in hand, the paper.
    fn menu(&self) -> Value {
        let setup = self.setup();
        menu::menu(&menu::State {
            view: self.view,
            entry: self.entry,
            value: self.value,
            paper: setup.paper(),
            landscape: setup.landscape(),
            dialogs: dialogs::numbered(&self.dialogs),
            looping: self.looping,
        })
    }

    /// The page setup the score is on: the one somebody chose, or the default.
    pub fn setup(&self) -> PageSetup {
        self.held()
            .sheet()
            .and_then(|sheet| sheet.page)
            .unwrap_or_default()
    }

    /// The pixels a page unit is drawn at: the paper's width across the window
    /// it opened in, in either view.
    fn scale(&self) -> f64 {
        scale_for(&self.setup(), self.size)
    }

    /// **Lay the score out for this window**: on its paper, as this view --
    /// again only when one of the two changed since it was last laid out, an
    /// edit to the page setup included. It changes the layout of the score the
    /// holder has, which is one engraver's: two windows over one score show one
    /// layout.
    fn lay_out(&mut self) {
        let options = layout_options(&self.setup(), self.view);
        if options != self.laid && self.held().relayout(&options) {
            self.laid = options;
        }
    }

    /// **The drawing**: every page of the paper one under another, each in its
    /// frame, or the one system of a continuous view.
    fn page(&mut self) -> Page {
        self.lay_out();
        let paged = self.view == View::Page;
        self.held().pages(if paged { PAGE_GAP } else { 0.0 }, paged)
    }

    /// **The elements an item is drawn as**: every id of the page that is the
    /// item's -- itself, the parts a barline split it into, a chord's pitches
    /// -- or its own id where the page has not been drawn.
    fn elements_of(&self, items: &[u64]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for &item in items {
            let before = out.len();
            out.extend(
                self.drawn
                    .iter()
                    .filter(|id| item_id(id) == Some(item))
                    .cloned(),
            );
            if out.len() == before {
                out.push(format!("n{item}"));
            }
        }
        out
    }

    /// **What a press on `element` selects**: the items of a measure, when it
    /// fell on a staff's own lines; the item the element is part of; or the
    /// element itself, where it is no item's (a slur, a clef).
    fn picked(&self, element: &str) -> Vec<String> {
        if let Some((measure, staff)) = measure_id(element) {
            let held = self.held();
            let items = held
                .sheet()
                .map(|sheet| verbs::measure_items(sheet, measure, staff))
                .unwrap_or_default();
            drop(held);
            return self.elements_of(&items);
        }
        match item_id(element) {
            Some(item) => self.elements_of(&[item]),
            None => vec![element.to_string()],
        }
    }

    /// **The selection after a press**, by how it was pressed: alone, it is
    /// what was picked; with Ctrl (`toggle`), what was picked joins or leaves;
    /// with Shift (`extend`), it reaches from the first selected item to the
    /// one picked, in time and across the staves between them.
    fn select_by(&mut self, element: &str, mode: &str) {
        if element.is_empty() {
            if mode.is_empty() {
                self.selection.clear();
                self.stretch = false;
            }
            return;
        }
        let picked = self.picked(element);
        let measure = measure_id(element).is_some();
        match mode {
            "toggle" => {
                self.stretch |= measure;
                if picked.iter().all(|id| self.selection.contains(id)) {
                    self.selection.retain(|id| !picked.contains(id));
                } else {
                    for id in picked {
                        if !self.selection.contains(&id) {
                            self.selection.push(id);
                        }
                    }
                }
            }
            "extend" => {
                let anchor = self.items().first().copied();
                let target = picked.iter().rev().find_map(|id| item_id(id));
                let between = match (anchor, target) {
                    (Some(from), Some(to)) => {
                        let held = self.held();
                        held.sheet()
                            .map(|sheet| verbs::range(sheet, from, to))
                            .unwrap_or_default()
                    }
                    _ => Vec::new(),
                };
                self.stretch = measure || !between.is_empty();
                self.selection = if between.is_empty() {
                    picked
                } else {
                    // the anchor stays first, so a second Shift+click
                    // extends from the same note
                    let mut items: Vec<u64> = anchor.into_iter().collect();
                    items.extend(between.into_iter().filter(|i| Some(*i) != anchor));
                    self.elements_of(&items)
                };
            }
            _ => {
                self.stretch = measure;
                self.selection = picked;
            }
        }
    }

    /// The selected items, as the model names them, each once.
    pub fn items(&self) -> Vec<u64> {
        let mut out: Vec<u64> = Vec::new();
        for id in self.selection.iter().filter_map(|e| item_id(e)) {
            if !out.contains(&id) {
                out.push(id);
            }
        }
        out
    }

    /// **What the status line says**: what is selected, and how it is
    /// written -- or what a hand can do, with nothing selected.
    pub fn describe(&self) -> String {
        let items = self.items();
        let held = self.held();
        let Some(sheet) = held.sheet() else {
            return "this document could not be read into a model: it draws, and takes no edit"
                .into();
        };
        // a text of the page: which field it is, and where it is written
        if let [element] = self.selection.as_slice()
            && let Some((field, index)) = field_of(element)
        {
            let header = &sheet.header;
            let text = if field == NOTE {
                header.notes.get(index).map(String::as_str)
            } else {
                header.text(field)
            };
            let place = header.place(field);
            let word = |value: serde_json::Value| value.as_str().unwrap_or_default().to_string();
            return format!(
                "{field}: \"{}\" -- {} {} {}, {} page",
                text.unwrap_or_default(),
                word(json!(place.region)),
                word(json!(place.halign)),
                word(json!(place.valign)),
                if place.pages == Pages::All {
                    "every"
                } else {
                    "first"
                },
            );
        }
        if let (true, Some(place)) = (self.entry, self.place) {
            let (measure, into) = sheet.grid.position(place.at);
            return format!(
                "note entry: staff {} voice {}, bar {} + {into} -- a to g write, Shift adds \
                 to the chord, 0 a rest, Esc leaves",
                place.staff + 1,
                place.voice + 1,
                measure + 1,
            );
        }
        // something written beside the notes: what it is, in the model
        let attached = selection::attached(&self.selection);
        if items.is_empty() {
            match attached.as_slice() {
                [] => {}
                [one] => return selection::describe(sheet, one),
                many => {
                    return format!("{} elements selected -- Delete takes them away", many.len());
                }
            }
        }
        match items.as_slice() {
            [] if self.selection.is_empty() => HINT.into(),
            [] => format!(
                "{} is the page's own, and no verb is its",
                self.selection[0]
            ),
            [one] => match verbs::locate(sheet, *one) {
                Some(at) => {
                    let mut line = format!(
                        "item {one}: {}, staff {} voice {}",
                        at.item.dur(),
                        at.staff + 1,
                        at.voice + 1
                    );
                    if let Some(marks) = at.item.marks().filter(|m| !m.is_empty())
                        && let Ok(marks) = serde_json::to_string(marks)
                    {
                        line.push_str(&format!(", {marks}"));
                    }
                    if matches!(
                        at.item,
                        clausters_core::notation::Item::Note { tie: true, .. }
                    ) {
                        line.push_str(", tied");
                    }
                    line
                }
                None => format!("item {one} is gone"),
            },
            many => format!("{} items selected", many.len()),
        }
    }

    /// What `widget` is corrected with, as the score now stands -- nothing for
    /// a widget that is not this window's.
    ///
    /// **Only the page and its scroll are worth an engraving.** The status
    /// line is a sentence and a tool a state, so each is answered with that
    /// alone: a client asks for a window's widgets one by one (the door's
    /// `props`), and answering each with the whole window engraved the score
    /// once per tool and palette entry -- some hundred and sixty times for
    /// one selection made from a script.
    fn resync_widget(&mut self, widget: i64) -> Vec<Correction> {
        let Some(ids) = self.ids else {
            return Vec::new();
        };
        let is = |id: i32| i64::from(id) == widget;
        if is(ids.page) || ids.scroll.is_some_and(is) {
            return self.corrections();
        }
        if ids.status.is_some_and(is) {
            return vec![Correction {
                widget,
                props: json!({"text": self.describe()}),
            }];
        }
        let chrome = self.tools.values().any(|id| is(*id)) || self.window.is_some_and(is);
        if chrome {
            return self.chrome();
        }
        Vec::new()
    }

    /// **Every widget of the window, corrected**: the page re-engraved, the
    /// scroll sized to it, the selection drawn and the status line told.
    fn corrections(&mut self) -> Vec<Correction> {
        let Some(ids) = self.ids else {
            return Vec::new();
        };
        let page = self.page();
        self.drawn = page.draw.kinds.keys().cloned().collect();
        // an item re-engraved may be drawn as other parts than it was, and
        // what else was selected stays while the page still draws it: a slur
        // taken away is no longer selected
        let items = self.items();
        let others: Vec<String> = self
            .selection
            .iter()
            .filter(|id| item_id(id).is_none() && self.drawn.contains(*id))
            .cloned()
            .collect();
        self.selection = self.elements_of(&items);
        self.selection.extend(others);
        let mut out: Vec<Correction> = correction(&page, ids, self.scale())
            .into_iter()
            .map(|(widget, props)| Correction {
                widget: i64::from(widget),
                props,
            })
            .collect();
        if let Some(Value::Object(props)) = out.first_mut().map(|c| &mut c.props) {
            props.insert("selected".into(), json!(self.selection));
            props.insert("entry".into(), json!(self.entry));
            props.insert("edit_cursor".into(), self.edit_cursor());
        }
        if let Some(status) = ids.status {
            out.push(Correction {
                widget: i64::from(status),
                props: json!({"text": self.describe()}),
            });
        }
        // the bar and the tools show the editor's state, so they are told
        // when that may have moved: a paper undone, a layout switched from a
        // script, another voice selected
        out.extend(self.chrome());
        out
    }

    /// **Every widget of the window, corrected** -- what a history step leaves
    /// behind.
    pub fn resync_all(&mut self, version: i64) -> Answer {
        conversation::answer(0, version, None, self.corrections())
    }

    /// Answers the stamp a [`Kind::Step`] carried, once the caller has walked.
    pub fn acknowledge(&self, seq: i64, version: i64, reason: Option<String>) -> Answer {
        conversation::answer(seq, version, reason, Vec::new())
    }

    /// **One message from the host**, read and answered ([`turn::turn`]).
    pub fn event(&mut self, event: &Event, version: i64) -> Outcome {
        turn::turn(self, event, version)
    }

    /// **One payload of a history step**, put back on the score: the MEI it
    /// names, as a step forward (`{"edit": {"mei"}}`) or back (`{"mei"}`).
    /// Answers whether it changed.
    pub fn apply(&mut self, payload: &Value) -> bool {
        let mei = payload
            .get("edit")
            .unwrap_or(payload)
            .get("mei")
            .and_then(Value::as_str);
        let Some(mei) = mei else {
            return false;
        };
        let mut held = self.held();
        if held.mei() == mei {
            return false;
        }
        held.load(mei)
    }

    /// **One verb a client calls** -- `{"action", ...}`, as [`verbs::Action`]
    /// reads it -- over what is selected, as one entry of the history. The
    /// answer corrects the window, unasked.
    pub fn act(&mut self, request: &Value, version: i64) -> Outcome {
        let mut out = <Outcome as turn::Turned>::at(version);
        out.turn = Kind::Route;
        let reason = self.perform(request, &mut out);
        out.answer = Some(conversation::answer(
            0,
            out.version,
            reason,
            self.corrections(),
        ));
        out
    }

    /// One verb, read, planned over the selection and applied; what it left
    /// is on `out`, and the answer is why it was refused, if it was.
    fn perform(&mut self, request: &Value, out: &mut Outcome) -> Option<String> {
        match serde_json::from_value::<Action>(request.clone()) {
            Err(why) => Some(format!("no such verb: {why}")),
            Ok(Action::Open { data }) => self.open_document(&data, out),
            Ok(action) => {
                // **A verb means what the selection makes it mean**
                // (`selection`): Delete takes away what is selected, the slur
                // and not its notes; any other verb, with nothing but what is
                // written beside the notes selected, reads the notes it is
                // attached to.
                let items = self.items();
                let attached = selection::attached(&self.selection);
                let planned = {
                    let held = self.held();
                    match held.sheet() {
                        None => Err("this document has no model to edit".into()),
                        Some(sheet) if action == Action::Delete && !attached.is_empty() => {
                            let mut ops = selection::removal(sheet, &attached);
                            if items.is_empty() {
                                Ok(ops)
                            } else {
                                verbs::ops(sheet, &items, &action).map(|deleted| {
                                    ops.extend(deleted);
                                    ops
                                })
                            }
                        }
                        Some(sheet) if items.is_empty() && !attached.is_empty() => {
                            verbs::ops(sheet, &selection::anchors(&attached), &action)
                        }
                        Some(sheet) => verbs::ops(sheet, &items, &action),
                    }
                };
                planned
                    .and_then(|ops| self.edit(&ops, &action.label(), out))
                    .err()
            }
        }
    }

    /// **Open the document `data` in place of the score**, as one entry: the
    /// engraver reads it, whatever format it is in, and the model is read off
    /// what it loaded. A document it cannot read leaves the score as it was.
    fn open_document(&mut self, data: &str, out: &mut Outcome) -> Option<String> {
        let before = self.held().mei();
        if !self.held().load(data) {
            self.held().load(&before);
            return Some("that document could not be read as a score".into());
        }
        // the page is written from the model, as when an editor opens, so a
        // press names an item at once
        let written = self
            .held()
            .sheet()
            .and_then(|sheet| sheet_to_mei(sheet).ok());
        if let Some(mei) = written {
            self.held().load(&mei);
        }
        self.selection.clear();
        self.stretch = false;
        out.selected = Some(Vec::new());
        self.laid.clear();
        // the file it was read from is the score's from now on, and holds it
        if let Some(path) = self.opening.take() {
            self.path = Some(path);
        }
        let saved = self.held().mei();
        self.saved = saved;
        self.recorded(before, "open", out);
        None
    }

    /// **A new score in place of this one**, as one entry, as an Open is:
    /// one staff in the treble clef, common time, C major, four empty bars,
    /// and no file yet.
    fn new_score(&mut self, out: &mut Outcome) -> Option<String> {
        let mut sheet = Sheet::default();
        sheet.staves[0].voices[0].items = (0..4)
            .map(|_| Item::Rest {
                id: 0,
                dur: sheet.grid.meter_at(0).bar(),
            })
            .collect();
        sheet.assign_ids();
        let Ok(mei) = sheet_to_mei(&sheet) else {
            return Some("a new score could not be written".into());
        };
        let before = self.held().mei();
        if !self.held().load(&mei) {
            self.held().load(&before);
            return Some("a new score could not be written".into());
        }
        self.selection.clear();
        self.stretch = false;
        out.selected = Some(Vec::new());
        self.laid.clear();
        self.cursor = 0.0;
        self.place = None;
        self.left = None;
        self.entered = None;
        self.path = None;
        let saved = self.held().mei();
        self.saved = saved;
        self.recorded(before, "new", out);
        None
    }

    /// **Whether the score has changes its file does not hold.**
    #[must_use]
    pub fn unsaved(&self) -> bool {
        self.held().mei() != self.saved
    }

    /// **The score as it stands is what its file holds**: what a holder says
    /// once it has written the file a save named.
    pub fn mark_saved(&mut self) {
        let saved = self.held().mei();
        self.saved = saved;
    }

    /// **Close**: the window goes, at once when nothing is unsaved, and
    /// otherwise once the close form is answered.
    fn close(&mut self, out: &mut Outcome) -> (Option<String>, Option<Vec<Correction>>) {
        if !self.unsaved() {
            out.close = true;
            return (None, Some(Vec::new()));
        }
        // a window with no dialogs cannot ask, and so does not lose the work
        let (reason, shown) = self.open_form(dialogs::Form::Close);
        (reason, Some(shown))
    }

    /// **Back to the start**: the cursor a pass starts from is the score's
    /// first beat, and a stopped playback is cued there.
    fn rewind(&mut self, out: &mut Outcome) {
        self.cursor = 0.0;
        out.locate = Some(0.0);
    }

    /// **The loop switch**, turned: a pass in progress follows it, and the
    /// chrome that shows it -- the toolbar's switch, the menu's check -- is
    /// corrected.
    ///
    /// **It is one switch with the host's**, the one `L` turns. `L` arrives
    /// with the state the host left it in; a turn of the toolbar's switch or
    /// of the menu's (`mine`) tells the host -- `looping`, on the window --
    /// so its next `L` starts from here and never asks for the state the
    /// switch already has.
    fn set_looping(&mut self, on: bool, mine: bool, out: &mut Outcome) -> Vec<Correction> {
        self.looping = on;
        out.relooped = Some(self.pass());
        let mut shown = self.chrome();
        if mine
            && let Some(window) = self.window
            && let Some(Value::Object(props)) = shown
                .iter_mut()
                .find(|c| c.widget == i64::from(window))
                .map(|c| &mut c.props)
        {
            props.insert("looping".into(), json!(on));
        }
        shown
    }

    /// **Save**: the score goes to its file, which the turn's outcome names
    /// for whoever drives the editor to write; a score with no file yet is
    /// asked for one.
    fn save(&mut self, out: &mut Outcome) -> (Option<String>, Option<Vec<Correction>>) {
        match &self.path {
            Some(path) => {
                out.save = Some(path.clone());
                (None, Some(Vec::new()))
            }
            None => {
                let (reason, shown) = self.open_form(dialogs::Form::File(dialogs::File::SaveAs));
                (reason, Some(shown))
            }
        }
    }

    /// **A pick of the menu bar**, answered: a step of the history is the
    /// context's to walk, an edit is the verb it wrote, and what is the
    /// window's own -- the layout, entry, the value in hand, selecting
    /// everything -- moves the editor and enters no history.
    fn pick(&mut self, message: &conversation::Message, args: &[Value], out: &mut Outcome) {
        let verb = args.get(4).map(text).unwrap_or_default();
        let state = args.get(5).map(int);
        let mut reason = None;
        // a form opened corrects its own widgets, and no page is engraved
        let mut shown = None;
        match menu::read(&verb, state) {
            menu::Pick::Undo | menu::Pick::Redo => {
                out.turn = Kind::Step;
                out.seq = message.seq;
                out.redo = verb == "redo";
                return;
            }
            menu::Pick::Act(action) => reason = self.perform(&action, out),
            menu::Pick::SelectAll => {
                let all = self.known_items();
                self.selection = self.elements_of(&all);
                self.stretch = true;
                out.selected = Some(self.selection.clone());
            }
            menu::Pick::Dialog(form) => {
                let (why, corrections) = self.open_form(form);
                reason = why;
                shown = Some(corrections);
            }
            menu::Pick::Save => (reason, shown) = self.save(out),
            menu::Pick::New => reason = self.new_score(out),
            menu::Pick::Close => (reason, shown) = self.close(out),
            menu::Pick::Play => {
                out.play = Some(self.pass());
                shown = Some(self.leave_to_play());
            }
            menu::Pick::Rewind => {
                self.rewind(out);
                shown = Some(Vec::new());
            }
            menu::Pick::Loop(on) => shown = Some(self.set_looping(on, true, out)),
            menu::Pick::Layout(view) => self.view = view,
            menu::Pick::Entry(on) => self.set_entry(on),
            menu::Pick::Value(value) => self.value = value,
            menu::Pick::Unknown => reason = Some(format!("the menu has no entry for {verb}")),
        }
        out.turn = Kind::Route;
        let corrections = shown.unwrap_or_else(|| self.corrections());
        out.answer = Some(conversation::answer(
            message.seq,
            out.version,
            reason,
            corrections,
        ));
    }

    /// Apply `ops` as one entry called `label`: the MEI before them is its
    /// inverse, and the one after is what redoes it. Nothing is recorded when
    /// nothing changed; a refusal leaves the score as it was.
    fn edit(&mut self, ops: &[Op], label: &str, out: &mut Outcome) -> Result<(), String> {
        let before = self.held().mei();
        {
            let mut held = self.held();
            for op in ops {
                if !held.apply(op) {
                    held.load(&before);
                    return Err(format!("{label}: the score refused it"));
                }
            }
        }
        self.recorded(before, label, out);
        Ok(())
    }

    /// **Write one item**, as one entry: `op` applied, the accidental that
    /// was armed given to the note it made -- and let go, since it was for
    /// that note -- and the new item selected. Answers the new item.
    fn write(
        &mut self,
        op: Op,
        label: &str,
        note: bool,
        out: &mut Outcome,
    ) -> Result<Option<u64>, String> {
        let before = self.held().mei();
        let known = self.known_items();
        if !self.held().apply(&op) {
            self.held().load(&before);
            return Err(format!("{label}: the score refused it"));
        }
        let new = self.known_items().into_iter().find(|i| !known.contains(i));
        if let (Some(id), Some(alter), true) = (new, self.accidental, note) {
            let planned = {
                let held = self.held();
                held.sheet()
                    .map(|sheet| verbs::ops(sheet, &[id], &Action::Accidental { alter }))
            };
            if let Some(Ok(ops)) = planned {
                let mut held = self.held();
                for op in &ops {
                    held.apply(op);
                }
            }
            self.accidental = None;
        }
        if let Some(id) = new {
            self.selection = vec![format!("n{id}")];
            self.stretch = false;
            out.selected = Some(self.selection.clone());
        }
        self.recorded(before, label, out);
        Ok(new)
    }

    /// The score moved from `before`: the entry, and the version.
    fn recorded(&mut self, before: String, label: &str, out: &mut Outcome) {
        let after = self.held().mei();
        if after == before {
            return;
        }
        out.record = Some(Record {
            label: label.into(),
            legs: vec![Leg {
                forward: json!({"edit": {"mei": after}}),
                backward: json!({"mei": before}),
                ..Leg::default()
            }],
        });
        out.changed = true;
        out.version += 1;
    }

    fn gesture(
        &mut self,
        widget: i64,
        tag: &str,
        values: &[Value],
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        if let Some(name) = self.tool_of(widget).map(str::to_string) {
            return self.tool(&name, tag, out);
        }
        if let Some(name) = self.dialog_widget(widget).map(str::to_string) {
            return self.form_said(&name, tag, out);
        }
        // an entry of a palette is a verb over what is selected
        if let Some(name) = self.palette_entry(widget) {
            return match palettes::read(name, tag) {
                // an entry that needs words opens the form that asks for them
                Some(action) if action.get("dialog").is_some() => {
                    match action["dialog"].as_str().and_then(dialogs::Form::named) {
                        Some(form) => self.open_form(form),
                        None => (None, Vec::new()),
                    }
                }
                Some(action) => {
                    let reason = self.perform(&action, out);
                    (reason, self.corrections())
                }
                None => (None, Vec::new()),
            };
        }
        if self.ids.map(|ids| i64::from(ids.page)) != Some(widget) {
            return (None, Vec::new());
        }
        match tag {
            // A press named the element under it, or nothing.
            "element" => {
                let picked = values.first().map(text).unwrap_or_default();
                let mode = values.get(1).map(text).unwrap_or_default();
                self.select_by(&picked, &mode);
                out.selected = Some(self.selection.clone());
                (None, self.selected())
            }
            // A drag named the staff position a note reaches.
            "transpose" => {
                let (Some(element), Some(position)) = (values.first().map(text), values.get(1))
                else {
                    return (None, Vec::new());
                };
                let before = self.held().mei();
                if !self
                    .held()
                    .transpose_to_on_any_page(&element, int(position) as i32)
                {
                    return (
                        Some("that cannot be moved there".into()),
                        self.corrections(),
                    );
                }
                self.recorded(before, "move", out);
                (None, self.corrections())
            }
            // A text of the page was typed over where it is drawn: the field
            // its element is, and what it now says.
            "text" => {
                let element = values.first().map(text).unwrap_or_default();
                let written = values.get(1).map(text).unwrap_or_default();
                let Some((field, index)) = field_of(&element) else {
                    return (None, Vec::new());
                };
                let mut request = json!({"action": "text", "field": field, "text": written});
                if field == NOTE {
                    request["index"] = json!(index);
                }
                let reason = self.perform(&request, out);
                (reason, self.corrections())
            }
            // A press on a staff during note entry named a place: the column
            // it fell in and the line or space under it. A note of the
            // cursor's voice starting there takes the pitch into its chord;
            // anywhere else the input state is written over the stretch --
            // a note or a rest of the value in hand -- and the cursor goes on.
            "enter" => {
                let column = values.first().map(text).unwrap_or_default();
                let position = values.get(1).map(int).unwrap_or(0) as i32;
                let staff = values.get(2).map(int).unwrap_or(0).max(0) as usize;
                let reason = self.enter_pressed(&column, position, staff, out);
                (reason, self.corrections())
            }
            _ => (None, Vec::new()),
        }
    }

    /// **A press during note entry**, written: into the chord of a note of
    /// the cursor's voice that starts at the column pressed, or over the
    /// stretch from it -- after which the cursor stands past what was
    /// written. Answers why it was refused, if it was.
    fn enter_pressed(
        &mut self,
        column: &str,
        position: i32,
        staff: usize,
        out: &mut Outcome,
    ) -> Option<String> {
        if !self.entry {
            return None;
        }
        let voice = self.place.map_or(0, |p| p.voice);
        let (at, chord) = {
            let held = self.held();
            let Some(sheet) = held.sheet() else {
                return Some("this document has no model to edit".into());
            };
            let at = item_id(column)
                .and_then(|id| verbs::locate(sheet, id))
                .map_or(Ratio::ZERO, |located| located.onset);
            let place = Place { staff, voice, at };
            let chord = entry::item_at(sheet, place).filter(|item| item.sounds());
            (at, chord.map(|item| (item.id(), item.dur())))
        };
        let place = Place { staff, voice, at };
        if let Some((id, dur)) = chord {
            let op = Op::Enter {
                at: Some(at),
                item: None,
                dur: self.written(),
                pitches: Vec::new(),
                position: Some(position),
                staff,
                voice,
                chord: true,
            };
            if let Err(why) = self.edit(&[op], "add to the chord", out) {
                return Some(why);
            }
            // the cursor stands past the chord, as after any entry
            self.entered = Some(id);
            self.place = Some(Place {
                at: at + dur,
                ..place
            });
            return None;
        }
        let note = !self.rest;
        let op = Op::Enter {
            at: Some(at),
            item: None,
            dur: self.written(),
            pitches: Vec::new(),
            position: note.then_some(position),
            staff,
            voice,
            chord: false,
        };
        let label = if note { "write a note" } else { "write a rest" };
        match self.write(op, label, note, out) {
            Err(why) => Some(why),
            Ok(new) => {
                self.entered = new.filter(|_| note);
                self.place = Some(Place {
                    at: at + self.written(),
                    ..place
                });
                None
            }
        }
    }

    /// **A key of note entry**, performed: a pitch written at the cursor or
    /// added to the chord, a rest, the cursor moved, the note just written
    /// moved, the voice or the value in hand changed. `None` for a verb that
    /// is none of them; else why it was refused, if it was, and what the
    /// window is corrected with.
    fn entry_key(
        &mut self,
        verb: &str,
        out: &mut Outcome,
    ) -> Option<(Option<String>, Vec<Correction>)> {
        if verb == "entry" {
            self.set_entry(!self.entry);
            let mut shown = self.cursor_shown();
            shown.extend(self.chrome());
            return Some((None, shown));
        }
        if verb == "entry_off" {
            self.set_entry(false);
            let mut shown = self.cursor_shown();
            shown.extend(self.chrome());
            return Some((None, shown));
        }
        if let Some(name) = verb.strip_prefix("value_") {
            self.value = entry::value_of(name)?;
            return Some((None, self.chrome()));
        }
        if verb == "dot" {
            self.dotted = !self.dotted;
            return Some((None, self.chrome()));
        }
        let place = self.place.filter(|_| self.entry)?;
        let value = self.written();
        let moved = |editor: &mut Self, to: Place| {
            editor.place = Some(to);
            editor.entered = None;
            (None, editor.cursor_shown())
        };
        let sheet = self.held().sheet().cloned()?;
        if let Some(letter) = verb.strip_prefix("pitch_") {
            let step = entry::step_of(letter)?;
            let entered = self.entered.and_then(|id| verbs::locate(&sheet, id));
            let near = entry::near(&sheet, place, entered.map(|at| at.item));
            let pitch = match pitch_near(&sheet, place.staff, place.at, step, near) {
                Ok(pitch) => pitch,
                Err(why) => return Some((Some(why), Vec::new())),
            };
            let op = Op::Enter {
                at: Some(place.at),
                item: None,
                dur: value,
                pitches: vec![pitch],
                position: None,
                staff: place.staff,
                voice: place.voice,
                chord: false,
            };
            return Some(match self.write(op, "write a note", true, out) {
                Err(why) => (Some(why), self.corrections()),
                Ok(new) => {
                    self.entered = new;
                    self.place = Some(Place {
                        at: place.at + value,
                        ..place
                    });
                    (None, self.corrections())
                }
            });
        }
        if let Some(letter) = verb.strip_prefix("chord_") {
            let step = entry::step_of(letter)?;
            // the note just written, or the one the cursor stands on
            let target = self
                .entered
                .and_then(|id| verbs::locate(&sheet, id))
                .map(|at| (at.staff, at.voice, at.onset, at.item.clone()))
                .or_else(|| {
                    entry::item_at(&sheet, place)
                        .map(|item| (place.staff, place.voice, place.at, item.clone()))
                })
                .filter(|(.., item)| item.sounds());
            let Some((staff, voice, at, item)) = target else {
                return Some((Some("there is no note here to add to".into()), Vec::new()));
            };
            // the letter above the chord's top note
            let top = item.pitches().iter().max_by_key(|p| p.midi()).copied();
            let mut pitch = match pitch_near(&sheet, staff, at, step, top) {
                Ok(pitch) => pitch,
                Err(why) => return Some((Some(why), Vec::new())),
            };
            if top.is_some_and(|top| pitch.midi() <= top.midi()) {
                pitch.octave += 1;
            }
            let op = Op::Enter {
                at: Some(at),
                item: None,
                dur: item.dur(),
                pitches: vec![pitch],
                position: None,
                staff,
                voice,
                chord: true,
            };
            let reason = self.edit(&[op], "add to the chord", out).err();
            return Some((reason, self.corrections()));
        }
        if verb == "enter_rest" {
            let op = Op::Enter {
                at: Some(place.at),
                item: None,
                dur: value,
                pitches: Vec::new(),
                position: None,
                staff: place.staff,
                voice: place.voice,
                chord: false,
            };
            return Some(match self.write(op, "write a rest", false, out) {
                Err(why) => (Some(why), self.corrections()),
                Ok(_) => {
                    self.entered = None;
                    self.place = Some(Place {
                        at: place.at + value,
                        ..place
                    });
                    (None, self.corrections())
                }
            });
        }
        if let Some(steps) = match verb {
            "step_up" => Some(1),
            "step_down" => Some(-1),
            "octave_up" => Some(7),
            "octave_down" => Some(-7),
            _ => None,
        } {
            let id = self
                .entered
                .or_else(|| entry::item_at(&sheet, place).map(Item::id))?;
            let reason = self.edit(&[Op::MoveSteps { id, steps }], "move", out).err();
            return Some((reason, self.corrections()));
        }
        if let Some(n) = verb.strip_prefix("voice_") {
            let voice: usize = n.parse().ok()?;
            if !(1..=entry::VOICES).contains(&voice) {
                return None;
            }
            return Some(moved(
                self,
                Place {
                    voice: voice - 1,
                    ..place
                },
            ));
        }
        let to = match verb {
            "cursor_left" => entry::back(&sheet, place, value),
            "cursor_right" => entry::on(&sheet, place, value),
            "bar_left" => entry::bar_back(&sheet, place),
            "bar_right" => entry::bar_on(&sheet, place),
            "staff_up" => entry::staff_by(&sheet, place, -1),
            "staff_down" => entry::staff_by(&sheet, place, 1),
            _ => return None,
        };
        Some(moved(self, to))
    }

    /// **A key over the selection**, where the window is not writing notes
    /// with it: the same keys note entry reads, meaning what the mode makes
    /// them mean. Delete takes away what is selected, whatever it is; the
    /// arrows up and down move the selected notes a step, or an octave with
    /// Ctrl, where in note entry they move the note just written; left and
    /// right move the selection to the item beside it, where there they move
    /// the cursor; Escape lets the selection go, where there it leaves the
    /// mode. `None` for a verb that is none of them.
    fn select_key(
        &mut self,
        verb: &str,
        out: &mut Outcome,
    ) -> Option<(Option<String>, Vec<Correction>)> {
        let action = match verb {
            "delete" => json!({"action": "delete"}),
            "step_up" => json!({"action": "move", "steps": 1}),
            "step_down" => json!({"action": "move", "steps": -1}),
            "octave_up" => json!({"action": "move", "steps": 7}),
            "octave_down" => json!({"action": "move", "steps": -7}),
            "deselect" => {
                self.select_by("", "");
                out.selected = Some(Vec::new());
                return Some((None, self.selected()));
            }
            "select_left" | "select_right" => {
                let forward = verb == "select_right";
                let to = {
                    let held = self.held();
                    let sheet = held.sheet()?;
                    // from the end of the selection the arrow points away from
                    let ids = verbs::in_time(sheet, &self.items());
                    let from = if forward { ids.last() } else { ids.first() };
                    from.and_then(|id| selection::beside(sheet, *id, forward))
                };
                let to = to?;
                self.selection = self.elements_of(&[to]);
                self.stretch = false;
                out.selected = Some(self.selection.clone());
                return Some((None, self.selected()));
            }
            _ => return None,
        };
        let reason = self.perform(&action, out);
        Some((reason, self.corrections()))
    }

    /// Every item id the model holds.
    fn known_items(&self) -> Vec<u64> {
        let held = self.held();
        held.sheet()
            .map(|sheet| {
                sheet
                    .staves
                    .iter()
                    .flat_map(|s| s.voices.iter())
                    .flat_map(|v| v.items.iter().map(|i| i.id()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// **The selection, shown**: the page told every element that is selected
    /// -- a press names one and the editor may have selected a measure or a
    /// range -- and the status line told what that is. The page is not
    /// engraved again: nothing was edited.
    fn selected(&self) -> Vec<Correction> {
        let Some(ids) = self.ids else {
            return Vec::new();
        };
        let mut out = vec![Correction {
            widget: i64::from(ids.page),
            props: json!({"selected": self.selection}),
        }];
        if let Some(status) = ids.status {
            out.push(Correction {
                widget: i64::from(status),
                props: json!({"text": self.describe()}),
            });
        }
        // the voice tool shows the selection's
        out.extend(
            tools::corrections(&self.tools, &self.input())
                .into_iter()
                .filter(|(widget, _)| self.tools.get("voice") == Some(widget))
                .map(|(widget, props)| Correction {
                    widget: i64::from(widget),
                    props,
                }),
        );
        out
    }

    /// **A tool reported**: the input state moves and the chrome says so, the
    /// layout switches, or a verb is applied to what is selected.
    fn tool(
        &mut self,
        name: &str,
        tag: &str,
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        let Some(tool) = tools::read(name, tag) else {
            return (None, Vec::new());
        };
        match tool {
            tools::Tool::Value(value) => self.value = value,
            tools::Tool::Dot(on) => self.dotted = on,
            tools::Tool::Rest(on) => self.rest = on,
            tools::Tool::Accidental(alter) => {
                // with notes selected it is theirs, and nothing stays armed
                if let (Some(alter), false) = (alter, self.items().is_empty()) {
                    let reason =
                        self.perform(&json!({"action": "accidental", "alter": alter}), out);
                    self.accidental = None;
                    return (reason, self.corrections());
                }
                self.accidental = alter;
            }
            tools::Tool::Voice(to) => {
                let reason = self.perform(&json!({"action": "voice", "to": to}), out);
                return (reason, self.corrections());
            }
            tools::Tool::Layout(view) => {
                self.view = view;
                return (None, self.corrections());
            }
            tools::Tool::Play => {
                out.play = Some(self.pass());
                return (None, self.leave_to_play());
            }
            tools::Tool::Entry(on) => {
                self.set_entry(on);
                let mut out = self.cursor_shown();
                out.extend(self.chrome());
                return (None, out);
            }
            tools::Tool::Rewind => {
                self.rewind(out);
                return (None, Vec::new());
            }
            tools::Tool::Loop(on) => return (None, self.set_looping(on, true, out)),
            tools::Tool::Act(action) => {
                let reason = self.perform(&action, out);
                return (reason, self.corrections());
            }
        }
        (None, self.chrome())
    }
}

impl Converse for ScoreEditor {
    type Outcome = Outcome;

    fn conversation(&mut self) -> &mut Conversation {
        &mut self.conversation
    }

    fn window_id(&self) -> Option<i32> {
        self.window
    }

    fn closed(&mut self) {
        self.window = None;
    }

    fn owns(&self, widget: i64, _tag: &str) -> bool {
        self.ids.map(|ids| i64::from(ids.page)) == Some(widget)
            || self.tool_of(widget).is_some()
            || self.dialog_widget(widget).is_some()
            || self.palette_entry(widget).is_some()
    }

    fn resync(&mut self, widget: i64) -> Vec<Correction> {
        self.resync_widget(widget)
    }

    fn route(
        &mut self,
        widget: i64,
        tag: &str,
        values: &[Value],
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        self.gesture(widget, tag, values, out)
    }

    /// **A pick of the menu bar is the window's own verb**, and so is its
    /// save: the bar is the window's, so the pick arrives addressed to it
    /// rather than to a widget.
    fn window_verb(
        &mut self,
        message: &conversation::Message,
        args: &[Value],
        out: &mut Outcome,
    ) -> bool {
        if message.addr != "/gui_event" || !message.is_window {
            return false;
        }
        match message.tag.as_str() {
            "menu" => self.pick(message, args, out),
            // the space bar and `L` are the window's own: what they ask of
            // the playback is the pass, with the loop switch the host sends
            "play" | "loop" => {
                out.turn = Kind::Route;
                let corrections = if message.tag == "play" {
                    out.play = Some(self.pass());
                    self.leave_to_play()
                } else {
                    let on = args.get(4).and_then(Value::as_i64).is_some_and(|v| v != 0);
                    self.set_looping(on, false, out)
                };
                out.answer = Some(conversation::answer(
                    message.seq,
                    out.version,
                    None,
                    corrections,
                ));
            }
            // the window's own save -- Ctrl+S -- is the menu's
            "save" => {
                let (reason, shown) = self.save(out);
                out.turn = Kind::Route;
                out.answer = Some(conversation::answer(
                    message.seq,
                    out.version,
                    reason,
                    shown.unwrap_or_default(),
                ));
            }
            // the keys of note entry, and `N` that enters it; then the keys
            // that act on the selection, which a key note entry has no use
            // for falls through to
            verb => {
                let Some((reason, corrections)) = self
                    .entry_key(verb, out)
                    .or_else(|| self.select_key(verb, out))
                else {
                    return false;
                };
                out.turn = Kind::Route;
                out.answer = Some(conversation::answer(
                    message.seq,
                    out.version,
                    reason,
                    corrections,
                ));
            }
        }
        true
    }
}

/// **A score editor from JSON**: `{"title", "w", "h", "value", "version",
/// "chrome"}` over `score`. `chrome`, `false`, opens the page alone
/// ([`ScoreEditor::set_bare`]); left out, the window is the whole
/// application's.
pub fn new_json(score: Shared, request: &str) -> ScoreEditor {
    let opened: Opened = serde_json::from_str(request).unwrap_or_default();
    let mut editor = ScoreEditor::new(score, opened.version);
    editor.title = opened.title;
    editor.size = (opened.w, opened.h);
    editor.bare = !opened.chrome;
    if let Some(value) = opened.value.filter(Ratio::is_positive) {
        editor.value = value;
    }
    editor
}

/// **One verb of the editor's own door**, over JSON:
///
/// - `window` -- `widget` (the page), `scroll`, `status`, `tools` (the id of
///   each tool of the toolbar, by its name; left out, the window has none):
///   the GuiDef.
/// - `tools` -- `{"tools"}`: the names of the toolbar's tools, in order, for a
///   caller to number.
/// - `dialogs` -- `{"dialogs"}`: the names of the dialogs' widgets, for a
///   caller to number and hand to `window` the same way (`dialogs`: the id
///   of each by name; without all of them the window has no dialogs).
/// - `palettes` -- `{"palettes"}`: the names of the palettes' entries, for a
///   caller to number and hand to `window` as `palettes`; left out, the
///   window has none.
/// - `props` -- `widget`: what it is corrected with (`{}` for another widget).
/// - `sync` -- `window` (the id it is open in, or `null`), `title`, `w`, `h`,
///   `path` (the file the score is saved to, or `null` for none),
///   `value` (the written value a note is entered with, `[n, d]`), `entry`
///   (whether a press on empty staff writes a note), `dotted`, `rest`
///   (whether that press writes a rest), `accidental` (the one armed for the
///   next note, in semitones, or `null`), `layout` (`"page"` or
///   `"continuous"`, how the window looks at the score): `{}`.
/// - `select` -- `elements`: the page's element ids to select. `{}`.
/// - `selected` -- `{"elements", "items"}`: what is selected, as the page and
///   the model name it.
/// - `value` -- `{"value"}`: the written value a note is entered with, `[n, d]`.
/// - `entry` -- `{"entry"}`: whether a press on empty staff writes a note.
/// - `input` -- `{"value", "dotted", "rest", "accidental"}`: what the next
///   item written takes.
/// - `layout` -- `{"layout"}`: how the window looks at the score.
/// - `page` -- `{"page", "paper", "landscape", "papers"}`: the page setup,
///   the name of its paper when it is a known one, which way up it is, and the
///   papers there are.
/// - `mei` -- `{"mei"}`: the score as MEI.
/// - `saved` -- the score as it stands is what its file holds: what a holder
///   says once it has written the file a save named. `{}`.
/// - `unsaved` -- `{"unsaved"}`: whether the score has changes its file does
///   not hold, which the File menu's Close asks about before it closes.
/// - `render` -- `{"sequence"}`: the score as the sequence it plays as, on the
///   engraver's time, or `{"error"}`.
///
/// A verb that edits (`act`) is the context's, since it leaves an entry. An
/// unknown verb answers `{}`.
pub fn call_json(editor: &mut ScoreEditor, request: &str) -> String {
    let Ok(request) = serde_json::from_str::<Value>(request) else {
        return "{}".into();
    };
    let id = |key: &str| request.get(key).and_then(Value::as_i64).map(|i| i as i32);
    match request
        .get("verb")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "window" => {
            // what the caller numbered, by the names the crate gave: a name
            // that is none of them numbers nothing
            let named = |key: &str| -> BTreeMap<String, i32> {
                request
                    .get(key)
                    .and_then(Value::as_object)
                    .map(|map| {
                        map.iter()
                            .filter_map(|(name, id)| Some((name.clone(), id.as_i64()? as i32)))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let mut chrome = Chrome {
                tools: named("tools"),
                dialogs: named("dialogs"),
                palettes: named("palettes"),
            };
            chrome
                .tools
                .retain(|name, _| tools::TOOLS.contains(&name.as_str()));
            let entries = palettes::names();
            chrome.palettes.retain(|name, _| entries.contains(name));
            editor
                .window(
                    Ids {
                        page: id("widget").unwrap_or(0),
                        scroll: id("scroll"),
                        status: id("status"),
                    },
                    chrome,
                )
                .to_string()
        }
        // a bare editor names no chrome, so a caller numbers none
        "tools" | "dialogs" | "palettes" if editor.bare => {
            let verb = request["verb"].as_str().unwrap_or_default();
            json!({verb: []}).to_string()
        }
        "tools" => json!({"tools": tools::TOOLS}).to_string(),
        "dialogs" => json!({"dialogs": dialogs::names()}).to_string(),
        "palettes" => json!({"palettes": palettes::names()}).to_string(),
        "chrome" => json!({"chrome": !editor.bare}).to_string(),
        "props" => {
            let widget = id("widget").unwrap_or(0);
            match editor
                .resync_widget(i64::from(widget))
                .into_iter()
                .find(|c| c.widget == i64::from(widget))
            {
                Some(c) => c.props.to_string(),
                None => "{}".into(),
            }
        }
        "sync" => {
            if let Some(window) = request.get("window") {
                editor.window = window.as_i64().map(|w| w as i32);
            }
            if let Some(title) = request.get("title").and_then(Value::as_str) {
                editor.title = title.into();
            }
            if let (Some(w), Some(h)) = (
                request.get("w").and_then(Value::as_i64),
                request.get("h").and_then(Value::as_i64),
            ) {
                editor.size = (w, h);
            }
            if let Some(entry) = request.get("entry").and_then(Value::as_bool) {
                editor.set_entry(entry);
            }
            // `null` is a score with no file; left out, the path stays
            if let Some(path) = request.get("path") {
                editor.path = path.as_str().filter(|p| !p.is_empty()).map(str::to_string);
            }
            if let Some(dotted) = request.get("dotted").and_then(Value::as_bool) {
                editor.dotted = dotted;
            }
            if let Some(rest) = request.get("rest").and_then(Value::as_bool) {
                editor.rest = rest;
            }
            // `null` lets the armed accidental go; left out, it stays
            if let Some(accidental) = request.get("accidental") {
                editor.accidental = accidental
                    .as_i64()
                    .map(|alter| alter as i32)
                    .filter(|alter| (-2..=2).contains(alter));
            }
            if let Some(view) = request
                .get("layout")
                .and_then(Value::as_str)
                .and_then(View::parse)
            {
                editor.view = view;
            }
            if let Some(value) = request
                .get("value")
                .and_then(|v| serde_json::from_value::<Ratio>(v.clone()).ok())
                .filter(Ratio::is_positive)
            {
                editor.value = value;
            }
            "{}".into()
        }
        "select" => {
            editor.selection = request
                .get("elements")
                .and_then(Value::as_array)
                .map(|ids| {
                    ids.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            "{}".into()
        }
        "selected" => json!({
            "elements": editor.selection,
            "items": editor.items(),
        })
        .to_string(),
        "value" => json!({"value": editor.value}).to_string(),
        "entry" => json!({"entry": editor.entry}).to_string(),
        "input" => json!({
            "value": editor.value,
            "dotted": editor.dotted,
            "rest": editor.rest,
            "accidental": editor.accidental,
        })
        .to_string(),
        "layout" => json!({"layout": editor.view.word()}).to_string(),
        "page" => {
            let setup = editor.setup();
            json!({
                "page": setup,
                "paper": setup.paper(),
                "landscape": setup.landscape(),
                "papers": PAPERS.iter().map(|p| p.name).collect::<Vec<_>>(),
            })
            .to_string()
        }
        "mei" => json!({"mei": editor.held().mei()}).to_string(),
        "saved" => {
            editor.mark_saved();
            "{}".into()
        }
        "unsaved" => json!({"unsaved": editor.unsaved()}).to_string(),
        "render" => match editor.rendered() {
            Ok(sequence) => json!({"sequence": sequence}).to_string(),
            Err(why) => json!({"error": why}).to_string(),
        },
        _ => "{}".into(),
    }
}

mod forms;
#[cfg(test)]
mod tests;
