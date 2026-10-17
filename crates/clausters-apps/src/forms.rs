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
}

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
fn dialog<F: Spec>(form: F, ids: &Ids) -> Value {
    let prefix = form.prefix();
    let id = |name: &str| ids.get(&format!("{prefix}:{name}")).copied();
    if let Some(question) = form.closes() {
        return crate::closing::dialog(ids.get(prefix).copied(), question, id);
    }
    let rows: Vec<Value> = form
        .fields()
        .iter()
        .map(|(name, label)| {
            let mut caption = json!({"type": "label", "text": label, "w": CAPTION_W});
            // the caption a form corrects is the question it is asking
            if let (Some(_), Some(map)) = (form.label(), caption.as_object_mut()) {
                map.insert("id".into(), json!(id("label")));
            }
            let mut field = json!({"type": "text", "id": id(name), "value": "", "w": FIELD_W});
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
    if !numbered(forms, ids) {
        return None;
    }
    let pages: Vec<Value> = std::iter::once(json!({"type": "layout"}))
        .chain(
            forms
                .iter()
                .map(|form| json!({"type": "layout", "children": [dialog(*form, ids)]})),
        )
        .collect();
    Some(json!({
        "type": "layout",
        "id": ids.get(STACK),
        "flow": "stack",
        "index": 0,
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
}

/// **What the widget `name` reporting `tag` said** -- or `None` for a name
/// that is no widget of `form`, and for a report that says nothing (a
/// button's value, which rises and falls with the hand).
#[must_use]
pub fn read<F: Spec>(form: F, name: &str, tag: &str) -> Option<Said> {
    let prefix = form.prefix();
    if name == prefix {
        return (tag == "cancel").then_some(Said::Cancel);
    }
    let part = name.strip_prefix(prefix)?.strip_prefix(':')?;
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
                Some((id, json!({"value": self.value(name)})))
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

/// What the stack is corrected with to take a dialog down.
#[must_use]
pub fn hidden(ids: &Ids) -> Vec<(i32, Value)> {
    ids.get(STACK)
        .map(|stack| (*stack, json!({"index": 0})))
        .into_iter()
        .collect()
}
