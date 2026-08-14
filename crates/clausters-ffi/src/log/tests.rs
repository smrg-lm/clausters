//! O11's acceptance: a run of gestures applied across the ABI inverts back to
//! the starting document exactly, through the crate's log rather than one the
//! caller keeps.

use super::*;

const DOC: &str = r#"{"version":1,"root":{"id":1,"kind":"set","grouping":"concrete",
    "members":[
      {"offset":0.0,"node":{"id":2,"kind":"event"}},
      {"offset":4.0,"node":{"id":3,"kind":"event"}}
    ]}}"#;

/// A handle that frees itself, so a failing assertion does not leak.
struct Held(*mut FfiLog);

impl Held {
    fn new() -> Self {
        Self(clausters_log_new(0, 0))
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        unsafe { clausters_log_free(self.0) };
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

fn apply(log: &Held, document: &str, intent: &str, quant: f64) -> String {
    let label = "move";
    sized(|out, cap| unsafe {
        clausters_log_apply(
            log.0,
            document.as_ptr(),
            document.len(),
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
    })
}

fn undo(log: &Held, document: &str) -> serde_json::Value {
    serde_json::from_str(&sized(|out, cap| unsafe {
        clausters_log_undo(log.0, document.as_ptr(), document.len(), out, cap)
    }))
    .unwrap()
}

fn redo(log: &Held, document: &str) -> serde_json::Value {
    serde_json::from_str(&sized(|out, cap| unsafe {
        clausters_log_redo(log.0, document.as_ptr(), document.len(), out, cap)
    }))
    .unwrap()
}

fn place(node: u64, offset: f64) -> String {
    format!(r#"{{"intent":"place","node":{node},"offset":{offset}}}"#)
}

#[test]
fn a_run_of_gestures_inverts_back_to_where_it_started() {
    // O11's acceptance. The log lives in Rust with its spill store; what
    // crosses is the document and the pointer.
    let log = Held::new();
    let start: serde_json::Value = serde_json::from_str(DOC).unwrap();
    let mut document = DOC.to_string();

    for (node, offset) in [(2u64, 1.0), (3, 7.0), (2, 2.5), (3, 0.5)] {
        let result: serde_json::Value =
            serde_json::from_str(&apply(&log, &document, &place(node, offset), 0.0)).unwrap();
        assert_eq!(result["outcome"]["applied"], true);
        document = result["document"].to_string();
    }
    assert_eq!(unsafe { clausters_log_len(log.0) }, 4);
    assert_ne!(
        serde_json::from_str::<serde_json::Value>(&document).unwrap()["root"],
        start["root"]
    );

    while unsafe { clausters_log_can_undo(log.0) } == 1 {
        let result = undo(&log, &document);
        document = result["document"].to_string();
    }
    let ended: serde_json::Value = serde_json::from_str(&document).unwrap();
    assert_eq!(ended["root"], start["root"], "exactly, not approximately");
    assert_eq!(unsafe { clausters_log_can_redo(log.0) }, 1);
}

#[test]
fn a_redo_puts_back_what_the_undo_took() {
    let log = Held::new();
    let applied: serde_json::Value =
        serde_json::from_str(&apply(&log, DOC, &place(2, 3.0), 0.0)).unwrap();
    let after_edit = applied["document"].clone();

    let undone = undo(&log, &after_edit.to_string());
    assert_eq!(undone["undone"].as_array().unwrap().len(), 1);

    let redone = redo(&log, &undone["document"].to_string());
    assert_eq!(redone["document"]["root"], after_edit["root"]);
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
    let applied: serde_json::Value =
        serde_json::from_str(&apply(&log, DOC, &place(2, 4.3), 1.0)).unwrap();
    assert_eq!(applied["outcome"]["effective"]["offset"], 4.0);

    let undone = undo(&log, &applied["document"].to_string());
    let redone = redo(&log, &undone["document"].to_string());
    assert_eq!(redone["document"]["root"]["members"][0]["offset"], 4.0);
}

#[test]
fn a_refused_edit_leaves_no_entry_to_undo() {
    let log = Held::new();
    apply(&log, DOC, &place(99, 1.0), 0.0);
    assert_eq!(unsafe { clausters_log_len(log.0) }, 0);
    assert_eq!(unsafe { clausters_log_can_undo(log.0) }, 0);
}

#[test]
fn nothing_to_undo_is_answered_rather_than_failing() {
    // `{}` -- distinguishable from a parse failure (0) and from an undo that
    // legitimately changed nothing.
    let log = Held::new();
    let result = undo(&log, DOC);
    assert_eq!(result, serde_json::json!({}));
    assert_eq!(redo(&log, DOC), serde_json::json!({}));
}

#[test]
fn a_destructive_inverse_is_recorded_by_the_caller_and_undone_here() {
    // The one edit the document cannot supply the inverse for: its samples are
    // not in the tree, so the caller reads the span it is about to overwrite
    // and hands the pair over.
    let document = r#"{"version":1,"root":{"id":1,"kind":"buffer",
        "source":{"source":7,"lifetime":"temporary","generation":4}}}"#;
    let log = Held::new();
    let forward = r#"{"edit":{"intent":"writesamples","node":1,"start":10,"values":[0.5,0.5]}}"#;
    let backward = r#"{"intent":"writesamples","node":1,"start":10,"values":[0.125,0.25]}"#;
    let label = "draw";
    let code = unsafe {
        clausters_log_record(
            log.0,
            forward.as_ptr(),
            forward.len(),
            backward.as_ptr(),
            backward.len(),
            label.as_ptr(),
            label.len(),
            0,
        )
    };
    assert_eq!(code, 0);
    assert_eq!(unsafe { clausters_log_len(log.0) }, 1);

    let undone = undo(&log, document);
    assert_eq!(
        undone["undone"][0]["values"],
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
    let document = r#"{"version":1,"root":{"id":1,"kind":"buffer",
        "source":{"source":7,"lifetime":"temporary","generation":4}}}"#;
    let log = Held::new();
    let forward = r#"{"recompute":{"op":"normalize","peak":1.0}}"#;
    let backward = r#"{"intent":"writesamples","node":1,"start":0,"values":[0.25]}"#;
    let label = "normalize";
    unsafe {
        clausters_log_record(
            log.0,
            forward.as_ptr(),
            forward.len(),
            backward.as_ptr(),
            backward.len(),
            label.as_ptr(),
            label.len(),
            0,
        )
    };
    let undone = undo(&log, document);
    let redone = redo(&log, &undone["document"].to_string());
    let remaining = redone["remaining"].as_array().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0]["recompute"]["op"], "normalize");
}

