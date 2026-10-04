//! **A menu is a tree of entries**: what a window's `menu`, a widget's
//! `context` and a button's `menu` all hold, and what a popup lists.
//!
//! It is a prop value and not a node of the widget tree. A menu is shown in
//! three places -- along a window as a bar, at the pointer as a context menu,
//! under a button -- and in none of them is it laid out among the widgets: it
//! opens over them, in the host's popup layer ([`super::popup`]). So the tree
//! is data a prop carries, the way a patcher's `boxes` are, and what a pick
//! reports is the entry's **verb**, the name whoever owns the window answers
//! to.
//!
//! **The wire.** An entry is a JSON object -- `label`, `verb` (the label when
//! it names none), `enabled`, `icon`, and one of: `checked` (a check),
//! `group` with `checked` (one of several: the entries of a list that share a
//! group), `menu` (a submenu holding entries of its own). The string `"-"` is
//! a separator. On the scalar wire (`/gui_set`) the whole list rides as its
//! JSON string, like `theme` and `points`.

use serde_json::{Map, Value};

/// What an entry is, beside its label.
#[derive(Debug, Clone, PartialEq)]
pub enum EntryKind {
    /// A command: a pick reports its verb.
    Action,
    /// A switch: a pick flips it and reports the verb with the new state.
    Check(bool),
    /// One of several: the entries of one list sharing `group`. A pick turns
    /// this one on and the others off, and reports the verb.
    Radio { group: String, on: bool },
    /// A line between groups of entries; never picked.
    Separator,
    /// An entry that opens a list of its own.
    Submenu(Vec<Entry>),
}

/// One entry of a menu.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub label: String,
    /// What a pick reports. `None` only for a separator.
    pub verb: Option<String>,
    pub kind: EntryKind,
    pub enabled: bool,
    /// A glyph of the font drawn before the label (see `host::font`).
    pub icon: Option<char>,
    /// The chord bound to this entry's verb, drawn at the right of its row.
    /// Not a prop: the host writes it from its key table when the list opens
    /// ([`show_keys`]), so a list shows the keys of the moment it opened.
    pub key: Option<String>,
}

impl Entry {
    /// A plain row with no verb of its own -- what a chooser lists: a pick is
    /// answered by the element that opened the list, by position.
    pub fn option(label: &str, on: bool) -> Entry {
        Entry {
            label: label.to_string(),
            verb: None,
            kind: EntryKind::Radio {
                group: String::new(),
                on,
            },
            enabled: true,
            icon: None,
            key: None,
        }
    }

    pub fn separator() -> Entry {
        Entry {
            label: String::new(),
            verb: None,
            kind: EntryKind::Separator,
            enabled: false,
            icon: None,
            key: None,
        }
    }

    /// Whether a pointer or a key can land on this entry.
    pub fn pickable(&self) -> bool {
        self.enabled && !matches!(self.kind, EntryKind::Separator)
    }

    pub fn is_separator(&self) -> bool {
        matches!(self.kind, EntryKind::Separator)
    }

    /// The entries this one opens, when it is a submenu.
    pub fn submenu(&self) -> Option<&[Entry]> {
        match &self.kind {
            EntryKind::Submenu(entries) => Some(entries),
            _ => None,
        }
    }

    /// Whether the entry can ever draw a mark, so its list keeps the gutter
    /// for it whether or not this one is on.
    pub fn can_mark(&self) -> bool {
        matches!(self.kind, EntryKind::Check(_) | EntryKind::Radio { .. })
    }

    /// Whether the entry draws a mark: a check that is on, or the chosen one
    /// of a group.
    pub fn marked(&self) -> bool {
        matches!(
            self.kind,
            EntryKind::Check(true) | EntryKind::Radio { on: true, .. }
        )
    }

    fn from_value(v: &Value) -> Option<Entry> {
        match v {
            Value::String(s) if s == "-" => Some(Entry::separator()),
            // A bare string is the shortest action: its label is its verb.
            Value::String(s) => Some(Entry {
                label: s.clone(),
                verb: Some(s.clone()),
                kind: EntryKind::Action,
                enabled: true,
                icon: None,
                key: None,
            }),
            Value::Object(o) => Entry::from_object(o),
            _ => None,
        }
    }

    fn from_object(o: &Map<String, Value>) -> Option<Entry> {
        if o.get("separator").and_then(Value::as_bool) == Some(true) {
            return Some(Entry::separator());
        }
        let label = o.get("label").and_then(Value::as_str)?.to_string();
        let verb = o
            .get("verb")
            .and_then(Value::as_str)
            .map_or_else(|| label.clone(), str::to_string);
        let checked = o.get("checked").and_then(truthy);
        let kind = if let Some(sub) = o.get("menu") {
            EntryKind::Submenu(parse(sub))
        } else if let Some(group) = o.get("group").and_then(Value::as_str) {
            EntryKind::Radio {
                group: group.to_string(),
                on: checked.unwrap_or(false),
            }
        } else if let Some(on) = checked {
            EntryKind::Check(on)
        } else {
            EntryKind::Action
        };
        Some(Entry {
            label,
            verb: Some(verb),
            kind,
            enabled: o.get("enabled").and_then(truthy).unwrap_or(true),
            icon: o.get("icon").and_then(icon_of),
            key: None,
        })
    }

