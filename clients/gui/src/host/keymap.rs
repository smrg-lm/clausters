//! **Which key performs which verb**: the host's key table, set outside the
//! code.
//!
//! A key is a *binding*, a name for a verb chosen by whoever uses the program,
//! so no element spells a letter: an element performs verbs
//! ([`Element::verb`](super::widget::Element::verb)) and this table says which
//! chord asks for each. It is the theme's peer -- one per host, read by both
//! fronts through the one dispatch in the gesture machine -- and it is set at
//! the same levels: the defaults here, `[gui.keys]` in the config, a
//! `--keys <file>` and `/gui_keys` at run time, each overlaying the one before.
//!
//! **A verb is a name, and not every name is the host's.** The host performs
//! the ones in [`Verb`]; any other name is the application's, and pressing its
//! chord reports it to the window's owner. That is what lets a program with no
//! chrome at all have commands: it names them in the table and answers them.

use super::widget::element::{Key, Mods};

/// A verb the **host** performs. Every other bound name is the application's,
/// reported rather than performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    /// Every navigable view back to its whole extent.
    ViewAll,
    /// Play what is under the pointer, or stop what is playing.
    Play,
    /// Switch the take monitor's loop.
    Loop,
    /// The position cursor to the start of what is under the pointer.
    ToStart,
    /// ...and to its end.
    ToEnd,
    Copy,
    Cut,
    Paste,
    /// A paste that adds the block onto what is there.
    Mix,
    /// The selection onto the grid.
    Quantize,
    /// Cut what is held at the window's cursor.
    Split,
    /// Join a touching run of what is held.
    Join,
    /// Remove what is held.
    Delete,
}

impl Verb {
    /// Every host verb, with the name the table and the wire spell it with.
    pub const ALL: [(Verb, &'static str); 13] = [
        (Verb::ViewAll, "view_all"),
        (Verb::Play, "play"),
        (Verb::Loop, "loop"),
        (Verb::ToStart, "to_start"),
        (Verb::ToEnd, "to_end"),
        (Verb::Copy, "copy"),
        (Verb::Cut, "cut"),
        (Verb::Paste, "paste"),
        (Verb::Mix, "mix"),
        (Verb::Quantize, "quantize"),
        (Verb::Split, "split"),
        (Verb::Join, "join"),
        (Verb::Delete, "delete"),
    ];

    /// The host verb a name is, or `None` for an application's.
    pub fn named(name: &str) -> Option<Verb> {
        Verb::ALL.iter().find(|(_, n)| *n == name).map(|(v, _)| *v)
    }

    /// The name this verb is spelled with.
    pub fn name(self) -> &'static str {
        Verb::ALL
            .iter()
            .find(|(v, _)| *v == self)
            .map(|(_, n)| *n)
            .expect("every verb is in ALL")
    }
}

/// The table the host starts from: every key the views answered to before the
/// table existed, now as rows of it, and the application verbs every editor
/// has (`undo`, `redo`, `save`).
const DEFAULTS: &[(&str, &[&str])] = &[
    ("undo", &["Ctrl+Z"]),
    // Ctrl+Shift+Z is the spelling that works on a keyboard with no Y where an
    // English one has one.
    ("redo", &["Ctrl+Shift+Z", "Ctrl+Y"]),
    ("save", &["Ctrl+S"]),
    ("view_all", &["R"]),
    ("play", &["Space"]),
    ("loop", &["L"]),
    ("to_start", &["Home"]),
    ("to_end", &["End"]),
    ("copy", &["Ctrl+C"]),
    ("cut", &["Ctrl+X"]),
    ("paste", &["Ctrl+V"]),
    ("mix", &["Ctrl+Shift+V"]),
    ("quantize", &["Q"]),
    ("split", &["E"]),
    ("join", &["J"]),
    ("delete", &["Delete", "Backspace"]),
];

/// A key and the modifiers held with it, **normalized** so the table and a
/// press compare equal whichever way they were spelled or typed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Chord {
    pub key: Key,
    pub mods: Mods,
}

