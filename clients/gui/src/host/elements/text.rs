//! `text` -- an editable field: the leaf whose state is a **caret**.
//!
//! The one leaf that reads the keyboard, and the reason the focus seam exists.
//! What it holds between keystrokes is not a value the script sent: it is where
//! the insertion point is and what is selected, which no `/gui_set` writes and
//! no `/gui_query` reports. That lived on the widget's enum arm, and the editing
//! itself lived in the gesture machine -- a whole parallel path through the
//! host's *one* focused field, because a `WidgetKind` arm cannot own a caret and
//! the machine had nowhere else to put the arms.
//!
//! Here the field owns both. [`Element::key`] is the editing that used to be
//! `gestures::keys::text_key`, moved verbatim onto the type that holds the
//! string; [`Element::press`] drops the caret where the click landed and
//! [`Element::drag`] extends the selection from it, so the drag's anchor is a
//! field of this struct rather than a variant of the machine's `Drag`. The
//! *model* stays [`crate::host::graphics::textedit`] -- pure caret arithmetic
//! over a `String`, unit-tested without a window -- and the drawing stays
//! [`controls::field`], which is the same rule every ported leaf follows: the
//! element owns its state, not its geometry.
//!
//! **Every content change delivers**, exactly as a numeric control delivers on
//! every drag step -- never gated on Enter. A single-line field ignores Enter
//! altogether, because there is nothing for it to mean when the value has
//! already been sent.

use serde_json::{Map, Value};

use clausters_core::osc::OscType;

use crate::host::graphics::controls;
use crate::host::graphics::textedit::{self, Caret, Unit};
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::widget::element::{Claim, Ctx, Element, Events, HitArea, Input, Key, KeyInput};
use crate::host::widget::parse;
use crate::host::widget::size::{Natural, body_inset, control_box, label_strip};

/// An editable text-entry field: the string, how it is presented, and -- while
/// it is being edited -- the caret and the selection anchor.
#[derive(Debug, Clone)]
pub struct Text {
    pub value: String,
    pub label: Option<String>,
    pub text_size: f32,
    /// Whether Enter inserts a newline and the field draws a block of rows
    /// (a text *surface*) instead of one scrolling line.
    pub multiline: bool,
    /// **View state**, never parsed from or sent over the wire: the insertion
    /// point and the selection, meaningful only while the field is focused.
    caret: Caret,
    /// What the press selected and the unit it selected by, while a drag is
    /// extending it: a caret's point for one click, a word for two, a line
    /// for three -- and the drag grows the selection by the same unit.
    held: Option<((usize, usize), Unit)>,
}

pub(super) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(from_props(props)))
}

fn from_props(props: &Map<String, Value>) -> Text {
    Text {
        value: props
            .get("value")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        label: parse::label(props),
        text_size: parse::text_size(props),
        multiline: props
            .get("multiline")
            .and_then(parse::truthy)
            .unwrap_or(false),
        caret: Caret::default(),
        held: None,
    }
}

impl Text {
    /// The caret offset a point lands on, reconstructing the layout the
    /// renderer drew through -- so a click lands on the glyph it points at.
    fn caret_at(&self, at: (f64, f64), input: &Input) -> usize {
        controls::caret_at(
            input.rect,
            &self.value,
            self.label.is_some(),
            self.text_size * input.scale,
            self.multiline,
            self.caret,
            at.0,
            at.1,
            input.metrics,
        )
    }

    /// The field's value as the one thing it reports.
    fn events(&self) -> Events {
        Events::value(OscType::String(self.value.clone()))
    }
}