    fn to_value(&self) -> Value {
        if self.is_separator() {
            return Value::String("-".into());
        }
        let mut o = Map::new();
        o.insert("label".into(), Value::from(self.label.clone()));
        if let Some(verb) = &self.verb {
            o.insert("verb".into(), Value::from(verb.clone()));
        }
        match &self.kind {
            EntryKind::Check(on) => {
                o.insert("checked".into(), Value::from(*on));
            }
            EntryKind::Radio { group, on } => {
                o.insert("group".into(), Value::from(group.clone()));
                o.insert("checked".into(), Value::from(*on));
            }
            EntryKind::Submenu(entries) => {
                o.insert("menu".into(), to_json(entries));
            }
            EntryKind::Action | EntryKind::Separator => {}
        }
        if !self.enabled {
            o.insert("enabled".into(), Value::from(false));
        }
        if let Some(icon) = self.icon {
            o.insert("icon".into(), Value::from(icon.to_string()));
        }
        Value::Object(o)
    }
}

/// The `icon` prop's value: the first character of a string.
pub fn icon_of(v: &Value) -> Option<char> {
    v.as_str()?.chars().next()
}

fn truthy(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => n.as_f64().map(|n| n != 0.0),
        _ => None,
    }
}

/// Reads a menu off the wire: a JSON array of entries, or that array as a
/// string (the scalar carrier a `/gui_set` uses). Anything else is an empty
/// menu, and an entry that cannot be read is dropped rather than failing the
/// list around it.
pub fn parse(v: &Value) -> Vec<Entry> {
    match v {
        Value::Array(items) => items.iter().filter_map(Entry::from_value).collect(),
        Value::String(s) => serde_json::from_str::<Value>(s)
            .ok()
            .filter(Value::is_array)
            .map(|v| parse(&v))
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// The menu as the JSON it was read from -- what `/gui_query` answers, so a
/// check a hand flipped reads back as the prop it is.
pub fn to_json(entries: &[Entry]) -> Value {
    Value::Array(entries.iter().map(Entry::to_value).collect())
}

/// The list at `path` inside `entries`: the root for an empty path, else the
/// submenu each index opens in turn.
pub fn list_at<'a>(entries: &'a [Entry], path: &[usize]) -> Option<&'a [Entry]> {
    match path.split_first() {
        None => Some(entries),
        Some((i, tail)) => list_at(entries.get(*i)?.submenu()?, tail),
    }
}

fn list_at_mut<'a>(entries: &'a mut Vec<Entry>, path: &[usize]) -> Option<&'a mut Vec<Entry>> {
    match path.split_first() {
        None => Some(entries),
        Some((i, tail)) => match &mut entries.get_mut(*i)?.kind {
            EntryKind::Submenu(sub) => list_at_mut(sub, tail),
            _ => None,
        },
    }
}

/// **Writes the chord bound to each entry's verb** beside it, at every depth --
/// what a list shows at the right of a row, so a reader finds the key by
/// opening the menu. `chord` answers a verb's chord, or `None` for one no key
/// performs.
pub fn show_keys(entries: &mut [Entry], chord: &dyn Fn(&str) -> Option<String>) {
    for e in entries {
        match &mut e.kind {
            EntryKind::Submenu(sub) => show_keys(sub, chord),
            EntryKind::Separator => {}
            _ => e.key = e.verb.as_deref().and_then(chord),
        }
    }
}

/// **Where the entry reporting `verb` is**, as the path a pick takes, and
/// whether it can be picked -- an entry inside a disabled submenu cannot,
/// whatever it says itself. The first one in reading order, when several
/// report the same verb.
///
/// What a key bound to that verb asks for: the desktop rule, where the
/// accelerator and the entry are one command.
pub fn find_verb(entries: &[Entry], verb: &str) -> Option<(Vec<usize>, bool)> {
    for (i, e) in entries.iter().enumerate() {
        if let Some(sub) = e.submenu() {
            if let Some((mut path, live)) = find_verb(sub, verb) {
                path.insert(0, i);
                return Some((path, live && e.enabled));
            }
        } else if e.verb.as_deref() == Some(verb) && !e.is_separator() {
            return Some((vec![i], e.enabled));
        }
    }
    None
}

/// **What a pick was**: the verb to report, and the state that goes with it
/// when the entry holds one.
#[derive(Debug, Clone, PartialEq)]
pub struct Pick {
    pub verb: String,
    /// The new state of a check, `true` for the chosen one of a group, `None`
    /// for an action.
    pub state: Option<bool>,
}

