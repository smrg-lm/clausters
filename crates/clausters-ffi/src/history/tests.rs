//! O11's acceptance: a run of gestures applied across the ABI inverts back to
//! the starting document exactly, through the crate's history rather than one
//! the caller keeps. Since O16 the history holds structures rather than one
//! document, so undoing hands the inverses back with the structure each belongs
//! to and the caller applies them -- which is what `walk` below does, and what
//! a binding does.

use super::*;

const DOC: &str = r#"{"version":1,"root":{"id":1,"kind":"aggregate","grouping":"concrete",
    "members":[
      {"offset":0.0,"node":{"id":2,"kind":"clang"}},
      {"offset":4.0,"node":{"id":3,"kind":"clang"}}
    ]}}"#;

/// A handle that frees itself, so a failing assertion does not leak, with the
/// one structure these tests edit already registered.
struct Held {
    handle: *mut FfiHistory,
    tree: u64,
}

impl Held {
    fn new() -> Self {
        let handle = clausters_history_new(0, 0);
        let domain = "tree";
        let tree = unsafe { clausters_history_register(handle, domain.as_ptr(), domain.len()) };
        assert_ne!(tree, 0, "a registered structure has an identity");
        Self { handle, tree }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        unsafe { clausters_history_free(self.handle) };
    }
}

/// The same, for the document the log edits. Since O12 the tree stays in Rust
/// and only the intent and the outcome cross, so a test holds a pointer and
/// reads the tree back with `snapshot` when it wants to assert on it.
struct Doc(*mut FfiDocument);

impl Doc {
    fn new(json: &str) -> Self {
        let h = unsafe { crate::clausters_document_open(json.as_ptr(), json.len()) };
        assert!(!h.is_null(), "the fixture parses");
        Self(h)
    }

    /// The tree as it now stands.
    fn tree(&self) -> serde_json::Value {
        serde_json::from_str(&sized(|out, cap| unsafe {
            crate::clausters_document_snapshot(self.0, out, cap)
        }))
        .unwrap()
    }
}

impl Drop for Doc {
    fn drop(&mut self) {
        unsafe { crate::clausters_document_free(self.0) };
    }
}

/// The size-then-fill dance a binding does, as one call.
fn sized(mut call: impl FnMut(*mut u8, usize) -> usize) -> String {
    let n = call(std::ptr::null_mut(), 0);
    let mut buf = vec![0u8; n];
    let written = call(buf.as_mut_ptr(), buf.len());
    assert_eq!(written, n, "sizing and filling must agree");
    String::from_utf8(buf).unwrap()
}

fn apply(log: &Held, doc: &Doc, intent: &str, quant: f64) -> serde_json::Value {
    let label = "move";
    serde_json::from_str(&sized(|out, cap| unsafe {
        clausters_history_apply(
            log.handle,
            log.tree,
            doc.0,
            intent.as_ptr(),
            intent.len(),
            std::ptr::null(),
            0,
            quant,
            label.as_ptr(),
            label.len(),
            out,
            cap,
        )
    }))
    .unwrap()
}

fn undo(log: &Held) -> serde_json::Value {
    serde_json::from_str(&sized(|out, cap| unsafe {
        clausters_history_undo(log.handle, out, cap)
    }))
    .unwrap()
}

fn redo(log: &Held) -> serde_json::Value {
    serde_json::from_str(&sized(|out, cap| unsafe {
        clausters_history_redo(log.handle, out, cap)
    }))
    .unwrap()
}

/// Applies one leg to the document, the way a binding does: the payload is an
/// intent because the leg named the arrangement's structure.
fn project(doc: &Doc, leg: &serde_json::Value) {
    let intent = serde_json::to_string(&leg["payload"]).unwrap();
    let n = unsafe {
        crate::clausters_document_apply(
            doc.0,
            intent.as_ptr(),
            intent.len(),
            std::ptr::null(),
            0,
            0.0,
            std::ptr::null_mut(),
            0,
        )
    };
    let mut buf = vec![0u8; n];
    unsafe {
        crate::clausters_document_apply(
            doc.0,
            intent.as_ptr(),
            intent.len(),
            std::ptr::null(),
            0,
            0.0,
            buf.as_mut_ptr(),
            buf.len(),
        )
    };
}

/// Undo, and apply what it handed back. What every caller does, since the
/// history reaches no state of its own.
fn undo_into(log: &Held, doc: &Doc) -> serde_json::Value {
    let reply = undo(log);
    for leg in reply["inverses"].as_array().into_iter().flatten() {
        assert_eq!(leg["structure"], log.tree);
        project(doc, leg);
    }
    reply
}

