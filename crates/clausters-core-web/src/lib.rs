//! The shared core's JS door (the W track): a thin wasm-bindgen shell over
//! `clausters-core`, the browser sibling of `clausters-ffi`'s C door and the
//! core twin of `crates/clausters-web` (the whole-server shell).
//!
//! W0 exposes the OSC codec: the web client encodes and decodes through the
//! same `clausters_core::osc` the server and every other client use, so the
//! bytes are identical by construction (the parity vectors in
//! `clients/web/tests/` hold this against the Python client). Later W
//! milestones grow this surface (TempoClock queue, timetag assembly,
//! builtins) — always the same shape: the logic lives in `clausters-core`,
//! natively tested; this shell only converts values at the JS boundary, and
//! only on the wasm target.
//!
//! The JS argument convention mirrors the tagged pairs the interim page codec
//! used: encode takes `[tag, value]` pairs (`"i"`/`"h"`/`"f"`/`"d"`/`"s"`/
//! `"b"`), preserving the int/float distinction explicitly; decode returns
//! plain values (`{addr, args}` per message, bundles flattened), which is
//! what reply consumers want.

use clausters_core::osc::{OscMessage, OscPacket, OscType, decode_packet, encode};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// Encodes one OSC message from already-typed args (the native face; the wasm
/// face converts JS tagged pairs into `OscType` and calls this).
pub fn encode_message(addr: &str, args: Vec<OscType>) -> Result<Vec<u8>, String> {
    encode(&OscPacket::Message(OscMessage {
        addr: addr.to_string(),
        args,
    }))
    .map_err(|e| e.to_string())
}

