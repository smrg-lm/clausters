//! The page's half of **listing a directory**: the origin private file system
//! (OPFS), the storage a path names in a tab.
//!
//! OPFS answers with promises, so a listing is asked here, awaited off the
//! event loop and handed back through the proxy as [`WebEvent::Listed`] -- the
//! shape a bulk fetch already has. The path rule is the web client's
//! (`clients/web/src/engine/opfs.ts`): `/`-separated from the storage's root,
//! and a path that climbs out of it is refused.

use js_sys::{Function, Promise, Reflect};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use super::*;
use crate::host::files::{DirEntry, Listing};

/// The segments of `path`, refusing one that would leave the root.
fn parts(path: &str) -> Result<Vec<String>, String> {
    let out: Vec<String> = path
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .map(str::to_string)
        .collect();
    if out.iter().any(|p| p == "..") {
        return Err(format!(
            "{path}: a path may not climb out of the page's storage"
        ));
    }
    Ok(out)
}

fn js_err(what: &str, e: JsValue) -> String {
    format!(
        "{what}: {}",
        e.as_string().unwrap_or_else(|| format!("{e:?}"))
    )
}

/// Calls `name` on `target` with `args` and awaits the promise it returns.
async fn call(target: &JsValue, name: &str, args: &[JsValue]) -> Result<JsValue, String> {
    let f: Function = Reflect::get(target, &JsValue::from_str(name))
        .map_err(|e| js_err(name, e))?
        .dyn_into()
        .map_err(|_| format!("{name} is not a function here"))?;
    let array: js_sys::Array = args.iter().collect();
    let result = f.apply(target, &array).map_err(|e| js_err(name, e))?;
    match result.dyn_into::<Promise>() {
        Ok(p) => JsFuture::from(p).await.map_err(|e| js_err(name, e)),
        Err(value) => Ok(value),
    }
}

fn field(target: &JsValue, name: &str) -> JsValue {
    Reflect::get(target, &JsValue::from_str(name)).unwrap_or(JsValue::UNDEFINED)
}

/// Lists `path` in the page's storage.
pub(super) async fn list_opfs(path: &str) -> Result<Listing, String> {
    let segments = parts(path)?;
    let navigator = field(&js_sys::global(), "navigator");
    let storage = field(&navigator, "storage");
    if storage.is_undefined() {
        return Err("this page has no storage to list".into());
    }
    let mut dir = call(&storage, "getDirectory", &[]).await?;
    for segment in &segments {
        dir = call(&dir, "getDirectoryHandle", &[JsValue::from_str(segment)])
            .await
            .map_err(|_| format!("{path}: no such directory"))?;
    }
    // `values()` is an async iterator over the directory's handles.
    let iter = call(&dir, "values", &[]).await?;
    let mut entries = Vec::new();
    loop {
        let step = call(&iter, "next", &[]).await?;
        if field(&step, "done").as_bool().unwrap_or(true) {
            break;
        }
        let handle = field(&step, "value");
        let name = field(&handle, "name").as_string().unwrap_or_default();
        let is_dir = field(&handle, "kind").as_string().as_deref() == Some("directory");
        let size = if is_dir {
            0
        } else {
            call(&handle, "getFile", &[])
                .await
                .ok()
                .and_then(|file| field(&file, "size").as_f64())
                .map_or(0, |s| s as u64)
        };
        entries.push(DirEntry {
            name,
            dir: is_dir,
            size,
        });
    }
    Ok(Listing::ordered(
        format!("/{}", segments.join("/")),
        entries,
    ))
}

/// Lists one directory and hands the answer back through the proxy.
pub(super) async fn fetch_listing(host: HostId, def_id: i32, widget_id: i32, asked: String) {
    let result = list_opfs(&asked).await;
    if let Some(proxy) = web_proxy() {
        let _ = proxy.send_event(HostEvent::To(
            host,
            WebEvent::Listed {
                def_id,
                widget_id,
                asked,
                result,
            },
        ));
    }
}

impl WebApp {
    /// Starts a listing for every directory an element is waiting on.
    pub(super) fn start_listings(&mut self) {
        for (def, widget, path) in self.host.pending_listings() {
            wasm_bindgen_futures::spawn_local(fetch_listing(self.id, def, widget, path));
        }
    }

    /// A listing came back: the element takes it, and its window repaints.
    pub(super) fn on_listed(
        &mut self,
        def: i32,
        widget: i32,
        asked: &str,
        result: Result<Listing, String>,
    ) {
        if self.host.deliver_listing(def, widget, asked, result) {
            self.request_redraw(def);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_path_is_split_from_the_root_and_cannot_climb_out() {
        assert_eq!(parts("/a/b").unwrap(), vec!["a", "b"]);
        assert!(parts(".").unwrap().is_empty());
        assert!(parts("/a/../b").is_err());
    }
}
