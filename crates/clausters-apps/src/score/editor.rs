//! **The score editor, one turn at a time**: what a gesture on the page or a
//! verb a client calls does to the score, the entry it leaves, and what the
//! window is corrected with.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use clausters_core::notation::{
    AnyEngraver, NOTE, Op, PAPERS, Page, PageSetup, Pages, Score, View, field_of, item_id,
    layout_options, measure_id, sheet_to_mei,
};
use clausters_core::ratio::Ratio;
use clausters_editing::conversation::{self, Answer, Conversation, Correction};

use super::menu;
use super::verbs::{self, Action};
use super::{Ids, PAGE_GAP, Shared, Window, correction, scale_for, window};
use crate::turn::{self, Converse, Event, Kind, Leg, Record, int, text};

/// The vocabulary the editor's structure is registered under.
pub const DOMAIN: &str = "score";

/// What the status line says when nothing is selected.
pub const HINT: &str = "click a note, or press empty staff to write one";

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
}

turn::turned!(Outcome);

/// **A score editor**: a shared score, what is selected on its page, the value
/// a note is written with, and its end of the conversation.
#[derive(Clone)]
pub struct ScoreEditor {
    score: Shared,
    conversation: Conversation,
    window: Option<i32>,
    ids: Option<Ids>,
    title: String,
    size: (i64, i64),
    /// The selected elements, as the page names them (`n7`, `n7-2`), in the
    /// order they were picked.
    selection: Vec<String>,
    /// The written value a note entered on the page takes.
    value: Ratio,
    /// Whether a press on empty staff writes a note. Off, the same press on a
    /// staff selects the measure it fell in.
    entry: bool,
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
}

