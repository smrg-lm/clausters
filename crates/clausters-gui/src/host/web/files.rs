//! The page's half of **listing a directory**, and of **changing what is in
//! one**: the origin private file system (OPFS), the storage a path names in
//! a tab. A change ([`Change`]) is carried out here as a listing is, and an
//! upload and a download are the browser's own file picker and download.
//!
//! OPFS answers with promises, so a listing is asked here, awaited off the
//! event loop and handed back through the proxy as [`WebEvent::Listed`] -- the
//! shape a bulk fetch already has -- carrying the ask's ticket, which is what
//! lets a chooser that moved on meanwhile refuse it. The path rule is the web client's
//! (`clients/web/src/engine/opfs.ts`): `/`-separated from the storage's root,
//! and a path that climbs out of it is refused.

use js_sys::{Function, Promise, Reflect};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use super::*;
use crate::host::files::{Ask, Change, ChangeAsk, DirEntry, Listing, Usage};

/// The segments of `path`, refusing one that would leave the root.
pub(super) fn parts(path: &str) -> Result<Vec<String>, String> {
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

pub(super) fn js_err(what: &str, e: JsValue) -> String {
    format!(
        "{what}: {}",
        e.as_string().unwrap_or_else(|| format!("{e:?}"))
    )
}

/// Calls `name` on `target` with `args` and awaits the promise it returns.
pub(super) async fn call(
    target: &JsValue,
    name: &str,
    args: &[JsValue],
) -> Result<JsValue, String> {
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

pub(super) fn field(target: &JsValue, name: &str) -> JsValue {
    Reflect::get(target, &JsValue::from_str(name)).unwrap_or(JsValue::UNDEFINED)
}

/// The page's storage: `navigator.storage`, or why there is none.
fn storage() -> Result<JsValue, String> {
    let storage = field(&field(&js_sys::global(), "navigator"), "storage");
    if storage.is_undefined() {
        return Err("this page has no storage".into());
    }
    Ok(storage)
}

/// **The directory `segments` name**, from the storage's root, made on the
/// way when `create` says so.
async fn dir_at(segments: &[String], create: bool) -> Result<JsValue, String> {
    let options = js_sys::Object::new();
    Reflect::set(
        &options,
        &JsValue::from_str("create"),
        &JsValue::from_bool(create),
    )
    .map_err(|e| js_err("create", e))?;
    let mut dir = call(&storage()?, "getDirectory", &[]).await?;
    for segment in segments {
        dir = call(
            &dir,
            "getDirectoryHandle",
            &[JsValue::from_str(segment), options.clone().into()],
        )
        .await
        .map_err(|_| format!("/{}: no such folder", segments.join("/")))?;
    }
    Ok(dir)
}

/// **How much of the page's storage is used**, as the browser estimates it
/// (`navigator.storage.estimate`), or nothing where it does not say.
async fn usage() -> Option<Usage> {
    let estimate = call(&storage().ok()?, "estimate", &[]).await.ok()?;
    let used = field(&estimate, "usage").as_f64()?;
    let quota = field(&estimate, "quota").as_f64()?;
    Some(Usage {
        used: used as u64,
        quota: quota as u64,
    })
}

/// The folder a path is in, and its name: `None` for the root.
fn split(path: &str) -> Result<Option<(Vec<String>, String)>, String> {
    let mut segments = parts(path)?;
    Ok(segments.pop().map(|name| (segments, name)))
}

/// **Removes `path`**: a file, or a folder and everything in it.
async fn remove_opfs(path: &str) -> Result<(), String> {
    let Some((folder, name)) = split(path)? else {
        return Err("the storage's root is not removed".into());
    };
    let options = js_sys::Object::new();
    Reflect::set(&options, &JsValue::from_str("recursive"), &JsValue::TRUE)
        .map_err(|e| js_err("recursive", e))?;
    let dir = dir_at(&folder, false).await?;
    call(
        &dir,
        "removeEntry",
        &[JsValue::from_str(&name), options.into()],
    )
    .await
    .map(|_| ())
    .map_err(|_| format!("{path}: not removed"))
}

/// **Copies `from` to `to`**: a file's bytes, or a folder and everything in
/// it, folder by folder.
async fn copy_opfs(from: &str, to: &str) -> Result<(), String> {
    match read_opfs(from).await {
        Ok(bytes) => write_opfs(to, &bytes).await,
        Err(_) => {
            let listing = list_opfs(from).await?;
            dir_at(&parts(to)?, true).await?;
            for entry in listing.entries {
                let (a, b) = (
                    crate::host::files::join(from, &entry.name),
                    crate::host::files::join(to, &entry.name),
                );
                Box::pin(copy_opfs(&a, &b)).await?;
            }
            Ok(())
        }
    }
}

/// **Gives `from` the path `to`**: moved where the browser moves a handle,
/// copied and removed where it does not.
async fn rename_opfs(from: &str, to: &str) -> Result<(), String> {
    let (Some((from_folder, from_name)), Some((to_folder, to_name))) = (split(from)?, split(to)?)
    else {
        return Err("the storage's root is not renamed".into());
    };
    if to_folder.starts_with(&parts(from)?) {
        return Err(format!("{from} is not moved into itself"));
    }
    let target = dir_at(&to_folder, true).await?;
    let exists = |kind: &'static str| {
        let target = target.clone();
        let to_name = to_name.clone();
        async move {
            call(&target, kind, &[JsValue::from_str(&to_name)])
                .await
                .is_ok()
        }
    };
    if exists("getFileHandle").await || exists("getDirectoryHandle").await {
        return Err(format!("{to}: there is one already"));
    }
    let source = dir_at(&from_folder, false).await?;
    let handle = match call(&source, "getFileHandle", &[JsValue::from_str(&from_name)]).await {
        Ok(file) => file,
        Err(_) => call(
            &source,
            "getDirectoryHandle",
            &[JsValue::from_str(&from_name)],
        )
        .await
        .map_err(|_| format!("{from}: no such file or folder"))?,
    };
    let moved = call(&handle, "move", &[target, JsValue::from_str(&to_name)]).await;
    if moved.is_ok() {
        return Ok(());
    }
    copy_opfs(from, to).await?;
    remove_opfs(from).await
}

/// **Hands the file at `path` to the browser as a download**, under its
/// name.
async fn download_opfs(path: &str) -> Result<(), String> {
    let bytes = read_opfs(path).await?;
    let global = js_sys::global();
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes.as_slice()).into());
    let blob_class = field(&global, "Blob");
    let blob = Reflect::construct(
        blob_class.unchecked_ref::<Function>(),
        &js_sys::Array::of1(&parts),
    )
    .map_err(|e| js_err("Blob", e))?;
    let url_class = field(&global, "URL");
    let url = call(&url_class, "createObjectURL", &[blob]).await?;
    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or("this page has no document to download from")?;
    let a = document.create_element("a").map_err(|e| js_err("a", e))?;
    a.set_attribute("href", &url.as_string().unwrap_or_default())
        .map_err(|e| js_err("href", e))?;
    let name = path
        .rsplit('/')
        .next()
        .filter(|n| !n.is_empty())
        .unwrap_or("file");
    a.set_attribute("download", name)
        .map_err(|e| js_err("download", e))?;
    a.unchecked_ref::<web_sys::HtmlElement>().click();
    // the browser has taken the file once the click is handled
    let later = wasm_bindgen::closure::Closure::once_into_js(move || {
        let _ = Reflect::get(&url_class, &JsValue::from_str("revokeObjectURL"))
            .ok()
            .and_then(|f| f.dyn_into::<Function>().ok())
            .map(|f| f.call1(&url_class, &url));
    });
    if let Some(window) = web_sys::window() {
        let _ = window
            .set_timeout_with_callback_and_timeout_and_arguments_0(later.unchecked_ref(), 1000);
    }
    Ok(())
}

