//! What a window's message is as the applications read it.

/// An OSC atom as the JSON a reading, or an editor's turn, takes.
///
/// The wire's framing is the host's and the reading is the crate's, so this is
/// where the one becomes the other -- the same line each client draws for its own
/// transport.
#[cfg(any(not(target_arch = "wasm32"), feature = "apps", feature = "notation"))]
pub(crate) fn atom(value: &clausters_core::osc::OscType) -> serde_json::Value {
    use clausters_core::osc::OscType;
    match value {
        OscType::String(s) => serde_json::Value::String(s.clone()),
        OscType::Float(v) => serde_json::json!(f64::from(*v)),
        OscType::Double(v) => serde_json::json!(*v),
        OscType::Int(v) => serde_json::json!(*v),
        OscType::Long(v) => serde_json::json!(*v),
        OscType::Bool(v) => serde_json::json!(*v),
        _ => serde_json::Value::Null,
    }
}