impl Default for Opened {
    fn default() -> Self {
        Self {
            title: "Score".into(),
            w: 960,
            h: 640,
            value: None,
            version: 1,
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
        {
            let mut held = score.lock().unwrap_or_else(|e| e.into_inner());
            let written = held.sheet().and_then(|sheet| sheet_to_mei(sheet).ok());
            if let Some(mei) = written
                && mei != held.mei()
            {
                held.load(&mei);
            }
        }
        Self {
            score,
            conversation: Conversation::new(version),
            window: None,
            ids: None,
            title: "Score".into(),
            size: (960, 640),
            selection: Vec::new(),
            value: Ratio::new(1, 4),
            entry: true,
            drawn: Vec::new(),
            view: View::Page,
            laid: String::new(),
        }
    }

    /// The score it edits.
    pub fn score(&self) -> &Shared {
        &self.score
    }

    fn held(&self) -> std::sync::MutexGuard<'_, Score<AnyEngraver>> {
        // A poisoned lock is a panic elsewhere while the score was held; the
        // document is still the score, and refusing it here would lose it.
        self.score.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// **The window**, with the page under `ids.page` -- which is then the
    /// widget the editor answers for.
    pub fn window(&mut self, ids: Ids) -> Value {
        self.ids = Some(ids);
        let page = self.page();
        self.drawn = page.draw.kinds.keys().cloned().collect();
        window(Window {
            page: &page,
            ids,
            title: &self.title,
            size: self.size,
            status: &self.describe(),
            entry: self.entry,
            scale: self.scale(),
            menu: self.menu(),
        })
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
            }
            return;
        }
        let picked = self.picked(element);
        match mode {
            "toggle" => {
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
            _ => self.selection = picked,
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
        match items.as_slice() {
            [] if self.selection.is_empty() => HINT.into(),
            [] => format!("{} is not one of this model's items", self.selection[0]),
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

    /// What `widget` is corrected with: the page, the scroll it sits in or the
    /// status line, as the score now stands -- nothing for another widget.
    fn resync_widget(&mut self, widget: i64) -> Vec<Correction> {
        let Some(ids) = self.ids else {
            return Vec::new();
        };
        let ours = [Some(ids.page), ids.scroll, ids.status]
            .into_iter()
            .flatten()
            .any(|id| i64::from(id) == widget);
        if !ours {
            return Vec::new();
        }
        self.corrections()
    }

    /// **Every widget of the window, corrected**: the page re-engraved, the
    /// scroll sized to it, the selection drawn and the status line told.
    fn corrections(&mut self) -> Vec<Correction> {
        let Some(ids) = self.ids else {
            return Vec::new();
        };
        let page = self.page();
        self.drawn = page.draw.kinds.keys().cloned().collect();
        // an item re-engraved may be drawn as other parts than it was
        let items = self.items();
        if !items.is_empty() {
            self.selection = self.elements_of(&items);
        }
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
        }
        if let Some(status) = ids.status {
            out.push(Correction {
                widget: i64::from(status),
                props: json!({"text": self.describe()}),
            });
        }
        // the bar shows the editor's state, so it is told when that may have
        // moved: a paper undone, a layout switched from a script
        if let Some(window) = self.window {
            out.push(Correction {
                widget: i64::from(window),
                props: json!({"menu": self.menu()}),
            });
        }
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
            Ok(action) => {
                let planned = {
                    let held = self.held();
                    match held.sheet() {
                        Some(sheet) => verbs::ops(sheet, &self.items(), &action),
                        None => Err("this document has no model to edit".into()),
                    }
                };
                planned
                    .and_then(|ops| self.edit(&ops, &action.label(), out))
                    .err()
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
                out.selected = Some(self.selection.clone());
            }
            menu::Pick::Layout(view) => self.view = view,
            menu::Pick::Entry(on) => self.entry = on,
            menu::Pick::Value(value) => self.value = value,
            menu::Pick::Unknown => reason = Some(format!("the menu has no entry for {verb}")),
        }
        out.turn = Kind::Route;
        out.answer = Some(conversation::answer(
            message.seq,
            out.version,
            reason,
            self.corrections(),
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
            // A press on empty staff named a place: a note of the value in
            // hand is written there, and selected.
            "insert" => {
                let after = values.first().map(text).unwrap_or_default();
                let position = values.get(1).map(int).unwrap_or(0) as i32;
                let staff = values.get(2).map(int).unwrap_or(0).max(0) as usize;
                let op = Op::Insert {
                    after: item_id(&after),
                    pitches: Vec::new(),
                    position: Some(position),
                    dur: self.value,
                    staff,
                    voice: 0,
                };
                let known = self.known_items();
                if let Err(why) = self.edit(&[op], "write a note", out) {
                    return (Some(why), self.corrections());
                }
                if let Some(new) = self.known_items().into_iter().find(|i| !known.contains(i)) {
                    self.selection = vec![format!("n{new}")];
                    out.selected = Some(self.selection.clone());
                }
                (None, self.corrections())
            }
            _ => (None, Vec::new()),
        }
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
        out
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

    /// **A pick of the menu bar is the window's own verb**: the bar is the
    /// window's, so the pick arrives addressed to it rather than to a widget.
    fn window_verb(
        &mut self,
        message: &conversation::Message,
        args: &[Value],
        out: &mut Outcome,
    ) -> bool {
        if message.addr != "/gui_event" || !message.is_window || message.tag != "menu" {
            return false;
        }
        self.pick(message, args, out);
        true
    }
}

/// **A score editor from JSON**: `{"title", "w", "h", "value", "version"}`
/// over `score`.
pub fn new_json(score: Shared, request: &str) -> ScoreEditor {
    let opened: Opened = serde_json::from_str(request).unwrap_or_default();
    let mut editor = ScoreEditor::new(score, opened.version);
    editor.title = opened.title;
    editor.size = (opened.w, opened.h);
    if let Some(value) = opened.value.filter(Ratio::is_positive) {
        editor.value = value;
    }
    editor
}

/// **One verb of the editor's own door**, over JSON:
///
/// - `window` -- `widget` (the page), `scroll`, `status`: the GuiDef.
/// - `props` -- `widget`: what it is corrected with (`{}` for another widget).
/// - `sync` -- `window` (the id it is open in, or `null`), `title`, `w`, `h`,
///   `value` (the written value a note is entered with, `[n, d]`), `entry`
///   (whether a press on empty staff writes a note), `layout` (`"page"` or
///   `"continuous"`, how the window looks at the score): `{}`.
/// - `select` -- `elements`: the page's element ids to select. `{}`.
/// - `selected` -- `{"elements", "items"}`: what is selected, as the page and
///   the model name it.
/// - `value` -- `{"value"}`: the written value a note is entered with, `[n, d]`.
/// - `entry` -- `{"entry"}`: whether a press on empty staff writes a note.
/// - `layout` -- `{"layout"}`: how the window looks at the score.
/// - `page` -- `{"page", "paper", "landscape", "papers"}`: the page setup,
///   the name of its paper when it is a known one, which way up it is, and the
///   papers there are.
/// - `mei` -- `{"mei"}`: the score as MEI.
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
        "window" => editor
            .window(Ids {
                page: id("widget").unwrap_or(0),
                scroll: id("scroll"),
                status: id("status"),
            })
            .to_string(),
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
                editor.entry = entry;
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
        _ => "{}".into(),
    }
}

#[cfg(test)]
mod tests;