/// **The keys that edit a line of text**, over a string and its caret: what a
/// field does with a key, as a function of the two, so that anything typed
/// into -- a field, a text of an engraved page edited where it is drawn -- is
/// typed into the same way. `Some(changed)` is consumed, `changed` saying
/// whether the string moved; `None` is a key that edits nothing and goes on
/// (Tab, Escape, a function key, a chord that is the window's).
///
/// Enter is the caller's to mean something by: here it breaks the line of a
/// `multiline` text and is otherwise consumed and inert.
pub(crate) fn edit_key(
    value: &mut String,
    caret: &mut Caret,
    multiline: bool,
    key: &Key,
    input: &mut KeyInput,
) -> Option<bool> {
    let mods = input.mods;
    let mut changed = false;
    match key {
        Key::Char(c) if mods.ctrl => match c.to_ascii_lowercase() {
            'c' => {
                if let Some(s) = textedit::selected(value, caret) {
                    input.clipboard.set_text(s);
                }
            }
            'x' => {
                if let Some(s) = textedit::selected(value, caret) {
                    input.clipboard.set_text(s);
                    changed = textedit::delete_selection(value, caret);
                }
            }
            'v' => {
                // What another program copied since is what is pasted.
                input.clipboard.refresh();
                if !input.clipboard.is_empty() {
                    // A single-line field takes a pasted block as one line.
                    let pasted = input.clipboard.text();
                    let text = if multiline {
                        pasted
                    } else {
                        pasted.replace('\n', " ")
                    };
                    changed = textedit::insert(value, caret, &text);
                }
            }
            'a' => textedit::select_all(value, caret),
            // **Undo and redo are the field's while it is typed in**, as
            // in every text field -- this one keeps no history, so they
            // are consumed and inert rather than undoing the document
            // behind it in the middle of a word.
            'z' | 'y' => {}
            // Any other chord is not editing, so it is the window's: a
            // field holding the focus does not take Ctrl+S or Ctrl+O away
            // from the program, as no platform's field does.
            _ => return None,
        },
        // A plain printable char inserts; an Alt chord is a command, the
        // key table's.
        Key::Char(c) if !mods.alt => {
            changed = textedit::insert(value, caret, c.encode_utf8(&mut [0; 4]));
        }
        Key::Char(_) => return None,
        Key::Backspace if mods.ctrl => changed = textedit::backspace_word(value, caret),
        Key::Backspace => changed = textedit::backspace(value, caret),
        Key::Delete if mods.ctrl => changed = textedit::delete_word(value, caret),
        Key::Delete => changed = textedit::delete(value, caret),
        Key::Left if mods.ctrl => textedit::move_word_left(value, caret, mods.shift),
        Key::Left => textedit::move_left(value, caret, mods.shift),
        Key::Right if mods.ctrl => textedit::move_word_right(value, caret, mods.shift),
        Key::Right => textedit::move_right(value, caret, mods.shift),
        Key::Up => textedit::move_up(value, caret, mods.shift),
        Key::Down => textedit::move_down(value, caret, mods.shift),
        Key::Home => textedit::move_home(value, caret, mods.shift),
        Key::End => textedit::move_end(value, caret, mods.shift),
        Key::Enter if multiline => {
            changed = textedit::insert(value, caret, "\n");
        }
        // A single-line field ignores Enter: the value has already been
        // delivered, so there is no send for it to trigger.
        Key::Enter => {}
        // The ring's, never the field's -- and a function key edits
        // nothing, so it goes on to the key table.
        Key::Tab | Key::Escape | Key::F(_) => return None,
    }
    Some(changed)
}

