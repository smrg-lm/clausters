//! **The page's answerer for lists of segments over files**: the native
//! front's, driven over the page's own storage (OPFS) instead of a thread.
//!
//! OPFS answers with promises, so an ask is driven off the event loop: the
//! answerer says what it needs, each need is read (`getFile`, then a slice of
//! it), handed over, and the ask is answered again; the answer comes back
//! through the proxy as a server's reply does and is read by the same code.
//! Two asks may be under way at once, and neither holds the answerer across a
//! read.

use std::cell::RefCell;
use std::collections::HashMap;
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
}

impl WebDisk {
    pub(super) fn new(host: HostId) -> Self {
        Self {
            host,
            answerer: Rc::new(RefCell::new(Answerer::default())),
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
        wasm_bindgen_futures::spawn_local(drive(self.host, Rc::clone(&self.answerer), msg));
    }
}

/// One ask, read through to its answer and handed back.
async fn drive(host: HostId, answerer: Rc<RefCell<Answerer>>, msg: OscMessage) {
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
                for need in needs {
                    let got = read(&need).await;
                    answerer.borrow_mut().deliver(&need, got);
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

/// Reads what `need` names out of the page's storage.
async fn read(need: &Need) -> Result<Got, String> {
    let file = file(need.path()).await?;
    match need {
        Need::Stat { .. } => Ok(Got::Stat(Stat {
            len: field(&file, "size").as_f64().unwrap_or(0.0) as u64,
            // Milliseconds since the epoch, as the native clock's seconds.
            modified: field(&file, "lastModified").as_f64().map(|ms| ms / 1000.0),
        })),
        Need::Read {
            path, offset, len, ..
        } => {
            let end = *offset + *len as u64;
            let slice = call(
                &file,
                "slice",
                &[
                    JsValue::from_f64(*offset as f64),
                    JsValue::from_f64(end as f64),
                ],
            )
            .await?;
            let buffer = call(&slice, "arrayBuffer", &[]).await?;
            let bytes = js_sys::Uint8Array::new(
                buffer
                    .dyn_ref::<js_sys::ArrayBuffer>()
                    .ok_or_else(|| format!("{path}: a read that is not bytes"))?,
            )
            .to_vec();
            if bytes.len() < *len {
                return Err(format!(
                    "{path}: {} bytes where {len} were asked",
                    bytes.len()
                ));
            }
            Ok(Got::Bytes(bytes))
        }
    }
}
