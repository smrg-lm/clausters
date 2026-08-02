//! A component bundle's requirements, one instance's resolution, and the writers' pre-flight.

// ---- the component bundle ----
//
// The same three calls the browser gets over wasm, opened to a native binding
// (the Python writer's pre-flight first). The pass is
// `clausters_core::bundle`; these are only the JSON boundary, and they follow
// `clausters_core_patch_compile`'s convention exactly — JSON in, JSON out into
// a caller buffer, returning the size it needs, with an error coming back *as*
// the output object so a caller reads one channel.

/// Copies `json` into `out` when it fits, returning the size it needs.
fn write_json(json: Vec<u8>, out: *mut u8, out_cap: usize) -> usize {
    let n = json.len();
    if !out.is_null() && out_cap >= n {
        // SAFETY: out is non-null and writable for out_cap >= n bytes.
        unsafe { std::ptr::copy_nonoverlapping(json.as_ptr(), out, n) };
    }
    n
}

/// What one instance of a bundle needs allocated: a
/// [`RequirementsRequest`](clausters_core::bundle::RequirementsRequest) as JSON
/// in (the `bundle.json` manifest, plus the template when the caller has it —
/// a bundle written before the contract has its id block measured from it), the
/// [`Requirements`](clausters_core::bundle::Requirements) as JSON out. `0` when
/// the input is not a readable request.
///
/// # Safety
/// `request` must be readable for `request_len` bytes and `out` writable for
/// `out_cap` bytes (or null, to size only).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_bundle_requirements(
    request: *const u8,
    request_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    use clausters_core::bundle::{RequirementsRequest, requirements_request};

    if request.is_null() {
        return 0;
    }
    // SAFETY: caller guarantees `request` is readable for `request_len` bytes.
    let bytes = unsafe { std::slice::from_raw_parts(request, request_len) };
    let Ok(request) = serde_json::from_slice::<RequirementsRequest>(bytes) else {
        return 0;
    };
    write_json(
        serde_json::to_vec(&requirements_request(&request)).unwrap_or_default(),
        out,
        out_cap,
    )
}

/// One mounted instance: a
/// [`ResolveRequest`](clausters_core::bundle::ResolveRequest) as JSON in (the
/// manifest, the template, the allocation and the supplied parameters), the
/// [`Resolved`](clausters_core::bundle::Resolved) tree and boot list as JSON
/// out. A resolution error comes back as `{"error": …}`; `0` means the input
/// was not a readable request.
///
/// # Safety
/// `request` must be readable for `request_len` bytes and `out` writable for
/// `out_cap` bytes (or null, to size only).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_bundle_resolve(
    request: *const u8,
    request_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    use clausters_core::bundle::{ResolveRequest, resolve_request};

    if request.is_null() {
        return 0;
    }
    // SAFETY: caller guarantees `request` is readable for `request_len` bytes.
    let bytes = unsafe { std::slice::from_raw_parts(request, request_len) };
    let Ok(request) = serde_json::from_slice::<ResolveRequest>(bytes) else {
        return 0;
    };
    let json = match resolve_request(&request) {
        Ok(resolved) => serde_json::to_value(&resolved).unwrap_or_default(),
        Err(e) => serde_json::json!({ "error": e.to_string() }),
    };
    write_json(serde_json::to_vec(&json).unwrap_or_default(), out, out_cap)
}