/// **Asks the browser for files from the disk** and copies them into folder
/// `dir`: none when the reader cancels.
async fn upload_opfs(dir: &str) -> Result<(), String> {
    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or("this page has no document to upload into")?;
    let input = document
        .create_element("input")
        .map_err(|e| js_err("input", e))?;
    input
        .set_attribute("type", "file")
        .map_err(|e| js_err("type", e))?;
    input
        .set_attribute("multiple", "")
        .map_err(|e| js_err("multiple", e))?;
    // the picker's answer: a change when files were chosen, a cancel when
    // none were
    let chosen = Promise::new(&mut |resolve, _reject| {
        let on = |event: &str, picked: bool| {
            let resolve = resolve.clone();
            let f = wasm_bindgen::closure::Closure::once_into_js(move || {
                let _ = resolve.call1(&JsValue::NULL, &JsValue::from_bool(picked));
            });
            let _ = Reflect::set(&input, &JsValue::from_str(event), &f);
        };
        on("onchange", true);
        on("oncancel", false);
    });
    input.unchecked_ref::<web_sys::HtmlElement>().click();
    let picked = JsFuture::from(chosen)
        .await
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !picked {
        return Ok(());
    }
    dir_at(&parts(dir)?, true).await?;
    let files = field(&input, "files");
    let count = field(&files, "length").as_f64().unwrap_or(0.0) as u32;
    for i in 0..count {
        let file = call(&files, "item", &[JsValue::from(i)]).await?;
        let name = field(&file, "name").as_string().unwrap_or_default();
        let buffer = call(&file, "arrayBuffer", &[]).await?;
        let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
        write_opfs(&crate::host::files::join(dir, &name), &bytes).await?;
    }
    Ok(())
}

