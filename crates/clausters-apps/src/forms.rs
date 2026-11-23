//! **An application's dialogs**: forms a menu entry opens over the window
//! while it is up, one mechanism for every application that has any.
//!
//! A dialog is a node of the tree and an editor answers with props, never with
//! nodes, so every dialog is **in the window from the start, on a page of a
//! `stack`**: a hidden page is not placed, which is a dialog that is not up,
//! and opening one is the stack's `index` set to its page, with its fields
//! corrected to what the editor holds. Nothing is defined and nothing is freed,
//! so the standalone host and every client open the same dialog by the same
//! correction.
//!
//! An application says what its forms are ([`Spec`]) and the widgets are named
//! here ([`names`]) and numbered by the caller, as the tools are; a window
//! composed without them has no dialogs, and the menu entries that would open
//! one are disabled.
//!
//! A field reports its text as it is typed, which the editor keeps ([`Open`]);
//! nothing reaches the edited structure until `OK`.
//!
//! **A field may be chosen rather than typed** ([`Spec::chooser`]): it is
//! then the host's `files` chooser, which lists the disk in a window and the
//! page's storage in a tab, so no application builds a browser of its own.
//! **It is every application's Open and Save As**, the code editor's too
//! ([`chooser_widget`], [`field_row`], [`footer`]): a chooser that opens
//! chooses files, and one that saves ([`Chooser::save`]) fills the name
//! field under it ([`NAME`]) with the file it selects, a typed name being
//! in the folder it shows ([`Open::path`]).
//! The chooser keeps what it is doing -- the directory, the listing it waits
//! on, an error -- and reports what the hand chose; the form keeps the paths
//! chosen and the directory the field opens in; and what the paths are for
//! is the application's, read at `OK` or at a pick ([`Said::Picked`]).
//!
//! **A file dialog manages the storage it shows** ([`storage`]): under its
//! chooser are the tools -- a new folder, a rename, a delete, and in a page
//! an upload and a download -- and a line they speak in, so every
//! application's Open and Save As has them, as the code editor's does. A
//! form whose chooser fills a name field ([`NAME`]) takes the name a tool
//! needs from it, and an Open form opens a path typed there as one chosen.
//!
//! **A field may be one of several** ([`Spec::options`]): it is then the
//! host's `choice`, and what the field holds is the text of the option
//! chosen, as a typed field holds its text.
//!
//! [`Dialogs`] is the whole mechanism held for one window: the ids, the form
//! that is up, what its choosers chose and where each opens again. An
//! application that holds one opens, reads and hides its forms through it,
//! and a window drawn again shows the form that was up.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

pub mod storage;

use storage::{SAYS, Storage, Tool};

/// The caller's widget id for each widget of the dialogs it numbered.
pub type Ids = BTreeMap<String, i32>;

/// The stack the dialogs are pages of.
pub const STACK: &str = "stack";

/// **What an application's form is**: everything the mechanism needs to name,
/// build, show and read it.
pub trait Spec: Copy + PartialEq {
    /// The prefix its widgets are named under.
    fn prefix(self) -> &'static str;
    /// The page of the stack it is on; page 0 is no dialog.
    fn page(self) -> i32;
    /// What its dialog is titled.
    fn title(self) -> &'static str;
    /// Its fields, by name, each with what it is labelled.
    fn fields(self) -> Vec<(&'static str, &'static str)>;
    /// The caption a form that asks several questions corrects when it is
    /// shown (`prefix:label`), or `None` for a form whose captions stand.
    fn label(self) -> Option<&'static str> {
        None
    }
    /// **The question a close form asks**, for the form that is one: it is
    /// built by [`crate::closing::dialog`], every application's.
    fn closes(self) -> Option<&'static str> {
        None
    }
    /// **What its accepting button says**: `OK`, or the verb a file form
    /// performs (`Open`, `Save`, `Export`).
    fn accept(self) -> &'static str {
        "OK"
    }
    /// Props a field adds to the text widget it is (a multiline one).
    fn field(self, _name: &str, _widget: &mut Map<String, Value>) {}
    /// **Whether field `name` is chosen in the host's file chooser** rather
    /// than typed, and how; `None` (the default) for a text field.
    fn chooser(self, _name: &str) -> Option<Chooser> {
        None
    }
    /// **The options field `name` is chosen among** rather than typed: each
    /// the text the field then holds, shown in the host's `choice`. `None`
    /// (the default) for a text field.
    fn options(self, _name: &str) -> Option<&'static [&'static str]> {
        None
    }
}

/// Where `text` is among `options`: the first when it is none of them.
fn option_index(options: &[&str], text: &str) -> usize {
    options.iter().position(|o| *o == text).unwrap_or(0)
}

/// **A field chosen in the host's file chooser**: which files it shows,
/// whether several can be chosen at once, whether it saves, and how much
/// room it takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chooser {
    /// The extensions a file must have to be shown, without the dot; empty
    /// shows every file.
    pub filter: &'static [&'static str],
    /// Whether Ctrl and Shift choose several files.
    pub multiple: bool,
    /// **Whether it says where a file is written**: the file it selects
    /// fills the form's name field ([`NAME`]), and a name typed there is in
    /// the folder it shows.
    pub saves: bool,
    /// Its width and height, in logical pixels.
    pub w: f64,
    pub h: f64,
}

/// **The field a saving chooser fills**, which a form that saves lists
/// after it: the file's name, or a path from the root.
pub const NAME: &str = "name";

/// How large every file chooser is drawn, in logical pixels.
pub const CHOOSER_W: f64 = 520.0;
pub const CHOOSER_H: f64 = 300.0;

impl Chooser {
    /// **A chooser that opens** the files `filter` shows: one, or several
    /// with Ctrl and Shift.
    #[must_use]
    pub const fn open(filter: &'static [&'static str], multiple: bool) -> Self {
        Chooser {
            filter,
            multiple,
            saves: false,
            w: CHOOSER_W,
            h: CHOOSER_H,
        }
    }

    /// **A chooser that saves**, listing the files `filter` shows beside
    /// the one written.
    #[must_use]
    pub const fn save(filter: &'static [&'static str]) -> Self {
        Chooser {
            saves: true,
            ..Chooser::open(filter, false)
        }
    }
}