impl Element for Text {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "value" => v
                .as_str()
                .map(|s| {
                    self.value = s.to_string();
                    // The caret/selection may now point past the new string or
                    // off a char boundary -- re-land it.
                    textedit::clamp(&self.value, &mut self.caret);
                })
                .is_some(),
            "label" => parse::set_label(&mut self.label, v),
            "text_size" => parse::set_size(&mut self.text_size, v),
            "multiline" => parse::truthy(v).map(|b| self.multiline = b).is_some(),
            _ => false,
        }
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        controls::field(
            d,
            &self.value,
            self.label.as_deref(),
            ctx.rect,
            self.text_size * ctx.scale,
            self.multiline,
            // The caret and the selection are drawn only while the field is
            // focused: unfocused, the value is text like any other.
            ctx.focused.then_some(self.caret),
        );
    }

    fn natural(&self, m: &Metrics, scale: f32) -> Natural {
        let size = self.text_size * scale;
        (
            None,
            // A multiline field is a text *surface*: its height is the caller's,
            // and it scrolls its rows inside it.
            (!self.multiline).then(|| {
                label_strip(self.label.is_some(), size, m) + body_inset(m) + control_box(size, m)
            }),
        )
    }

    /// Squeezed, a single-line field gives up its **label strip** and keeps
    /// its box: a field shorter than the row it is typed into cannot show the
    /// caret it is being typed with. A multiline field is elastic and floors
    /// at nothing, the same answer its natural size gives.
    fn floor(&self, m: &Metrics, scale: f32) -> Natural {
        let size = self.text_size * scale;
        (
            None,
            (!self.multiline).then(|| body_inset(m) + control_box(size, m)),
        )
    }

    fn value(&self) -> Option<OscType> {
        Some(OscType::String(self.value.clone()))
    }

    fn info(&self) -> Vec<(String, Value)> {
        vec![("value".into(), Value::from(self.value.clone()))]
    }

    fn accepts_focus(&self) -> bool {
        true
    }

    /// Every key it answers is a character it stores, which is the whole of
    /// what this asks -- so the browser shell hands it a composed one.
    fn takes_text(&self) -> bool {
        true
    }

    /// **The field, not the cell.** A field is the body drawn under its label,
    /// single-line or multiline alike, and a press on the label strip or on the
    /// air a row stretched around it used to focus the field and land a caret
    /// in text nobody pointed at.
    fn hit_area(&self, input: &Input) -> HitArea {
        HitArea::Rect(controls::body_rect_at(
            input.rect,
            self.label.is_some(),
            self.text_size * input.scale,
            input.metrics,
        ))
    }

    /// **One click places the caret, two select a word, three a line** --
    /// the convention of every text field, counted by the machine
    /// ([`Input::clicks`]). The press is held: a drag from here extends the
    /// selection by the same unit.
    fn press(&mut self, at: (f64, f64), input: &Input) -> Claim {
        // A stale caret is re-landed first.
        textedit::clamp(&self.value, &mut self.caret);
        let pos = self.caret_at(at, input);
        // **Shift+click extends** what is selected to where it landed, from
        // the end the selection was made from -- and a drag carries on from
        // there.
        if input.mods.shift && input.clicks == 1 {
            let from = self.caret.anchor.unwrap_or(self.caret.pos);
            self.caret = textedit::extend_by(&self.value, (from, from), pos, Unit::Char);
            self.held = Some(((from, from), Unit::Char));
            return Claim::take();
        }
        let unit = Unit::of_clicks(input.clicks);
        let span = textedit::unit_at(&self.value, pos, unit);
        self.caret = textedit::extend_by(&self.value, span, pos, Unit::Char);
        self.held = Some((span, unit));
        Claim::take()
    }

    fn selected_text(&self) -> Option<String> {
        textedit::selected(&self.value, &self.caret).map(str::to_string)
    }

    /// The middle button: the caret goes where the pointer is and the text is
    /// put there -- one line of it in a single-line field, as a paste is.
    fn paste_at(&mut self, at: (f64, f64), input: &Input, text: &str) -> Option<Events> {
        textedit::clamp(&self.value, &mut self.caret);
        let pos = self.caret_at(at, input);
        self.caret = Caret { pos, anchor: None };
        let text = if self.multiline {
            text.to_string()
        } else {
            text.replace('\n', " ")
        };
        Some(
            if textedit::insert(&mut self.value, &mut self.caret, &text) {
                self.events()
            } else {
                Events::none()
            },
        )
    }

    fn drag(&mut self, at: (f64, f64), input: &Input) -> Events {
        let Some((span, unit)) = self.held else {
            return Events::none();
        };
        let pos = self.caret_at(at, input);
        // An empty selection keeps no anchor, so a click-and-return draws no
        // highlight.
        self.caret = textedit::extend_by(&self.value, span, pos, unit);
        // Selecting changes nothing about the value, so there is nothing to
        // report -- the redraw a claim already asks for is the whole effect.
        Events::none()
    }

    fn key(&mut self, key: &Key, input: &mut KeyInput) -> Option<Events> {
        let changed = edit_key(&mut self.value, &mut self.caret, self.multiline, key, input)?;
        // Consumed either way -- the caret moved, which is a repaint -- and a
        // content change also delivers the new value, ungated.
        Some(if changed {
            self.events()
        } else {
            Events::none()
        })
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::layout::Rect;
    use crate::host::widget::element::Mods;

    fn props(json: &str) -> Map<String, Value> {
        serde_json::from_str(json).unwrap()
    }

    fn input<'a>(m: &'a Metrics) -> Input<'a> {
        Input {
            metrics: m,
            indent: 0.0,
            rect: Rect::new(0.0, 0.0, 200.0, 24.0),
            scale: 1.0,
            mods: Mods::default(),
            viewport: (400.0, 300.0),
            clicks: 1,
            time: None,
        }
    }

    /// Types `keys` into `field` with `mods` held, returning what the last one
    /// reported -- the whole of what a front does per keystroke.
    fn type_keys(field: &mut Text, mods: Mods, keys: &[Key]) -> Option<Events> {
        let mut clipboard = crate::host::clipboard::Clip::default();
        let mut last = None;
        for key in keys {
            let mut input = KeyInput {
                mods,
                clipboard: &mut clipboard,
                cursor: None,
            };
            last = field.key(key, &mut input);
        }
        last
    }

    #[test]
    fn props_parse_and_a_set_relands_the_caret() {
        let mut field = from_props(&props(r#"{"value":"hola","label":"name"}"#));
        assert_eq!(field.value, "hola");
        assert!(!field.multiline);
        field.caret.pos = 4;
        assert!(field.set("value", &Value::from("hi")));
        assert_eq!(field.caret.pos, 2, "the caret cannot sit past the string");
    }

    /// Every keystroke that changes the content delivers the new value -- the
    /// rule a numeric control follows on every drag step, never gated on Enter.
    #[test]
    fn typing_delivers_the_value_on_every_change() {
        let mut field = from_props(&props("{}"));
        let events = type_keys(&mut field, Mods::default(), &[Key::Char('a')]);
        assert_eq!(
            events,
            Some(Events::value(OscType::String("a".into()))),
            "a typed char reports the whole value"
        );
        assert_eq!(field.value, "a");

        // A motion is consumed (the caret moved, so the field repaints) and
        // reports nothing.
        let events = type_keys(&mut field, Mods::default(), &[Key::Left]);
        assert_eq!(events, Some(Events::none()));
    }

    /// Enter is a newline in a multiline field and nothing at all in a
    /// single-line one -- which is still *consumed*, so it cannot reach a view's
    /// shortcut behind the field.
    #[test]
    fn enter_is_a_newline_only_where_there_are_lines() {
        let mut single = from_props(&props(r#"{"value":"a"}"#));
        single.caret.pos = 1;
        assert_eq!(
            type_keys(&mut single, Mods::default(), &[Key::Enter]),
            Some(Events::none())
        );
        assert_eq!(single.value, "a");

        let mut multi = from_props(&props(r#"{"value":"a","multiline":true}"#));
        multi.caret.pos = 1;
        type_keys(&mut multi, Mods::default(), &[Key::Enter]);
        assert_eq!(multi.value, "a\n");
    }

    /// Cut, copy and paste travel through the host-wide clipboard the front
    /// owns, so a selection cut in one field pastes into another.
    #[test]
    fn cut_and_paste_go_through_the_shared_clipboard() {
        let ctrl = Mods {
            ctrl: true,
            ..Mods::default()
        };
        let mut field = from_props(&props(r#"{"value":"hola"}"#));
        let mut clipboard = crate::host::clipboard::Clip::default();

        // Select all, then cut.
        for key in [Key::Char('a'), Key::Char('x')] {
            let mut input = KeyInput {
                mods: ctrl,
                clipboard: &mut clipboard,
                cursor: None,
            };
            field.key(&key, &mut input);
        }
        assert_eq!(field.value, "");
        assert_eq!(clipboard.text(), "hola");

        // ...and it pastes back, into this field or any other.
        let mut other = from_props(&props("{}"));
        let mut input = KeyInput {
            mods: ctrl,
            clipboard: &mut clipboard,
            cursor: None,
        };
        let events = other.key(&Key::Char('v'), &mut input);
        assert_eq!(other.value, "hola");
        assert_eq!(events, Some(Events::value(OscType::String("hola".into()))));
    }

    /// Ctrl+Backspace deletes a whole word, and delivers the new value the way
    /// every other edit does.
    #[test]
    fn ctrl_backspace_deletes_a_word() {
        let ctrl = Mods {
            ctrl: true,
            ..Mods::default()
        };
        let mut field = from_props(&props(r#"{"value":"hola mundo"}"#));
        field.caret.pos = field.value.len();
        let events = type_keys(&mut field, ctrl, &[Key::Backspace]);
        assert_eq!(field.value, "hola ");
        assert_eq!(
            events,
            Some(Events::value(OscType::String("hola ".into()))),
            "a word delete is an edit like any other"
        );
        // Without the modifier it is still one character.
        type_keys(&mut field, Mods::default(), &[Key::Backspace]);
        assert_eq!(field.value, "hola");
    }

    /// Tab is the ring's: the field declines it, whatever it is in the middle
    /// of, so the focus can leave a field being typed into.
    #[test]
    fn tab_is_declined_so_the_focus_can_leave() {
        let mut field = from_props(&props(r#"{"value":"a"}"#));
        assert_eq!(type_keys(&mut field, Mods::default(), &[Key::Tab]), None);
        assert_eq!(field.value, "a");
    }

    /// A press drops the caret and a drag extends the selection from it -- the
    /// drag's anchor being the field's own state, which is what let the
    /// machine's `TextSelect` variant go.
    #[test]
    fn a_press_anchors_the_selection_a_drag_extends() {
        let metrics = Metrics::default();
        let mut field = from_props(&props(r#"{"value":"hello world"}"#));
        assert_eq!(field.press((0.0, 12.0), &input(&metrics)), Claim::take());
        assert_eq!(field.caret.pos, 0);
        assert_eq!(field.caret.selection(), None, "a press selects nothing");

        field.drag((200.0, 12.0), &input(&metrics));
        assert_eq!(
            field.caret.selection(),
            Some((0, "hello world".len())),
            "the drag ran past the end of the string"
        );

        // Back to where it started: an empty selection is no selection.
        field.drag((0.0, 12.0), &input(&metrics));
        assert_eq!(field.caret.selection(), None);
    }

    /// **A double click selects the word under it, a triple the line**, and
    /// the field's own selection is what Ctrl+C then copies.
    #[test]
    fn a_double_click_selects_a_word_and_a_triple_the_line() {
        let metrics = Metrics::default();
        let mut field = from_props(&props(r#"{"value":"hello world"}"#));
        let mut twice = input(&metrics);
        twice.clicks = 2;
        // Somewhere over the second word.
        field.press((150.0, 12.0), &twice);
        assert_eq!(field.caret.selection(), Some((6, 11)), "world");
        let mut thrice = input(&metrics);
        thrice.clicks = 3;
        field.press((150.0, 12.0), &thrice);
        assert_eq!(field.caret.selection(), Some((0, 11)), "the whole line");
    }

    /// **Shift+click extends** the selection from where it was made.
    #[test]
    fn a_shift_click_extends_from_the_caret() {
        let metrics = Metrics::default();
        let mut field = from_props(&props(r#"{"value":"hello world"}"#));
        field.press((0.0, 12.0), &input(&metrics));
        let mut shift = input(&metrics);
        shift.mods.shift = true;
        field.press((200.0, 12.0), &shift);
        assert_eq!(field.caret.selection(), Some((0, 11)));
        assert_eq!(field.caret.pos, 11, "the caret is where the click landed");
    }
}