impl Chord {
    /// The chord a press is, normalized: a letter is compared in lower case
    /// with its Shift kept (`Q` is not `Shift+Q`), and a punctuation character
    /// drops Shift, since the character already says it (`?` is Shift+`/` on
    /// one layout and not on another). The space bar keeps its Shift.
    pub fn of(key: &Key, mods: Mods) -> Chord {
        let mut mods = mods;
        let key = match *key {
            Key::Char(c) if c.is_alphabetic() => Key::Char(c.to_lowercase().next().unwrap_or(c)),
            Key::Char(' ') => Key::Char(' '),
            Key::Char(c) => {
                mods.shift = false;
                Key::Char(c)
            }
            ref other => other.clone(),
        };
        Chord { key, mods }
    }

    /// Reads a chord as the table spells it: modifiers and a key joined by
    /// `+`, case-insensitive (`"Ctrl+Shift+Z"`, `"space"`, `"F5"`, `"Ctrl++"`).
    pub fn parse(text: &str) -> Result<Chord, String> {
        let text = text.trim();
        // A trailing `+` is the plus key itself: `Ctrl++` is Ctrl and `+`.
        let (head, last) = match text.strip_suffix("++") {
            Some(head) => (head, "+"),
            None if text == "+" => ("", "+"),
            None => match text.rsplit_once('+') {
                Some((head, last)) => (head, last),
                None => ("", text),
            },
        };
        let mut mods = Mods::default();
        for m in head.split('+').filter(|m| !m.is_empty()) {
            match m.trim().to_ascii_lowercase().as_str() {
                "ctrl" | "control" => mods.ctrl = true,
                "shift" => mods.shift = true,
                "alt" | "option" => mods.alt = true,
                other => return Err(format!("unknown modifier '{other}' in '{text}'")),
            }
        }
        let name = last.trim();
        let lower = name.to_ascii_lowercase();
        let key = match lower.as_str() {
            "" => return Err(format!("'{text}' names no key")),
            "space" => Key::Char(' '),
            "delete" | "del" => Key::Delete,
            "backspace" => Key::Backspace,
            "enter" | "return" => Key::Enter,
            "home" => Key::Home,
            "end" => Key::End,
            "left" => Key::Left,
            "right" => Key::Right,
            "up" => Key::Up,
            "down" => Key::Down,
            // The ring's and the dismissal's: the platform's, never a verb's.
            "tab" | "escape" | "esc" => {
                return Err(format!("'{name}' is reserved and cannot be bound"));
            }
            f if f.len() > 1 && f.starts_with('f') && f[1..].parse::<u8>().is_ok() => {
                let n: u8 = f[1..].parse().unwrap_or(0);
                if !(1..=12).contains(&n) {
                    return Err(format!("no key '{name}' (F1 to F12)"));
                }
                Key::F(n)
            }
            _ => {
                let mut chars = name.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Key::Char(c),
                    _ => return Err(format!("unknown key '{name}' in '{text}'")),
                }
            }
        };
        Ok(Chord::of(&key, mods))
    }

    /// The chord as a reader sees it beside a menu entry: `Ctrl+Shift+Z`.
    pub fn label(&self) -> String {
        let mut out = String::new();
        for (on, name) in [
            (self.mods.ctrl, "Ctrl+"),
            (self.mods.alt, "Alt+"),
            (self.mods.shift, "Shift+"),
        ] {
            if on {
                out.push_str(name);
            }
        }
        match &self.key {
            Key::Char(' ') => out.push_str("Space"),
            Key::Char(c) => out.extend(c.to_uppercase()),
            Key::F(n) => out.push_str(&format!("F{n}")),
            other => out.push_str(&format!("{other:?}")),
        }
        out
    }
}

/// One verb and the chords that perform it.
#[derive(Debug, Clone, PartialEq)]
struct Binding {
    verb: String,
    chords: Vec<Chord>,
}

/// **The host's key table**: verbs and the chords bound to them.
#[derive(Debug, Clone, PartialEq)]
pub struct Keymap {
    bindings: Vec<Binding>,
}