/// Redo, and apply the ordinary edits it handed back.
fn redo_into(log: &Held, doc: &Doc) -> serde_json::Value {
    let reply = redo(log);
    for leg in reply["edits"].as_array().into_iter().flatten() {
        project(doc, leg);
    }
    reply
}

/// One entry as `record` takes it: a label, and legs.
fn entry(legs: &str) -> String {
    format!(r#"{{"label":"draw","legs":[{legs}]}}"#)
}

/// Records one entry, the way a binding does.
fn record(log: &Held, request: &str) -> i32 {
    unsafe { clausters_history_record(log.handle, request.as_ptr(), request.len()) }
}

fn place(node: u64, offset: f64) -> String {
    format!(r#"{{"intent":"place","node":{node},"offset":{offset}}}"#)
}

#[test]
fn a_run_of_gestures_inverts_back_to_where_it_started() {
    // O11's acceptance. The log lives in Rust with its spill store; what
    // crosses is the document and the pointer.
    let log = Held::new();
    let doc = Doc::new(DOC);
    let start: serde_json::Value = serde_json::from_str(DOC).unwrap();

    for (node, offset) in [(2u64, 1.0), (3, 7.0), (2, 2.5), (3, 0.5)] {
        let outcome = apply(&log, &doc, &place(node, offset), 0.0);
        assert_eq!(outcome["applied"], true);
    }
    assert_eq!(unsafe { clausters_history_len(log.handle) }, 4);
    assert_ne!(doc.tree()["root"], start["root"]);

    while unsafe { clausters_history_can_undo(log.handle) } == 1 {
        undo_into(&log, &doc);
    }
    assert_eq!(
        doc.tree()["root"],
        start["root"],
        "exactly, not approximately"
    );
    assert_eq!(unsafe { clausters_history_can_redo(log.handle) }, 1);
}

#[test]
fn a_redo_puts_back_what_the_undo_took() {
    let log = Held::new();
    let doc = Doc::new(DOC);
    apply(&log, &doc, &place(2, 3.0), 0.0);
    let after_edit = doc.tree();

    let undone = undo_into(&log, &doc);
    assert_eq!(undone["inverses"].as_array().unwrap().len(), 1);

    let redone = redo_into(&log, &doc);
    assert_eq!(doc.tree()["root"], after_edit["root"]);
    assert!(
        redone["remaining"].as_array().unwrap().is_empty(),
        "nothing for the owner to re-run"
    );
}

#[test]
fn what_the_rules_did_is_what_gets_replayed() {
    // The forward half records the *effective* edit, so a redo does not snap a
    // second time. Across the ABI as inside the crate.
    let log = Held::new();
    let doc = Doc::new(DOC);
    let outcome = apply(&log, &doc, &place(2, 4.3), 1.0);
    assert_eq!(outcome["effective"]["offset"], 4.0);

    undo_into(&log, &doc);
    redo_into(&log, &doc);
    assert_eq!(doc.tree()["root"]["members"][0]["offset"], 4.0);
}

#[test]
fn a_refused_edit_leaves_no_entry_to_undo() {
    let log = Held::new();
    let doc = Doc::new(DOC);
    apply(&log, &doc, &place(99, 1.0), 0.0);
    assert_eq!(unsafe { clausters_history_len(log.handle) }, 0);
    assert_eq!(unsafe { clausters_history_can_undo(log.handle) }, 0);
}

#[test]
fn nothing_to_undo_is_answered_rather_than_failing() {
    // `{}` -- distinguishable from a parse failure (0) and from an undo that
    // legitimately changed nothing.
    let log = Held::new();
    let doc = Doc::new(DOC);
    let _ = &doc;
    assert_eq!(undo(&log), serde_json::json!({}));
    assert_eq!(redo(&log), serde_json::json!({}));
}

#[test]
fn a_destructive_inverse_is_recorded_by_the_caller_and_undone_here() {
    // The one edit the document cannot supply the inverse for: its samples are
    // not in the tree, so the caller reads the span it is about to overwrite
    // and hands the pair over.
    let document = r#"{"version":1,"root":{"id":1,"kind":"vector",
        "source":{"source":7,"lifetime":"temporary","generation":4}}}"#;
    let log = Held::new();
    let forward = r#"{"edit":{"intent":"writesamples","node":1,"start":10,"values":[0.5,0.5]}}"#;
    let backward = r#"{"intent":"writesamples","node":1,"start":10,"values":[0.125,0.25]}"#;
    let code = record(
        &log,
        &entry(&format!(
            r#"{{"structure":{},"forward":{forward},"backward":{backward}}}"#,
            log.tree
        )),
    );
    assert_eq!(code, 0);
    assert_eq!(unsafe { clausters_history_len(log.handle) }, 1);

    let _ = document;
    let undone = undo(&log);
    assert_eq!(
        undone["inverses"][0]["payload"]["values"],
        // Exactly representable in `f32`, so the assertion is about the span
        // travelling whole rather than about float printing.
        serde_json::json!([0.125, 0.25]),
        "the span the caller read before writing"
    );
}

