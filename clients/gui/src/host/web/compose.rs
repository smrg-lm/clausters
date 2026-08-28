//! **Composed text in a tab**: the hidden field that owns the keyboard while a
//! widget is being typed into.
//!
//! A `<canvas>` is not editable, so no browser will run an input method over
//! one. What reaches it from a dead key is `key: "Dead"` and then the *base*
//! letter — the composed character is never produced at all, and neither is a
//! `beforeinput` or a composition event. The same hole swallows every IME:
//! Japanese, Chinese, Korean, and any Latin keyboard whose accents are dead
//! keys.
//!
//! The answer is the one thing browsers do offer: an **editable element**. Each
//! canvas gets an invisible `<input>` beside it, and the keyboard is aimed at
//! whichever of the two is right —
//!
//! - **the canvas**, whenever the focus is not in a text widget, so every
//!   gesture, shortcut and ring walk keeps arriving exactly as it did;
//! - **the field**, the moment a widget that [takes
//!   text](crate::host::widget::element::Element::takes_text) is focused, so
//!   the browser composes into it and hands over finished characters.
//!
//! While the field holds the keyboard it is the shell's only key source, so it
//! forwards each thing exactly once, over three listeners:
//!
//! - **`beforeinput`** — a letter typed outright, or a pasted run.
//! - **`compositionend`** — what an input method *finished*, and only that.
//!   A composition is a negotiation: a dead key emits the bare accent first
//!   and settles on the accented letter afterwards, an IME shows a whole
//!   phrase in progress. The host has no notion of text that is still being
//!   decided — a widget stores what it is given — so every intermediate step
//!   is dropped and the result is delivered whole. Forwarding the steps is
//!   what put a stray `´` in front of the `é`.
//! - **`keydown`** — what is not text at all (the arrows, Backspace, Tab,
//!   Escape) plus the chords, since a `Ctrl+C` is a command and never a
//!   character. A keydown that *is* a character is dropped here and left to
//!   the two above, which is what keeps a letter from arriving twice.
//!
//! The field is never read and never holds anything: the host owns the text,
//! and every event is cancelled so the element stays empty. It is one input per
//! canvas rather than one per page because focus, and the def a key belongs to,
//! are per canvas.
//!
//! **It is a workaround and it is kept where one belongs.** Nothing about a
//! host, a widget or a protocol wants an invisible `<input>` in the document;
//! it is there because the platform offers no other way to reach an input
//! method, and the whole of it — the element, its two listeners, the focus
//! aiming and the key vocabulary it maps back — is this file. The native front
//! needs none of it (winit composes through xkb before the host sees a key),
//! and this module is compiled only for wasm, so nothing outside a browser
//! build reads a line of it.

use super::*;

/// The hidden editable element for one canvas, and the listeners that read it.
///
/// Dropping it removes the element from the document and its closures with it.
pub(super) struct Composer {
    input: web_sys::HtmlElement,
    _typed: Closure<dyn FnMut(web_sys::InputEvent)>,
    _composed: Closure<dyn FnMut(web_sys::CompositionEvent)>,
    _keys: Closure<dyn FnMut(web_sys::KeyboardEvent)>,
}

impl Drop for Composer {
    fn drop(&mut self) {
        self.input.remove();
    }
}

/// Off-screen rather than `display:none`: a hidden element cannot take focus,
/// and an element that cannot take focus composes nothing. One pixel, fully
/// transparent, out of the way of the pointer, and `position: fixed` so
/// focusing it never scrolls the page to it.
const HIDDEN: &str = "position:fixed;top:0;left:0;width:1px;height:1px;\
                      opacity:0;padding:0;border:0;outline:none;\
                      pointer-events:none;z-index:-1";