/// **The `files` widget a chooser is**, numbered `id`, listing `dir`; it
/// fills the field numbered `name`, where there is one, with the file it
/// selects.
#[must_use]
pub fn chooser_widget(id: Option<i32>, dir: &str, chooser: &Chooser, name: Option<i32>) -> Value {
    let mut widget = json!({
        "type": "files",
        "id": id,
        "path": Some(dir).filter(|dir| !dir.is_empty()).unwrap_or(START),
        "filter": chooser.filter,
        "multiple": chooser.multiple,
        "weight": 1,
        "h": chooser.h,
    });
    if let (Some(name), Some(map)) = (name, widget.as_object_mut()) {
        map.insert("bind".into(), json!(["widget", name, "value"]));
    }
    widget
}

/// **A field of a dialog**: its caption over it, as the design system's
/// form sets one, in the caption's weight -- none for an empty caption --
/// `w` wide where it is given. `caption` is the caption's id, for a form
/// that corrects it.
#[must_use]
pub fn field_row(caption: &str, caption_id: Option<i32>, field: Value, w: Option<f64>) -> Value {
    let mut children = Vec::new();
    if !caption.is_empty() {
        let mut label = json!({"type": "label", "text": caption, "variant": "caption"});
        if let (Some(id), Some(map)) = (caption_id, label.as_object_mut()) {
            map.insert("id".into(), json!(id));
        }
        children.push(label);
    }
    children.push(field);
    let mut row = json!({
        "type": "layout",
        "flow": "col",
        "hug": true,
        "margin": 0,
        "gap": CAPTION_GAP,
        "children": children,
    });
    if let (Some(w), Some(map)) = (w, row.as_object_mut()) {
        map.insert("w".into(), json!(w));
    }
    row
}

/// **A dialog's footer**: the buttons at its end, `Cancel` and the one
/// that accepts, saying `accept`.
#[must_use]
pub fn footer(cancel: Option<i32>, ok: Option<i32>, accept: &str) -> Value {
    let spring = json!({"type": "separator", "weight": 1, "line": false});
    let cancel = json!({"type": "button", "id": cancel, "label": "Cancel", "variant": "outline"});
    let ok = json!({"type": "button", "id": ok, "label": accept, "default": true,
        "variant": "default"});
    json!({
        "type": "layout",
        "flow": "row",
        "hug": true,
        "pack": true,
        "margin": 0,
        "gap": crate::closing::FOOTER_GAP,
        "children": [spring, cancel, ok],
    })
}

/// The parent folder and the name of the file at `path`.
fn split_path(path: &str) -> (String, String) {
    let path = std::path::Path::new(path);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = path
        .parent()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    (dir, name)
}

/// The directory a chooser opens in before the hand has been anywhere: the
/// one the host resolves `.` to -- the working directory natively, the
/// storage's root in a page.
pub const START: &str = ".";

/// **Every widget of the dialogs, by name**, for a caller to number: the
/// stack; and for each form its dialog (`text`), its fields (`text:title`),
/// the caption a form corrects (`param:label`) and its two buttons
/// (`text:ok`, `text:cancel`) -- three for the close form, whose third closes
/// without saving (`close:discard`).
#[must_use]
pub fn names<F: Spec>(forms: &[F]) -> Vec<String> {
    let mut out = vec![STACK.to_string()];
    for form in forms {
        let prefix = form.prefix();
        out.push(prefix.to_string());
        out.extend(
            form.fields()
                .iter()
                .map(|(name, _)| format!("{prefix}:{name}")),
        );
        if form.label().is_some() {
            out.push(format!("{prefix}:label"));
        }
        out.push(format!("{prefix}:ok"));
        out.push(format!("{prefix}:cancel"));
        if form.closes().is_some() {
            out.push(format!("{prefix}:discard"));
        }
        // a file form's tools, and the line they speak in
        if has_chooser(*form) {
            out.extend(
                Tool::ALL
                    .iter()
                    .filter(|t| t.shown())
                    .map(|t| format!("{prefix}:{}", t.name())),
            );
            out.push(format!("{prefix}:{SAYS}"));
        }
    }
    out
}

/// Whether `form` chooses a file: it is then a file dialog, with the
/// storage's tools.
fn has_chooser<F: Spec>(form: F) -> bool {
    form.fields()
        .iter()
        .any(|(name, _)| form.chooser(name).is_some())
}

/// Whether `ids` numbers every widget of the dialogs: a window has all of
/// them or none.
#[must_use]
pub fn numbered<F: Spec>(forms: &[F], ids: &Ids) -> bool {
    names(forms).iter().all(|name| ids.contains_key(name))
}

/// Between a field's caption and the field under it, in logical pixels:
/// the specification's.
const CAPTION_GAP: f64 = 8.0;

/// Between the fields of a form, in logical pixels: the specification's
/// gap inside a dialog's body.
const BODY_GAP: f64 = 16.0;