#[test]
fn a_deterministic_operation_comes_back_for_the_owner_to_re_run() {
    // The asymmetry, across the ABI: going back is data, going forward may be a
    // recipe -- and the crate holds no algorithms, so it hands the recipe out.
    let document = r#"{"version":1,"root":{"id":1,"kind":"vector",
        "source":{"source":7,"lifetime":"temporary","generation":4}}}"#;
    let log = Held::new();
    let forward = r#"{"recompute":{"op":"normalize","peak":1.0}}"#;
    let backward = r#"{"intent":"writesamples","node":1,"start":0,"values":[0.25]}"#;
    record(
        &log,
        &entry(&format!(
            r#"{{"structure":{},"forward":{forward},"backward":{backward}}}"#,
            log.tree
        )),
    );
    let _ = document;
    undo(&log);
    let redone = redo(&log);
    assert!(
        redone["edits"].as_array().unwrap().is_empty(),
        "the operation is the first step, so nothing precedes it"
    );
    let remaining = redone["remaining"].as_array().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0]["step"]["recompute"]["op"], "normalize");
}

#[test]
fn a_continuing_run_of_adjustments_is_one_undo() {
    // The caller decides where the hand stopped, because only the caller knows.
    let document = r#"{"version":1,"root":{"id":1,"kind":"aggregate","grouping":"concrete",
        "members":[{"offset":0.0,"node":{"id":2,"kind":"clang"}}]}}"#;
    let log = Held::new();
    for (i, offset) in [1.0, 1.5, 2.0].into_iter().enumerate() {
        let forward = format!(r#"{{"edit":{}}}"#, place(2, offset));
        let previous = if i == 0 { 0.0 } else { offset - 0.5 };
        let backward = place(2, previous);
        // The key is the domain's sentence for "the same thing done the same
        // way", so it is the caller that states it -- the crate cannot read a
        // vocabulary it does not know.
        let request = format!(
            r#"{{"label":"move","coalesce":{},"legs":[
                {{"structure":{},"forward":{forward},"backward":{backward},"key":"place:2"}}]}}"#,
            i > 0,
            log.tree
        );
        record(&log, &request);
    }
    assert_eq!(
        unsafe { clausters_history_len(log.handle) },
        1,
        "one thing was done"
    );
    let doc = Doc::new(document);
    undo_into(&log, &doc);
    assert_eq!(
        doc.tree()["root"]["members"][0]["offset"],
        0.0,
        "and it lands where the run started"
    );
}

#[test]
fn the_labels_cross_for_a_menu_to_read() {
    let log = Held::new();
    apply(&log, &Doc::new(DOC), &place(2, 1.0), 0.0);
    let label = sized(|out, cap| unsafe { clausters_history_undo_label(log.handle, out, cap) });
    assert_eq!(label, "move");
    assert_eq!(
        sized(|out, cap| unsafe { clausters_history_redo_label(log.handle, out, cap) }),
        "",
        "nothing to redo yet"
    );
}

#[test]
fn clearing_forgets_everything() {
    let log = Held::new();
    apply(&log, &Doc::new(DOC), &place(2, 1.0), 0.0);
    unsafe { clausters_history_clear(log.handle) };
    assert_eq!(unsafe { clausters_history_len(log.handle) }, 0);
    assert_eq!(unsafe { clausters_history_can_undo(log.handle) }, 0);
}

