//! **What the audio editor hands on by reference**: the take a window draws
//! and the copy on the clipboard, each a [`FileList`] -- a tree of parts
//! shared with the take it came from -- kept under a number.
//!
//! The editor runs on its application's thread, and what it says to the
//! window and to the clipboard is JSON. A take of thousands of parts written
//! out as JSON for every edit is what made an edit cost the take; so the JSON
//! names the list by number and the list itself crosses as a reference:
//! [`said`] gives the host the lists the editor named since it last asked,
//! with whatever the editor answered, and a paste reads a copy back from
//! [`copy`]. Both are this thread's, which is the one the editor runs on.
//!
//! **A history entry names its takes the same way** ([`keep_take`]): the
//! take before an edit and the take after it are kept under numbers the
//! entry carries, shared between every window over the file, and let go of
//! when the history lets go of the entry.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

use clausters_document::parts::{FileList, Take};

/// How many lists wait for the host, at most: one per picture an answer
/// names, and a caller that never asks loses the oldest.
const SAID: usize = 64;

/// How many copies are kept: the clipboard holds one, and the few before it
/// let a paste that was already on its way still read its copy.
const COPIES: usize = 4;

static NEXT: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static WAITING: RefCell<VecDeque<(u64, FileList)>> = const { RefCell::new(VecDeque::new()) };
    static KEPT: RefCell<VecDeque<(u64, FileList)>> = const { RefCell::new(VecDeque::new()) };
}

fn keep(queue: &RefCell<VecDeque<(u64, FileList)>>, list: FileList, at_most: usize) -> u64 {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let mut queue = queue.borrow_mut();
    queue.push_back((id, list));
    while queue.len() > at_most {
        queue.pop_front();
    }
    id
}

/// **A list for the host to draw**, under the number the JSON names it by.
pub fn say(list: FileList) -> u64 {
    WAITING.with(|w| keep(w, list, SAID))
}

/// **The lists named since the last call**, for the host: it reads the
/// numbers in what the editor answered against these.
pub fn said() -> Vec<(u64, FileList)> {
    WAITING.with(|w| w.borrow_mut().drain(..).collect())
}

/// The list named `id` and not yet asked for: what a test reads where the
/// host would.
#[cfg(test)]
pub(crate) fn told(id: &serde_json::Value) -> Option<FileList> {
    let id = id.get("shared")?.as_u64()?;
    WAITING.with(|w| {
        w.borrow()
            .iter()
            .find(|(said, _)| *said == id)
            .map(|(_, list)| list.clone())
    })
}

/// **A copy put on the clipboard**, kept under the number its document
/// names it by.
pub fn keep_copy(list: FileList) -> u64 {
    KEPT.with(|k| keep(k, list, COPIES))
}

/// The copy kept under `id`, while it is.
pub fn copy(id: u64) -> Option<FileList> {
    KEPT.with(|k| {
        k.borrow()
            .iter()
            .find(|(kept, _)| *kept == id)
            .map(|(_, list)| list.clone())
    })
}

/// The bit a kept take's number carries, so the history can hold it beside
/// the files a take reads (whose ids carry bit 62) and the server buffers
/// (small numbers) and the three never meet.
const TAKE: u64 = 1 << 61;

/// The takes the histories name, by number.
static TAKES: LazyLock<Mutex<HashMap<u64, Take>>> = LazyLock::new(Default::default);

/// **A take a history entry names**, kept under the number it answers.
/// Keeping it costs a reference: its parts are shared.
pub fn keep_take(take: &Take) -> u64 {
    let id = TAKE | NEXT.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut takes) = TAKES.lock() {
        takes.insert(id, take.clone());
    }
    id
}

/// The take kept under `id`, while a history names it.
pub fn take(id: u64) -> Option<Take> {
    TAKES.lock().ok()?.get(&id).cloned()
}

/// **No entry names `id` any more**: the take goes. Answers whether `id`
/// was a kept take's number at all.
pub fn let_go_take(id: u64) -> bool {
    if id & TAKE == 0 || id & (1 << 62) != 0 {
        return false;
    }
    if let Ok(mut takes) = TAKES.lock() {
        takes.remove(&id);
    }
    true
}

/// How many takes are kept: what a test reads to see them let go of.
pub fn takes_kept() -> usize {
    TAKES.lock().map_or(0, |t| t.len())
}