impl Composer {
    /// Builds the field for `def_id` and wires its two listeners, or `None` if
    /// the document will not have it (which leaves the canvas keyboard exactly
    /// as it was — the field only ever *adds* composition).
    pub(super) fn attach(host: HostId, def_id: i32) -> Option<Composer> {
        let document = web_sys::window()?.document()?;
        let input = document.create_element("input").ok()?;
        input.set_attribute("type", "text").ok()?;
        // Everything a browser would helpfully do to a text field is wrong
        // here: the host owns the value, and this one is never read.
        for (name, value) in [
            ("autocomplete", "off"),
            ("autocorrect", "off"),
            ("autocapitalize", "off"),
            ("spellcheck", "false"),
            ("aria-hidden", "true"),
            ("tabindex", "-1"),
            ("style", HIDDEN),
        ] {
            input.set_attribute(name, value).ok()?;
        }
        let input: web_sys::HtmlElement = input.dyn_into().ok()?;
        document.body()?.append_child(&input).ok()?;

        let typed =
            Closure::<dyn FnMut(web_sys::InputEvent)>::new(move |event: web_sys::InputEvent| {
                // **Never mid-composition.** An input method reports its work
                // in progress through this same event, and what it has so far
                // is not text anyone typed: the accent alone, a half-spelled
                // phrase. `compositionend` is where its result comes from, so
                // everything under composition is left to it.
                if event.is_composing() {
                    return;
                }
                // A letter or a paste. Everything else (a deletion, a format)
                // is a command and comes off `keydown`.
                let kind = event.input_type();
                if !matches!(kind.as_str(), "insertText" | "insertFromPaste") {
                    return;
                }
                // Cancelled either way: the element must stay empty, since a
                // field that accumulated would start reporting its own history.
                event.prevent_default();
                let Some(text) = event.data().filter(|t| !t.is_empty()) else {
                    return;
                };
                send(host, WebEvent::Typed { def_id, text });
            });
        // The other half of the text road: what an input method settled on.
        //
        // **The element is emptied when a composition starts, not when it
        // ends.** A composition is the one thing here that cannot be
        // cancelled: the browser uses the element's own value as its pending
        // buffer, and it writes into it around `compositionend` rather than
        // before it — so clearing there races the write and sometimes loses.
        // Clearing at the start cannot: whatever the last one left is gone
        // before this one puts anything in. Without it the buffer accumulates,
        // and Chrome re-reports the whole of it when a sequence it cannot
        // compose (three dead keys in a row) makes it give up and start over,
        // which is how a stray accent turned into two.
        let composed = Closure::<dyn FnMut(web_sys::CompositionEvent)>::new(
            move |event: web_sys::CompositionEvent| {
                let field = event
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                if event.type_() == "compositionstart" {
                    if let Some(field) = field {
                        field.set_value("");
                    }
                    return;
                }
                // A composition that ends with nothing is one the browser
                // abandoned; it has nothing to hand a widget, and what it left
                // in the buffer goes with the next `compositionstart`.
                if let Some(text) = event.data().filter(|t| !t.is_empty()) {
                    send(host, WebEvent::Typed { def_id, text });
                }
            },
        );
        let keys = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
            move |event: web_sys::KeyboardEvent| {
                let key = event.key();
                // Mid-composition, and the dead key that opens one: the
                // character is still being decided and `beforeinput` will say
                // what it became.
                if event.is_composing() || key == "Dead" || key == "Process" {
                    return;
                }
                let chord = event.ctrl_key() || event.alt_key() || event.meta_key();
                // A character with no chord over it is text, and text has one
                // road. Dropping it here is what keeps `a` from arriving as
                // two `a`s.
                if !chord && key.chars().count() == 1 {
                    return;
                }
                // Tab must not walk the document, Backspace must not go back a
                // page, Ctrl+S must not save it: while this field holds the
                // keyboard it is a widget's, not the browser's.
                event.prevent_default();
                let mods = u8::from(event.shift_key())
                    | u8::from(event.ctrl_key()) << 1
                    | u8::from(event.alt_key()) << 2;
                send(host, WebEvent::ComposedKey { def_id, key, mods });
            },
        );
        let target: &web_sys::EventTarget = input.as_ref();
        target
            .add_event_listener_with_callback("beforeinput", typed.as_ref().unchecked_ref())
            .ok()?;
        target
            .add_event_listener_with_callback("compositionstart", composed.as_ref().unchecked_ref())
            .ok()?;
        target
            .add_event_listener_with_callback("compositionend", composed.as_ref().unchecked_ref())
            .ok()?;
        target
            .add_event_listener_with_callback("keydown", keys.as_ref().unchecked_ref())
            .ok()?;
        Some(Composer {
            input,
            _typed: typed,
            _composed: composed,
            _keys: keys,
        })
    }

    /// Points the keyboard at the field (`text`) or back at the canvas.
    ///
    /// Idempotent, because it is called after every event that could have moved
    /// the focus: re-focusing what is already focused is what makes the caller
    /// free of state.
    fn aim(&self, canvas: &web_sys::HtmlCanvasElement, text: bool) {
        let active = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.active_element());
        if text {
            if active.as_deref() != Some(self.input.as_ref()) {
                let _ = self.input.focus();
            }
        } else if active.as_deref() == Some(self.input.as_ref()) {
            let _ = canvas.focus();
        }
    }
}