/// Decodes one packet into its messages, in order, bundles flattened
/// recursively (replies are immediate; timetag scheduling is the server's
/// business, not the reply reader's).
pub fn decode_messages(bytes: &[u8]) -> Result<Vec<OscMessage>, String> {
    fn walk(packet: OscPacket, out: &mut Vec<OscMessage>) {
        match packet {
            OscPacket::Message(m) => out.push(m),
            OscPacket::Bundle(b) => {
                for inner in b.content {
                    walk(inner, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(decode_packet(bytes)?, &mut out);
    Ok(out)
}

/// JS face: `osc_encode_message(addr, [[tag, value], ...]) -> Uint8Array`.
/// Tags: `"i"` int32, `"h"` int64 (number or BigInt), `"f"` float32, `"d"`
/// float64, `"s"` string, `"b"` blob (`Uint8Array`).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn osc_encode_message(addr: &str, args: js_sys::Array) -> Result<Vec<u8>, JsError> {
    let mut typed = Vec::with_capacity(args.length() as usize);
    for entry in args.iter() {
        let pair = js_sys::Array::from(&entry);
        let tag = pair
            .get(0)
            .as_string()
            .ok_or_else(|| JsError::new("osc arg: [tag, value] expected"))?;
        let value = pair.get(1);
        typed.push(js_arg(&tag, value)?);
    }
    encode_message(addr, typed).map_err(|e| JsError::new(&e))
}

#[cfg(target_arch = "wasm32")]
fn js_arg(tag: &str, value: JsValue) -> Result<OscType, JsError> {
    let num = |v: &JsValue| {
        v.as_f64()
            .ok_or_else(|| JsError::new(&format!("osc arg '{tag}': number expected")))
    };
    Ok(match tag {
        "i" => OscType::Int(num(&value)? as i32),
        "h" => {
            // An int64 arrives as a BigInt (exact) or a plain number.
            if let Ok(big) = js_sys::BigInt::new(&value) {
                let as_str = String::from(big.to_string(10).map_err(|_| {
                    JsError::new("osc arg 'h': BigInt failed to render in base 10")
                })?);
                OscType::Long(
                    as_str
                        .parse::<i64>()
                        .map_err(|_| JsError::new("osc arg 'h': out of i64 range"))?,
                )
            } else {
                OscType::Long(num(&value)? as i64)
            }
        }
        "f" => OscType::Float(num(&value)? as f32),
        "d" => OscType::Double(num(&value)?),
        "s" => OscType::String(
            value
                .as_string()
                .ok_or_else(|| JsError::new("osc arg 's': string expected"))?,
        ),
        "b" => OscType::Blob(js_sys::Uint8Array::new(&value).to_vec()),
        other => return Err(JsError::new(&format!("osc arg: unsupported tag '{other}'"))),
    })
}

/// JS face: `osc_decode_packet(bytes) -> [{addr, args}, ...]`, bundles
/// flattened, args as plain JS values (numbers/strings/`Uint8Array`/bool/
/// null).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn osc_decode_packet(bytes: &[u8]) -> Result<js_sys::Array, JsError> {
    let messages = decode_messages(bytes).map_err(|e| JsError::new(&e))?;
    let out = js_sys::Array::new();
    for msg in messages {
        let args = js_sys::Array::new();
        for arg in msg.args {
            args.push(&js_value(arg));
        }
        let entry = js_sys::Object::new();
        js_sys::Reflect::set(&entry, &"addr".into(), &msg.addr.into())
            .map_err(|_| JsError::new("osc decode: cannot build the message object"))?;
        js_sys::Reflect::set(&entry, &"args".into(), &args)
            .map_err(|_| JsError::new("osc decode: cannot build the message object"))?;
        out.push(&entry);
    }
    Ok(out)
}

#[cfg(target_arch = "wasm32")]
fn js_value(arg: OscType) -> JsValue {
    match arg {
        OscType::Int(v) => JsValue::from_f64(v as f64),
        OscType::Long(v) => JsValue::from_f64(v as f64), // exact through 2^53
        OscType::Float(v) => JsValue::from_f64(v as f64),
        OscType::Double(v) => JsValue::from_f64(v),
        OscType::String(s) => JsValue::from_str(&s),
        OscType::Blob(b) => js_sys::Uint8Array::from(b.as_slice()).into(),
        OscType::Bool(b) => JsValue::from_bool(b),
        OscType::Nil => JsValue::NULL,
        OscType::Time(t) => {
            JsValue::from_f64(t.seconds as f64 + t.fractional as f64 / (1u64 << 32) as f64)
        }
        // The remaining rosc types (char, color, midi, arrays, inf) do not
        // appear on this server's wire; surface them as null rather than
        // failing the whole packet.
        _ => JsValue::NULL,
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use clausters_core::osc::{OscBundle, OscTime};

    /// The shell encodes through the shared core: a known message round-trips
    /// and its bytes match the OSC 1.0 layout by hand.
    #[test]
    fn encode_matches_the_wire_layout() {
        let bytes = encode_message(
            "/s_new",
            vec![
                OscType::String("default".into()),
                OscType::Int(1000),
                OscType::Float(440.0),
            ],
        )
        .unwrap();
        assert_eq!(bytes.len() % 4, 0);
        assert!(bytes.starts_with(b"/s_new\0\0,sif\0\0\0\0"));
        let msgs = decode_messages(&bytes).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].addr, "/s_new");
        assert_eq!(msgs[0].args[1], OscType::Int(1000));
    }

    /// Bundles flatten recursively, in order.
    #[test]
    fn bundles_flatten_in_order() {
        let msg = |addr: &str| {
            OscPacket::Message(OscMessage {
                addr: addr.into(),
                args: vec![],
            })
        };
        let inner = OscPacket::Bundle(OscBundle {
            timetag: OscTime {
                seconds: 0,
                fractional: 0,
            },
            content: vec![msg("/b")],
        });
        let outer = OscPacket::Bundle(OscBundle {
            timetag: OscTime {
                seconds: 0,
                fractional: 0,
            },
            content: vec![msg("/a"), inner, msg("/c")],
        });
        let bytes = encode(&outer).unwrap();
        let addrs: Vec<String> = decode_messages(&bytes)
            .unwrap()
            .into_iter()
            .map(|m| m.addr)
            .collect();
        assert_eq!(addrs, ["/a", "/b", "/c"]);
    }
}
