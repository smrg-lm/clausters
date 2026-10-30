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
//! The chooser keeps what it is doing -- the directory, the listing it waits
//! on, an error -- and reports what the hand chose; the form keeps the paths
//! chosen and the directory the field opens in; and what the paths are for
//! is the application's, read at `OK` or at a pick ([`Said::Picked`]).
//!
//! [`Dialogs`] is the whole mechanism held for one window: the ids, the form
//! that is up, what its choosers chose and where each opens again. An
//! application that holds one opens, reads and hides its forms through it,
//! and a window drawn again shows the form that was up.

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
    /// Props a field adds to the text widget it is (a multiline one).
    fn field(self, _name: &str, _widget: &mut Map<String, Value>) {}
    /// **Whether field `name` is chosen in the host's file chooser** rather
    /// than typed, and how; `None` (the default) for a text field.
    fn chooser(self, _name: &str) -> Option<Chooser> {
        None
    }
}

/// **A field chosen in the host's file chooser**: which files it shows,
/// whether several can be chosen at once, and how much room it takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chooser {
    /// The extensions a file must have to be shown, without the dot; empty
    /// shows every file.
    pub filter: &'static [&'static str],
    /// Whether Ctrl and Shift choose several files.
    pub multiple: bool,
    /// Its width and height, in logical pixels.
    pub w: f64,
    pub h: f64,
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
    }
    out
}

/// Whether `ids` numbers every widget of the dialogs: a window has all of
/// them or none.
#[must_use]
pub fn numbered<F: Spec>(forms: &[F], ids: &Ids) -> bool {
    names(forms).iter().all(|name| ids.contains_key(name))
}

/// How wide a row's caption is: the longest of them, "Bottom margin (mm)",
/// at the size text is drawn, in either face.
const CAPTION_W: f64 = 170.0;

/// How wide a field is: a title, a path, a line of a footnote.
const FIELD_W: f64 = 340.0;

/// One form as a dialog: a row per field, and `Cancel` and `OK` under them --
/// or, closing, the close form every application asks in
/// ([`crate::closing::dialog`]).
fn dialog<F: Spec>(form: F, ids: &Ids, up: Option<&Open<F>>) -> Value {
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
        .map(|(name, label)| {
            let mut caption = json!({"type": "label", "text": label, "w": CAPTION_W});
            // the caption a form corrects is the question it is asking
            if let (Some(_), Some(map)) = (form.label(), caption.as_object_mut()) {
                map.insert("id".into(), json!(id("label")));
            }
            let mut field = match form.chooser(name) {
                Some(chooser) => json!({
                    "type": "files",
                    "id": id(name),
                    "path": Some(held(name)).filter(|dir| !dir.is_empty()).unwrap_or(START),
                    "filter": chooser.filter,
                    "multiple": chooser.multiple,
                    "w": chooser.w,
                    "h": chooser.h,
                }),
                None => json!({"type": "text", "id": id(name), "value": held(name), "w": FIELD_W}),
            };
            if let Some(map) = field.as_object_mut() {
                form.field(name, map);
            }
            // a row is its caption and its field and no air of its own: the
            // dialog's gap is what separates one row from the next
            json!({
                "type": "layout",
                "flow": "row",
                "hug": true,
                "margin": 0,
                "children": [caption, field],
            })
        })
        .collect();
    let spring = json!({"type": "separator", "weight": 1, "line": false});
    let cancel = json!({"type": "button", "id": id("cancel"), "label": "Cancel"});
    let children = json!([spring, cancel, {"type": "button", "id": id("ok"), "label": "OK"}]);
    let buttons = json!({
        "type": "layout",
        "flow": "row",
        "hug": true,
        "pack": true,
        "margin": 0,
        "children": children,
    });
    json!({
        "type": "layout",
        "id": ids.get(prefix),
        "modal": true,
        "frame": true,
        "hug": true,
        "title": form.title(),
        "flow": "col",
        "children": rows.into_iter().chain([buttons]).collect::<Vec<_>>(),
    })
}

