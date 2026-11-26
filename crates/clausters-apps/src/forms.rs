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
//! **A form may be a file request** ([`Spec::file`]): it is then the host's
//! own file dialog, the `file_dialog` element, and nothing of it is drawn or
//! kept here. It is every application's Open, Save As and Export, the code
//! editor's too: the form says what it chooses (a file to open, or several,
//! the file to write, a folder) and the kinds of file, the host's dialog
//! shows the storage, its path, its kinds and its tools, and the form hears
//! one report -- the kind chosen and the paths ([`Open::paths`]) -- which
//! reads as `OK`, or a cancel. What the paths are for is the application's.
//!
//! **A field may be one of several** ([`Spec::options`]): it is then the
//! host's `choice`, and what the field holds is the text of the option
//! chosen, as a typed field holds its text.
//!
//! [`Dialogs`] is the whole mechanism held for one window: the ids, the form
//! that is up and what it holds. An application that holds one opens, reads
//! and hides its forms through it, and a window drawn again shows the form
//! that was up.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

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
    /// Its fields, by name, each with what it is labelled; none for a file
    /// request.
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
    /// **Whether the form is a file request**, asked in the host's file
    /// dialog, and what it asks for; `None` (the default) for a form of
    /// fields.
    fn file(self) -> Option<FileRequest> {
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

/// **A kind of file a request opens or saves**: what it is called and the
/// extensions its files have, without the dot -- the first the one a save
/// gives a name that has none of them. None is every file
/// ([`ALL_FILES`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileType {
    pub name: &'static str,
    pub extensions: &'static [&'static str],
}

impl FileType {
    /// The kind `name`, whose files have `extensions`.
    #[must_use]
    pub const fn new(name: &'static str, extensions: &'static [&'static str]) -> Self {
        FileType { name, extensions }
    }
}

/// **Every file, whatever its extension**: among a save's kinds, it says
/// the application writes whatever name it is given, as an editor of text
/// does. An open always offers it.
pub const ALL_FILES: FileType = FileType::new("All files", &[]);

/// **What a file request chooses.**
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileMode {
    /// A file to read, or several.
    Open,
    /// The file to write.
    Save,
    /// A folder.
    Folder,
}

/// **A file request**: what it chooses, among which kinds, and whether
/// several files at once. The host's dialog says the rest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FileRequest {
    pub mode: FileMode,
    /// The kinds of file it opens, or the kinds a save writes.
    pub types: &'static [FileType],
    /// Whether Ctrl and Shift choose several files.
    pub multiple: bool,
}

/// **Where a file form starts**, among the values it is opened with: a
/// folder, or for a save the file it writes.
pub const PATH: &str = "path";

impl FileRequest {
    /// **A request that opens** files of `types`: one, or several with
    /// Ctrl and Shift.
    #[must_use]
    pub const fn open(types: &'static [FileType], multiple: bool) -> Self {
        FileRequest {
            mode: FileMode::Open,
            types,
            multiple,
        }
    }

    /// **A request that saves** a file of one of `types`.
    #[must_use]
    pub const fn save(types: &'static [FileType]) -> Self {
        FileRequest {
            mode: FileMode::Save,
            types,
            multiple: false,
        }
    }

    /// **A request for a folder.**
    #[must_use]
    pub const fn folder() -> Self {
        FileRequest {
            mode: FileMode::Folder,
            types: &[],
            multiple: false,
        }
    }

    /// **What the `file_dialog` element is asked**, titled `title`, its
    /// accepting button saying `accept`, starting at `path`.
    #[must_use]
    pub fn props(&self, title: &str, accept: &str, path: &str) -> Value {
        let kinds: Vec<Value> = self
            .types
            .iter()
            .filter(|t| !t.extensions.is_empty())
            .map(|t| json!([t.name, t.extensions]))
            .collect();
        let all_files = match self.mode {
            FileMode::Open => true,
            FileMode::Save => self.types.iter().any(|t| t.extensions.is_empty()),
            FileMode::Folder => false,
        };
        let mode = match self.mode {
            FileMode::Open => "open",
            FileMode::Save => "save",
            FileMode::Folder => "folder",
        };
        json!({
            "mode": mode,
            "kinds": kinds,
            "all_files": all_files,
            "multiple": self.multiple,
            "path": path,
            "title": title,
            "accept": accept,
        })
    }
}