/// One form as a dialog: a row per field, and `Cancel` and `OK` under them --
/// or, closing, the close form every application asks in
/// ([`crate::closing::dialog`]).
fn dialog<F: Spec>(form: F, ids: &Ids, up: Option<&Open<F>>, says_now: &str) -> Value {
    let prefix = form.prefix();
    let id = |name: &str| ids.get(&format!("{prefix}:{name}")).copied();
    if let Some(question) = form.closes() {
        return crate::closing::dialog(ids.get(prefix).copied(), question, id);
    }
    // what a field holds: the form's text when it is the one up, else nothing
    let held = |name: &str| up.map_or("", |open| open.value(name));
    let rows: Vec<Value> = form
        .fields()
        .iter()
        .flat_map(|(name, label)| {
            let chooser = form.chooser(name);
            let mut field = match chooser {
                Some(chooser) => chooser_widget(id(name), held(name), &chooser, id(NAME)),
                None => match form.options(name) {
                    Some(options) => json!({
                        "type": "choice",
                        "id": id(name),
                        "options": options,
                        "index": option_index(options, held(name)),
                    }),
                    None => json!({"type": "text", "id": id(name), "value": held(name)}),
                },
            };
            if let Some(map) = field.as_object_mut() {
                form.field(name, map);
            }
            // the caption a form corrects is the question it is asking; the
            // field a chooser stands in is as wide as the chooser, so the
            // dialog takes its width
            let caption = form.label().and_then(|_| id("label"));
            let row = field_row(label, caption, field, chooser.map(|c| c.w));
            // the storage's tools, under the chooser that shows it
            match chooser {
                Some(_) => vec![row, storage::tools_row(|tool| id(tool.name()))],
                None => vec![row],
            }
        })
        .collect();
    let mut rows = rows;
    if has_chooser(form) {
        let says = up.map_or("", |_| says_now);
        rows.push(storage::says_line(id(SAYS), says));
    }
    let buttons = footer(id("cancel"), id("ok"), form.accept());
    // the fields are the dialog's body, closer to each other than the
    // dialog's parts are
    let body = json!({
        "type": "layout",
        "flow": "col",
        "hug": true,
        "margin": 0,
        "gap": BODY_GAP,
        "children": rows,
    });
    json!({
        "type": "layout",
        "id": ids.get(prefix),
        "modal": true,
        "hug": true,
        "title": form.title(),
        "flow": "col",
        "children": [body, buttons],
    })
}

/// **The dialogs, as the stack the window holds** -- or `None` when the
/// caller did not number them. It takes no room in the window's flow: page 0
/// is empty, and each form's dialog is alone on a page of its own, in the
/// order `forms` gives, which is the order of their pages.
#[must_use]
pub fn stack<F: Spec>(forms: &[F], ids: &Ids) -> Option<Value> {
    stack_with(forms, ids, None, "")
}

/// The stack, turned to the page of the form `up` with its fields holding
/// what it holds -- the window drawn again while a dialog is up. **Forms may
/// share a page**, a dialog that serves several questions: `forms` names the
/// page by one of them, and a page is drawn as the question it is asking.
fn stack_with<F: Spec>(forms: &[F], ids: &Ids, up: Option<&Open<F>>, says: &str) -> Option<Value> {
    if !numbered(forms, ids) {
        return None;
    }
    let up = up.filter(|open| forms.iter().any(|form| form.page() == open.form.page()));
    let pages: Vec<Value> = std::iter::once(json!({"type": "layout"}))
        .chain(forms.iter().map(|form| {
            let open = up.filter(|open| open.form.page() == form.page());
            let asked = open.map_or(*form, |open| open.form);
            json!({"type": "layout", "children": [dialog(asked, ids, open, says)]})
        }))
        .collect();
    Some(json!({
        "type": "layout",
        "id": ids.get(STACK),
        "flow": "stack",
        "index": up.map_or(0, |open| open.form.page()),
        "margin": 0,
        "h": 0,
        "children": pages,
    }))
}

/// **What a widget of the dialogs said.**
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Said {
    /// A field now holds this text.
    Field(String, String),
    /// `OK`.
    Accept,
    /// `Cancel`, Escape, or the dialog's close mark.
    Cancel,
    /// The close form's `Don't save`.
    Discard,
    /// A chooser's selection is now these files.
    Chosen(String, Vec<String>),
    /// A chooser's files were picked -- a double click or Enter on a file --
    /// which accepts them as `OK` would.
    Picked(String, Vec<String>),
    /// A chooser went into this directory.
    Entered(String, String),
    /// A chooser's change of the storage came to its end: why it was
    /// refused, or nothing.
    Changed(String, String),
    /// A tool of the storage was pressed.
    Tool(Tool),
    /// **What the window is corrected with**, and nothing else to do: what
    /// a tool asked of the chooser and what the tools say. [`Dialogs::read`]
    /// answers it for [`Said::Tool`] and [`Said::Changed`].
    Corrected(Vec<(i32, Value)>),
}

/// The paths a chooser's report carries, whether as arguments or as one
/// array of them.
fn paths(values: &[Value]) -> Vec<String> {
    values
        .iter()
        .flat_map(|v| v.as_array().cloned().unwrap_or_else(|| vec![v.clone()]))
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

/// **What the widget `name` reporting `tag` said**, with the `values` after
/// the tag -- or `None` for a name that is no widget of `form`, and for a
/// report that says nothing: a button's value, which rises and falls with the
/// hand, and a field's **focus**, which reports `"focus" 1|0` where its text
/// reports the text alone. The trailing number is what tells the two apart, so
/// a field holding the word `focus` is still a field.
#[must_use]
pub fn read<F: Spec>(form: F, name: &str, tag: &str, values: &[Value]) -> Option<Said> {
    if tag == "focus" && values.first().is_some_and(Value::is_number) {
        return None;
    }
    let prefix = form.prefix();
    if name == prefix {
        return (tag == "cancel").then_some(Said::Cancel);
    }
    let part = name.strip_prefix(prefix)?.strip_prefix(':')?;
    // A chooser reports what the hand did by its tag; choosing one file, its
    // selection is its value, which arrives as the tag itself.
    if let Some(chooser) = form.chooser(part) {
        let field = part.to_string();
        return match tag {
            "changed" => Some(Said::Changed(
                field,
                values
                    .first()
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )),
            "select" => Some(Said::Chosen(field, paths(values))),
            "pick" => Some(Said::Picked(field, paths(values))),
            "path" => values
                .first()
                .and_then(Value::as_str)
                .map(|dir| Said::Entered(field, dir.to_string())),
            path if !chooser.multiple => Some(Said::Chosen(field, vec![path.to_string()])),
            _ => None,
        };
    }
    // A choice reports the index of what was chosen, which the field holds
    // as the option's text.
    if let Some(options) = form.options(part) {
        let index = tag.parse::<f64>().ok()?.round().max(0.0) as usize;
        let option = options.get(index)?;
        return Some(Said::Field(part.to_string(), (*option).to_string()));
    }
    if let Some(tool) = Tool::named(part).filter(|_| has_chooser(form)) {
        return (tag == "click").then_some(Said::Tool(tool));
    }
    match part {
        "ok" => (tag == "click").then_some(Said::Accept),
        "cancel" => (tag == "click").then_some(Said::Cancel),
        "discard" if form.closes().is_some() => (tag == "click").then_some(Said::Discard),
        field if form.fields().iter().any(|(name, _)| *name == field) => {
            Some(Said::Field(field.to_string(), tag.to_string()))
        }
        _ => None,
    }
}

/// **A form that is up**: which, and what its fields hold as they are typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Open<F> {
    pub form: F,
    pub values: BTreeMap<String, String>,
}

