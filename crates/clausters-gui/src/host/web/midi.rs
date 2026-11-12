//! The page's MIDI **device**: the browser's own inputs (Web MIDI).
//!
//! The desktop front opens a virtual port for other programs to route into; a
//! page has no port to offer, and what it has instead is every input the
//! browser can see. So while some canvas holds an element that reads live
//! MIDI the page listens to all of them, and what arrives goes through the
//! shared walk (`crate::host::midi`) -- the reader and the delivery the desktop
//! front feeds from its port, which is what makes a roll paint the same note
//! on both.
//!
//! **The one part with no desktop twin is the permission.** The browser asks
//! the person before it hands a page its MIDI devices, so the access is asked
//! for only once a def holds an element that listens -- a page that never
//! mounts one is never prompted -- and a refusal is said once and leaves the
//! roll exactly as editable by hand as it was.
//!
//! Bytes that come from somewhere other than a device have a door of their own
//! ([`GuiBridge::midi`]): the page's stand-in for routing into the desktop's
//! virtual port.

use wasm_bindgen_futures::JsFuture;
use web_sys::{MidiAccess, MidiInput, MidiMessageEvent};

use super::*;

/// Where this instance stands with the browser's MIDI inputs.
#[derive(Default)]
pub(super) enum MidiDevice {
    /// Nothing listens, so nothing was asked for.
    #[default]
    Closed,
    /// The access was asked for and the browser -- or the person -- has not
    /// answered.
    Asking,
    /// Every input is listened to. Held for what dropping it does, which is
    /// to stop listening -- nothing reads it in between.
    Open(#[allow(dead_code)] Inputs),
    /// The access was refused, or this browser has none to give. Kept apart
    /// from `Closed` so a tree change does not ask again while the same
    /// elements wait.
    Refused,
}

/// The inputs being listened to, and the two handlers that do it. Dropping
/// this stops listening: the handlers are taken off before the closures go.
pub(super) struct Inputs {
    access: MidiAccess,
    on_message: Closure<dyn FnMut(MidiMessageEvent)>,
    _on_state: Closure<dyn FnMut(JsValue)>,
}

/// Every input `access` has now.
fn inputs(access: &MidiAccess) -> impl Iterator<Item = MidiInput> {
    access
        .inputs()
        .values()
        .into_iter()
        .filter_map(Result::ok)
        .map(JsCast::unchecked_into::<MidiInput>)
}

impl Inputs {
    /// Listens to every input of `access` for instance `host`, the ones
    /// plugged in later included.
    fn listen(access: MidiAccess, host: HostId) -> Self {
        let on_message = Closure::<dyn FnMut(MidiMessageEvent)>::new(move |e: MidiMessageEvent| {
            let (Ok(bytes), Some(proxy)) = (e.data(), web_proxy()) else {
                return;
            };
            let _ = proxy.send_event(HostEvent::To(host, WebEvent::Midi(bytes)));
        });
        let handler: js_sys::Function = on_message
            .as_ref()
            .unchecked_ref::<js_sys::Function>()
            .clone();
        for input in inputs(&access) {
            input.set_onmidimessage(Some(&handler));
        }
        // A device plugged in after the access was given is a new input, and
        // setting the same handler again on the others changes nothing.
        let on_state = {
            let access = access.clone();
            Closure::<dyn FnMut(JsValue)>::new(move |_| {
                for input in inputs(&access) {
                    input.set_onmidimessage(Some(&handler));
                }
            })
        };
        access.set_onstatechange(Some(on_state.as_ref().unchecked_ref()));
        Self {
            access,
            on_message,
            _on_state: on_state,
        }
    }
}

impl Drop for Inputs {
    fn drop(&mut self) {
        self.access.set_onstatechange(None);
        let ours: &JsValue = self.on_message.as_ref();
        for input in inputs(&self.access) {
            // Only what this instance set: a second host on the page listens
            // to the same inputs through its own access object, but a page
            // may have put a handler of its own on one of them since.
            if input
                .onmidimessage()
                .is_some_and(|f| JsValue::from(f) == *ours)
            {
                input.set_onmidimessage(None);
            }
        }
    }
}

/// Asks the browser for its MIDI inputs and hands the answer back through the
/// proxy.
async fn ask(host: HostId) {
    let result = async {
        let navigator = web_sys::window().ok_or("no window")?.navigator();
        // A browser without Web MIDI has no such method, which arrives here as
        // the exception calling it raises.
        let promise = navigator
            .request_midi_access()
            .map_err(|_| "this browser has no Web MIDI".to_string())?;
        let access = JsFuture::from(promise)
            .await
            .map_err(|e| String::from(js_sys::Error::from(e).message()))?;
        Ok::<_, String>(access.unchecked_into::<MidiAccess>())
    }
    .await;
    if let Some(proxy) = web_proxy() {
        let _ = proxy.send_event(HostEvent::To(host, WebEvent::MidiAccess(result)));
    }
}

impl WebApp {
    /// Whether any canvas holds an element that reads live MIDI.
    fn midi_wanted(&self) -> bool {
        !self
            .host
            .midi_readers(self.canvases.keys().copied())
            .is_empty()
    }

    /// Brings the device in step with the trees: asked for when the first
    /// element that listens appears, let go when the last one is freed.
    pub(super) fn sync_midi(&mut self) {
        match (&self.midi_device, self.midi_wanted()) {
            (MidiDevice::Closed, true) => {
                self.midi_device = MidiDevice::Asking;
                wasm_bindgen_futures::spawn_local(ask(self.id));
            }
            (MidiDevice::Open(_) | MidiDevice::Refused, false) => {
                self.midi_device = MidiDevice::Closed;
            }
            _ => {}
        }
    }

    /// The browser answered. The elements that asked may be gone by now -- the
    /// person can take as long as they like over a permission -- and then the
    /// access is let go unused.
    pub(super) fn on_midi_access(&mut self, result: Result<MidiAccess, String>) {
        let wanted = self.midi_wanted();
        self.midi_device = match result {
            Ok(access) if wanted => {
                log("listening to the browser's MIDI inputs");
                MidiDevice::Open(Inputs::listen(access, self.id))
            }
            Ok(_) => MidiDevice::Closed,
            Err(why) => {
                if !self.midi_warned {
                    log(&format!("could not open the browser's MIDI inputs: {why}"));
                    self.midi_warned = true;
                }
                if wanted {
                    MidiDevice::Refused
                } else {
                    MidiDevice::Closed
                }
            }
        };
    }

    /// The bytes of one message, from an input or from the page: read into
    /// notes and delivered to every element that listens, reporting whatever
    /// comes back the way a gesture's edit is reported.
    pub(super) fn on_midi(&mut self, bytes: &[u8]) {
        self.midi.feed(bytes);
        let notes = self.midi.notes();
        let readers = self.host.midi_readers(self.canvases.keys().copied());
        let effects = self
            .host
            .deliver_midi(&readers, &notes, Some(self.buses.as_ref()));
        self.apply_gesture_effects(effects);
    }
}

#[wasm_bindgen]
impl GuiBridge {
    /// Feeds the bytes of one MIDI message to the host, as a device would
    /// deliver them: every element that reads live MIDI (a roll with
    /// `midi_in`) takes the note.
    ///
    /// The host listens to the browser's own inputs by itself, so a page
    /// calls this only for MIDI that comes from somewhere else -- another
    /// program's output arriving over a socket, a keyboard drawn on the page.
    /// It is the page's form of routing into the desktop host's virtual port.
    pub fn midi(&self, bytes: &[u8]) {
        self.send(WebEvent::Midi(bytes.to_vec()));
    }
}