impl Default for Keymap {
    fn default() -> Self {
        let mut map = Keymap {
            bindings: Vec::new(),
        };
        for (verb, chords) in DEFAULTS {
            let warnings = map.bind(verb, chords);
            debug_assert!(warnings.is_empty(), "{warnings:?}");
        }
        map
    }
}

impl Keymap {
    /// The verb a press asks for, if any chord in the table is it.
    pub fn lookup(&self, key: &Key, mods: Mods) -> Option<&str> {
        let chord = Chord::of(key, mods);
        self.bindings
            .iter()
            .find(|b| b.chords.contains(&chord))
            .map(|b| b.verb.as_str())
    }

    /// The first chord bound to `verb`, as a reader sees it -- what a menu
    /// entry naming the verb shows beside its label.
    pub fn label(&self, verb: &str) -> Option<String> {
        self.bindings
            .iter()
            .find(|b| b.verb == verb)
            .and_then(|b| b.chords.first())
            .map(Chord::label)
    }

    /// Binds `verb` to exactly `chords`, replacing what it had: an empty list
    /// unbinds it. A chord another verb had is taken from that verb, since a
    /// chord means one thing. Returns a warning per chord it could not read,
    /// which is skipped -- never fatal, as the theme's bad colors are not.
    pub fn bind(&mut self, verb: &str, chords: &[&str]) -> Vec<String> {
        let mut warnings = Vec::new();
        let verb = verb.trim();
        if verb.is_empty() {
            warnings.push("keys: an entry with no verb name".to_string());
            return warnings;
        }
        let mut parsed = Vec::new();
        for text in chords.iter().filter(|c| !c.trim().is_empty()) {
            match Chord::parse(text) {
                Ok(chord) if !parsed.contains(&chord) => parsed.push(chord),
                Ok(_) => {}
                Err(e) => warnings.push(format!("keys: {verb}: {e}")),
            }
        }
        for b in &mut self.bindings {
            b.chords.retain(|c| !parsed.contains(c));
        }
        match self.bindings.iter_mut().find(|b| b.verb == verb) {
            Some(b) => b.chords = parsed,
            None => self.bindings.push(Binding {
                verb: verb.to_string(),
                chords: parsed,
            }),
        }
        warnings
    }

    /// Overlays a table: each entry binds one verb, the ones it does not name
    /// keep what they had.
    pub fn overlay<'a, I>(&mut self, entries: I) -> Vec<String>
    where
        I: IntoIterator<Item = (&'a str, Vec<&'a str>)>,
    {
        entries
            .into_iter()
            .flat_map(|(verb, chords)| self.bind(verb, &chords))
            .collect()
    }