/// **The dialogs, as the stack the window holds** -- or `None` when the
/// caller did not number them. It takes no room in the window's flow: page 0
/// is empty, and each form's dialog is alone on a page of its own, in the
/// order `forms` gives, which is the order of their pages.
#[must_use]
pub fn stack<F: Spec>(forms: &[F], ids: &Ids) -> Option<Value> {
    stack_with(forms, ids, None)
}

/// The stack, turned to the page of the form `up` with its fields holding
/// what it holds -- the window drawn again while a dialog is up.
fn stack_with<F: Spec>(forms: &[F], ids: &Ids, up: Option<&Open<F>>) -> Option<Value> {
    if !numbered(forms, ids) {
        return None;
    }
    let up = up.filter(|open| forms.contains(&open.form));
    let pages: Vec<Value> = std::iter::once(json!({"type": "layout"}))
        .chain(forms.iter().map(|form| {
            let open = up.filter(|open| open.form == *form);
            json!({"type": "layout", "children": [dialog(*form, ids, open)]})
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
                Some(match self.form.chooser(name) {
                    Some(_) => {
                        let dir = Some(self.value(name)).filter(|d| !d.is_empty());
                        (id, json!({"path": dir.unwrap_or(START), "focus": 1}))
                    }
                    None => (id, json!({"value": self.value(name)})),
                })
            })
            .collect();
        if let (Some(label), Some(id)) = (self.form.label(), ids.get(&format!("{prefix}:label"))) {
            out.push((*id, json!({"text": label})));
        }
        // a dialog that serves several questions says which it is asking
        if let Some(id) = ids.get(prefix) {
            out.push((*id, json!({"title": self.form.title()})));
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
}

impl<F> Default for Dialogs<F> {
    fn default() -> Self {
        Dialogs {
            ids: Ids::new(),
            up: None,
            chosen: BTreeMap::new(),
            dirs: BTreeMap::new(),
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
        self.chosen.clear();
        stack_with(forms, &self.ids, self.up.as_ref())
    }

    /// **Opens `form`**, its fields holding `values`, and answers what the
    /// window is corrected with to show it. A chooser opens where the hand
    /// last left it, and with nothing chosen.
    pub fn open(&mut self, form: F, mut values: BTreeMap<String, String>) -> Vec<(i32, Value)> {
        let prefix = form.prefix();
        for (name, _) in form.fields() {
            if form.chooser(name).is_some() {
                let dir = self.dirs.get(&format!("{prefix}:{name}")).cloned();
                values.insert(name.to_string(), dir.unwrap_or_else(|| START.into()));
            }
        }
        let open = Open { form, values };
        let shown = open.shown(&self.ids);
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
        let open = self.up.as_mut()?;
        let said = read(open.form, name, tag, values)?;
        match &said {
            Said::Field(field, text) => {
                open.values.insert(field.clone(), text.clone());
            }
            Said::Chosen(field, paths) => {
                self.chosen.insert(field.clone(), paths.clone());
            }
            Said::Entered(field, dir) => {
                open.values.insert(field.clone(), dir.clone());
                self.dirs
                    .insert(format!("{}:{field}", open.form.prefix()), dir.clone());
                self.chosen.remove(field);
            }
            Said::Picked(..) | Said::Accept | Said::Cancel | Said::Discard => {}
        }
        Some(said)
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
                filter: &["wav"],
                multiple: true,
                w: 400.0,
                h: 300.0,
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
        assert_eq!(
            (chooser["w"].as_f64(), chooser["h"].as_f64()),
            (Some(400.0), Some(300.0))
        );
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
        assert!(shown.contains(&(id, json!({"path": START, "focus": 1}))));
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
        assert!(again.contains(&(id, json!({"path": "/takes", "focus": 1}))));
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