#[test]
fn a_continuing_run_of_adjustments_is_one_undo() {
    // The caller decides where the hand stopped, because only the caller knows.
    let document = r#"{"version":1,"root":{"id":1,"kind":"set","grouping":"concrete",
        "members":[{"offset":0.0,"node":{"id":2,"kind":"event"}}]}}"#;
    let log = Held::new();
    for (i, offset) in [1.0, 1.5, 2.0].into_iter().enumerate() {
        let forward = format!(r#"{{"edit":{}}}"#, place(2, offset));
        let previous = if i == 0 { 0.0 } else { offset - 0.5 };
        let backward = place(2, previous);
        let label = "move";
        unsafe {
            clausters_log_record(
                log.0,
                forward.as_ptr(),
                forward.len(),
                backward.as_ptr(),
                backward.len(),
                label.as_ptr(),
                label.len(),
                i32::from(i > 0),
            )
        };
    }
    assert_eq!(unsafe { clausters_log_len(log.0) }, 1, "one thing was done");
    let undone = undo(&log, document);
    assert_eq!(
        undone["document"]["root"]["members"][0]["offset"], 0.0,
        "and it lands where the run started"
    );
}

#[test]
fn the_labels_cross_for_a_menu_to_read() {
    let log = Held::new();
    apply(&log, DOC, &place(2, 1.0), 0.0);
    let label = sized(|out, cap| unsafe { clausters_log_undo_label(log.0, out, cap) });
    assert_eq!(label, "move");
    assert_eq!(
        sized(|out, cap| unsafe { clausters_log_redo_label(log.0, out, cap) }),
        "",
        "nothing to redo yet"
    );
}

#[test]
fn clearing_forgets_everything() {
    let log = Held::new();
    apply(&log, DOC, &place(2, 1.0), 0.0);
    unsafe { clausters_log_clear(log.0) };
    assert_eq!(unsafe { clausters_log_len(log.0) }, 0);
    assert_eq!(unsafe { clausters_log_can_undo(log.0) }, 0);
}

#[test]
fn a_null_handle_is_answered_rather_than_a_crash() {
    // Every binding gets this wrong once, and a segfault across an FFI is the
    // least debuggable failure there is.
    let null = std::ptr::null_mut();
    assert_eq!(unsafe { clausters_log_can_undo(null) }, 0);
    assert_eq!(unsafe { clausters_log_len(null) }, 0);
    unsafe { clausters_log_clear(null) };
    unsafe { clausters_log_free(null) };
    let n = unsafe { clausters_log_undo(null, DOC.as_ptr(), DOC.len(), std::ptr::null_mut(), 0) };
    assert_eq!(n, 2, "`{{}}`: there is nothing to undo on no log");
}

#[test]
fn a_sizing_pass_changes_nothing_however_many_times_it_runs() {
    // The invariant the whole module is built around, and the bug that found
    // it: size-then-fill needs the payload identical on both calls, and
    // everything here mutates -- so a naive `apply` recorded two entries per
    // edit and a naive `undo` undid on the sizing call and reported "nothing to
    // undo" on the fill. The mutation happens only when the bytes are written.
    let log = Held::new();
    let intent = place(2, 3.0);
    let label = "move";
    let size = |_n: usize| unsafe {
        clausters_log_apply(
            log.0,
            DOC.as_ptr(),
            DOC.len(),
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
        unsafe { clausters_log_len(log.0) },
        0,
        "and records nothing"
    );

    // One real call, one entry.
    apply(&log, DOC, &intent, 0.0);
    assert_eq!(unsafe { clausters_log_len(log.0) }, 1);

    // Sizing an undo does not undo it.
    let doc = DOC.to_string();
    for _ in 0..3 {
        unsafe { clausters_log_undo(log.0, doc.as_ptr(), doc.len(), std::ptr::null_mut(), 0) };
    }
    assert_eq!(unsafe { clausters_log_can_undo(log.0) }, 1, "still there");
    undo(&log, &doc);
    assert_eq!(unsafe { clausters_log_can_undo(log.0) }, 0, "now it is not");
}

#[test]
fn a_buffer_too_small_is_a_size_query_and_not_a_half_done_edit() {
    // The other half of the same rule: `fill` writes only if it fits, so a
    // caller that guessed too small gets the count and an untouched log rather
    // than a truncated payload and a moved cursor.
    let log = Held::new();
    apply(&log, DOC, &place(2, 1.0), 0.0);
    let doc = DOC.to_string();
    let mut tiny = [0u8; 4];
    let need = unsafe {
        clausters_log_undo(
            log.0,
            doc.as_ptr(),
            doc.len(),
            tiny.as_mut_ptr(),
            tiny.len(),
        )
    };
    assert!(need > tiny.len());
    assert_eq!(unsafe { clausters_log_can_undo(log.0) }, 1, "not consumed");
    assert_eq!(tiny, [0u8; 4], "and nothing was written");
}