/// **Carries `change` out** on the page's storage.
pub(super) async fn change_opfs(change: &Change) -> Result<(), String> {
    match change {
        Change::MakeFolder(path) => dir_at(&parts(path)?, true).await.map(|_| ()),
        Change::Rename(from, to) => rename_opfs(from, to).await,
        Change::Remove(path) => remove_opfs(path).await,
        Change::Upload(dir) => upload_opfs(dir).await,
        Change::Download(path) => download_opfs(path).await,
    }
}

/// Carries one change out and hands its end back through the proxy.
pub(super) async fn fetch_change(host: HostId, def_id: i32, widget_id: i32, ask: ChangeAsk) {
    let result = change_opfs(&ask.change).await;
    if let Some(proxy) = web_proxy() {
        let _ = proxy.send_event(HostEvent::To(
            host,
            WebEvent::Changed {
                def_id,
                widget_id,
                ask,
                result,
            },
        ));
    }
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
    let mut listing = Listing::ordered(format!("/{}", segments.join("/")), entries);
    listing.usage = usage().await;
    Ok(listing)
}

/// **Reads `path`** from the page's storage: the bytes of the file, or why
/// there are none (no storage, no such directory or file). What a widget's
/// `path` or `cache` names in a tab, as a file command's path does.
pub(super) async fn read_opfs(path: &str) -> Result<Vec<u8>, String> {
    let mut segments = parts(path)?;
    let Some(name) = segments.pop() else {
        return Err("an empty path names no file".into());
    };
    let navigator = field(&js_sys::global(), "navigator");
    let storage = field(&navigator, "storage");
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
    let file = call(&handle, "getFile", &[]).await?;
    let buffer = call(&file, "arrayBuffer", &[]).await?;
    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
}