    /// Overlays a JSON object of `verb: "chord"` or `verb: ["chord", ...]` --
    /// `/gui_keys`'s payload, the same table the TOML file carries. A value of
    /// any other shape is reported and skipped.
    pub fn overlay_json(
        &mut self,
        table: &serde_json::Map<String, serde_json::Value>,
    ) -> Vec<String> {
        let mut warnings = Vec::new();
        for (verb, value) in table {
            let chords: Vec<&str> = match value {
                serde_json::Value::String(s) => vec![s.as_str()],
                serde_json::Value::Array(items) => {
                    let texts: Vec<&str> = items.iter().filter_map(|v| v.as_str()).collect();
                    if texts.len() != items.len() {
                        warnings.push(format!("keys: {verb}: every chord is a string"));
                    }
                    texts
                }
                serde_json::Value::Null => Vec::new(),
                _ => {
                    warnings.push(format!("keys: {verb}: a chord or a list of chords"));
                    continue;
                }
            };
            warnings.extend(self.bind(verb, &chords));
        }
        warnings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctrl() -> Mods {
        Mods {
            ctrl: true,
            ..Mods::default()
        }
    }

    fn shift() -> Mods {
        Mods {
            shift: true,
            ..Mods::default()
        }
    }

    /// The defaults are the keys the views answered to before the table: a
    /// letter alone, a chord, a named key, and a verb with two chords.
    #[test]
    fn the_defaults_are_the_keys_the_views_had() {
        let map = Keymap::default();
        assert_eq!(
            map.lookup(&Key::Char('q'), Mods::default()),
            Some("quantize")
        );
        assert_eq!(map.lookup(&Key::Char('Z'), ctrl()), Some("undo"));
        let both = Mods {
            shift: true,
            ..ctrl()
        };
        assert_eq!(map.lookup(&Key::Char('z'), both), Some("redo"));
        assert_eq!(map.lookup(&Key::Char('y'), ctrl()), Some("redo"));
        assert_eq!(map.lookup(&Key::Char(' '), Mods::default()), Some("play"));
        assert_eq!(map.lookup(&Key::Backspace, Mods::default()), Some("delete"));
        assert_eq!(map.lookup(&Key::Char('x'), Mods::default()), None);
        // Every host verb is bound to something by default.
        for (_, name) in Verb::ALL {
            assert!(map.label(name).is_some(), "{name} has no default key");
        }
    }

    /// **A letter's Shift is part of the chord, a punctuation's is not**: `Q`
    /// alone does not fire on Shift+Q, and `?` fires however the layout made it.
    #[test]
    fn shift_counts_on_a_letter_and_not_on_what_it_already_spells() {
        let mut map = Keymap::default();
        assert_eq!(map.lookup(&Key::Char('Q'), shift()), None);
        map.bind("help", &["?"]);
        assert_eq!(map.lookup(&Key::Char('?'), shift()), Some("help"));
        assert_eq!(map.lookup(&Key::Char('?'), Mods::default()), Some("help"));
        map.bind("from_start", &["Shift+Space"]);
        assert_eq!(map.lookup(&Key::Char(' '), shift()), Some("from_start"));
        assert_eq!(map.lookup(&Key::Char(' '), Mods::default()), Some("play"));
    }

    /// Rebinding replaces a verb's chords, takes a chord from the verb that had
    /// it, and an empty list unbinds.
    #[test]
    fn a_binding_replaces_and_a_chord_means_one_verb() {
        let mut map = Keymap::default();
        assert!(map.bind("split", &["S"]).is_empty());
        assert_eq!(map.lookup(&Key::Char('s'), Mods::default()), Some("split"));
        assert_eq!(map.lookup(&Key::Char('e'), Mods::default()), None);
        // An application verb takes Ctrl+S from `save`.
        map.bind("export", &["Ctrl+S"]);
        assert_eq!(map.lookup(&Key::Char('s'), ctrl()), Some("export"));
        assert_eq!(map.label("save"), None);
        map.bind("quantize", &[]);
        assert_eq!(map.lookup(&Key::Char('q'), Mods::default()), None);
    }

    /// What cannot be read is said and skipped, and the reserved keys are
    /// refused.
    #[test]
    fn a_chord_that_cannot_be_read_is_warned_about_and_skipped() {
        let mut map = Keymap::default();
        let warnings = map.bind("split", &["Hyper+E", "Tab", "F13", "Ctrl+E"]);
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert_eq!(map.lookup(&Key::Char('e'), ctrl()), Some("split"));
    }

    #[test]
    fn a_chord_reads_and_prints_the_way_a_reader_spells_it() {
        for text in [
            "Ctrl+Shift+Z",
            "Space",
            "Delete",
            "F5",
            "Alt+Left",
            "Ctrl++",
        ] {
            assert_eq!(Chord::parse(text).unwrap().label(), text);
        }
        assert_eq!(
            Chord::parse("ctrl+shift+z").unwrap().label(),
            "Ctrl+Shift+Z"
        );
    }

    #[test]
    fn the_json_table_takes_a_chord_a_list_or_nothing() {
        let mut map = Keymap::default();
        let table = serde_json::json!({"about": "F1", "redo": ["Ctrl+Y"], "loop": null, "save": 3});
        let warnings = map.overlay_json(table.as_object().unwrap());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(map.lookup(&Key::F(1), Mods::default()), Some("about"));
        assert_eq!(map.label("redo").as_deref(), Some("Ctrl+Y"));
        assert_eq!(map.label("loop"), None);
        assert_eq!(map.label("save").as_deref(), Some("Ctrl+S"));
    }
}
