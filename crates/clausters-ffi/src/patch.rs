//! The patcher's cord-to-bus compile: a patch JSON in, its GraphDef wiring out.

/// The GUI patcher's **cord → bus pass**: a directed
/// [`Patch`](clausters_core::patch::Patch) as JSON in (`patch`/`patch_len`), its
/// [`Compiled`](clausters_core::patch::Compiled) bus wiring as JSON written to
/// `out` (capacity `out_cap`). Returns the number of bytes the output JSON needs
/// — written iff it fit, so a caller sizes with a first call (a null/small `out`)
/// then fills — or `0` when the input is not readable JSON for a `Patch`.
///
/// A **compile** error (a malformed cord: reversed, mismatched rate, out of
/// range) is not a `0`: it comes back *as* the output JSON, the object
/// `{"error": "<message>"}`, so the caller reads one channel. Success is the
/// `Compiled` object (`{"buses": …, "members": …}`).
///
/// # Safety
/// `patch` must be readable for `patch_len` bytes and `out` writable for
/// `out_cap` bytes (or null, to size only).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_patch_compile(
    patch: *const u8,
    patch_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    use clausters_core::patch::{Patch, compile};

    if patch.is_null() {
        return 0;
    }
    // SAFETY: caller guarantees `patch` is readable for `patch_len` bytes.
    let bytes = unsafe { std::slice::from_raw_parts(patch, patch_len) };
    let Ok(p) = serde_json::from_slice::<Patch>(bytes) else {
        return 0;
    };
    let json = match compile(&p) {
        Ok(c) => serde_json::to_vec(&c).unwrap_or_default(),
        Err(e) => serde_json::to_vec(&serde_json::json!({ "error": e })).unwrap_or_default(),
    };
    let n = json.len();
    if !out.is_null() && out_cap >= n {
        // SAFETY: out is writable for out_cap >= n bytes.
        unsafe { std::ptr::copy_nonoverlapping(json.as_ptr(), out, n) };
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile_json(patch: &str) -> (usize, serde_json::Value) {
        let b = patch.as_bytes();
        // Size query first (null out), then fill.
        let need =
            unsafe { clausters_core_patch_compile(b.as_ptr(), b.len(), std::ptr::null_mut(), 0) };
        if need == 0 {
            return (0, serde_json::Value::Null);
        }
        let mut buf = vec![0u8; need];
        let n = unsafe {
            clausters_core_patch_compile(b.as_ptr(), b.len(), buf.as_mut_ptr(), buf.len())
        };
        assert_eq!(n, need, "the fill matches the size query");
        (need, serde_json::from_slice(&buf).unwrap())
    }

    #[test]
    fn patch_compile_wires_a_chain() {
        // tone -> dac (dac is a terminal sink: an inlet, no outlet).
        let (need, v) = compile_json(
            r#"{"boxes":[
                {"def":"tone","ports":[{"name":"out","dir":"out","rate":"audio"}]},
                {"def":"dac","ports":[{"name":"in","dir":"in","rate":"audio"}]}],
              "cords":[{"from_box":0,"from_port":0,"to_box":1,"to_port":0}]}"#,
        );
        assert!(need > 0);
        assert_eq!(
            v["buses"].as_array().unwrap().len(),
            1,
            "one private bus (tone->dac)"
        );
        let members = v["members"].as_array().unwrap();
        assert_eq!(members.len(), 2);
        assert_eq!(members[0]["controls"][0]["bus"], "b0"); // tone.out
        assert_eq!(members[1]["controls"][0]["bus"], "b0"); // dac.in
    }

    #[test]
    fn patch_compile_reports_a_bad_cord_as_an_error_object() {
        // A reversed cord (inlet -> outlet) comes back as an error channel, not 0.
        let (_need, v) = compile_json(
            r#"{"boxes":[
                {"def":"tone","ports":[{"name":"out","dir":"out","rate":"audio"}]},
                {"def":"dac","ports":[{"name":"in","dir":"in","rate":"audio"}]}],
              "cords":[{"from_box":1,"from_port":0,"to_box":0,"to_port":0}]}"#,
        );
        assert!(v["error"].is_string());
    }

    #[test]
    fn patch_compile_rejects_malformed_input_with_zero() {
        let bad = b"not a patch";
        let n = unsafe {
            clausters_core_patch_compile(bad.as_ptr(), bad.len(), std::ptr::null_mut(), 0)
        };
        assert_eq!(n, 0);
    }
}
