//! **What the audio editor hands on by reference**: the take a window draws
//! and the copy on the clipboard, each a [`FileList`] -- a tree of parts
//! shared with the take it came from -- kept under a number.
//!
//! What the editor says to the window and to the clipboard is JSON. A take
//! of thousands of parts written out as JSON for every edit is what made an
//! edit cost the take; so the JSON names the list by number and the list
//! itself crosses as a reference: [`Shared::said`] gives the host the lists
//! the editors named since it last asked, with whatever they answered, and a
//! paste reads a copy back from [`Shared::copy`].
//!
//! **A history entry names its takes the same way** ([`Shared::keep_take`]):
//! the take before an edit and the take after it are kept under numbers the
//! entry carries, shared between every window over the file, and let go of
//! when the history lets go of the entry.
//!
//! **What is kept is the editing context's** ([`crate::editing::Editing`]):
//! one [`Shared`] per context, handed to each audio editor it holds, so a
//! copy pastes into any window of the context and what one context keeps
//! goes with it. An editor outside any context keeps its own.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use clausters_document::parts::{FileList, Take};

/// How many lists wait for the host, at most: one per picture an answer
/// names, and a caller that never asks loses the oldest.
const SAID: usize = 64;

/// How many copies are kept: the clipboard holds one, and the few before it
/// let a paste that was already on its way still read its copy.
const COPIES: usize = 4;

/// The bit a kept take's number carries, so the history can hold it beside
/// the files a take reads (whose ids carry bit 62) and the server buffers
/// (small numbers) and the three never meet.
const TAKE: u64 = 1 << 61;

/// **What a context's audio editors hand on by reference**, behind one
/// handle each editor holds a clone of.
#[derive(Clone, Debug, Default)]
pub struct Shared(Arc<Mutex<Held>>);

#[derive(Debug, Default)]
struct Held {
    next: u64,
    /// The lists named and not yet asked for by the host.
    waiting: VecDeque<(u64, FileList)>,
    /// The copies kept for a paste.
    kept: VecDeque<(u64, FileList)>,
    /// The takes the histories name, by number.
    takes: HashMap<u64, Take>,
}

impl Held {
    fn number(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    fn keep(
        &mut self,
        list: FileList,
        at_most: usize,
        queue: fn(&mut Held) -> &mut VecDeque<(u64, FileList)>,
    ) -> u64 {
        let id = self.number();
        let queue = queue(self);
        queue.push_back((id, list));
        while queue.len() > at_most {
            queue.pop_front();
        }
        id
    }
}

impl Shared {
    fn held(&self) -> MutexGuard<'_, Held> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// **A list for the host to draw**, under the number the JSON names it by.
    pub fn say(&self, list: FileList) -> u64 {
        self.held().keep(list, SAID, |h| &mut h.waiting)
    }

    /// **The lists named since the last call**, for the host: it reads the
    /// numbers in what the editor answered against these.
    pub fn said(&self) -> Vec<(u64, FileList)> {
        self.held().waiting.drain(..).collect()
    }

    /// The list named `id` and not yet asked for: what a test reads where the
    /// host would.
    #[cfg(test)]
    pub(crate) fn told(&self, id: &serde_json::Value) -> Option<FileList> {
        let id = id.get("shared")?.as_u64()?;
        self.held()
            .waiting
            .iter()
            .find(|(said, _)| *said == id)
            .map(|(_, list)| list.clone())
    }

    /// **A copy put on the clipboard**, kept under the number its document
    /// names it by.
    pub fn keep_copy(&self, list: FileList) -> u64 {
        self.held().keep(list, COPIES, |h| &mut h.kept)
    }

    /// The copy kept under `id`, while it is.
    pub fn copy(&self, id: u64) -> Option<FileList> {
        self.held()
            .kept
            .iter()
            .find(|(kept, _)| *kept == id)
            .map(|(_, list)| list.clone())
    }

    /// **A take a history entry names**, kept under the number it answers.
    /// Keeping it costs a reference: its parts are shared.
    pub fn keep_take(&self, take: &Take) -> u64 {
        let mut held = self.held();
        let id = TAKE | held.number();
        held.takes.insert(id, take.clone());
        id
    }

    /// The take kept under `id`, while a history names it.
    pub fn take(&self, id: u64) -> Option<Take> {
        self.held().takes.get(&id).cloned()
    }

    /// **No entry names `id` any more**: the take goes. Answers whether `id`
    /// was a kept take's number at all.
    pub fn let_go_take(&self, id: u64) -> bool {
        if id & TAKE == 0 || id & (1 << 62) != 0 {
            return false;
        }
        self.held().takes.remove(&id);
        true
    }

    /// How many takes are kept: what a test reads to see them let go of.
    pub fn takes_kept(&self) -> usize {
        self.held().takes.len()
    }
}
