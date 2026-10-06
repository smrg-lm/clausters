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
//!
//! **A binding may hold only inside a scope.** A window says which scopes are
//! in force in it (its `keys` prop), and a chord bound in one of them is read
//! before the table's own: inside a score editor's note entry the letters are
//! pitches, while `E` is still the host's `split` everywhere else. A scope is
//! a sub-table, `[gui.keys.note_entry]` in the config and a nested object on
//! `/gui_keys`. Escape, the dismissal, may be bound in a scope -- leaving a
//! mode is a dismissal -- and never in the table's own rows.

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
    /// Show the window's keys: the sheet of what each key does in it
    /// ([`Keymap::sheet`]).
    Keys,
}

impl Verb {
    /// Every host verb, with the name the table and the wire spell it with.
    pub const ALL: [(Verb, &'static str); 14] = [
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
        (Verb::Keys, "keys"),
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
    ("keys", &["F1"]),
];

/// **What each verb of the default table does**, in the words a key sheet
/// shows it with ([`Keymap::sheet`]). A verb not here -- one a config or an
/// application bound -- is shown by its name ([`describe`]).
const DESCRIBED: &[(&str, &str)] = &[
    ("undo", "Undo"),
    ("redo", "Redo"),
    ("save", "Save"),
    ("view_all", "Show the whole view"),
    ("play", "Play or stop"),
    ("loop", "Loop on or off"),
    ("to_start", "Cursor to the start"),
    ("to_end", "Cursor to the end"),
    ("copy", "Copy"),
    ("cut", "Cut"),
    ("paste", "Paste"),
    ("mix", "Paste onto what is there"),
    ("quantize", "Quantize to the grid"),
    ("split", "Split at the cursor"),
    ("join", "Join what touches"),
    ("delete", "Delete"),
    ("keys", "Show the keys"),
    ("entry", "Note entry on or off"),
    ("entry_off", "Leave note entry"),
    ("deselect", "Select nothing"),
    ("select_left", "Select the item before"),
    ("select_right", "Select the item after"),
    ("step_up", "Up a step"),
    ("step_down", "Down a step"),
    ("octave_up", "Up an octave"),
    ("octave_down", "Down an octave"),
    ("pitch_a", "Write an A"),
    ("pitch_b", "Write a B"),
    ("pitch_c", "Write a C"),
    ("pitch_d", "Write a D"),
    ("pitch_e", "Write an E"),
    ("pitch_f", "Write an F"),
    ("pitch_g", "Write a G"),
    ("chord_a", "Add an A to the chord"),
    ("chord_b", "Add a B to the chord"),
    ("chord_c", "Add a C to the chord"),
    ("chord_d", "Add a D to the chord"),
    ("chord_e", "Add an E to the chord"),
    ("chord_f", "Add an F to the chord"),
    ("chord_g", "Add a G to the chord"),
    ("cursor_left", "Cursor back"),
    ("cursor_right", "Cursor forward"),
    ("bar_left", "Cursor to the bar before"),
    ("bar_right", "Cursor to the bar after"),
    ("staff_up", "Cursor to the staff above"),
    ("staff_down", "Cursor to the staff below"),
    ("voice_1", "Voice 1"),
    ("voice_2", "Voice 2"),
    ("voice_3", "Voice 3"),
    ("voice_4", "Voice 4"),
    ("value_64th", "Sixty-fourth note"),
    ("value_32nd", "Thirty-second note"),
    ("value_16th", "Sixteenth note"),
    ("value_eighth", "Eighth note"),
    ("value_quarter", "Quarter note"),
    ("value_half", "Half note"),
    ("value_whole", "Whole note"),
    ("dot", "Dotted"),
    ("enter_rest", "Rest"),
];

/// **What verb `verb` does**, as a key sheet says it: the default table's
/// words for its own verbs, and for any other the name, read as words
/// (`select_all` is "Select all").
pub fn describe(verb: &str) -> String {
    if let Some((_, words)) = DESCRIBED.iter().find(|(v, _)| *v == verb) {
        return (*words).to_string();
    }
    words(verb)
}

/// A name as words: underscores are spaces and the first letter is a
/// capital.
fn words(name: &str) -> String {
    let spaced = name.replace('_', " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// One section of a window's key sheet ([`Keymap::sheet`]): what it is
/// called, and each verb in it with the keys that perform it there.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub title: String,
    /// `(what the verb does, its keys)`, the keys spelled as a menu shows
    /// them and joined by commas.
    pub rows: Vec<(String, String)>,
}

/// A table's rows: each verb and the chords that perform it.
type Rows = &'static [(&'static str, &'static [&'static str])];

