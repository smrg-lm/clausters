//! **The browser's clipboard, reached from a page**: the
//! [`SystemClipboard`] of the browser front, so text copied in a canvas
//! pastes in another tab or program, and text copied there pastes here.
//!
//! The browser gives the clipboard three ways, and each is used where it
//! is the one that works:
//!
//! - **A copy writes** `navigator.clipboard.writeText` in the gesture that
//!   made it, which a browser allows without asking.
//! - **Ctrl+V takes the text the browser's `paste` event carries**, which
//!   needs no permission: the key is let through to the browser, its
//!   `paste` lands where the keyboard is (a window's canvas, the dock's, or
//!   the field a text widget is typed through), and the host performs the
//!   paste with that text handed over first: one paste of the whole text,
//!   never its characters typed one by one.
//! - **A paste from a menu reads it asynchronously**
//!   (`navigator.clipboard.readText`, which a browser may ask the reader's
//!   permission for): the paste waits for the text
//!   ([`Clip::must_ask`](crate::host::clipboard::Clip::must_ask)) and is
//!   performed again once it is in -- with the host's own clipboard where the
//!   read was refused.
//!
//! A page has no primary selection at all, so the middle button stays the
//! host's own.

use std::sync::{Arc, Mutex};

use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::Closure;

use crate::host::clipboard::SystemClipboard;
use crate::host::dock::Which;

use super::files::{call, field};
use super::{HostEvent, HostId, WebApp, WebEvent, web_proxy};

/// The text the browser last handed over, shared by the clipboard and the
/// front that fills it.
pub(super) type Text = Arc<Mutex<Option<String>>>;

/// **Whose paste the browser's lands as**: a window's, or that of the pane
/// a dock's shell (its canvas) has the keyboard going to.
#[derive(Clone, Copy, Debug)]
pub(super) enum PasteTo {
    Window(i32),
    Shell(Which),
}

/// **The page's clipboard**, as the host's [`SystemClipboard`].
pub(super) struct PageClipboard(pub Text);

impl SystemClipboard for PageClipboard {
    fn read_text(&mut self) -> Option<String> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn write_text(&mut self, text: &str) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(text.to_string());
        let text = JsValue::from_str(text);
        wasm_bindgen_futures::spawn_local(async move {
            let clipboard = field(&field(&js_sys::global(), "navigator"), "clipboard");
            if clipboard.is_undefined() {
                return;
            }
            if let Err(e) = call(&clipboard, "writeText", &[text]).await {
                super::log(&format!("the browser's clipboard was not written: {e}"));
            }
        });
    }

    fn reads_later(&mut self) -> bool {
        true
    }
}

/// **The listeners that let Ctrl+V through to the browser** on one element
/// the keyboard goes to (a canvas, or a text widget's field): the key is
/// kept from the shell, which would cancel it, so the browser fires its
/// `paste`; and the `paste` hands its text to the host.
pub(super) struct Paster {
    canvas: web_sys::EventTarget,
    _keys: Closure<dyn FnMut(web_sys::KeyboardEvent)>,
    _paste: Closure<dyn FnMut(web_sys::Event)>,
}

impl Drop for Paster {
    fn drop(&mut self) {
        let _ = self.canvas.remove_event_listener_with_callback_and_bool(
            "keydown",
            self._keys.as_ref().unchecked_ref(),
            true,
        );
        let _ = self
            .canvas
            .remove_event_listener_with_callback("paste", self._paste.as_ref().unchecked_ref());
    }
}

/// How long after the paste key a paste the browser's event did not make
/// waits, in milliseconds: the event comes within the same turn of the page.
const PASTE_WAIT_MS: i32 = 150;

/// Whether `event` is the paste key: Ctrl+V, or Command+V on a Mac.
pub(super) fn is_paste_key(event: &web_sys::KeyboardEvent) -> bool {
    (event.ctrl_key() || event.meta_key())
        && !event.alt_key()
        && !event.shift_key()
        && event.key().eq_ignore_ascii_case("v")
}