impl<F: Spec> Open<F> {
    /// What the field `name` holds.
    #[must_use]
    pub fn value(&self, name: &str) -> &str {
        self.values.get(name).map_or("", String::as_str)
    }

    /// **The path a saving chooser `field` says**: the name typed or the
    /// file selected ([`NAME`]), in the folder the chooser shows unless it
    /// is a path from the root -- or `None` with no name.
    #[must_use]
    pub fn path(&self, field: &str) -> Option<String> {
        let name = self.value(NAME).trim();
        if name.is_empty() {
            return None;
        }
        let named = std::path::Path::new(name);
        if named.has_root() {
            return Some(name.to_string());
        }
        // in the folder the host starts in, a name is already where it is
        let dir = self.value(field);
        if dir.is_empty() || dir == START {
            return Some(name.to_string());
        }
        Some(
            std::path::Path::new(dir)
                .join(named)
                .to_string_lossy()
                .into_owned(),
        )
    }

    /// **What the window is corrected with to show it**: every field's text,
    /// the caption it asks with, its title, and the stack turned to its page.
    #[must_use]
    pub fn shown(&self, ids: &Ids) -> Vec<(i32, Value)> {
        let prefix = self.form.prefix();
        let mut out: Vec<(i32, Value)> = self
            .form
            .fields()
            .iter()
            .filter_map(|(name, _)| {
                let id = *ids.get(&format!("{prefix}:{name}"))?;
                // A chooser's text is the directory it opens in; setting its
                // path lists it afresh with nothing selected, and the keyboard
                // goes to it, where Enter picks and the arrows move.
                Some(match (self.form.chooser(name), self.form.options(name)) {
                    (Some(chooser), _) => {
                        let dir = Some(self.value(name)).filter(|d| !d.is_empty());
                        let props = json!({"path": dir.unwrap_or(START), "focus": 1,
                            "filter": chooser.filter});
                        (id, props)
                    }
                    (None, Some(options)) => (
                        id,
                        json!({"index": option_index(options, self.value(name))}),
                    ),
                    (None, None) => (id, json!({"value": self.value(name)})),
                })
            })
            .collect();
        if let (Some(label), Some(id)) = (self.form.label(), ids.get(&format!("{prefix}:label"))) {
            out.push((*id, json!({"text": label})));
        }
        // a dialog that serves several questions says which it is asking,
        // and what its accepting button does
        if let Some(id) = ids.get(prefix) {
            out.push((*id, json!({"title": self.form.title()})));
        }
        if let Some(id) = ids.get(&format!("{prefix}:ok")) {
            out.push((*id, json!({"label": self.form.accept()})));
        }
        if let Some(stack) = ids.get(STACK) {
            out.push((*stack, json!({"index": self.form.page()})));
        }
        out
    }
}

/// **What a window is corrected with**, as the editor's answer carries it:
/// the pairs [`Dialogs::open`] and [`Dialogs::hide`] answer.
#[must_use]
pub fn corrections(list: Vec<(i32, Value)>) -> Vec<clausters_editing::conversation::Correction> {
    list.into_iter()
        .map(
            |(widget, props)| clausters_editing::conversation::Correction {
                widget: i64::from(widget),
                props,
            },
        )
        .collect()
}

/// What the stack is corrected with to take a dialog down.
#[must_use]
pub fn hidden(ids: &Ids) -> Vec<(i32, Value)> {
    ids.get(STACK)
        .map(|stack| (*stack, json!({"index": 0})))
        .into_iter()
        .collect()
}

/// **The dialogs of one window**, held whole: the ids its widgets are
/// numbered with, the form that is up, the files its choosers have chosen,
/// and the directory each chooser opens in next -- the last one the hand
/// went into, kept from one opening to the next.
#[derive(Clone, Debug, PartialEq)]
pub struct Dialogs<F> {
    pub ids: Ids,
    up: Option<Open<F>>,
    chosen: BTreeMap<String, Vec<String>>,
    dirs: BTreeMap<String, String>,
    /// The storage's tools of the file form up.
    storage: Storage,
}

impl<F> Default for Dialogs<F> {
    fn default() -> Self {
        Dialogs {
            ids: Ids::new(),
            up: None,
            chosen: BTreeMap::new(),
            dirs: BTreeMap::new(),
            storage: Storage::default(),
        }
    }
}

impl<F: Spec> Dialogs<F> {
    /// Whether the window numbered every widget of `forms`.
    #[must_use]
    pub fn numbered(&self, forms: &[F]) -> bool {
        numbered(forms, &self.ids)
    }

    /// The form that is up, and what its fields hold.
    #[must_use]
    pub fn up(&self) -> Option<&Open<F>> {
        self.up.as_ref()
    }

    /// The name of the widget of the dialogs numbered `widget`.
    #[must_use]
    pub fn name(&self, widget: i64) -> Option<&str> {
        self.ids
            .iter()
            .find(|(_, id)| i64::from(**id) == widget)
            .map(|(name, _)| name.as_str())
    }

    /// **The files the chooser `field` of the form up has chosen.**
    #[must_use]
    pub fn chosen(&self, field: &str) -> &[String] {
        self.chosen.get(field).map_or(&[], Vec::as_slice)
    }

