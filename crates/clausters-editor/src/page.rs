//! **The page's platform**: the editor in a browser tab, beside the page's
//! host. The session is JavaScript in a worker on the page's engine, the
//! files are the page's storage, the documentation is served beside the
//! page, and the look follows the browser's. What takes a page's script --
//! the workers, the storage, the served books -- the page's script answers
//! ([`PagePlatform`]); what a binding reaches -- the local storage, the
//! timers, the color scheme, a new tab -- is asked here.
//!
//! The application runs on the page's turn: every answer, every packet of
//! the host's and every message of the session is queued, and the queue is
//! emptied into the application at once, unless it is being emptied
//! already (an answer that arrives while the application is working waits
//! for it to finish).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::mpsc::{Receiver, Sender, channel};

use clausters_core::osc::{OscPacket, decode_packet, encode};
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{JsFuture, spawn_local};

use crate::app::{App, Msg};
use crate::docs::Folder;
use crate::platform::{self, Language, Offers, Platform, Read};
use crate::settings::Settings;
use crate::wire::{Link, Said};

#[wasm_bindgen]
extern "C" {
    /// **What the page's script answers the editor with**: the packets to
    /// the host, the session's workers, the page's storage and the books it
    /// serves. A promise that fails says why in its error.
    pub type PagePlatform;

    /// One OSC packet to the host; `false` once it has gone.
    #[wasm_bindgen(method)]
    fn send(this: &PagePlatform, packet: &[u8]) -> bool;
    /// Starts the session if it is not running.
    #[wasm_bindgen(method, js_name = startSession)]
    fn start_session(this: &PagePlatform) -> js_sys::Promise;
    /// One request to run, as the driver's JSON.
    #[wasm_bindgen(method)]
    fn evaluate(this: &PagePlatform, request: &str) -> js_sys::Promise;
    /// One question about code, as the driver's JSON.
    #[wasm_bindgen(method)]
    fn ask(this: &PagePlatform, request: &str);
    #[wasm_bindgen(method)]
    fn interrupt(this: &PagePlatform) -> js_sys::Promise;
    #[wasm_bindgen(method)]
    fn restart(this: &PagePlatform) -> js_sys::Promise;
    #[wasm_bindgen(method)]
    fn stop(this: &PagePlatform);
    /// A file's text.
    #[wasm_bindgen(method, js_name = readText)]
    fn read_text(this: &PagePlatform, path: &str) -> js_sys::Promise;
    #[wasm_bindgen(method)]
    fn write(this: &PagePlatform, path: &str, text: &str) -> js_sys::Promise;
    /// Every book's folder, read, as the JSON of
    /// `[{prefix, name, summary, documents: [[path, text]], at}]`.
    #[wasm_bindgen(method, js_name = readDocs)]
    fn read_docs(this: &PagePlatform) -> js_sys::Promise;
}

/// The application and its queue.
struct Inner {
    app: RefCell<Option<App>>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

/// **Empties the queue into the application**, unless it is being emptied.
fn pump(inner: &Inner) {
    let Ok(mut app) = inner.app.try_borrow_mut() else {
        return;
    };
    let Some(app) = app.as_mut() else {
        return;
    };
    while let Ok(msg) = inner.rx.try_recv() {
        app.on(msg);
    }
}

/// **The editor, as the page's script holds it.** See the module.
#[wasm_bindgen]
pub struct PageEditor {
    inner: Rc<Inner>,
}

#[wasm_bindgen]
impl PageEditor {
    /// The editor over what the page's script answers with.
    #[wasm_bindgen(constructor)]
    pub fn new(platform: PagePlatform) -> PageEditor {
        let (tx, rx) = channel();
        let inner = Rc::new(Inner {
            app: RefCell::new(None),
            tx,
            rx,
        });
        let js = Rc::new(platform);
        let carrier = js.clone();
        let link = Link::new(
            Box::new(move |msg| match encode(&OscPacket::Message(msg)) {
                Ok(bytes) => carrier.send(&bytes),
                Err(_) => true,
            }),
            // a page's tab is the browser's to close
            Box::new(|| {}),
        );
        let page = Page {
            js,
            inner: Rc::downgrade(&inner),
        };
        let app = App::new(link, Box::new(page), Settings::load());
        *inner.app.borrow_mut() = Some(app);
        PageEditor { inner }
    }