impl Paster {
    /// The listeners on `canvas`, for the paste of `to` on host `host`.
    pub(super) fn attach(host: HostId, to: PasteTo, canvas: web_sys::EventTarget) -> Option<Self> {
        // the key, before the shell's own listener: kept from it, so nothing
        // cancels the browser's paste
        let keys = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
            move |event: web_sys::KeyboardEvent| {
                if !is_paste_key(&event) {
                    return;
                }
                event.stop_immediate_propagation();
                // a browser that fires no `paste` here still pastes: the
                // paste goes on by a read once the browser's turn is over
                if let Some(proxy) = web_proxy() {
                    let _ = proxy.send_event(HostEvent::To(host, WebEvent::PasteKey { to }));
                }
            },
        );
        let paste = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
            event.prevent_default();
            let data = field(&event, "clipboardData");
            let text = js_sys::Reflect::get(&data, &JsValue::from_str("getData"))
                .ok()
                .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
                .and_then(|f| f.call1(&data, &JsValue::from_str("text/plain")).ok())
                .and_then(|t| t.as_string());
            if let Some(proxy) = web_proxy() {
                let _ = proxy.send_event(HostEvent::To(host, WebEvent::Pasted { to, text }));
            }
        });
        canvas
            .add_event_listener_with_callback_and_bool(
                "keydown",
                keys.as_ref().unchecked_ref(),
                true,
            )
            .ok()?;
        canvas
            .add_event_listener_with_callback("paste", paste.as_ref().unchecked_ref())
            .ok()?;
        Some(Paster {
            canvas,
            _keys: keys,
            _paste: paste,
        })
    }
}

impl WebApp {
    /// **The window a paste of `to` is for**: the window, or the pane the
    /// shell's keyboard goes to.
    pub(super) fn paste_window(&self, to: PasteTo) -> Option<i32> {
        match to {
            PasteTo::Window(def) => Some(def),
            PasteTo::Shell(which) => self.shell_focus(which),
        }
    }

    /// **The browser handed text over for window `def`'s paste**: the paste
    /// event's, or the menu's read (`None` where it was refused, when the
    /// host's own clipboard is pasted). The paste is performed with it.
    pub(super) fn on_pasted(&mut self, def: i32, verb: &str, text: Option<String>) {
        self.paste_keys.remove(&def);
        if let Some(text) = text {
            *self
                .clipboard_text
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(text);
        }
        self.host.clipboard.mark_fresh();
        self.perform_verb(def, verb);
    }

    /// **The paste key on window `def`**, let through to the browser: its
    /// `paste` normally follows within the same turn of the page; where none
    /// does, the paste is performed a moment later through a read.
    pub(super) fn on_paste_key(&mut self, def: i32) {
        self.paste_keys.insert(def);
        let host = self.id;
        let later = Closure::once_into_js(move || {
            if let Some(proxy) = web_proxy() {
                let _ =
                    proxy.send_event(HostEvent::To(host, WebEvent::PasteKeyLate { def_id: def }));
            }
        });
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                later.unchecked_ref(),
                PASTE_WAIT_MS,
            );
        }
    }

    /// The moment after the paste key: a paste the browser's event did not
    /// make is made now.
    pub(super) fn on_paste_key_late(&mut self, def: i32) {
        if self.paste_keys.remove(&def) {
            self.perform_verb(def, "paste");
        }
    }

    /// **A paste waits for the browser's text**: it is read, and the paste
    /// performed again once it is in.
    pub(super) fn read_clipboard_if_asked(&mut self) {
        let Some((def_id, verb)) = self.host.clipboard.take_ask() else {
            return;
        };
        let host = self.id;
        wasm_bindgen_futures::spawn_local(async move {
            let clipboard = field(&field(&js_sys::global(), "navigator"), "clipboard");
            let text = if clipboard.is_undefined() {
                None
            } else {
                call(&clipboard, "readText", &[])
                    .await
                    .ok()
                    .and_then(|t| t.as_string())
            };
            if let Some(proxy) = web_proxy() {
                let _ = proxy.send_event(HostEvent::To(
                    host,
                    WebEvent::ClipboardRead { def_id, verb, text },
                ));
            }
        });
    }
}