    /// **The stack for a window drawn now**: turned to the form that is up,
    /// its fields holding what they hold. The choosers in it are new, with
    /// nothing selected, so what they had chosen is forgotten with them.
    pub fn stack(&mut self, forms: &[F]) -> Option<Value> {
        let fields: Vec<String> = self.chosen.keys().cloned().collect();
        for field in fields {
            self.forget_selection(&field);
        }
        self.chosen.clear();
        stack_with(forms, &self.ids, self.up.as_ref(), &self.storage.says)
    }

    /// **Opens `form`**, its fields holding `values`, and answers what the
    /// window is corrected with to show it. A chooser opens where the hand
    /// last left it, and with nothing chosen.
    ///
    /// A saving chooser is given the path of the file the window writes,
    /// when it has one: it opens in that file's folder, the file's name in
    /// the name field.
    pub fn open(&mut self, form: F, mut values: BTreeMap<String, String>) -> Vec<(i32, Value)> {
        let prefix = form.prefix();
        for (name, _) in form.fields() {
            let Some(chooser) = form.chooser(name) else {
                continue;
            };
            let mut dir = self.dirs.get(&format!("{prefix}:{name}")).cloned();
            let given = values.get(name).filter(|p| !p.trim().is_empty()).cloned();
            if let (true, Some(file)) = (chooser.saves, given) {
                let (folder, file) = split_path(&file);
                if !folder.is_empty() {
                    dir = Some(folder);
                }
                values.insert(NAME.to_string(), file);
            }
            values.insert(name.to_string(), dir.unwrap_or_else(|| START.into()));
        }
        let open = Open { form, values };
        let mut shown = open.shown(&self.ids);
        // the tools start with nothing to say
        self.storage.reset();
        if let Some(says) = self.ids.get(&format!("{prefix}:{SAYS}")) {
            shown.insert(0, (*says, storage::says_props("")));
        }
        self.up = Some(open);
        self.chosen.clear();
        shown
    }

    /// **Takes the dialog down**, and answers the window's correction.
    pub fn hide(&mut self) -> Vec<(i32, Value)> {
        self.up = None;
        self.chosen.clear();
        hidden(&self.ids)
    }

    /// The window is gone, and no dialog is up in it; where the choosers
    /// open next is kept.
    pub fn closed(&mut self) {
        self.up = None;
        self.chosen.clear();
    }

    /// **What the widget `name` of the form up said**, kept as it says it: a
    /// field's text, a chooser's selection and the directory it went into.
    /// `None` when no form is up, for a widget of another form, and for a
    /// report that says nothing. Taking the dialog down is the caller's,
    /// once it has read what it needs.
    pub fn read(&mut self, name: &str, tag: &str, values: &[Value]) -> Option<Said> {
        let said = read(self.up.as_ref()?.form, name, tag, values)?;
        match said {
            Said::Tool(tool) => return Some(Said::Corrected(self.tool(tool))),
            Said::Changed(field, why) => {
                self.storage.changed(&why);
                self.chosen.remove(&field);
                return Some(Said::Corrected(self.says()));
            }
            _ => {}
        }
        let open = self.up.as_mut()?;
        let saves = |field: &str| open.form.chooser(field).is_some_and(|c| c.saves);
        let names = open.form.fields().iter().any(|(name, _)| *name == NAME);
        match &said {
            Said::Field(field, text) => {
                open.values.insert(field.clone(), text.clone());
            }
            // a saving chooser's file is the name the form writes to, and a
            // pick of one accepts it
            Said::Chosen(field, paths) if saves(field) => {
                if let Some(path) = paths.first() {
                    open.values.insert(NAME.to_string(), path.clone());
                }
                self.chosen.insert(field.clone(), paths.clone());
            }
            Said::Picked(field, paths) if saves(field) => {
                if let Some(path) = paths.first() {
                    open.values.insert(NAME.to_string(), path.clone());
                }
                return Some(Said::Accept);
            }
            // what a pick picks is what is chosen, for a form that reads it
            // as `OK`, and the name field follows it
            Said::Chosen(field, paths) | Said::Picked(field, paths) => {
                if let (true, Some(path)) = (names, paths.last()) {
                    open.values.insert(NAME.to_string(), path.clone());
                }
                self.chosen.insert(field.clone(), paths.clone());
            }
            Said::Entered(field, dir) => {
                open.values.insert(field.clone(), dir.clone());
                self.dirs
                    .insert(format!("{}:{field}", open.form.prefix()), dir.clone());
                let field = field.clone();
                let cleared = self.forget_selection(&field);
                self.chosen.remove(&field);
                if !cleared.is_empty() {
                    return Some(Said::Corrected(cleared));
                }
            }
            // a path typed in an Open form is opened as one chosen
            Said::Accept if names => {
                for (field, _) in open.form.fields() {
                    let typed = open.path(field);
                    if let (Some(_), Some(typed)) = (open.form.chooser(field), typed) {
                        let chosen = self.chosen.entry(field.to_string()).or_default();
                        if !chosen.contains(&typed) {
                            *chosen = vec![typed];
                        }
                    }
                }
            }
            Said::Accept
            | Said::Cancel
            | Said::Discard
            | Said::Changed(..)
            | Said::Tool(_)
            | Said::Corrected(_) => {}
        }
        Some(said)
    }

    /// **A tool pressed in the file form up**: the change its chooser is
    /// asked for, and what the tools now say.
    fn tool(&mut self, tool: Tool) -> Vec<(i32, Value)> {
        let Some(open) = self.up.as_mut() else {
            return Vec::new();
        };
        let prefix = open.form.prefix();
        let Some(field) = open
            .form
            .fields()
            .into_iter()
            .map(|(name, _)| name)
            .find(|name| open.form.chooser(name).is_some())
        else {
            return Vec::new();
        };
        let dir = open.value(field).to_string();
        let selected = self.chosen.get(field).and_then(|c| c.last()).cloned();
        let typed = open.path(field);
        let pressed = self
            .storage
            .press(tool, &dir, selected.as_deref(), typed.as_deref());
        let mut out = Vec::new();
        let chooser = self.ids.get(&format!("{prefix}:{field}")).copied();
        if let (Some(to), Some(id)) = (&pressed.go, chooser) {
            open.values.insert(field.to_string(), to.clone());
            self.dirs.insert(format!("{prefix}:{field}"), to.clone());
            out.push((id, json!({"path": to})));
        }
        if let (Some(change), Some(id)) = (pressed.change, chooser) {
            out.push((id, json!({"change": change})));
            self.chosen.remove(field);
        }
        if pressed.clear_name {
            open.values.insert(NAME.to_string(), String::new());
            if let Some(id) = self.ids.get(&format!("{prefix}:{NAME}")) {
                out.push((*id, json!({"value": ""})));
            }
        }
        out.extend(self.says());
        out
    }

