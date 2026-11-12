//! The edit history.
//!
//! A `Drop`-backed object, like `Document` beside it and for a related reason:
//! a history is state, and the state is the point. The spill store is why -- a
//! bulk payload leaves the pile deliberately, so passing one by value would
//! carry every spilled span on every call, which is the cost spilling exists to
//! avoid.

#[cfg(target_arch = "wasm32")]
use crate::document::JsDocument;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// One editing context's history, the JS face of
/// [`clausters_document::History`].
///
/// A history holds the structures registered in it and one ordered pile over
/// them, so what a caller decides by choosing an instance is *what shares an
/// undo order*: a structure it built with no multitrack behind it is a history
/// with one structure in it, an application composing several editable views
/// registers them all in one, and two views of one structure hold one history
/// between them.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = History)]
pub struct JsHistory(clausters_document::History);

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = History)]
impl JsHistory {
    /// A new, empty history. `budget` is how many entries it keeps before the
    /// oldest falls off and `spillAbove` how many **bytes** a payload must
    /// reach, serialized, before it leaves the pile; either as 0 takes the
    /// crate's default.
    #[wasm_bindgen(constructor)]
    pub fn new(budget: usize, spill_above: usize) -> JsHistory {
        let mut history = clausters_document::History::new();
        if budget > 0 {
            history = history.budget(budget);
        }
        if spill_above > 0 {
            history = history.spill_above(spill_above);
        }
        JsHistory(history)
    }

    /// Takes a structure into this history and returns its identity.
    ///
    /// `domain` names the vocabulary its payloads are written in -- `"tree"` for
    /// the arrangement -- and the history carries it so a caller routing what
    /// comes back knows which reader a leg belongs to. The identity is minted
    /// here rather than carried by the data, and it is also the read-back path.
    pub fn register(&mut self, domain: &str) -> u64 {
        self.0.register(domain).0
    }