    /// The editor starts: its panels, the files the last visit left open,
    /// the session.
    pub fn start(&self) {
        if let Ok(mut app) = self.inner.app.try_borrow_mut()
            && let Some(app) = app.as_mut()
        {
            app.start(Vec::new());
        }
        pump(&self.inner);
    }

    /// One packet the host sent the editor.
    pub fn host(&self, packet: &[u8]) {
        if let Ok(packet) = decode_packet(packet) {
            for said in Said::read(packet) {
                let _ = self.inner.tx.send(Msg::Host(said));
            }
        }
        pump(&self.inner);
    }

    /// One message of the session's workers, as the driver's JSON.
    pub fn session(&self, message: &str) {
        let msg = serde_json::from_str(message)
            .unwrap_or_else(|_| json!({"type": "sys", "text": format!("{message}\n")}));
        let _ = self.inner.tx.send(Msg::Py(msg));
        pump(&self.inner);
    }

    /// The session's worker started.
    #[wasm_bindgen(js_name = sessionStarted)]
    pub fn session_started(&self, generation: f64) {
        let generation = generation as u64;
        let _ = self.inner.tx.send(Msg::PyStart {
            generation,
            env: None,
        });
        pump(&self.inner);
    }

    /// The session's worker ended, with its code if it gave one.
    #[wasm_bindgen(js_name = sessionEnded)]
    pub fn session_ended(&self, generation: f64, code: Option<i32>) {
        let generation = generation as u64;
        let _ = self.inner.tx.send(Msg::PyExit { generation, code });
        pump(&self.inner);
    }

    /// Whether a file has changes that are not saved: what a tab asks
    /// before it is closed.
    pub fn unsaved(&self) -> bool {
        self.inner
            .app
            .try_borrow()
            .ok()
            .and_then(|app| app.as_ref().map(App::unsaved))
            .unwrap_or(false)
    }
}

/// The page's answers. See the module.
struct Page {
    js: Rc<PagePlatform>,
    inner: Weak<Inner>,
}

impl Page {
    /// `msg` on the application's turn.
    fn deliver(inner: &Weak<Inner>, msg: Msg) {
        if let Some(inner) = inner.upgrade() {
            let _ = inner.tx.send(msg);
            pump(&inner);
        }
    }

    /// `promise` awaited off the application's turn, and what it came to
    /// told to it as `then` makes it, if `then` makes anything.
    fn answer_maybe(
        &self,
        promise: js_sys::Promise,
        then: impl FnOnce(Result<JsValue, String>) -> Option<Msg> + 'static,
    ) {
        let inner = self.inner.clone();
        spawn_local(async move {
            let result = JsFuture::from(promise).await.map_err(error_text);
            if let Some(msg) = then(result) {
                Page::deliver(&inner, msg);
            }
        });
    }

    /// `promise` awaited off the application's turn, and what it came to
    /// told to it as `then` makes it.
    fn answer(
        &self,
        promise: js_sys::Promise,
        then: impl FnOnce(Result<JsValue, String>) -> Msg + 'static,
    ) {
        self.answer_maybe(promise, |result| Some(then(result)));
    }