#[test]
fn a_null_handle_is_answered_rather_than_a_crash() {
    // Every binding gets this wrong once, and a segfault across an FFI is the
    // least debuggable failure there is.
    let null = std::ptr::null_mut();
    assert_eq!(unsafe { clausters_history_can_undo(null) }, 0);
    assert_eq!(unsafe { clausters_history_len(null) }, 0);
    unsafe { clausters_history_clear(null) };
    unsafe { clausters_history_free(null) };
    let domain = "tree";
    assert_eq!(
        unsafe { clausters_history_register(null, domain.as_ptr(), domain.len()) },
        0,
        "no history, no identity"
    );
    let n = unsafe { clausters_history_undo(null, std::ptr::null_mut(), 0) };
    assert_eq!(n, 2, "`{{}}`: there is nothing to undo on no history");
    // And the mirror: no document either, which since O12 is the other handle
    // a caller can get wrong.
    let no_doc: *mut FfiDocument = std::ptr::null_mut();
    let intent = place(2, 1.0);
    let label = "move";
    assert_eq!(
        unsafe {
            clausters_history_apply(
                null,
                0,
                no_doc,
                intent.as_ptr(),
                intent.len(),
                std::ptr::null(),
                0,
                0.0,
                label.as_ptr(),
                label.len(),
                std::ptr::null_mut(),
                0,
            )
        },
        0
    );
}

#[test]
fn a_sizing_pass_changes_nothing_however_many_times_it_runs() {
    // The invariant the whole module is built around, and the bug that found
    // it: size-then-fill needs the payload identical on both calls, and
    // everything here mutates -- so a naive `apply` recorded two entries per
    // edit and a naive `undo` undid on the sizing call and reported "nothing to
    // undo" on the fill. The mutation happens only when the bytes are written.
    let log = Held::new();
    let doc = Doc::new(DOC);
    let before = doc.tree();
    let intent = place(2, 3.0);
    let label = "move";
    let size = |_n: usize| unsafe {
        clausters_history_apply(
            log.handle,
            log.tree,
            doc.0,
            intent.as_ptr(),
            intent.len(),
            std::ptr::null(),
            0,
            0.0,
            label.as_ptr(),
            label.len(),
            std::ptr::null_mut(),
            0,
        )
    };
    let first = size(0);
    for _ in 0..5 {
        assert_eq!(size(0), first, "a sizing pass is idempotent");
    }
    assert_eq!(
        unsafe { clausters_history_len(log.handle) },
        0,
        "and records nothing"
    );
    // With the tree behind a handle the sizing pass could edit it silently --
    // the reason a mutating call still commits only on the write, even though
    // its payload is now small enough that the second pass costs nothing.
    assert_eq!(doc.tree(), before, "and edits nothing");

    // One real call, one entry.
    apply(&log, &doc, &intent, 0.0);
    assert_eq!(unsafe { clausters_history_len(log.handle) }, 1);

    // Sizing an undo does not undo it.
    for _ in 0..3 {
        unsafe { clausters_history_undo(log.handle, std::ptr::null_mut(), 0) };
    }
    assert_eq!(
        unsafe { clausters_history_can_undo(log.handle) },
        1,
        "still there"
    );
    undo_into(&log, &doc);
    assert_eq!(
        unsafe { clausters_history_can_undo(log.handle) },
        0,
        "now it is not"
    );
}

#[test]
fn a_buffer_too_small_is_a_size_query_and_not_a_half_done_edit() {
    // The other half of the same rule: `fill` writes only if it fits, so a
    // caller that guessed too small gets the count and an untouched log rather
    // than a truncated payload and a moved cursor.
    let log = Held::new();
    let doc = Doc::new(DOC);
    apply(&log, &doc, &place(2, 1.0), 0.0);
    let mut tiny = [0u8; 4];
    let need = unsafe { clausters_history_undo(log.handle, tiny.as_mut_ptr(), tiny.len()) };
    assert!(need > tiny.len());
    assert_eq!(
        unsafe { clausters_history_can_undo(log.handle) },
        1,
        "not consumed"
    );
    assert_eq!(tiny, [0u8; 4], "and nothing was written");
}

#[test]
fn a_second_domain_shares_the_pile_and_comes_back_addressed_to_itself() {
    // O16's acceptance across the ABI. A curve is not a document, so this
    // surface cannot apply its edits -- the caller does, and hands over the
    // pair. What the history gives back is the leg with the structure on it,
    // which is all a caller needs to route it to the reader that knows the
    // vocabulary.
    let log = Held::new();
    let doc = Doc::new(DOC);
    let domain = "points";
    let curve = unsafe { clausters_history_register(log.handle, domain.as_ptr(), domain.len()) };
    assert_ne!(curve, log.tree, "two structures, two identities");

    apply(&log, &doc, &place(2, 1.0), 0.0);

    let forward = r#"{"edit":{"intent":"setpoints","points":[{"at":0.0,"value":1.0}]}}"#;
    let backward = r#"{"intent":"setpoints","points":[{"at":0.0,"value":0.0}]}"#;
    let code = record(
        &log,
        &entry(&format!(
            r#"{{"structure":{curve},"forward":{forward},"backward":{backward},"key":"points"}}"#
        )),
    );
    assert_eq!(code, 0);
    assert_eq!(unsafe { clausters_history_len(log.handle) }, 2, "one pile");

    // The order is the pile's: the curve's edit was last, so it undoes first.
    let undone = undo(&log);
    assert_eq!(undone["inverses"][0]["structure"], curve);
    assert_eq!(undone["inverses"][0]["payload"]["intent"], "setpoints");

    let undone = undo_into(&log, &doc);
    assert_eq!(undone["inverses"][0]["structure"], log.tree);
    assert_eq!(
        doc.tree()["root"]["members"][0]["offset"],
        0.0,
        "and the document is back where it started"
    );
}