    /// Apply an edit to `document` **and record it** against `structure`, in
    /// one call: the inverse has to be read out of the document before the edit
    /// lands, so applying first and recording second would record the wrong
    /// thing. `apply(structure, document, requestJson) -> outcomeJson`, the
    /// request carrying `{ intent, against?, quant?, label? }`.
    ///
    /// The arrangement's door alone, because the document is the one state this
    /// surface can reach; a caller editing anything else applies the edit
    /// itself and hands the pair to {@link History.record}.
    pub fn apply(
        &mut self,
        structure: u64,
        document: &mut JsDocument,
        request: &str,
    ) -> Result<String, JsError> {
        #[derive(serde::Deserialize)]
        struct Request {
            intent: clausters_document::Intent,
            #[serde(default)]
            against: Option<clausters_document::Against>,
            #[serde(default)]
            quant: f64,
            #[serde(default)]
            label: Option<String>,
        }
        let request: Request = serde_json::from_str(request)
            .map_err(|e| JsError::new(&format!("history.apply: {e}")))?;
        let against = request
            .against
            .unwrap_or_else(clausters_document::Against::unstated);
        let mut tree = clausters_document::Tree {
            document: &mut document.0,
            against,
            rules: clausters_document::Rules {
                quant: request.quant,
            },
        };
        let applied = self.0.apply(
            clausters_document::StructureId(structure),
            &mut tree,
            &clausters_document::log::payload(&request.intent),
            request.label.unwrap_or_else(|| "edit".into()),
        );
        serde_json::to_string(&serde_json::json!({
            "effective": clausters_document::log::intent_of(&applied.effective)
                .unwrap_or(request.intent),
            "applied": applied.applied,
            "reason": applied.reason,
            "stale": applied.stale,
        }))
        .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Record one entry -- the door for everything {@link History.apply} cannot
    /// do: a destructive write, whose overwritten samples are not in the tree,
    /// and every domain that is not the arrangement, whose state this surface
    /// cannot reach. Applies nothing.
    ///
    /// `record(requestJson)` with
    /// `{ label?, coalesce?, legs: [{ structure, forward, backward, key? }] }`.
    ///
    /// **Several legs are one transaction**: applied in the order given,
    /// inverted in reverse, and undone in one step -- what a gesture touching
    /// more than one structure needs, and why the whole entry crosses in one
    /// call. It is not coalescing, which merges *successive* entries over one
    /// structure. A leg's `key` is what makes two edits *the same thing done
    /// the same way* ({@link Document.coalesceKey} for the arrangement); an
    /// absent one never coalesces.
    ///
    /// Returns whether it was recorded: an entry with no leg, or one naming a
    /// structure this history did not mint, is refused rather than opening a
    /// second order over data that already has one. The inverse of an
    /// arrangement leg comes from {@link Document.inverse}, read before the
    /// edit lands.
    pub fn record(&mut self, request: &str) -> Result<bool, JsError> {
        #[derive(serde::Deserialize)]
        struct Leg {
            structure: u64,
            forward: clausters_document::history::Step,
            /// How to put this leg back, or absent when nothing can -- an act
            /// whose inverse the owner cannot write. The entry is still
            /// recorded, marked, and walked past in both directions.
            #[serde(default)]
            backward: Option<clausters_document::Opaque>,
            #[serde(default)]
            key: String,
        }
        #[derive(serde::Deserialize)]
        struct Request {
            #[serde(default)]
            label: Option<String>,
            #[serde(default)]
            coalesce: bool,
            legs: Vec<Leg>,
        }
        let request: Request = serde_json::from_str(request)
            .map_err(|e| JsError::new(&format!("history.record: {e}")))?;
        let mut legs = request.legs.into_iter();
        let Some(first) = legs.next() else {
            return Ok(false);
        };
        let label = request.label.unwrap_or_else(|| "edit".into());
        let structure = clausters_document::StructureId(first.structure);
        let mut entry = match first.backward {
            Some(backward) => {
                clausters_document::history::Entry::new(label, structure, first.forward, backward)
            }
            None => {
                clausters_document::history::Entry::uninvertible(label, structure, first.forward)
            }
        };
        if !first.key.is_empty() {
            entry = entry.keyed(first.key);
        }
        for leg in legs {
            let structure = clausters_document::StructureId(leg.structure);
            entry = match leg.backward {
                Some(backward) => entry.and(structure, leg.forward, backward),
                None => entry.and_uninvertible(structure, leg.forward),
            };
            if !leg.key.is_empty() {
                entry = entry.keyed(leg.key);
            }
        }
        if request.coalesce {
            entry = entry.continuing();
        }
        Ok(self.0.record(entry))
    }

    /// **One step of the pile, routed** -- the legs each structure has to
    /// apply, in order, and what only its owner can re-run. `direction` is
    /// `"undo"` or `"redo"`; `undefined` when there was nothing to walk, and a
    /// throw for a word that is neither.
    ///
    /// Returns `{ label, legs, remaining, skipped }`, where `legs` is
    /// `[{ structure, payloads }, ...]` -- one entry per structure rather than
    /// one per leg. It is one call and not two because picking the side a
    /// direction reads, and keeping the legs one structure owns, are rules and
    /// not plumbing, and every caller was writing both for itself.
    ///
    /// `remaining` holds the steps from the first one the crate cannot describe
    /// as an edit onward, for the owner to re-run; it stops at the first rather
    /// than skipping it, so a later edit is never applied over a state the
    /// operation before it was meant to produce. Going back it is always empty:
    /// an inverse is always an edit. `skipped` names the entries the walk had
    /// to pass over because nothing can invert them -- a hole in the history
    /// that announces itself, which is what lets a person understand why an
    /// undo did not go where they expected.
    ///
    /// **The order kept is the order within a structure.** A caller applies
    /// through one vocabulary at a time, so an entry naming two structures is
    /// walked one structure at a time.
    ///
    /// It applies nothing: a history holds structures this surface cannot
    /// reach, so applying the legs it *could* would leave the rest to the
    /// caller out of order, which is how a transaction half-happens.
    pub fn walk(&mut self, direction: &str) -> Result<Option<String>, JsError> {
        let Some(direction) = clausters_document::history::Direction::parse(direction) else {
            return Err(JsError::new("a walk goes \"undo\" or \"redo\""));
        };
        let Some(walked) = self.0.walk(direction) else {
            return Ok(None);
        };
        let legs: Vec<_> = walked
            .legs
            .into_iter()
            .map(|(structure, payloads)| {
                serde_json::json!({"structure": structure.0, "payloads": payloads})
            })
            .collect();
        let remaining: Vec<_> = walked
            .remaining
            .into_iter()
            .map(|(structure, steps)| serde_json::json!({"structure": structure.0, "steps": steps}))
            .collect();
        serde_json::to_string(&serde_json::json!({
            "label": walked.label,
            "legs": legs,
            "remaining": remaining,
            "skipped": walked.skipped,
        }))
        .map(Some)
        .map_err(|e| JsError::new(&e.to_string()))
    }

    /// The data behind a structure is gone: drop it from the registry, and say
    /// whether its memory may go now.
    ///
    /// `true` when nothing in the pile names it any more, `false` when the
    /// caller must wait for {@link History.released} -- because undoing a
    /// deletion has to be able to give the data back, so a structure that is
    /// out of the tree stays alive while an entry can still restore what
    /// referred to it.
    ///
    /// It also **invalidates the entries that name it**: they cannot be applied
    /// to data that is gone, so they become non-invertible -- kept, marked, and
    /// walked past with the walk saying so. Undoing a deletion returns the
    /// data, not its history.
    pub fn forget(&mut self, structure: u64) -> bool {
        self.0.forget(clausters_document::StructureId(structure))
    }

    /// The forgotten structures no entry names any more -- the caller may free
    /// their data now. Drains: each is reported once.
    pub fn released(&mut self) -> Vec<u64> {
        self.0
            .released()
            .into_iter()
            .map(|structure| structure.0)
            .collect()
    }

    /// Stamps the pile where it stands: this is what is on disk.
    ///
    /// A save is an event of the whole editing context and the mark is the
    /// pile's, so one save stamps one mark, and a structure registered later
    /// starts behind it.
    #[wasm_bindgen(js_name = markSaved)]
    pub fn mark_saved(&mut self) {
        self.0.mark_saved();
    }

    /// Whether the work differs from what was last saved.
    ///
    /// Crossing the mark backwards is allowed, and this is the announcement --
    /// which has to be accurate: nothing on disk changed, and the file still
    /// holds those edits until the next save. Crossing forward again returns to
    /// clean.
    #[wasm_bindgen(getter)]
    pub fn dirty(&self) -> bool {
        self.0.dirty()
    }

    /// Whether the saved state can still be reached by walking this history.
    ///
    /// `false` after the case the warning earns its place for: undo past the
    /// mark and then edit, and the redo is truncated -- so the saved state stops
    /// being reachable, and {@link History.dirty} will never go quiet again on
    /// its own.
    #[wasm_bindgen(getter, js_name = savedReachable)]
    pub fn saved_reachable(&self) -> bool {
        self.0.saved_reachable()
    }

    /// Whether there is anything to undo.
    #[wasm_bindgen(getter, js_name = canUndo)]
    pub fn can_undo(&self) -> bool {
        self.0.can_undo()
    }

    /// Whether there is anything to redo.
    #[wasm_bindgen(getter, js_name = canRedo)]
    pub fn can_redo(&self) -> bool {
        self.0.can_redo()
    }

    /// What an undo would be called, for a menu item -- and what a person needs
    /// when one pile holds several structures, since the label is the only
    /// thing saying which one a keystroke is about to move.
    #[wasm_bindgen(getter, js_name = undoLabel)]
    pub fn undo_label(&self) -> Option<String> {
        self.0.undo_label()
    }

    /// What a redo would be called.
    #[wasm_bindgen(getter, js_name = redoLabel)]
    pub fn redo_label(&self) -> Option<String> {
        self.0.redo_label()
    }

    /// How many entries the history holds.
    #[wasm_bindgen(getter)]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the history holds nothing -- `len == 0`, spelled the way a JS
    /// collection is read, as `JsScheduler` and `JsRegistry` already spell it.
    #[wasm_bindgen(getter, js_name = isEmpty)]
    pub fn is_empty(&self) -> bool {
        self.0.len() == 0
    }

    /// Forget every entry, releasing what was spilled. The structures stay
    /// registered: it is the order that is gone, not the identities the caller
    /// still holds.
    pub fn clear(&mut self) {
        self.0.clear();
    }
}

/// Apply one payload to a structure the crate can hold **as its own state**,
/// and get back what it now is together with the payload that puts it back.
///
/// The other half of what a domain brings: an edit and its inverse are one
/// vocabulary's rule, so a page that computed the inverse itself would be
/// spelling that rule again in TypeScript. Both directions come back in one
/// answer because the inverse has to be read *before* the edit lands.
///
/// `state` is the structure in its own vocabulary and the answer is
/// `{"state": ..., "applied": bool, "reason"?: ..., "current"?: ...}`, or an empty
/// string for a vocabulary whose state is not a value a caller can hand over:
/// the arrangement's tree (which has {@link JsDocument.apply} of its own) and a
/// span of samples (a borrowed view whose frames are in a buffer).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = domainEdit)]
pub fn domain_edit(domain: &str, state: &str, payload: &str) -> String {
    let (Ok(state), Ok(payload)) = (
        serde_json::from_str::<serde_json::Value>(state),
        serde_json::from_str::<serde_json::Value>(payload),
    ) else {
        return String::new();
    };
    let Some(edited) = clausters_document::domain::edit(
        domain,
        &clausters_document::Opaque(state),
        &clausters_document::Opaque(payload),
    ) else {
        return String::new();
    };
    serde_json::to_string(&edited).unwrap_or_default()
}

/// **The edits one gesture came to, applied as one: all of them or none** --
/// the rule every caller applies a gesture by.
///
/// `payloads` is a JSON array of payloads; the answer is `{"state", "applied",
/// "reason"?, "pairs": [{"forward", "backward"}]}` -- the state after the whole
/// gesture, or as it was when one of its edits was refused (and then why) --
/// or an empty string where {@link domainEdit} answers one.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = domainEditAll)]
pub fn domain_edit_all(domain: &str, state: &str, payloads: &str) -> String {
    let (Ok(state), Ok(payloads)) = (
        serde_json::from_str::<serde_json::Value>(state),
        serde_json::from_str::<Vec<serde_json::Value>>(payloads),
    ) else {
        return String::new();
    };
    let payloads: Vec<clausters_document::Opaque> = payloads
        .into_iter()
        .map(clausters_document::Opaque)
        .collect();
    let Some(gesture) =
        clausters_document::domain::edit_all(domain, &clausters_document::Opaque(state), &payloads)
    else {
        return String::new();
    };
    serde_json::to_string(&gesture).unwrap_or_default()
}

/// What makes two of a **domain's** edits *the same thing done the same way* --
/// the key a caller recording its own entry passes to {@link History.record},
/// or an empty string when the payload is not written in that vocabulary (or
/// the domain is one the crate does not speak).
///
/// It is a free function rather than a method because a caller here holds no
/// structure to ask: a curve, a span of samples and a timeline live in this
/// page's own memory, and only their *vocabulary* is the crate's.
/// {@link JsDocument.coalesceKey} stays as it is -- the arrangement's own door,
/// on the surface its sentence belongs to -- and this is the same rule for the
/// domains that have no handle here.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = domainCoalesceKey)]
pub fn domain_coalesce_key(domain: &str, payload: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return String::new();
    };
    clausters_document::domain::coalesce_key(domain, &clausters_document::Opaque(value))
        .unwrap_or_default()
}
