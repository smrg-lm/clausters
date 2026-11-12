//! The OSC codec: a message encoded from JS tagged pairs, a packet decoded
//! into plain values, and a bundle assembled with its timetag -- through the
//! same `clausters_core::osc` the server and every client encode with.

#[cfg(target_arch = "wasm32")]
use clausters_core::osc;
use clausters_core::osc::OscMessage;
use clausters_core::osc::OscPacket;
use clausters_core::osc::OscType;
use clausters_core::osc::decode_packet;
use clausters_core::osc::encode;
use clausters_core::osc::ntp_to_unix;
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
    encode_message(addr, js_args(args)?).map_err(|e| JsError::new(&e))
}

/// A JS array of `[tag, value]` pairs -> typed OSC arguments.
#[cfg(target_arch = "wasm32")]
fn js_args(args: js_sys::Array) -> Result<Vec<OscType>, JsError> {
    let mut typed = Vec::with_capacity(args.length() as usize);
    for entry in args.iter() {
        let pair = js_sys::Array::from(&entry);
        let tag = pair
            .get(0)
            .as_string()
            .ok_or_else(|| JsError::new("osc arg: [tag, value] expected"))?;
        typed.push(js_arg(&tag, pair.get(1))?);
    }
    Ok(typed)
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

/// Decodes one packet into `(message, time)` pairs, in order, bundles
/// flattened recursively and each message carrying **the time of the bundle
/// that contained it**: a timetag as Unix seconds, `None` for the immediate
/// timetag and for a bare message. A nested bundle's messages carry the
/// innermost timetag.
///
/// The same rule the Python client's receiving door applies, so a responder
/// sees the same `time` in both clients.
pub fn decode_messages_timed(bytes: &[u8]) -> Result<Vec<(OscMessage, Option<f64>)>, String> {
    fn walk(packet: OscPacket, time: Option<f64>, out: &mut Vec<(OscMessage, Option<f64>)>) {
        match packet {
            OscPacket::Message(m) => out.push((m, time)),
            OscPacket::Bundle(b) => {
                // The immediate timetag {0, 1} is "now", not an instant.
                let inner = if b.timetag.seconds == 0 && b.timetag.fractional == 1 {
                    None
                } else {
                    Some(ntp_to_unix(b.timetag))
                };
                for packet in b.content {
                    walk(packet, inner, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(decode_packet(bytes)?, None, &mut out);
    Ok(out)
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
        out.push(&js_decoded_message(msg, None)?.into());
    }
    Ok(out)
}

/// JS face: `osc_decode_packet_timed(bytes) -> [{addr, args, time}, ...]` --
/// [`osc_decode_packet`] plus the containing bundle's time, in Unix seconds
/// (`null` for an immediate bundle or a bare message). What the responder
/// layer reads, so a handler is given the same `time` the Python client hands
/// its own.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn osc_decode_packet_timed(bytes: &[u8]) -> Result<js_sys::Array, JsError> {
    let messages = decode_messages_timed(bytes).map_err(|e| JsError::new(&e))?;
    let out = js_sys::Array::new();
    for (msg, time) in messages {
        out.push(&js_decoded_message(msg, Some(time))?.into());
    }
    Ok(out)
}

/// One decoded message as `{addr, args}`, plus `time` when the caller asks for
/// the timed shape (`Some(None)` writes a `null` -- the field is there either
/// way, so a reader never has to test for its presence).
#[cfg(target_arch = "wasm32")]
fn js_decoded_message(
    msg: OscMessage,
    time: Option<Option<f64>>,
) -> Result<js_sys::Object, JsError> {
    let args = js_sys::Array::new();
    for arg in msg.args {
        args.push(&js_value(arg));
    }
    let entry = js_sys::Object::new();
    let set = |key: &str, value: &JsValue| {
        js_sys::Reflect::set(&entry, &key.into(), value)
            .map_err(|_| JsError::new("osc decode: cannot build the message object"))
    };
    set("addr", &JsValue::from_str(&msg.addr))?;
    set("args", &args)?;
    if let Some(time) = time {
        set("time", &time.map_or(JsValue::NULL, JsValue::from_f64))?;
    }
    Ok(entry)
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
        // A timetag crosses as **Unix** seconds, not the raw NTP epoch: it is
        // what the Python client's decoder yields for the same byte
        // (`/clock_query.reply`'s anchor, which a joining clock maps its grid
        // through), and the two clients must read one wire the same way.
        OscType::Time(t) => JsValue::from_f64(osc::ntp_to_unix(t)),
        // The remaining rosc types (char, color, midi, arrays, inf) do not
        // appear on this server's wire; surface them as null rather than
        // failing the whole packet.
        _ => JsValue::NULL,
    }
}

// ---- bundle assembly ----
//
// A message alone has no time; logical time rides on a bundle's timetag. Both
// doors take the same shape -- an array of `[addr, [[tag, value], ...]]` -- so
// the caller assembles a whole timed emission in one crossing.

/// One `[addr, args]` JS pair -> a core message.
#[cfg(target_arch = "wasm32")]
fn js_message(entry: &JsValue) -> Result<OscMessage, JsError> {
    let pair = js_sys::Array::from(entry);
    let addr = pair
        .get(0)
        .as_string()
        .ok_or_else(|| JsError::new("bundle: [addr, args] expected"))?;
    let args = js_args(js_sys::Array::from(&pair.get(1)))?;
    Ok(OscMessage { addr, args })
}

#[cfg(target_arch = "wasm32")]
fn encode_bundle(time: osc::OscTime, messages: js_sys::Array) -> Result<Vec<u8>, JsError> {
    let mut content = Vec::with_capacity(messages.length() as usize);
    for entry in messages.iter() {
        content.push(js_message(&entry)?);
    }
    encode(&OscPacket::Bundle(osc::bundle(time, content))).map_err(|e| JsError::new(&e.to_string()))
}

/// JS face: a bundle stamped at `unix_secs` (the wall clock the server reads
/// as an NTP timetag) -> `Uint8Array`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn osc_encode_bundle(unix_secs: f64, messages: js_sys::Array) -> Result<Vec<u8>, JsError> {
    encode_bundle(osc::unix_to_ntp(unix_secs), messages)
}

/// JS face: a bundle stamped at `secs` **from the start of a render** -> the
/// bundle an NRT score is made of. The same packing as [`osc_encode_bundle`]
/// on a different epoch: a score's time is not a wall clock, so nothing is
/// added to it (`clausters_core::osc::pack_timetag`, the rule every client
/// shares -- the Python client reaches it through `clausters_core_ntp_timetag`
/// and assembles the bundle itself).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn osc_encode_score_bundle(secs: f64, messages: js_sys::Array) -> Result<Vec<u8>, JsError> {
    encode_bundle(osc::pack_timetag(secs), messages)
}

/// JS face: a bundle with the *immediate* timetag -> `Uint8Array`. What rides
/// inside `/sched_at`, whose own absolute sample carries the time.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn osc_encode_immediate_bundle(messages: js_sys::Array) -> Result<Vec<u8>, JsError> {
    encode_bundle(osc::pack_timetag(0.0), messages)
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
            "/synth_new",
            vec![
                OscType::String("default".into()),
                OscType::Int(1000),
                OscType::Float(440.0),
            ],
        )
        .unwrap();
        assert_eq!(bytes.len() % 4, 0);
        assert!(bytes.starts_with(b"/synth_new\0\0,sif\0\0\0\0"));
        let msgs = decode_messages(&bytes).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].addr, "/synth_new");
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

    /// The timed door carries each message's *containing* bundle time, the
    /// rule the Python client's receiving door applies: an immediate bundle
    /// and a bare message read `None`, a stamped one reads Unix seconds, and a
    /// nested bundle overrides its parent for its own contents.
    #[test]
    fn the_timed_decode_carries_the_bundle_time() {
        let msg = |addr: &str| {
            OscPacket::Message(OscMessage {
                addr: addr.into(),
                args: vec![],
            })
        };
        let unix = 1_700_000_000.5_f64;
        let inner = OscPacket::Bundle(OscBundle {
            timetag: clausters_core::osc::unix_to_ntp(unix),
            content: vec![msg("/inner")],
        });
        let outer = OscPacket::Bundle(OscBundle {
            timetag: OscTime {
                seconds: 0,
                fractional: 1,
            },
            content: vec![msg("/immediate"), inner],
        });
        let decoded = decode_messages_timed(&encode(&outer).unwrap()).unwrap();
        let seen: Vec<(String, Option<f64>)> =
            decoded.into_iter().map(|(m, t)| (m.addr, t)).collect();
        assert_eq!(seen[0].0, "/immediate");
        assert_eq!(seen[0].1, None, "an immediate bundle is 'now', not a time");
        assert_eq!(seen[1].0, "/inner");
        assert!((seen[1].1.unwrap() - unix).abs() < 1e-6);

        let bare = decode_messages_timed(&encode(&msg("/bare")).unwrap()).unwrap();
        assert_eq!(bare[0].1, None, "a bare message carries no time");
    }
}