/// **Writes `bytes` as `path`** in the page's storage, replacing whatever was
/// there and creating the directories above it -- what the web client's
/// `writeFile` does, on the main thread the host runs on, where the writable
/// stream is the way in (a synchronous handle exists only in a worker).
pub(super) async fn write_opfs(path: &str, bytes: &[u8]) -> Result<(), String> {
    let mut segments = parts(path)?;
    let Some(name) = segments.pop() else {
        return Err("an empty path names no file".into());
    };
    let navigator = field(&js_sys::global(), "navigator");
    let storage = field(&navigator, "storage");
    if storage.is_undefined() {
        return Err("this page has no storage to write".into());
    }
    let create = js_sys::Object::new();
    Reflect::set(&create, &JsValue::from_str("create"), &JsValue::TRUE)
        .map_err(|e| js_err("create", e))?;
    let mut dir = call(&storage, "getDirectory", &[]).await?;
    for segment in &segments {
        dir = call(
            &dir,
            "getDirectoryHandle",
            &[JsValue::from_str(segment), create.clone().into()],
        )
        .await?;
    }
    let file = call(
        &dir,
        "getFileHandle",
        &[JsValue::from_str(&name), create.into()],
    )
    .await?;
    let stream = call(&file, "createWritable", &[]).await?;
    let data = js_sys::Uint8Array::from(bytes);
    call(&stream, "write", &[data.into()]).await?;
    call(&stream, "close", &[]).await?;
    Ok(())
}

/// Lists one directory and hands the answer back through the proxy, under
/// the ticket it was asked with.
pub(super) async fn fetch_listing(host: HostId, def_id: i32, widget_id: i32, ask: Ask) {
    let result = list_opfs(&ask.path).await;
    if let Some(proxy) = web_proxy() {
        let _ = proxy.send_event(HostEvent::To(
            host,
            WebEvent::Listed {
                def_id,
                widget_id,
                ticket: ask.ticket,
                result,
            },
        ));
    }
}

/// Reads one file of the page's storage and hands its bytes back through
/// the proxy, under the ticket it was asked with.
pub(super) async fn fetch_read(host: HostId, def_id: i32, widget_id: i32, ask: Ask) {
    let result = read_opfs(&ask.path).await;
    if let Some(proxy) = web_proxy() {
        let _ = proxy.send_event(HostEvent::To(
            host,
            WebEvent::Read {
                def_id,
                widget_id,
                ticket: ask.ticket,
                result,
            },
        ));
    }
}

impl WebApp {
    /// Starts a listing for every directory an element is waiting on, and a
    /// read for every file.
    pub(super) fn start_listings(&mut self) {
        for (def, widget, ask) in self.host.pending_listings() {
            wasm_bindgen_futures::spawn_local(fetch_listing(self.id, def, widget, ask));
        }
        for (def, widget, ask) in self.host.pending_reads() {
            wasm_bindgen_futures::spawn_local(fetch_read(self.id, def, widget, ask));
        }
        for (def, widget, ask) in self.host.pending_changes() {
            wasm_bindgen_futures::spawn_local(fetch_change(self.id, def, widget, ask));
        }
    }

    /// A change came to its end: the chooser says so to its owner and lists
    /// its directory again.
    pub(super) fn on_changed(
        &mut self,
        def: i32,
        widget: i32,
        ask: &ChangeAsk,
        result: Result<(), String>,
    ) {
        let effects = self.host.deliver_changed(def, widget, ask, result);
        if !effects.is_empty() {
            self.apply_gesture_effects(effects);
            self.start_listings();
        }
    }

    /// A file came back: the element takes it, and its window repaints.
    pub(super) fn on_read(
        &mut self,
        def: i32,
        widget: i32,
        ticket: u64,
        result: Result<Vec<u8>, String>,
    ) {
        if self.host.deliver_read(def, widget, ticket, result) {
            self.request_redraw(def);
        }
    }

    /// A listing came back: the element takes it if it is still the answer it
    /// waits on, and its window repaints.
    pub(super) fn on_listed(
        &mut self,
        def: i32,
        widget: i32,
        ticket: u64,
        result: Result<Listing, String>,
    ) {
        if self.host.deliver_listing(def, widget, ticket, result) {
            self.request_redraw(def);
            // what a file dialog answered to it
            self.post_host_effects();
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