#[test]
fn a_structure_another_history_minted_is_refused() {
    // The rule, across the ABI: an identity is minted by one history, and an
    // entry naming a foreign one records nothing rather than opening a second
    // order over data that already has one.
    let log = Held::new();
    let other = Held::new();
    let forward = r#"{"edit":{"intent":"setpoints","points":[]}}"#;
    let backward = r#"{"intent":"setpoints","points":[]}"#;
    let code = record(
        &log,
        &entry(&format!(
            r#"{{"structure":{},"forward":{forward},"backward":{backward}}}"#,
            other.tree
        )),
    );
    assert_eq!(code, -1);
    assert_eq!(unsafe { clausters_history_len(log.handle) }, 0);
}

#[test]
fn a_transaction_crosses_as_one_entry_with_several_legs() {
    // O17 across the ABI. The crate reaches one document and no curve, so a
    // gesture over both is applied by the caller and recorded whole -- one
    // call, because half a transaction is worse than none.
    let log = Held::new();
    let doc = Doc::new(DOC);
    let domain = "points";
    let curve = unsafe { clausters_history_register(log.handle, domain.as_ptr(), domain.len()) };

    // The document leg is applied by the caller too, so its inverse is read
    // *before* it lands -- which is what `clausters_document_inverse` is for.
    let intent = place(2, 6.0);
    let inverse = sized(|out, cap| unsafe {
        crate::clausters_document_inverse(doc.0, intent.as_ptr(), intent.len(), out, cap)
    });
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&inverse).unwrap()["offset"],
        0.0,
        "where the clip is now"
    );
    project(
        &doc,
        &serde_json::json!({ "payload": serde_json::json!({"intent":"place","node":2,"offset":6.0}) }),
    );

    let code = record(
        &log,
        &format!(
            r#"{{"label":"drag the clip and its curve","legs":[
                {{"structure":{},"forward":{{"edit":{intent}}},"backward":{inverse}}},
                {{"structure":{curve},"forward":{{"edit":{{"intent":"setpoints","points":[]}}}},
                  "backward":{{"intent":"setpoints","points":[{{"at":0.0,"value":1.0}}]}}}}]}}"#,
            log.tree
        ),
    );
    assert_eq!(code, 0);
    assert_eq!(
        unsafe { clausters_history_len(log.handle) },
        1,
        "one gesture"
    );

    // And it comes back as one step, inverted in reverse: the curve first.
    let undone = undo(&log);
    let legs = undone["inverses"].as_array().unwrap();
    assert_eq!(legs.len(), 2);
    assert_eq!(legs[0]["structure"], curve);
    assert_eq!(legs[1]["structure"], log.tree);
    assert_eq!(legs[1]["payload"]["offset"], 0.0);
    assert_eq!(
        unsafe { clausters_history_can_undo(log.handle) },
        0,
        "one step"
    );
}

#[test]
fn an_entry_with_no_leg_is_not_a_gesture() {
    let log = Held::new();
    assert_eq!(record(&log, r#"{"label":"nothing","legs":[]}"#), -1);
    assert_eq!(record(&log, "not json"), -1);
    assert_eq!(unsafe { clausters_history_len(log.handle) }, 0);
}

#[test]
fn an_inverse_the_document_cannot_describe_is_answered_with_zero() {
    // A node that is gone: there is nothing to state, and the caller learns it
    // before it edits rather than by recording something wrong.
    let doc = Doc::new(DOC);
    let intent = place(99, 1.0);
    let n = unsafe {
        crate::clausters_document_inverse(
            doc.0,
            intent.as_ptr(),
            intent.len(),
            std::ptr::null_mut(),
            0,
        )
    };
    assert_eq!(n, 0);
}