    /// A session's promise that failed says why in the post window.
    fn session(&self, promise: js_sys::Promise) {
        self.answer_maybe(promise, |result| {
            let e = result.err()?;
            Some(Msg::Py(json!({"type": "err", "text": format!("{e}\n")})))
        });
    }
}

/// What a failed promise says.
fn error_text(error: JsValue) -> String {
    if let Some(e) = error.dyn_ref::<js_sys::Error>() {
        return String::from(e.message());
    }
    error.as_string().unwrap_or_else(|| format!("{error:?}"))
}

/// A path as the page's script names it: absolute, `/` separated.
fn named(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

impl Platform for Page {
    fn language(&self) -> &'static Language {
        &platform::JAVASCRIPT
    }

    fn offers(&self) -> Offers {
        Offers {
            envs: false,
            docs_folder: false,
            fonts: false,
            quit: false,
        }
    }

    fn start_session(&self) -> Result<(), String> {
        self.session(self.js.start_session());
        Ok(())
    }

    fn send(&self, request: &Value) -> Result<(), String> {
        self.session(self.js.evaluate(&request.to_string()));
        Ok(())
    }

    fn ask(&self, request: &Value) -> Result<(), String> {
        self.js.ask(&request.to_string());
        Ok(())
    }

    /// The TypeScript language service is a worker of the page's, up before
    /// the session (which waits for the page's first gesture) and through
    /// its restarts.
    fn answers_without_session(&self) -> bool {
        true
    }

    fn interrupt(&self) -> Result<(), String> {
        self.session(self.js.interrupt());
        Ok(())
    }

    fn restart(&self) -> Result<(), String> {
        self.session(self.js.restart());
        Ok(())
    }

    fn stop(&self) {
        self.js.stop();
    }

    fn read(&self, path: PathBuf, why: Read) {
        let promise = self.js.read_text(&named(&path));
        self.answer(promise, move |text| Msg::Read {
            text: text.map(|t| t.as_string().unwrap_or_default()),
            path,
            why,
        });
    }

    fn write(&self, path: PathBuf, text: String, code: i32) {
        let promise = self.js.write(&named(&path), &text);
        self.answer(promise, move |done| Msg::Written {
            done: done.map(|_| ()),
            path,
            code,
        });
    }

    /// A page is given no paths but its own files', which are read to be
    /// known: nothing is known to be there before.
    fn exists(&self, _path: &Path) -> bool {
        false
    }

    fn is_dir(&self, _path: &Path) -> bool {
        false
    }

    fn home(&self) -> PathBuf {
        PathBuf::new()
    }

    /// The browser's `prefers-color-scheme`, now and at every change.
    fn follow_color_scheme(&self) {
        let Some(query) = web_sys::window()
            .and_then(|w| w.match_media("(prefers-color-scheme: dark)").ok())
            .flatten()
        else {
            return;
        };
        Page::deliver(&self.inner, Msg::SystemDark(query.matches()));
        let inner = self.inner.clone();
        let changed = Closure::<dyn Fn(web_sys::MediaQueryListEvent)>::new(
            move |event: web_sys::MediaQueryListEvent| {
                Page::deliver(&inner, Msg::SystemDark(event.matches()));
            },
        );
        let _ = query.add_event_listener_with_callback("change", changed.as_ref().unchecked_ref());
        // heard for as long as the page is
        changed.forget();
    }

    fn read_docs(&self) {
        let promise = self.js.read_docs();
        self.answer(promise, |read| {
            Msg::Docs(read.and_then(|json| {
                let folders = folders(&json.as_string().unwrap_or_default())?;
                Ok(crate::docs::read(folders))
            }))
        });
    }

    /// A page has no system's faces to offer.
    fn find_fonts(&self) {
        Page::deliver(&self.inner, Msg::Fonts(Vec::new()));
    }

    fn font(&self, _path: &Path) -> Vec<u8> {
        Vec::new()
    }

    /// In a new tab.
    fn open_url(&self, url: &str) {
        if let Some(window) = web_sys::window() {
            let _ = window.open_with_url_and_target_and_features(url, "_blank", "noopener");
        }
    }

    fn later(&self, ms: u32, msg: Msg) {
        let inner = self.inner.clone();
        let fire = Closure::once_into_js(move || Page::deliver(&inner, msg));
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                fire.unchecked_ref(),
                i32::try_from(ms).unwrap_or(i32::MAX),
            );
        }
    }
}

/// The books' folders, as the page's script writes them.
fn folders(json: &str) -> Result<Vec<Folder>, String> {
    let read: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let text = |v: &Value| v.as_str().unwrap_or("").to_string();
    Ok(read
        .as_array()
        .into_iter()
        .flatten()
        .map(|f| Folder {
            prefix: text(&f["prefix"]),
            name: text(&f["name"]),
            summary: f["summary"].as_str().map(str::to_string),
            documents: f["documents"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|d| Some((d[0].as_str()?.to_string(), d[1].as_str()?.to_string())))
                .collect(),
            at: text(&f["at"]),
        })
        .collect())
}