/// The scopes the host starts with, each a table of its own: the score
/// editor's window (`score`) and its note entry (`note_entry`), whose keys are
/// the settled ones of notation programs.
const SCOPED: &[(&str, Rows)] = &[
    (
        "score",
        &[
            ("entry", &["N"]),
            // over the selection: what note entry's own rows, read first
            // while the window is in it, give the cursor and the note written
            ("delete", &["Delete", "Backspace"]),
            ("deselect", &["Escape"]),
            ("select_left", &["Left"]),
            ("select_right", &["Right"]),
            ("step_up", &["Up"]),
            ("step_down", &["Down"]),
            ("octave_up", &["Ctrl+Up"]),
            ("octave_down", &["Ctrl+Down"]),
        ],
    ),
    (
        "note_entry",
        &[
            ("entry", &["N"]),
            ("entry_off", &["Escape"]),
            ("pitch_a", &["A"]),
            ("pitch_b", &["B"]),
            ("pitch_c", &["C"]),
            ("pitch_d", &["D"]),
            ("pitch_e", &["E"]),
            ("pitch_f", &["F"]),
            ("pitch_g", &["G"]),
            ("chord_a", &["Shift+A"]),
            ("chord_b", &["Shift+B"]),
            ("chord_c", &["Shift+C"]),
            ("chord_d", &["Shift+D"]),
            ("chord_e", &["Shift+E"]),
            ("chord_f", &["Shift+F"]),
            ("chord_g", &["Shift+G"]),
            ("cursor_left", &["Left"]),
            ("cursor_right", &["Right"]),
            ("bar_left", &["Ctrl+Left"]),
            ("bar_right", &["Ctrl+Right"]),
            ("staff_up", &["Alt+Up"]),
            ("staff_down", &["Alt+Down"]),
            ("step_up", &["Up"]),
            ("step_down", &["Down"]),
            ("octave_up", &["Ctrl+Up"]),
            ("octave_down", &["Ctrl+Down"]),
            ("voice_1", &["Ctrl+Alt+1"]),
            ("voice_2", &["Ctrl+Alt+2"]),
            ("voice_3", &["Ctrl+Alt+3"]),
            ("voice_4", &["Ctrl+Alt+4"]),
            ("value_64th", &["1"]),
            ("value_32nd", &["2"]),
            ("value_16th", &["3"]),
            ("value_eighth", &["4"]),
            ("value_quarter", &["5"]),
            ("value_half", &["6"]),
            ("value_whole", &["7"]),
            ("dot", &["."]),
            ("enter_rest", &["0"]),
        ],
    ),
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
                // The platform's command key: Control, or Command on a Mac,
                // so a file written on either reads the same on both.
                "ctrl" | "control" | "cmd" | "command" => mods.ctrl = true,
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
            // The ring's: the platform's, never a verb's.
            "tab" => {
                return Err(format!("'{name}' is reserved and cannot be bound"));
            }
            // The dismissal's, which only a scope may bind ([`Keymap::bind_in`]).
            "escape" | "esc" => Key::Escape,
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

    /// The chord as a reader sees it beside a menu entry: `Ctrl+Shift+Z`, or
    /// `Cmd+Shift+Z` on a Mac, where the host's Ctrl is Command.
    pub fn label(&self, mac: bool) -> String {
        let mut out = String::new();
        for (on, name) in [
            (self.mods.ctrl, if mac { "Cmd+" } else { "Ctrl+" }),
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

/// One verb and the chords that perform it, in the table's own rows or in a
/// scope's.
#[derive(Debug, Clone, PartialEq)]
struct Binding {
    scope: Option<String>,
    verb: String,
    chords: Vec<Chord>,
}

/// **The host's key table**: verbs and the chords bound to them.
#[derive(Debug, Clone, PartialEq)]
pub struct Keymap {
    bindings: Vec<Binding>,
    /// Whether the chords are shown as a Mac shows them: the host's Ctrl is
    /// Command there, so a menu says `Cmd+Z`. The front says which platform
    /// it is on ([`Keymap::on_mac`]); a native build knows at compile time.
    mac: bool,
}

impl Default for Keymap {
    fn default() -> Self {
        let mut map = Keymap {
            bindings: Vec::new(),
            mac: cfg!(target_os = "macos"),
        };
        for (verb, chords) in DEFAULTS {
            let warnings = map.bind(verb, chords);
            debug_assert!(warnings.is_empty(), "{warnings:?}");
        }
        for (scope, rows) in SCOPED {
            for (verb, chords) in *rows {
                let warnings = map.bind_in(Some(scope), verb, chords);
                debug_assert!(warnings.is_empty(), "{warnings:?}");
            }
        }
        map
    }
}

impl Keymap {
    /// The verb a press asks for, if any chord in the table's own rows is it.
    pub fn lookup(&self, key: &Key, mods: Mods) -> Option<&str> {
        self.lookup_in(key, mods, &[])
    }

    /// The verb a press asks for **in a window where `scopes` are in force**:
    /// the last scope named that binds the chord, else the table's own rows.
    pub fn lookup_in(&self, key: &Key, mods: Mods, scopes: &[String]) -> Option<&str> {
        let chord = Chord::of(key, mods);
        let bound = |scope: Option<&str>| {
            self.bindings
                .iter()
                .find(|b| b.scope.as_deref() == scope && b.chords.contains(&chord))
                .map(|b| b.verb.as_str())
        };
        scopes
            .iter()
            .rev()
            .find_map(|scope| bound(Some(scope)))
            .or_else(|| bound(None))
    }

    /// The first chord bound to `verb`, as a reader sees it -- what a menu
    /// entry naming the verb shows beside its label. The table's own rows
    /// first, then any scope's.
    pub fn label(&self, verb: &str) -> Option<String> {
        let first = |global: bool| {
            self.bindings
                .iter()
                .filter(|b| b.scope.is_none() == global && b.verb == verb)
                .find_map(|b| b.chords.first())
        };
        first(true)
            .or_else(|| first(false))
            .map(|c| c.label(self.mac))
    }

    /// **The keys of a window where `scopes` are in force**, as a sheet a
    /// reader looks them up in: one section per scope, the one read first
    /// first, then the table's own rows. A chord is listed only where it is
    /// what a press does in that window -- a scope's arrow hides the table's
    /// -- and a verb left with none is not listed, nor a section left empty.
    pub fn sheet(&self, scopes: &[String]) -> Vec<Section> {
        let mut named: Vec<Option<&str>> = scopes.iter().rev().map(|s| Some(s.as_str())).collect();
        named.dedup();
        named.push(None);
        named
            .into_iter()
            .filter_map(|scope| {
                let rows: Vec<(String, String)> = self
                    .bindings
                    .iter()
                    .filter(|b| b.scope.as_deref() == scope)
                    .filter_map(|b| {
                        let live: Vec<String> = b
                            .chords
                            .iter()
                            .filter(|c| {
                                self.lookup_in(&c.key, c.mods, scopes) == Some(b.verb.as_str())
                            })
                            .map(|c| c.label(self.mac))
                            .collect();
                        (!live.is_empty()).then(|| (describe(&b.verb), live.join(", ")))
                    })
                    .collect();
                let title = scope.map_or_else(|| "Window".to_string(), words);
                (!rows.is_empty()).then_some(Section { title, rows })
            })
            .collect()
    }

    /// Says the host runs on a Mac -- what a page learns from its browser,
    /// since the same wasm runs everywhere.
    pub fn on_mac(&mut self, mac: bool) {
        self.mac = mac;
    }

    /// Binds `verb` to exactly `chords`, replacing what it had: an empty list
    /// unbinds it. A chord another verb had is taken from that verb, since a
    /// chord means one thing. Returns a warning per chord it could not read,
    /// which is skipped -- never fatal, as the theme's bad colors are not.
    pub fn bind(&mut self, verb: &str, chords: &[&str]) -> Vec<String> {
        self.bind_in(None, verb, chords)
    }

    /// [`bind`](Self::bind), in `scope` -- `None` for the table's own rows. A
    /// chord means one verb **within** a scope, and Escape is refused outside
    /// one.
    pub fn bind_in(&mut self, scope: Option<&str>, verb: &str, chords: &[&str]) -> Vec<String> {
        let mut warnings = Vec::new();
        let verb = verb.trim();
        if verb.is_empty() {
            warnings.push("keys: an entry with no verb name".to_string());
            return warnings;
        }
        let mut parsed = Vec::new();
        for text in chords.iter().filter(|c| !c.trim().is_empty()) {
            match Chord::parse(text) {
                Ok(chord) if chord.key == Key::Escape && scope.is_none() => warnings.push(format!(
                    "keys: {verb}: Escape is reserved outside a scope and cannot be bound"
                )),
                Ok(chord) if !parsed.contains(&chord) => parsed.push(chord),
                Ok(_) => {}
                Err(e) => warnings.push(format!("keys: {verb}: {e}")),
            }
        }
        for b in self
            .bindings
            .iter_mut()
            .filter(|b| b.scope.as_deref() == scope)
        {
            b.chords.retain(|c| !parsed.contains(c));
        }
        match self
            .bindings
            .iter_mut()
            .find(|b| b.scope.as_deref() == scope && b.verb == verb)
        {
            Some(b) => b.chords = parsed,
            None => self.bindings.push(Binding {
                scope: scope.map(str::to_string),
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
        self.overlay_in(None, entries)
    }

    /// [`overlay`](Self::overlay), into `scope`.
    pub fn overlay_in<'a, I>(&mut self, scope: Option<&str>, entries: I) -> Vec<String>
    where
        I: IntoIterator<Item = (&'a str, Vec<&'a str>)>,
    {
        entries
            .into_iter()
            .flat_map(|(verb, chords)| self.bind_in(scope, verb, &chords))
            .collect()
    }

    /// Overlays a JSON object of `verb: "chord"` or `verb: ["chord", ...]` --
    /// `/gui_keys`'s payload, the same table the TOML file carries; a value
    /// that is itself an object is a scope's table. A value of any other shape
    /// is reported and skipped.
    pub fn overlay_json(
        &mut self,
        table: &serde_json::Map<String, serde_json::Value>,
    ) -> Vec<String> {
        self.overlay_json_in(None, table)
    }

    fn overlay_json_in(
        &mut self,
        scope: Option<&str>,
        table: &serde_json::Map<String, serde_json::Value>,
    ) -> Vec<String> {
        let mut warnings = Vec::new();
        for (verb, value) in table {
            if let (None, serde_json::Value::Object(inner)) = (scope, value) {
                warnings.extend(self.overlay_json_in(Some(verb), inner));
                continue;
            }
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
            warnings.extend(self.bind_in(scope, verb, &chords));
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
        let warnings = map.bind("split", &["Hyper+E", "Tab", "F13", "Ctrl+E", "Escape"]);
        assert_eq!(warnings.len(), 4, "{warnings:?}");
        assert_eq!(map.lookup(&Key::Char('e'), ctrl()), Some("split"));
    }

    /// **A scope's chord is read before the table's**, only in a window where
    /// the scope is in force, and Escape is a scope's alone.
    #[test]
    fn a_scope_binds_over_the_table_where_it_is_in_force() {
        let mut map = Keymap::default();
        let none = Mods::default();
        let entry = vec!["score".to_string(), "note_entry".to_string()];
        assert_eq!(map.lookup(&Key::Char('e'), none), Some("split"));
        assert_eq!(
            map.lookup_in(&Key::Char('e'), none, &entry),
            Some("pitch_e")
        );
        // a chord the scope does not bind is the table's
        assert_eq!(
            map.lookup_in(&Key::Char('q'), none, &entry),
            Some("quantize")
        );
        assert_eq!(
            map.lookup_in(&Key::Char('n'), none, &entry[..1]),
            Some("entry")
        );
        assert_eq!(map.lookup_in(&Key::Escape, none, &entry), Some("entry_off"));
        assert_eq!(map.lookup(&Key::Escape, none), None);
        // one key, two verbs by the mode the window is in: the arrows and
        // Escape are the selection's, and the cursor's in note entry
        for (key, selecting, writing) in [
            (Key::Left, "select_left", "cursor_left"),
            (Key::Right, "select_right", "cursor_right"),
            (Key::Escape, "deselect", "entry_off"),
            (Key::Up, "step_up", "step_up"),
            (Key::Delete, "delete", "delete"),
        ] {
            assert_eq!(map.lookup_in(&key, none, &entry[..1]), Some(selecting));
            assert_eq!(map.lookup_in(&key, none, &entry), Some(writing));
        }
        let warnings = map.bind("leave", &["Escape"]);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        // a scope's table overlays from JSON as a nested object, and takes a
        // chord from the scope's other verbs only
        let table = serde_json::json!({"note_entry": {"pitch_e": "Shift+E"}});
        assert!(map.overlay_json(table.as_object().unwrap()).is_empty());
        let shift = Mods {
            shift: true,
            ..none
        };
        assert_eq!(
            map.lookup_in(&Key::Char('e'), shift, &entry),
            Some("pitch_e")
        );
        assert_eq!(map.lookup(&Key::Char('e'), none), Some("split"));
        assert_eq!(map.label("pitch_c").as_deref(), Some("C"));
    }

    /// **A window's key sheet is what a press does in it**: its scopes
    /// first, the one read first at the top, and a chord a scope takes over
    /// listed only there.
    #[test]
    fn a_key_sheet_lists_what_each_key_does_in_the_window() {
        let mut map = Keymap::default();
        let entry = vec!["score".to_string(), "note_entry".to_string()];
        let sheet = map.sheet(&entry);
        let titles: Vec<&str> = sheet.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["Note entry", "Score", "Window"]);
        let row = |section: usize, what: &str| {
            sheet[section]
                .rows
                .iter()
                .find(|(w, _)| w == what)
                .map(|(_, keys)| keys.clone())
        };
        assert_eq!(row(0, "Write an E").as_deref(), Some("E"));
        assert_eq!(row(0, "Cursor back").as_deref(), Some("Left"));
        // the arrows are note entry's, so the selection's are not listed
        assert_eq!(row(1, "Select the item before"), None);
        // and E is a pitch, so the table's split is gone
        assert_eq!(row(2, "Split at the cursor"), None);
        assert_eq!(row(2, "Redo").as_deref(), Some("Ctrl+Shift+Z, Ctrl+Y"));
        assert_eq!(row(2, "Show the keys").as_deref(), Some("F1"));
        // a window with no scopes has the table alone, and a verb nobody
        // described is shown by its name
        assert!(map.bind("select_all", &["Ctrl+A"]).is_empty());
        let plain = map.sheet(&[]);
        assert_eq!(plain.len(), 1);
        assert!(
            plain[0]
                .rows
                .contains(&("Select all".to_string(), "Ctrl+A".to_string()))
        );
        assert!(
            plain[0]
                .rows
                .contains(&("Split at the cursor".to_string(), "E".to_string()))
        );
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
            assert_eq!(Chord::parse(text).unwrap().label(false), text);
        }
        assert_eq!(
            Chord::parse("ctrl+shift+z").unwrap().label(false),
            "Ctrl+Shift+Z"
        );
        // Ctrl is the command key, so a Mac spells it and shows it as Cmd.
        let undo = Chord::parse("Cmd+Z").unwrap();
        assert_eq!(undo, Chord::parse("Ctrl+Z").unwrap());
        assert_eq!(undo.label(true), "Cmd+Z");
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
