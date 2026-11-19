//! **The named GuiDefs of a page, kept in its storage**: the
//! [`DefStore`](crate::host::DefStore) of the browser front, so a page's
//! `/gui_load` finds what it saved, and a standalone page reopens an
//! application by name, as a native host does from its data directory.
//!
//! A page has a filesystem: the origin's private storage (OPFS), whose root
//! is the page's data directory. The records are the native store's
//! (`host::guistore`), under the same `defs/guidefs`. What stands between
//! the two is time: the trait answers at once and the page's storage answers
//! with promises. So the store is held **in memory and written through** --
//! a save is in memory at once and written to the storage on the page's own
//! turn -- and read from the storage when the page's host starts.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};

use crate::host::guistore::{self, GUIDEFS};

use super::files::{list_opfs, read_opfs, write_opfs};

/// The GuiDefs held, by name: their id and tree JSON.
type Held = Arc<Mutex<HashMap<String, (i32, Vec<u8>)>>>;

/// **The page's GuiDef store**: what it holds, filled from the storage as
/// the page starts and written through on every save.
#[derive(Clone, Default)]
pub struct PageStore {
    held: Held,
}

impl PageStore {
    /// A store, and the read of what the page's storage already keeps
    /// started: a save made before it lands is not overwritten by it.
    pub fn open() -> Self {
        let store = Self::default();
        let held = store.held.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let dir = format!("/{GUIDEFS}");
            let Ok(listing) = list_opfs(&dir).await else {
                return;
            };
            for entry in listing.entries.iter().filter(|e| !e.dir) {
                let Some(stem) = entry.name.strip_suffix(".json") else {
                    continue;
                };
                let Ok(bytes) = read_opfs(&format!("{dir}/{}", entry.name)).await else {
                    continue;
                };
                let Ok(record) = guistore::read_record(&bytes) else {
                    continue;
                };
                // the name is what the file's stem says, its escapes undone
                let name = unescape(stem);
                held.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .entry(name)
                    .or_insert(record);
            }
        });
        store
    }
}

impl crate::host::DefStore for PageStore {
    fn save(&self, name: &str, id: i32, tree_json: &[u8]) -> io::Result<()> {
        let bytes = guistore::record(id, tree_json)?;
        self.held
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(name.to_string(), (id, tree_json.to_vec()));
        let path = format!("/{GUIDEFS}/{}", guistore::file_name(name));
        wasm_bindgen_futures::spawn_local(async move {
            if let Err(e) = write_opfs(&path, &bytes).await {
                crate::host::diag::warn!("{path}: not kept in the page's storage: {e}");
            }
        });
        Ok(())
    }

    fn load(&self, name: &str) -> io::Result<(i32, Vec<u8>)> {
        self.held
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(name)
            .cloned()
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, format!("no GuiDef named {name:?}"))
            })
    }
}

/// A file stem back to the name [`guistore::sanitize_name`] escaped.
fn unescape(stem: &str) -> String {
    let bytes = stem.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(b) = stem
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