/// Picks the entry at `path` (the indices from the root down to it), **writing
/// the state it holds**: a check flips, and the chosen one of a group turns on
/// while the rest of its group in the same list turn off.
///
/// The state is a prop, so it is written where the prop lives -- the drawn menu
/// then shows what was picked without waiting for whoever owns the window to
/// send it back, exactly as a toggle shows its own flip.
pub fn pick(entries: &mut Vec<Entry>, path: &[usize]) -> Option<Pick> {
    let (&last, parents) = path.split_last()?;
    let list = list_at_mut(entries, parents)?;
    let entry = list.get(last)?;
    if !entry.pickable() {
        return None;
    }
    let verb = entry.verb.clone()?;
    let state = match entry.kind.clone() {
        EntryKind::Action => None,
        EntryKind::Check(on) => {
            list[last].kind = EntryKind::Check(!on);
            Some(!on)
        }
        EntryKind::Radio { group, .. } => {
            for (i, other) in list.iter_mut().enumerate() {
                if let EntryKind::Radio { group: g, on } = &mut other.kind
                    && *g == group
                {
                    *on = i == last;
                }
            }
            Some(true)
        }
        EntryKind::Separator | EntryKind::Submenu(_) => return None,
    };
    Some(Pick { verb, state })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A verb is found at any depth, and a disabled list makes everything in
    /// it unpickable.
    #[test]
    fn a_verb_is_found_where_a_pick_would_reach_it() {
        let menu = parse(&json!([
            {"label": "File", "menu": [{"label": "Save", "verb": "save"}]},
            {"label": "Old", "enabled": false, "menu": [{"label": "Undo", "verb": "undo"}]},
        ]));
        assert_eq!(find_verb(&menu, "save"), Some((vec![0, 0], true)));
        assert_eq!(find_verb(&menu, "undo"), Some((vec![1, 0], false)));
        assert_eq!(find_verb(&menu, "redo"), None);
    }

    fn file_menu() -> Vec<Entry> {
        parse(&json!([
            {"label": "Open", "verb": "open"},
            "-",
            {"label": "Loop", "verb": "loop", "checked": false},
            {"label": "Mode", "menu": [
                {"label": "Draw", "verb": "draw", "group": "mode", "checked": true},
                {"label": "Select", "verb": "select", "group": "mode"},
            ]},
            {"label": "Export", "enabled": false},
        ]))
    }

    #[test]
    fn the_wire_reads_into_the_five_kinds() {
        let m = file_menu();
        assert_eq!(m.len(), 5);
        assert_eq!(m[0].kind, EntryKind::Action);
        assert!(m[1].is_separator());
        assert_eq!(m[2].kind, EntryKind::Check(false));
        assert_eq!(m[3].submenu().map(<[Entry]>::len), Some(2));
        assert!(!m[4].enabled);
        assert_eq!(
            m[4].verb.as_deref(),
            Some("Export"),
            "the label is the verb"
        );
    }

    #[test]
    fn a_menu_rides_the_scalar_wire_as_its_json_string() {
        let m = parse(&json!(r#"[{"label":"Save","verb":"save"},"-","Quit"]"#));
        assert_eq!(m.len(), 3);
        assert_eq!(m[2].verb.as_deref(), Some("Quit"));
        assert!(parse(&json!("not json")).is_empty());
        assert!(parse(&json!(3)).is_empty());
    }

    #[test]
    fn a_pick_reports_the_verb_and_writes_the_state() {
        let mut m = file_menu();
        assert_eq!(
            pick(&mut m, &[0]),
            Some(Pick {
                verb: "open".into(),
                state: None
            })
        );
        assert_eq!(pick(&mut m, &[2]).unwrap().state, Some(true));
        assert_eq!(m[2].kind, EntryKind::Check(true), "the check flipped");
        assert_eq!(pick(&mut m, &[2]).unwrap().state, Some(false));
    }

    #[test]
    fn one_of_several_turns_the_rest_of_its_group_off() {
        let mut m = file_menu();
        let picked = pick(&mut m, &[3, 1]).unwrap();
        assert_eq!(picked.verb, "select");
        let sub = m[3].submenu().unwrap();
        assert!(!sub[0].marked());
        assert!(sub[1].marked());
    }

    #[test]
    fn a_separator_a_submenu_and_a_disabled_entry_are_not_picked() {
        let mut m = file_menu();
        assert_eq!(pick(&mut m, &[1]), None);
        assert_eq!(pick(&mut m, &[3]), None);
        assert_eq!(pick(&mut m, &[4]), None);
        assert_eq!(pick(&mut m, &[9]), None);
    }

    #[test]
    fn a_menu_reads_back_as_the_json_it_was() {
        let m = file_menu();
        assert_eq!(parse(&to_json(&m)), m);
    }
}