/// **The `file_dialog` element a request is**, numbered `id`, closed.
#[must_use]
pub fn file_dialog(id: Option<i32>, request: &FileRequest, title: &str, accept: &str) -> Value {
    let mut element = request.props(title, accept, "");
    if let Some(map) = element.as_object_mut() {
        map.insert("type".into(), json!("file_dialog"));
        map.insert("id".into(), json!(id));
    }
    element
}

/// **A field's caption**, in the caption's weight.
#[must_use]
pub fn caption(text: &str) -> Value {
    json!({"type": "label", "text": text, "variant": "caption"})
}

/// **The sentence under a field that says what it is for**, quieter.
#[must_use]
pub fn help(text: &str) -> Value {
    json!({"type": "label", "text": text, "wrap": 1, "variant": "help"})
}

/// **A field of a dialog, of its parts**: its caption, the widget, the
/// sentence that helps and the buttons beside it, over each other as the
/// design system's form sets them.
#[must_use]
pub fn field(parts: &[Value]) -> Value {
    json!({
        "type": "layout",
        "flow": "col",
        "hug": true,
        "margin": 0,
        "gap": CAPTION_GAP,
        "children": parts,
    })
}

/// **A row of buttons**, as wide as what they say.
#[must_use]
pub fn buttons(children: &[Value]) -> Value {
    json!({
        "type": "layout",
        "flow": "row",
        "pack": true,
        "hug": true,
        "margin": 0,
        "gap": crate::closing::FOOTER_GAP,
        "children": children,
    })
}

/// **A dialog's footer of `buttons`**: at its end.
#[must_use]
pub fn footer_of(buttons: &[Value]) -> Value {
    let mut children = vec![json!({"type": "separator", "weight": 1, "line": false})];
    children.extend(buttons.iter().cloned());
    self::buttons(&children)
}

/// **A field of a dialog**: its caption over it -- none for an empty
/// caption -- `w` wide where it is given. `caption` is the caption's id,
/// for a form that corrects it.
#[must_use]
pub fn field_row(caption: &str, caption_id: Option<i32>, widget: Value, w: Option<f64>) -> Value {
    let mut parts = Vec::new();
    if !caption.is_empty() {
        let mut label = self::caption(caption);
        if let (Some(id), Some(map)) = (caption_id, label.as_object_mut()) {
            map.insert("id".into(), json!(id));
        }
        parts.push(label);
    }
    parts.push(widget);
    let mut row = field(&parts);
    if let (Some(w), Some(map)) = (w, row.as_object_mut()) {
        map.insert("w".into(), json!(w));
    }
    row
}

/// **A dialog's footer**: the buttons at its end, `Cancel` and the one
/// that accepts, saying `accept`.
#[must_use]
pub fn footer(cancel: Option<i32>, ok: Option<i32>, accept: &str) -> Value {
    let cancel = json!({"type": "button", "id": cancel, "label": "Cancel", "variant": "outline"});
    let ok = json!({"type": "button", "id": ok, "label": accept, "default": true,
        "variant": "default"});
    footer_of(&[cancel, ok])
}

