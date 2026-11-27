//! **The page's answerer for lists of segments over files**: the native
//! front's, driven over the page's own storage (OPFS) instead of a thread.
//!
//! OPFS answers with promises, so an ask is driven off the event loop: the
//! answerer says what it needs, each need is read (`getFile`, then a slice of
//! it), handed over, and the ask is answered again; the answer comes back
//! through the proxy as a server's reply does and is read by the same code.
//! Two asks may be under way at once, and neither holds the answerer across a
//! read.
//!
//! **A round's reads go out together, and the near ones as one.** One answer
//! can need a hundred blocks of one peaks file, and a promise awaited after
//! another made a round of them last over a second, where natively each is a
//! read on a thread. Each file is looked up once a round, blocks that lie
//! close together in it are read as one slice, and the slices are asked at
//! once. An ask the same as one under way is not asked again, since the one
//! under way answers it.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use clausters_core::osc::{OscMessage, OscPacket, encode};
use wasm_bindgen::{JsCast, JsValue};

use super::files::{call, field, parts};
use super::{HostEvent, HostId, WebEvent, web_proxy};
use crate::host::disk::answer::{Answer, Answerer, Got, Need, Stat};
use crate::host::disk::{DiskLink, List};

/// Rounds of reading one ask may take, as natively.
const ROUNDS: usize = 6;

thread_local! {
    /// The file handles already found, by path: walking the directories to
    /// a file is a promise per level, and a list asks about the same few
    /// files over and over.
    static HANDLES: RefCell<HashMap<String, JsValue>> = RefCell::new(HashMap::new());
}

/// The page's [`DiskLink`].
pub(super) struct WebDisk {
    host: HostId,
    answerer: Rc<RefCell<Answerer>>,
    /// The asks under way, as their bytes: a frame clock that asks again
    /// for a summary still being read would start the same reads twice.
    under_way: Rc<RefCell<HashSet<Vec<u8>>>>,
}

impl WebDisk {
    pub(super) fn new(host: HostId) -> Self {
        Self {
            host,
            answerer: Rc::new(RefCell::new(Answerer::default())),
            under_way: Rc::default(),
        }
    }
}

impl DiskLink for WebDisk {
    fn define(&self, key: i32, list: Arc<List>) {
        self.answerer.borrow_mut().define(key, list);
    }

    fn forget(&self, key: i32) {
        self.answerer.borrow_mut().forget(key);
    }

    fn ask(&self, msg: OscMessage) {
        let Ok(key) = encode(&OscPacket::Message(msg.clone())) else {
            return;
        };
        if !self.under_way.borrow_mut().insert(key.clone()) {
            return;
        }
        let (host, answerer) = (self.host, Rc::clone(&self.answerer));
        let under_way = Rc::clone(&self.under_way);
        wasm_bindgen_futures::spawn_local(async move {
            drive(host, answerer, msg).await;
            under_way.borrow_mut().remove(&key);
        });
    }
}

/// One ask, read through to its answer and handed back.
async fn drive(host: HostId, answerer: Rc<RefCell<Answerer>>, msg: OscMessage) {
    // **A spectrum's answers give the page its turn.** Each analyzes a
    // bounded number of windows, but a reply's event asks for the next at
    // once, and a chain of answers run as promises would keep the page from
    // drawing until a view of minutes was analyzed whole; a timer between
    // them lets a frame in.
    if msg.addr == "/buffer_spectrum" {
        let _ = super::next_turn().await;
    }
    for _ in 0..ROUNDS {
        let answer = answerer.borrow_mut().answer(&msg);
        match answer {
            Answer::Reply(reply) => {
                if let (Some(proxy), Ok(bytes)) = (web_proxy(), encode(&OscPacket::Message(reply)))
                {
                    let _ = proxy.send_event(HostEvent::To(host, WebEvent::DiskReply(bytes)));
                }
                return;
            }
            Answer::None => return,
            Answer::Need(needs) => {
                let got = read_all(&needs).await;
                let mut answerer = answerer.borrow_mut();
                for (need, got) in needs.iter().zip(got) {
                    answerer.deliver(need, got);
                }
            }
        }
    }
    crate::host::diag::warn!(
        "{}: still reading after {ROUNDS} rounds; given up",
        msg.addr
    );
}