/// Hands one event to the running loop, or drops it if the loop is gone (a
/// canvas detached between the browser's event and ours).
fn send(host: HostId, event: WebEvent) {
    if let Some(proxy) = web_proxy() {
        let _ = proxy.send_event(HostEvent::To(host, event));
    }
}

impl WebApp {
    /// Points the keyboard at whichever element is right for the focus this
    /// def now has — the one call the rest of the shell makes into this module,
    /// after anything that could have moved a focus.
    pub(super) fn aim_keyboard(&mut self, def_id: i32) {
        let text = self.host.focus_takes_text(def_id);
        let Some(slot) = self.canvases.get(&def_id) else {
            return;
        };
        let (Some(composer), Some(canvas)) = (slot.composer.as_ref(), slot.window.canvas()) else {
            return;
        };
        composer.aim(&canvas, text);
    }

    /// Text the field produced: each character takes the ordinary key road, so
    /// a composed `á` reaches the widget exactly as a typed `a` does.
    pub(super) fn on_typed(&mut self, def_id: i32, text: &str) {
        for ch in text.chars() {
            self.on_key(def_id, &Key::Character(ch.to_string().into()));
        }
        self.aim_keyboard(def_id);
    }

    /// A key the field saw that was not text.
    ///
    /// The modifiers ride along because the browser reports them on the event
    /// and winit's `ModifiersChanged` does not reach a canvas that no longer
    /// holds the DOM focus — the same reason [`CanvasSlot::mods`] is read off
    /// the pointer events.
    pub(super) fn on_composed_key(&mut self, def_id: i32, key: &str, mods: u8) {
        if let Some(slot) = self.canvases.get(&def_id) {
            slot.mods.set(mods);
        }
        if let Some(key) = to_winit_key(key) {
            self.on_key(def_id, &key);
        }
        self.aim_keyboard(def_id);
    }
}

/// One `KeyboardEvent.key` as the key winit would have reported, so both
/// shells hand the host the same vocabulary.
///
/// Only what a focused widget can answer: the editing keys, the ring's Tab, the
/// window's Escape, and the letter of a chord. A name this does not know is a
/// key nothing here reads (`F5`, `CapsLock`), and `None` drops it.
fn to_winit_key(key: &str) -> Option<Key> {
    use winit::keyboard::NamedKey::*;
    Some(match key {
        "Backspace" => Key::Named(Backspace),
        "Delete" => Key::Named(Delete),
        "ArrowLeft" => Key::Named(ArrowLeft),
        "ArrowRight" => Key::Named(ArrowRight),
        "ArrowUp" => Key::Named(ArrowUp),
        "ArrowDown" => Key::Named(ArrowDown),
        "Home" => Key::Named(Home),
        "End" => Key::Named(End),
        "Enter" => Key::Named(Enter),
        "Tab" => Key::Named(Tab),
        "Escape" => Key::Named(Escape),
        " " => Key::Named(Space),
        other if other.chars().count() == 1 => Key::Character(other.into()),
        _ => return None,
    })
}