    /// **The name a chooser's selection put in the field, forgotten** with
    /// the selection: a file chosen in a folder left behind, or in a
    /// chooser drawn again, is no longer what an Open form opens. A name
    /// typed stays, and so does a saving form's, which is the file it
    /// writes. Answers the field's correction.
    fn forget_selection(&mut self, field: &str) -> Vec<(i32, Value)> {
        let Some(open) = self.up.as_mut() else {
            return Vec::new();
        };
        let opens = open.form.chooser(field).is_some_and(|c| !c.saves);
        let name = open.value(NAME).to_string();
        let selected = self.chosen.get(field).is_some_and(|c| c.contains(&name));
        if !(opens && selected) {
            return Vec::new();
        }
        open.values.insert(NAME.to_string(), String::new());
        self.ids
            .get(&format!("{}:{NAME}", open.form.prefix()))
            .map(|id| (*id, json!({"value": ""})))
            .into_iter()
            .collect()
    }

    /// What the line the tools speak in is corrected with.
    fn says(&self) -> Vec<(i32, Value)> {
        let Some(open) = self.up.as_ref() else {
            return Vec::new();
        };
        self.ids
            .get(&format!("{}:{SAYS}", open.form.prefix()))
            .map(|id| (*id, storage::says_props(&self.storage.says)))
            .into_iter()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A form with a text field and a chooser, as an application would have.
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Form {
        Name,
        Load,
    }

    impl Spec for Form {
        fn prefix(self) -> &'static str {
            match self {
                Form::Name => "name",
                Form::Load => "load",
            }
        }
        fn page(self) -> i32 {
            match self {
                Form::Name => 1,
                Form::Load => 2,
            }
        }
        fn title(self) -> &'static str {
            "Form"
        }
        fn fields(self) -> Vec<(&'static str, &'static str)> {
            match self {
                Form::Name => vec![("text", "Text")],
                Form::Load => vec![("files", "Files")],
            }
        }
        fn chooser(self, name: &str) -> Option<Chooser> {
            (self == Form::Load && name == "files").then_some(Chooser {
                w: 400.0,
                ..Chooser::open(&["wav"], true)
            })
        }
    }

    const FORMS: [Form; 2] = [Form::Name, Form::Load];

    fn dialogs() -> Dialogs<Form> {
        Dialogs {
            ids: names(&FORMS)
                .into_iter()
                .enumerate()
                .map(|(i, name)| (name, 100 + i as i32))
                .collect(),
            ..Dialogs::default()
        }
    }

    fn find(tree: &Value, id: i32) -> Option<&Value> {
        if tree["id"] == id {
            return Some(tree);
        }
        tree["children"]
            .as_array()?
            .iter()
            .find_map(|child| find(child, id))
    }

    #[test]
    fn a_chooser_field_is_the_host_s_files_widget_with_its_filter_and_size() {
        let d = dialogs();
        let stack = stack(&FORMS, &d.ids).unwrap();
        let chooser = find(&stack, d.ids["load:files"]).unwrap();
        assert_eq!(chooser["type"], "files");
        assert_eq!(chooser["path"], START);
        assert_eq!(chooser["filter"], json!(["wav"]));
        assert_eq!(chooser["multiple"], true);
        // The chooser takes what its row leaves its caption, which is only
        // as wide as its text: the row is as wide as a caption column and
        // the chooser's width together.
        assert_eq!(
            (chooser["weight"].as_f64(), chooser["h"].as_f64()),
            (Some(1.0), Some(300.0))
        );
        assert!(chooser.get("w").is_none());
        fn row_of(tree: &Value) -> Option<&Value> {
            if tree["children"][1]["type"] == "files" {
                return Some(tree);
            }
            tree["children"].as_array()?.iter().find_map(row_of)
        }
        let row = row_of(&stack).unwrap();
        assert_eq!(row["w"].as_f64(), Some(400.0));
        assert_eq!(row["flow"], "col", "the caption stands over the chooser");
        assert_eq!(row["children"][0]["variant"], "caption");
        assert!(
            chooser.get("value").is_none(),
            "a chooser is not typed into"
        );
        assert_eq!(find(&stack, d.ids["name:text"]).unwrap()["type"], "text");
    }

    #[test]
    fn a_chooser_reports_its_selection_its_pick_and_where_it_went() {
        let load = Form::Load;
        assert_eq!(
            read(
                load,
                "load:files",
                "select",
                &[json!("/a.wav"), json!("/b.wav")]
            ),
            Some(Said::Chosen(
                "files".into(),
                vec!["/a.wav".into(), "/b.wav".into()]
            ))
        );
        assert_eq!(
            read(load, "load:files", "pick", &[json!(["/a.wav"])]),
            Some(Said::Picked("files".into(), vec!["/a.wav".into()]))
        );
        assert_eq!(
            read(load, "load:files", "path", &[json!("/takes")]),
            Some(Said::Entered("files".into(), "/takes".into()))
        );
        assert_eq!(
            read(load, "load:files", "/a.wav", &[]),
            None,
            "choosing several, the value is not the selection"
        );
        assert_eq!(read(load, "load:files", "focus", &[json!(1)]), None);
    }

    /// **Cancelling and opening again restore only what is meant to stay**:
    /// the directory the hand went into, and not what it had chosen there.
    #[test]
    fn a_chooser_opens_again_where_it_was_left_with_nothing_chosen() {
        let mut d = dialogs();
        let id = d.ids["load:files"];
        let shown = d.open(Form::Load, BTreeMap::new());
        assert!(shown.contains(&(id, json!({"path": START, "focus": 1, "filter": ["wav"]}))));
        assert!(shown.contains(&(d.ids[STACK], json!({"index": 2}))));
        d.read("load:files", "path", &[json!("/takes")]);
        d.read("load:files", "select", &[json!("/takes/a.wav")]);
        assert_eq!(d.chosen("files"), ["/takes/a.wav"]);
        assert_eq!(d.read("load:cancel", "click", &[]), Some(Said::Cancel));
        assert_eq!(d.hide(), vec![(d.ids[STACK], json!({"index": 0}))]);
        assert!(d.up().is_none());
        assert_eq!(
            d.read("load:files", "select", &[json!("/takes/b.wav")]),
            None,
            "a report with no form up is nobody's"
        );
        let again = d.open(Form::Load, BTreeMap::new());
        assert!(again.contains(&(id, json!({"path": "/takes", "focus": 1, "filter": ["wav"]}))));
        assert!(d.chosen("files").is_empty());
    }

    #[test]
    fn going_into_a_directory_forgets_the_files_chosen_in_the_last() {
        let mut d = dialogs();
        d.open(Form::Load, BTreeMap::new());
        d.read("load:files", "select", &[json!("/a.wav")]);
        d.read("load:files", "path", &[json!("/takes")]);
        assert!(d.chosen("files").is_empty());
    }

    /// **A window drawn while a dialog is up shows it again**, with what its
    /// fields hold and its choosers where they were -- and with nothing
    /// chosen, as the new choosers have nothing selected.
    #[test]
    fn a_window_drawn_again_shows_the_dialog_that_was_up() {
        let mut d = dialogs();
        assert_eq!(d.stack(&FORMS).unwrap()["index"], 0);
        d.open(Form::Load, BTreeMap::new());
        d.read("load:files", "path", &[json!("/takes")]);
        d.read("load:files", "select", &[json!("/takes/a.wav")]);
        let stack = d.stack(&FORMS).unwrap();
        assert_eq!(stack["index"], 2);
        assert_eq!(find(&stack, d.ids["load:files"]).unwrap()["path"], "/takes");
        assert!(d.chosen("files").is_empty());
        d.hide();
        d.open(Form::Name, BTreeMap::from([("text".into(), "x".into())]));
        let stack = d.stack(&FORMS).unwrap();
        assert_eq!(find(&stack, d.ids["name:text"]).unwrap()["value"], "x");
        d.closed();
        assert_eq!(
            d.stack(&FORMS).unwrap()["index"],
            0,
            "a closed window has none up"
        );
    }

    /// **Two questions of one dialog**: the page is named by one and drawn
    /// as whichever is up.
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Asked {
        First,
        Second,
    }

    impl Spec for Asked {
        fn prefix(self) -> &'static str {
            "asked"
        }
        fn page(self) -> i32 {
            1
        }
        fn title(self) -> &'static str {
            match self {
                Asked::First => "First",
                Asked::Second => "Second",
            }
        }
        fn fields(self) -> Vec<(&'static str, &'static str)> {
            match self {
                Asked::First => vec![("value", "One")],
                Asked::Second => vec![("value", "Two")],
            }
        }
        fn label(self) -> Option<&'static str> {
            Some("asked")
        }
    }

    #[test]
    fn a_window_drawn_again_shows_the_question_a_shared_dialog_was_asking() {
        let forms = [Asked::First];
        let mut d = Dialogs {
            ids: names(&forms)
                .into_iter()
                .enumerate()
                .map(|(i, name)| (name, 100 + i as i32))
                .collect(),
            ..Dialogs::default()
        };
        d.open(
            Asked::Second,
            BTreeMap::from([("value".into(), "typed".into())]),
        );
        let stack = d.stack(&forms).unwrap();
        assert_eq!(stack["index"], 1);
        let dialog = find(&stack, d.ids["asked"]).unwrap();
        assert_eq!(dialog["title"], "Second");
        assert_eq!(
            dialog["children"][0]["children"][0]["children"][0]["text"],
            "Two"
        );
        assert_eq!(
            find(&stack, d.ids["asked:value"]).unwrap()["value"],
            "typed"
        );
    }

    /// A form that saves: its chooser, and the name it fills.
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Keep;

    impl Spec for Keep {
        fn prefix(self) -> &'static str {
            "keep"
        }
        fn page(self) -> i32 {
            1
        }
        fn title(self) -> &'static str {
            "Save as"
        }
        fn fields(self) -> Vec<(&'static str, &'static str)> {
            vec![("path", ""), (NAME, "Name")]
        }
        fn chooser(self, name: &str) -> Option<Chooser> {
            (name == "path").then_some(Chooser::save(&["wav"]))
        }
        fn accept(self) -> &'static str {
            "Save"
        }
    }

    /// **Save As is the chooser too**: it opens in the folder of the file
    /// written, that file's name in the field under it; a file it selects
    /// is the name, a name typed is in the folder shown, and a pick saves.
    #[test]
    fn a_saving_chooser_fills_its_name_and_a_pick_saves() {
        let forms = [Keep];
        let mut d = Dialogs {
            ids: names(&forms)
                .into_iter()
                .enumerate()
                .map(|(i, name)| (name, 100 + i as i32))
                .collect(),
            ..Dialogs::default()
        };
        let chooser = d.ids["keep:path"];
        let name = d.ids["keep:name"];
        let shown = d.open(
            Keep,
            BTreeMap::from([("path".into(), "/takes/a.wav".into())]),
        );
        assert!(shown.contains(&(
            chooser,
            json!({"path": "/takes", "focus": 1, "filter": ["wav"]})
        )));
        assert!(shown.contains(&(name, json!({"value": "a.wav"}))));
        assert!(shown.contains(&(d.ids["keep:ok"], json!({"label": "Save"}))));
        let up = d.up().unwrap().clone();
        assert_eq!(up.path("path").as_deref(), Some("/takes/a.wav"));
        // the chooser fills the name field, which it is bound to
        let stack = d.stack(&forms).unwrap();
        assert_eq!(
            find(&stack, chooser).unwrap()["bind"],
            json!(["widget", name, "value"])
        );
        assert_eq!(find(&stack, d.ids["keep:ok"]).unwrap()["label"], "Save");
        d.read("keep:name", "b.wav", &[]);
        assert_eq!(
            d.up().unwrap().path("path").as_deref(),
            Some("/takes/b.wav")
        );
        d.read("keep:path", "/other/c.wav", &[]);
        assert_eq!(
            d.up().unwrap().path("path").as_deref(),
            Some("/other/c.wav")
        );
        assert_eq!(
            d.read("keep:path", "pick", &[json!(["/other/d.wav"])]),
            Some(Said::Accept)
        );
        assert_eq!(
            d.up().unwrap().path("path").as_deref(),
            Some("/other/d.wav")
        );
        // nothing typed names nothing
        d.read("keep:name", "  ", &[]);
        assert_eq!(d.up().unwrap().path("path"), None);
    }

    /// **A file form manages the storage it shows**: its tools and the line
    /// they speak in are under its chooser; a rename asks the chooser to move
    /// the file selected to the name typed, a delete asks once and then asks
    /// the chooser to remove it, and the chooser's answer is said.
    #[test]
    fn a_file_form_manages_the_storage_it_shows() {
        let forms = [Keep];
        let mut d = Dialogs {
            ids: names(&forms)
                .into_iter()
                .enumerate()
                .map(|(i, name)| (name, 100 + i as i32))
                .collect(),
            ..Dialogs::default()
        };
        let (chooser, says) = (d.ids["keep:path"], d.ids["keep:says"]);
        let stack = d.stack(&forms).unwrap();
        // the copies to and from the disk are a page's alone
        for tool in Tool::ALL {
            let id = d.ids.get(&format!("keep:{}", tool.name()));
            assert_eq!(id.is_some(), tool.shown(), "{tool:?}");
            if let Some(id) = id {
                assert_eq!(find(&stack, *id).unwrap()["type"], "button");
            }
        }
        assert_eq!(find(&stack, says).unwrap()["shown"], false);
        d.open(
            Keep,
            BTreeMap::from([("path".into(), "/takes/a.wav".into())]),
        );
        d.read("keep:path", "/takes/a.wav", &[]);
        d.read("keep:name", "b.wav", &[]);
        let Some(Said::Corrected(renamed)) = d.read("keep:rename", "click", &[]) else {
            panic!("a tool corrects the window");
        };
        assert!(renamed.contains(&(
            chooser,
            json!({"change": {"op": "rename", "path": "/takes/a.wav", "to": "/takes/b.wav"}})
        )));
        d.read("keep:path", "/takes/b.wav", &[]);
        let Some(Said::Corrected(asked)) = d.read("keep:delete", "click", &[]) else {
            panic!("a delete asks");
        };
        assert!(asked.iter().all(|(id, _)| *id != chooser), "not yet");
        assert_eq!(asked.last().unwrap().0, says);
        assert_eq!(asked.last().unwrap().1["shown"], true);
        let Some(Said::Corrected(gone)) = d.read("keep:delete", "click", &[]) else {
            panic!("a delete asked goes on");
        };
        assert!(gone.contains(&(
            chooser,
            json!({"change": {"op": "remove", "path": "/takes/b.wav"}})
        )));
        assert_eq!(
            d.read("keep:path", "changed", &[json!("/takes/b.wav: denied")]),
            Some(Said::Corrected(vec![(
                says,
                json!({"text": "/takes/b.wav: denied", "shown": true})
            )]))
        );
    }

    /// An Open form with a name field: what is typed there is opened as a
    /// file chosen, and a file chosen in a folder left behind is not.
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Pick;

    impl Spec for Pick {
        fn prefix(self) -> &'static str {
            "pick"
        }
        fn page(self) -> i32 {
            1
        }
        fn title(self) -> &'static str {
            "Open"
        }
        fn fields(self) -> Vec<(&'static str, &'static str)> {
            vec![("path", ""), (NAME, "Name")]
        }
        fn chooser(self, name: &str) -> Option<Chooser> {
            (name == "path").then_some(Chooser::open(&["wav"], false))
        }
    }

    #[test]
    fn an_open_form_opens_a_path_typed_as_one_chosen() {
        let forms = [Pick];
        let mut d = Dialogs {
            ids: names(&forms)
                .into_iter()
                .enumerate()
                .map(|(i, name)| (name, 100 + i as i32))
                .collect(),
            ..Dialogs::default()
        };
        let name = d.ids["pick:name"];
        let stack = d.stack(&forms).unwrap();
        assert_eq!(
            find(&stack, d.ids["pick:path"]).unwrap()["bind"],
            json!(["widget", name, "value"]),
            "the chooser fills the name field"
        );
        d.open(Pick, BTreeMap::new());
        d.read("pick:path", "/takes/a.wav", &[]);
        assert_eq!(d.up().unwrap().value(NAME), "/takes/a.wav");
        // the folder left behind takes its file with it, and the field
        assert_eq!(
            d.read("pick:path", "path", &[json!("/other")]),
            Some(Said::Corrected(vec![(name, json!({"value": ""}))]))
        );
        assert_eq!(d.read("pick:ok", "click", &[]), Some(Said::Accept));
        assert!(d.chosen("path").is_empty());
        d.read("pick:name", "b.wav", &[]);
        assert_eq!(d.read("pick:ok", "click", &[]), Some(Said::Accept));
        assert_eq!(d.chosen("path"), ["/other/b.wav"]);
    }

    #[test]
    fn a_report_from_another_form_than_the_one_up_is_ignored() {
        let mut d = dialogs();
        d.open(Form::Name, BTreeMap::new());
        assert_eq!(d.read("load:ok", "click", &[]), None);
        assert_eq!(
            d.read("name:text", "typed", &[]),
            Some(Said::Field("text".into(), "typed".into()))
        );
        assert_eq!(d.up().unwrap().value("text"), "typed");
    }
}