/// The writers' pre-flight: a
/// [`ValidateRequest`](clausters_core::bundle::ValidateRequest) as JSON in (the
/// manifest, the template, and the def payloads to check for holes), and either
/// `{"ok":true}` or `{"error": …}` out — so a bundle that would fail to mount
/// fails to be written. `0` means the input was not a readable request.
///
/// # Safety
/// `request` must be readable for `request_len` bytes and `out` writable for
/// `out_cap` bytes (or null, to size only).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_bundle_validate(
    request: *const u8,
    request_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    use clausters_core::bundle::{ValidateRequest, validate_request};

    if request.is_null() {
        return 0;
    }
    // SAFETY: caller guarantees `request` is readable for `request_len` bytes.
    let bytes = unsafe { std::slice::from_raw_parts(request, request_len) };
    let Ok(request) = serde_json::from_slice::<ValidateRequest>(bytes) else {
        return 0;
    };
    let json = match validate_request(&request) {
        Ok(()) => serde_json::json!({ "ok": true }),
        Err(e) => serde_json::json!({ "error": e.to_string() }),
    };
    write_json(serde_json::to_vec(&json).unwrap_or_default(), out, out_cap)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Size query, then fill, over one of the three bundle calls.
    fn bundle_json(
        call: unsafe extern "C" fn(*const u8, usize, *mut u8, usize) -> usize,
        input: &str,
    ) -> (usize, serde_json::Value) {
        let b = input.as_bytes();
        let need = unsafe { call(b.as_ptr(), b.len(), std::ptr::null_mut(), 0) };
        if need == 0 {
            return (0, serde_json::Value::Null);
        }
        let mut buf = vec![0u8; need];
        let n = unsafe { call(b.as_ptr(), b.len(), buf.as_mut_ptr(), buf.len()) };
        assert_eq!(n, need, "the fill matches the size query");
        (need, serde_json::from_slice(&buf).unwrap())
    }

    const MANIFEST: &str = r#"{"gui":"fm","widgets":2,
        "symbols":{"buses":[{"name":"lfo","rate":"control","channels":1}]},
        "params":{"freq":{"type":"float","default":220.0,"max":700.0}}}"#;
    const TEMPLATE: &str = r#"{"id":1,"gui":{"type":"window",
        "children":[{"id":2,"type":"meter","bus":"@lfo","value":"$freq"}]}}"#;

    #[test]
    fn bundle_requirements_read_the_manifest() {
        let request = format!(r#"{{"manifest":{MANIFEST}}}"#);
        let (need, v) = bundle_json(clausters_core_bundle_requirements, &request);
        assert!(need > 0);
        assert_eq!(v["widgets"], 2);
        assert_eq!(v["buses"][0]["name"], "lfo");
    }

    /// A pre-contract manifest declares no count; the template it is handed
    /// sizes the id block, so two instances cannot overlap.
    #[test]
    fn bundle_requirements_measure_an_undeclared_block() {
        let request = r#"{"manifest":{"gui":"piano"},"template":{"id":1,"gui":
            {"type":"window","children":[{"id":20,"type":"meter","bus":0}]}}}"#;
        let (_need, v) = bundle_json(clausters_core_bundle_requirements, request);
        assert_eq!(v["widgets"], 20);
    }

    #[test]
    fn bundle_resolve_fills_one_instance() {
        let request = format!(
            r#"{{"manifest":{MANIFEST},"template":{TEMPLATE},
                 "allocation":{{"widget_base":50,"buses":{{"lfo":9}}}},
                 "params":{{"attributes":{{"freq":"440"}}}}}}"#
        );
        let (_need, v) = bundle_json(clausters_core_bundle_resolve, &request);
        assert_eq!(v["def_id"], 50);
        assert_eq!(v["tree"]["children"][0]["id"], 51);
        assert_eq!(v["tree"]["children"][0]["bus"], 9);
        assert_eq!(v["tree"]["children"][0]["value"], 440.0);
    }

    /// A resolution error comes back on the one channel, as `{"error": …}`.
    #[test]
    fn bundle_resolve_reports_a_bad_value_as_an_error_object() {
        let request = format!(
            r#"{{"manifest":{MANIFEST},"template":{TEMPLATE},
                 "allocation":{{"widget_base":50,"buses":{{"lfo":9}}}},
                 "params":{{"attributes":{{"freq":9000}}}}}}"#
        );
        let (_need, v) = bundle_json(clausters_core_bundle_resolve, &request);
        assert!(v["error"].as_str().unwrap().contains("outside"));
    }

    /// The writers' pre-flight: the dry run passes, and a hole baked into a def
    /// payload is refused.
    #[test]
    fn bundle_validate_is_the_writers_pre_flight() {
        let ok = format!(r#"{{"manifest":{MANIFEST},"template":{TEMPLATE}}}"#);
        let (_need, v) = bundle_json(clausters_core_bundle_validate, &ok);
        assert_eq!(v["ok"], true);

        let baked = format!(
            r#"{{"manifest":{MANIFEST},"template":{TEMPLATE},
                 "defs":[{{"name":"voice","ugens":[{{"inputs":["@lfo"]}}]}}]}}"#
        );
        let (_need, v) = bundle_json(clausters_core_bundle_validate, &baked);
        assert!(v["error"].as_str().unwrap().contains("@lfo"));
    }

    #[test]
    fn bundle_calls_reject_malformed_input_with_zero() {
        let bad = b"not a manifest";
        let n = unsafe {
            clausters_core_bundle_requirements(bad.as_ptr(), bad.len(), std::ptr::null_mut(), 0)
        };
        assert_eq!(n, 0);
    }
}