/// The `File` at `path` in the page's storage, as it is now.
async fn file(path: &str) -> Result<JsValue, String> {
    let handle = match HANDLES.with(|h| h.borrow().get(path).cloned()) {
        Some(handle) => handle,
        None => {
            let mut segments = parts(path)?;
            let name = segments
                .pop()
                .ok_or_else(|| "an empty path names no file".to_string())?;
            let storage = field(&field(&js_sys::global(), "navigator"), "storage");
            if storage.is_undefined() {
                return Err("this page has no storage to read".into());
            }
            let mut dir = call(&storage, "getDirectory", &[]).await?;
            for segment in &segments {
                dir = call(&dir, "getDirectoryHandle", &[JsValue::from_str(segment)])
                    .await
                    .map_err(|_| format!("{path}: no such directory"))?;
            }
            let handle = call(&dir, "getFileHandle", &[JsValue::from_str(&name)])
                .await
                .map_err(|_| format!("{path}: no such file"))?;
            HANDLES.with(|h| h.borrow_mut().insert(path.to_string(), handle.clone()));
            handle
        }
    };
    call(&handle, "getFile", &[]).await.inspect_err(|_| {
        // A file that went is looked up again next time.
        HANDLES.with(|h| h.borrow_mut().remove(path));
    })
}

/// What one need's read brought, or why it brought nothing.
type Read = Result<Got, String>;

/// **Reads every need of a round at once**: each file looked up once, the
/// reads that lie close together in it joined into one slice
/// ([`joined`](crate::host::disk::joined)),
/// every slice asked together, and the answers in the order of `needs`.
async fn read_all(needs: &[Need]) -> Vec<Read> {
    let mut files: HashMap<&str, Result<JsValue, String>> = HashMap::new();
    for need in needs {
        if !files.contains_key(need.path()) {
            files.insert(need.path(), file(need.path()).await);
        }
    }
    let got: Rc<RefCell<Vec<Option<Read>>>> =
        Rc::new(RefCell::new(needs.iter().map(|_| None).collect()));
    let asked = js_sys::Array::new();
    for (path, (from, to), members) in crate::host::disk::joined(needs) {
        let file = files[path].clone();
        let got = Rc::clone(&got);
        let lens: Vec<(usize, u64, usize)> = members
            .iter()
            .map(|&at| match &needs[at] {
                Need::Read { offset, len, .. } => (at, *offset - from, *len),
                Need::Stat { .. } => unreachable!("a stat is never joined"),
            })
            .collect();
        let path = path.to_string();
        asked.push(&wasm_bindgen_futures::future_to_promise(async move {
            let bytes = match file {
                Ok(file) => slice(&file, &path, from, to).await,
                Err(e) => Err(e),
            };
            let mut got = got.borrow_mut();
            for (at, offset, len) in lens {
                got[at] = Some(match &bytes {
                    Ok(bytes) => {
                        let start = offset as usize;
                        bytes
                            .get(start..start + len)
                            .map(|b| Got::Bytes(b.to_vec()))
                            .ok_or_else(|| {
                                format!(
                                    "{path}: {} bytes where {len} were asked",
                                    bytes.len().saturating_sub(start)
                                )
                            })
                    }
                    Err(e) => Err(e.clone()),
                });
            }
            Ok(JsValue::UNDEFINED)
        }));
    }
    for (at, need) in needs.iter().enumerate() {
        if let Need::Stat { path } = need {
            let stat = match &files[&**path] {
                Ok(file) => Ok(Got::Stat(Stat {
                    len: field(file, "size").as_f64().unwrap_or(0.0) as u64,
                    // Milliseconds since the epoch, as the native clock's
                    // seconds.
                    modified: field(file, "lastModified").as_f64().map(|ms| ms / 1000.0),
                })),
                Err(e) => Err(e.clone()),
            };
            got.borrow_mut()[at] = Some(stat);
        }
    }
    let _ = wasm_bindgen_futures::JsFuture::from(js_sys::Promise::all(&asked)).await;
    got.take()
        .into_iter()
        .map(|g| g.unwrap_or_else(|| Err("a read that never finished".into())))
        .collect()
}

/// Bytes `from..to` of `file`, the `File` at `path`.
async fn slice(file: &JsValue, path: &str, from: u64, to: u64) -> Result<Vec<u8>, String> {
    let slice = call(
        file,
        "slice",
        &[JsValue::from_f64(from as f64), JsValue::from_f64(to as f64)],
    )
    .await?;
    let buffer = call(&slice, "arrayBuffer", &[]).await?;
    Ok(js_sys::Uint8Array::new(
        buffer
            .dyn_ref::<js_sys::ArrayBuffer>()
            .ok_or_else(|| format!("{path}: a read that is not bytes"))?,
    )
    .to_vec())
}