/// **Every widget of the dialogs, by name**, for a caller to number: the
/// stack; and for each form its dialog (`text`), its fields (`text:title`),
/// the caption a form corrects (`param:label`) and its two buttons
/// (`text:ok`, `text:cancel`) -- three for the close form, whose third closes
/// without saving (`close:discard`). A file request is its dialog alone,
/// the host's `file_dialog`.
#[must_use]
pub fn names<F: Spec>(forms: &[F]) -> Vec<String> {
    let mut out = vec![STACK.to_string()];
    for form in forms {
        let prefix = form.prefix();
        out.push(prefix.to_string());
        if form.file().is_some() {
            continue;
        }
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
    }
    out
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
/// ([`crate::closing::dialog`]). A file request has none here: it is the
/// host's dialog, beside the pages.
fn dialog<F: Spec>(form: F, ids: &Ids, up: Option<&Open<F>>) -> Value {
    let prefix = form.prefix();
    let id = |name: &str| ids.get(&format!("{prefix}:{name}")).copied();
    if let Some(question) = form.closes() {
        return crate::closing::dialog(ids.get(prefix).copied(), question, id);
    }
    if form.file().is_some() {
        return json!({"type": "layout"});
    }
    // what a field holds: the form's text when it is the one up, else nothing
    let held = |name: &str| up.map_or("", |open| open.value(name));
    let rows: Vec<Value> = form
        .fields()
        .iter()
        .map(|(name, label)| {
            let mut field = match form.options(name) {
                Some(options) => json!({
                    "type": "choice",
                    "id": id(name),
                    "options": options,
                    "index": option_index(options, held(name)),
                }),
                None => json!({"type": "text", "id": id(name), "value": held(name)}),
            };
            if let Some(map) = field.as_object_mut() {
                form.field(name, map);
            }
            // the caption a form corrects is the question it is asking
            let caption = form.label().and_then(|_| id("label"));
            field_row(label, caption, field, None)
        })
        .collect();
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
/// holds the file requests' dialogs, the host's, which take none either, and
/// each other form's dialog is alone on a page of its own, in the order
/// `forms` gives, which is the order of their pages.
#[must_use]
pub fn stack<F: Spec>(forms: &[F], ids: &Ids) -> Option<Value> {
    stack_with(forms, ids, None)
}

/// The stack, turned to the page of the form `up` with its fields holding
/// what it holds -- the window drawn again while a dialog is up. **Forms may
/// share a page**, a dialog that serves several questions: `forms` names the
/// page by one of them, and a page is drawn as the question it is asking.
fn stack_with<F: Spec>(forms: &[F], ids: &Ids, up: Option<&Open<F>>) -> Option<Value> {
    if !numbered(forms, ids) {
        return None;
    }
    let up = up.filter(|open| forms.iter().any(|form| form.page() == open.form.page()));
    // the file requests: one host dialog each prefix, open while its form
    // is up
    let mut files: Vec<Value> = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for form in forms {
        let Some(request) = form.file() else {
            continue;
        };
        if seen.contains(&form.prefix()) {
            continue;
        }
        seen.push(form.prefix());
        let id = ids.get(form.prefix()).copied();
        let open = up.filter(|open| open.form.prefix() == form.prefix());
        let element = match open {
            Some(open) => {
                let request = open.form.file().unwrap_or(request);
                let mut element = file_dialog(id, &request, open.form.title(), open.form.accept());
                if let Some(map) = element.as_object_mut() {
                    map.insert("path".into(), json!(open.value(PATH)));
                    map.insert("open".into(), json!(true));
                }
                element
            }
            None => file_dialog(id, &request, form.title(), form.accept()),
        };
        files.push(element);
    }
    let pages: Vec<Value> = std::iter::once(json!({"type": "layout", "children": files}))
        .chain(forms.iter().map(|form| {
            let open = up.filter(|open| open.form.page() == form.page());
            let asked = open.map_or(*form, |open| open.form);
            json!({"type": "layout", "children": [dialog(asked, ids, open)]})
        }))
        .collect();
    let index = up
        .filter(|open| open.form.file().is_none())
        .map_or(0, |open| open.form.page());
    Some(json!({
        "type": "layout",
        "id": ids.get(STACK),
        "flow": "stack",
        "index": index,
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
    /// `OK`, or a file request's dialog chose ([`Open::paths`]).
    Accept,
    /// `Cancel`, Escape, or the dialog's close mark.
    Cancel,
    /// The close form's `Don't save`.
    Discard,
}

/// The texts a report carries after its tag, whether as arguments or as one
/// array of them.
fn texts(values: &[Value]) -> Vec<String> {
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
///
/// A file request's dialog reports `"chosen" <kind> <path>...`, which is
/// its `OK` ([`Dialogs::read`] keeps the paths), and `"cancel"`.
#[must_use]
pub fn read<F: Spec>(form: F, name: &str, tag: &str, values: &[Value]) -> Option<Said> {
    if tag == "focus" && values.first().is_some_and(Value::is_number) {
        return None;
    }
    let prefix = form.prefix();
    if name == prefix {
        return match tag {
            "cancel" => Some(Said::Cancel),
            "chosen" if form.file().is_some() => Some(Said::Accept),
            _ => None,
        };
    }
    let part = name.strip_prefix(prefix)?.strip_prefix(':')?;
    // A choice reports the index of what was chosen, which the field holds
    // as the option's text.
    if let Some(options) = form.options(part) {
        let index = tag.parse::<f64>().ok()?.round().max(0.0) as usize;
        let option = options.get(index)?;
        return Some(Said::Field(part.to_string(), (*option).to_string()));
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

/// **A form that is up**: which, what its fields hold as they are typed,
/// and for a file request what its dialog chose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Open<F> {
    pub form: F,
    pub values: BTreeMap<String, String>,
    /// The kind a file request's dialog chose, by its name.
    pub kind: String,
    /// The paths it chose: the files to open, the one to write, a folder.
    pub chosen: Vec<String>,
}

impl<F: Spec> Open<F> {
    /// **`form`, up**, its fields holding `values`, nothing chosen yet.
    #[must_use]
    pub fn new(form: F, values: BTreeMap<String, String>) -> Self {
        Open {
            form,
            values,
            kind: String::new(),
            chosen: Vec::new(),
        }
    }

    /// What the field `name` holds.
    #[must_use]
    pub fn value(&self, name: &str) -> &str {
        self.values.get(name).map_or("", String::as_str)
    }

    /// **The paths a file request's dialog chose**, as it said them: the
    /// files to open, or the one file a save writes (its kind's extension
    /// added where it had none), or a folder.
    #[must_use]
    pub fn paths(&self) -> &[String] {
        &self.chosen
    }

    /// **The one path a file request chose**: the first.
    #[must_use]
    pub fn path(&self) -> Option<String> {
        self.chosen.first().cloned()
    }

    /// **What the window is corrected with to show it**: every field's text,
    /// the caption it asks with, its title, and the stack turned to its page
    /// -- for a file request, its dialog asked and opened.
    #[must_use]
    pub fn shown(&self, ids: &Ids) -> Vec<(i32, Value)> {
        let prefix = self.form.prefix();
        if let Some(request) = self.form.file() {
            let mut out: Vec<(i32, Value)> = ids
                .get(STACK)
                .map(|stack| (*stack, json!({"index": 0})))
                .into_iter()
                .collect();
            if let Some(id) = ids.get(prefix) {
                let mut props =
                    request.props(self.form.title(), self.form.accept(), self.value(PATH));
                if let Some(map) = props.as_object_mut() {
                    map.insert("open".into(), json!(1));
                }
                out.push((*id, props));
            }
            return out;
        }
        let mut out: Vec<(i32, Value)> = self
            .form
            .fields()
            .iter()
            .filter_map(|(name, _)| {
                let id = *ids.get(&format!("{prefix}:{name}"))?;
                Some(match self.form.options(name) {
                    Some(options) => (
                        id,
                        json!({"index": option_index(options, self.value(name))}),
                    ),
                    None => (id, json!({"value": self.value(name)})),
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
/// numbered with and the form that is up. Where a file request's dialog
/// opens again is the host's dialog's to keep.
#[derive(Clone, Debug, PartialEq)]
pub struct Dialogs<F> {
    pub ids: Ids,
    up: Option<Open<F>>,
}

impl<F> Default for Dialogs<F> {
    fn default() -> Self {
        Dialogs {
            ids: Ids::new(),
            up: None,
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

    /// **The stack for a window drawn now**: turned to the form that is up,
    /// its fields holding what they hold.
    pub fn stack(&self, forms: &[F]) -> Option<Value> {
        stack_with(forms, &self.ids, self.up.as_ref())
    }

    /// **Opens `form`**, its fields holding `values`, and answers what the
    /// window is corrected with to show it. A file request starts where
    /// [`PATH`] says: a folder, or for a save the file the window writes;
    /// with none, where the host's dialog was last left.
    pub fn open(&mut self, form: F, values: BTreeMap<String, String>) -> Vec<(i32, Value)> {
        let open = Open::new(form, values);
        let shown = open.shown(&self.ids);
        self.up = Some(open);
        shown
    }

    /// **Takes the dialog down**, and answers the window's correction.
    pub fn hide(&mut self) -> Vec<(i32, Value)> {
        let mut out = hidden(&self.ids);
        if let Some(open) = self.up.take()
            && open.form.file().is_some()
            && let Some(id) = self.ids.get(open.form.prefix())
        {
            out.push((*id, json!({"open": 0})));
        }
        out
    }

    /// **An answer the application refused**: a form of fields stays up,
    /// holding what was typed, to be corrected; a file request's dialog
    /// went down with its answer, and the form goes with it. Answers the
    /// window's correction.
    pub fn refused(&mut self) -> Vec<(i32, Value)> {
        match &self.up {
            Some(open) if open.form.file().is_some() => self.hide(),
            _ => Vec::new(),
        }
    }

    /// The window is gone, and no dialog is up in it.
    pub fn closed(&mut self) {
        self.up = None;
    }

    /// **What the widget `name` of the form up said**, kept as it says it: a
    /// field's text, and what a file request's dialog chose. `None` when no
    /// form is up, for a widget of another form, and for a report that says
    /// nothing. Taking the dialog down is the caller's, once it has read
    /// what it needs.
    pub fn read(&mut self, name: &str, tag: &str, values: &[Value]) -> Option<Said> {
        let open = self.up.as_mut()?;
        let said = read(open.form, name, tag, values)?;
        match &said {
            Said::Field(field, text) => {
                open.values.insert(field.clone(), text.clone());
            }
            Said::Accept if open.form.file().is_some() && tag == "chosen" => {
                let mut said = texts(values).into_iter();
                open.kind = said.next().unwrap_or_default();
                open.chosen = said.collect();
            }
            Said::Accept | Said::Cancel | Said::Discard => {}
        }
        Some(said)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A form with a text field and a file request, as an application would
    /// have.
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Form {
        Name,
        Load,
        Keep,
    }

    impl Spec for Form {
        fn prefix(self) -> &'static str {
            match self {
                Form::Name => "name",
                Form::Load => "load",
                Form::Keep => "keep",
            }
        }
        fn page(self) -> i32 {
            match self {
                Form::Name => 1,
                Form::Load => 2,
                Form::Keep => 3,
            }
        }
        fn title(self) -> &'static str {
            match self {
                Form::Keep => "Save as",
                _ => "Form",
            }
        }
        fn fields(self) -> Vec<(&'static str, &'static str)> {
            match self {
                Form::Name => vec![("text", "Text")],
                Form::Load | Form::Keep => Vec::new(),
            }
        }
        fn file(self) -> Option<FileRequest> {
            match self {
                Form::Name => None,
                Form::Load => Some(FileRequest::open(WAVS, true)),
                Form::Keep => Some(FileRequest::save(WAVS)),
            }
        }
        fn accept(self) -> &'static str {
            match self {
                Form::Keep => "Save",
                _ => "OK",
            }
        }
    }

    const FORMS: [Form; 3] = [Form::Name, Form::Load, Form::Keep];

    /// The one kind the tests' requests open.
    const WAVS: &[FileType] = &[FileType::new("WAV", &["wav"])];

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

    /// **A file request is the host's dialog**: one element, named by the
    /// form's prefix alone, asking for its kinds; no chooser of its own.
    #[test]
    fn a_file_request_is_the_host_s_file_dialog() {
        let d = dialogs();
        assert!(names(&FORMS).iter().all(|n| !n.starts_with("load:")));
        let stack = stack(&FORMS, &d.ids).unwrap();
        let load = find(&stack, d.ids["load"]).unwrap();
        assert_eq!(load["type"], "file_dialog");
        assert_eq!(load["mode"], "open");
        assert_eq!(load["kinds"], json!([["WAV", ["wav"]]]));
        assert_eq!(load["multiple"], true);
        assert_eq!(load["all_files"], true);
        let keep = find(&stack, d.ids["keep"]).unwrap();
        assert_eq!(keep["mode"], "save");
        assert_eq!(keep["all_files"], false, "a save writes its kinds");
        assert_eq!(keep["accept"], "Save");
        assert!(
            !stack.to_string().contains("\"files\""),
            "no chooser of the forms' own"
        );
    }

    /// **Opened, it is asked and opened; chosen, its paths are the form's
    /// `OK`; hidden, it is closed.**
    #[test]
    fn a_file_request_opens_reads_its_choice_and_closes() {
        let mut d = dialogs();
        let id = d.ids["keep"];
        let shown = d.open(
            Form::Keep,
            BTreeMap::from([(PATH.into(), "/takes/a.wav".into())]),
        );
        let (_, props) = shown.iter().find(|(w, _)| *w == id).unwrap();
        assert_eq!(props["open"], 1);
        assert_eq!(props["path"], "/takes/a.wav");
        assert!(shown.contains(&(d.ids[STACK], json!({"index": 0}))));
        assert_eq!(
            d.read("keep", "chosen", &[json!("WAV"), json!("/takes/b.wav")]),
            Some(Said::Accept)
        );
        let open = d.up().unwrap();
        assert_eq!(open.kind, "WAV");
        assert_eq!(open.path().as_deref(), Some("/takes/b.wav"));
        assert!(d.hide().contains(&(id, json!({"open": 0}))));
        d.open(Form::Load, BTreeMap::new());
        d.read(
            "load",
            "chosen",
            &[json!("WAV"), json!("/a.wav"), json!("/b.wav")],
        );
        assert_eq!(d.up().unwrap().paths(), ["/a.wav", "/b.wav"]);
        d.hide();
        d.open(Form::Load, BTreeMap::new());
        assert_eq!(d.read("load", "cancel", &[]), Some(Said::Cancel));
    }

    #[test]
    fn a_window_drawn_again_shows_the_dialog_that_was_up() {
        let mut d = dialogs();
        assert_eq!(d.stack(&FORMS).unwrap()["index"], 0);
        d.open(Form::Load, BTreeMap::new());
        let stack = d.stack(&FORMS).unwrap();
        assert_eq!(stack["index"], 0, "a file request is no page");
        assert_eq!(find(&stack, d.ids["load"]).unwrap()["open"], true);
        d.hide();
        d.open(Form::Name, BTreeMap::from([("text".into(), "x".into())]));
        let stack = d.stack(&FORMS).unwrap();
        assert_eq!(stack["index"], 1);
        assert_eq!(find(&stack, d.ids["name:text"]).unwrap()["value"], "x");
        assert!(find(&stack, d.ids["load"]).unwrap().get("open").is_none());
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

    #[test]
    fn a_report_from_another_form_than_the_one_up_is_ignored() {
        let mut d = dialogs();
        d.open(Form::Name, BTreeMap::new());
        assert_eq!(
            d.read("load", "chosen", &[json!("WAV"), json!("/a.wav")]),
            None
        );
        assert_eq!(
            d.read("name:text", "typed", &[]),
            Some(Said::Field("text".into(), "typed".into()))
        );
        assert_eq!(d.up().unwrap().value("text"), "typed");
    }
}
