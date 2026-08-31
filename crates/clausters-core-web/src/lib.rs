//! The shared core's JS door (the W track): a thin wasm-bindgen shell over
//! `clausters-core`, the browser sibling of `clausters-ffi`'s C door and the
//! core twin of `crates/clausters-web` (the whole-server shell).
//!
//! W0 exposes the OSC codec: the web client encodes and decodes through the
//! same `clausters_core::osc` the server and every other client use, so the
//! bytes are identical by construction (the parity vectors in
//! `clients/web/tests/` hold this against the Python client). W1 adds the
//! **registry** — the id-allocation model behind node ids, buses and buffers,
//! the same door `clausters-ffi` opens for Python. W3 adds the sequencing
//! layer's core: the beat-ordered **queue**, the beat/second/sample
//! arithmetic, **bundle assembly** with a timetag, the seeded value stream,
//! the builtins and the pitch space, and the sample-clock model. W10 adds the
//! **data paths' analysis** — the stereo-field measurements and the peak
//! pyramid — so a page that reads buses, taps and buffers measures them with
//! the same functions the GUI host measures with. Only measurements: what a
//! *drawing* needs of them (a pixel row, a display window, a decibel curve)
//! stays in the host, which is the one thing that draws (W26).
//! Always the same shape: the logic lives in `clausters-core`, natively tested; this
//! shell only converts values at the JS boundary, and only on the wasm target.
//!
//! The JS argument convention mirrors the tagged pairs the interim page codec
//! used: encode takes `[tag, value]` pairs (`"i"`/`"h"`/`"f"`/`"d"`/`"s"`/
//! `"b"`), preserving the int/float distinction explicitly; decode returns
//! plain values (`{addr, args}` per message, bundles flattened, timetags as
//! Unix seconds), which is what reply consumers want.

use clausters_core::osc::{OscMessage, OscPacket, OscType, decode_packet, encode, ntp_to_unix};
#[cfg(target_arch = "wasm32")]
use clausters_core::{
    builtins, bundle,
    clocksync::SampleClockModel,
    measure, osc,
    peaks::MultiPyramid,
    registry::{self, NodeIdPartition, Registry},
    rng::Rng,
    tempoclock::{self, Scheduler},
    tempomap::{Curve, TempoMap},
};

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

/// A JS array of `[tag, value]` pairs → typed OSC arguments.
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

/// JS face: `osc_decode_packet_timed(bytes) -> [{addr, args, time}, ...]` —
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
/// the timed shape (`Some(None)` writes a `null` — the field is there either
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

// ---- the registry: node ids, buses, buffers ----
//
// The client-side allocators are all one model (`clausters_core::registry`),
// so they cross to JS as one class rather than three. The JS names are
// camelCase because wasm-bindgen renames methods for the language it lands
// in; the semantics are the Rust ones verbatim — exhaustion is `undefined`
// (never a wrap), and a refused release reports instead of corrupting the map.

/// A registry of one finite id space, the JS face of
/// [`clausters_core::registry::Registry`].
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = Registry)]
pub struct JsRegistry(Registry);

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = Registry)]
impl JsRegistry {
    /// A bounded registry over `[base, base + capacity)`.
    #[wasm_bindgen(constructor)]
    pub fn new(base: f64, capacity: u32) -> JsRegistry {
        JsRegistry(Registry::new(base as i64, capacity as usize))
    }

    /// The NRT/score registry: allocation never fails, ids ascend from `base`.
    pub fn unbounded(base: f64) -> JsRegistry {
        JsRegistry(Registry::unbounded(base as i64))
    }

    /// Allocates `width` contiguous ids and returns the first, or `undefined`
    /// when no such run is free. `width` 0 counts as 1.
    pub fn alloc(&mut self, width: u32) -> Option<f64> {
        self.0.alloc(width as usize).map(|id| id as f64)
    }

    /// Returns `width` ids starting at `first` to the pool. `true` when the
    /// release was accepted; `false` leaves the map untouched (out of range,
    /// or not currently allocated — a double release).
    pub fn release(&mut self, first: f64, width: u32) -> bool {
        self.0.release(first as i64, width as usize).is_ok()
    }

    /// Whether `id` falls inside this registry's space (allocated or not) —
    /// the filter for foreign `/node_end` ids.
    pub fn contains(&self, id: f64) -> bool {
        self.0.contains(id as i64)
    }

    /// Whether `id` is currently allocated.
    #[wasm_bindgen(js_name = isAllocated)]
    pub fn is_allocated(&self, id: f64) -> bool {
        self.0.is_allocated(id as i64)
    }

    /// How many ids are currently allocated.
    #[wasm_bindgen(getter, js_name = inUse)]
    pub fn in_use(&self) -> u32 {
        self.0.in_use() as u32
    }

    /// The first id of the space.
    #[wasm_bindgen(getter)]
    pub fn base(&self) -> f64 {
        self.0.base() as f64
    }

    /// The size of the id space; `undefined` when unbounded.
    #[wasm_bindgen(getter)]
    pub fn capacity(&self) -> Option<u32> {
        self.0.capacity().map(|c| c as u32)
    }

    /// Releases everything back to the pool (a client reset).
    pub fn clear(&mut self) {
        self.0.clear();
    }
}

/// JS face: the boot-derived node-id partition for a node table of
/// `max_nodes` slots — `{clientBase, clientCapacity, autoBase, autoCapacity,
/// midiBase, midiCapacity}`, the same formula the server applies.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn node_id_partition(max_nodes: u32) -> Result<js_sys::Object, JsError> {
    let p = NodeIdPartition::from_max_nodes(max_nodes.max(1) as usize);
    let out = js_sys::Object::new();
    let set = |key: &str, value: f64| -> Result<(), JsError> {
        js_sys::Reflect::set(&out, &key.into(), &JsValue::from_f64(value))
            .map(|_| ())
            .map_err(|_| JsError::new("node_id_partition: cannot build the result object"))
    };
    set("clientBase", p.client_base as f64)?;
    set("clientCapacity", p.client_capacity as f64)?;
    set("autoBase", p.auto_base as f64)?;
    set("autoCapacity", p.auto_capacity as f64)?;
    set("midiBase", p.midi_base as f64)?;
    set("midiCapacity", p.midi_capacity as f64)?;
    Ok(out)
}

/// JS face: the `[audio, control]` bus widths GraphDef instances reserve at
/// the top of each bus space (before clamping to a smaller configured count).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn graph_bus_reserved() -> Vec<u32> {
    vec![
        registry::GRAPH_AUDIO_BUS_RESERVED as u32,
        registry::GRAPH_CONTROL_BUS_RESERVED as u32,
    ]
}

// ---- musical time: the clock's arithmetic, and its queue ----
//
// The W3 doors. Every one of them is `clausters-core`'s own function reached
// from JS, the same way `clausters-ffi` reaches it from Python: the sequencing
// layer computes no time of its own in either language, so a beat resolves to
// the same second, and a second to the same sample, in the client and in the
// server.

/// Seconds at `beats` for the affine clock `(tempo, base_beats, base_seconds)`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn beats_to_secs(tempo: f64, base_beats: f64, base_seconds: f64, beats: f64) -> f64 {
    base_seconds + (beats - base_beats) / tempo
}

/// Beats at `secs` for the affine clock `(tempo, base_beats, base_seconds)`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn secs_to_beats(tempo: f64, base_beats: f64, base_seconds: f64, secs: f64) -> f64 {
    base_beats + (secs - base_seconds) * tempo
}

/// Seconds → sample count at `sample_rate` (ties to even).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn secs_to_samples(secs: f64, sample_rate: f64) -> f64 {
    tempoclock::secs_to_samples(secs, sample_rate) as f64
}

/// Sample count → seconds at `sample_rate`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn samples_to_secs(samples: f64, sample_rate: f64) -> f64 {
    tempoclock::samples_to_secs(samples as i64, sample_rate)
}

/// Beats to wait so a routine starts on the next `quant` boundary of the grid
/// (`quant <= 0` → now). The snapping rule every client shares.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn quant_delay(pos: f64, quant: f64) -> f64 {
    tempoclock::quant_delay(pos, quant)
}

/// The 0-based bar index `beats` falls in on a grid of `quant` beats per bar.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn bar(beats: f64, quant: f64) -> f64 {
    tempoclock::bar(beats, quant)
}

/// The beat within its bar, in `[0, quant)`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn beat_in_bar(beats: f64, quant: f64) -> f64 {
    tempoclock::beat_in_bar(beats, quant)
}

/// A Unix timestamp → the 64 NTP timetag bits, as a `BigInt` (the wire value
/// is a full 64-bit word; JS numbers would lose its low bits).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn unix_to_ntp(unix_secs: f64) -> u64 {
    osc::timetag_bits(osc::unix_to_ntp(unix_secs))
}

/// A Unix timestamp → the server's absolute sample, through a `/clock_query` anchor
/// (`anchor_unix`, `anchor_sample`) and the measured `rate`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn unix_to_sample(unix_secs: f64, anchor_unix: f64, anchor_sample: f64, rate: f64) -> f64 {
    osc::unix_to_sample(unix_secs, anchor_unix, anchor_sample as i64, rate) as f64
}

/// The beat-ordered scheduling queue, the JS face of
/// [`clausters_core::tempoclock::Scheduler`]. It holds `(time, id)` pairs and
/// nothing else: the language side maps each id back to the routine it queued,
/// which is what keeps the coroutine driver in the language.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = Scheduler)]
pub struct JsScheduler(Scheduler);

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = Scheduler)]
impl JsScheduler {
    #[wasm_bindgen(constructor)]
    pub fn new() -> JsScheduler {
        JsScheduler(Scheduler::new())
    }

    /// Queues `id` at `time` (beats). Equal times keep insertion order.
    pub fn push(&mut self, time: f64, id: f64) {
        self.0.push(time, id as u64);
    }

    /// The earliest queued time, or `undefined` when the queue is empty.
    #[wasm_bindgen(js_name = peekTime)]
    pub fn peek_time(&self) -> Option<f64> {
        self.0.peek_time()
    }

    /// Pops the earliest entry when it is due at `now`, as `[time, id]`.
    #[wasm_bindgen(js_name = popDue)]
    pub fn pop_due(&mut self, now: f64) -> Option<Vec<f64>> {
        self.0.pop_due(now).map(|(t, id)| vec![t, id as f64])
    }

    /// Drops every entry queued under `id`; returns how many went.
    pub fn remove(&mut self, id: f64) -> usize {
        self.0.remove(id as u64)
    }

    #[wasm_bindgen(getter)]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[wasm_bindgen(getter, js_name = isEmpty)]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }
}

#[cfg(target_arch = "wasm32")]
impl Default for JsScheduler {
    fn default() -> Self {
        Self::new()
    }
}

/// The piece's beat↔second time map, the JS face of
/// [`clausters_core::tempomap::TempoMap`].
///
/// A beat is a logical coordinate, not a unit of time; this is the function
/// that turns one into the other under a tempo that changes along the piece.
/// It is pure — it knows nothing of *now* — so an editor, an offline render
/// and a live clock share one, and there is a single implementation of the
/// integral behind all of them.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = TempoMap)]
pub struct JsTempoMap(TempoMap);

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = TempoMap)]
impl JsTempoMap {
    /// A map of one constant-tempo segment (beat 0 at second 0). A tempo that
    /// is not finite and positive falls back to 1.0.
    #[wasm_bindgen(constructor)]
    pub fn new(tempo: f64) -> JsTempoMap {
        JsTempoMap(TempoMap::new(tempo))
    }

    /// A map of one constant-tempo segment with `baseBeats` falling on
    /// `baseSeconds` — the affine triple a running clock already holds, so
    /// adopting a map changes no result. `undefined` on invalid arguments.
    pub fn anchored(tempo: f64, base_beats: f64, base_seconds: f64) -> Option<JsTempoMap> {
        TempoMap::anchored(tempo, base_beats, base_seconds)
            .ok()
            .map(JsTempoMap)
    }

    /// An independent copy — what handing a piece's map to a clock takes, so
    /// neither one's edits reach the other.
    #[wasm_bindgen(js_name = copy)]
    pub fn copy(&self) -> JsTempoMap {
        JsTempoMap(self.0.clone())
    }

    /// **The time map**: the second beat `b` falls on.
    #[wasm_bindgen(js_name = secsAt)]
    pub fn secs_at(&self, b: f64) -> f64 {
        self.0.secs_at(b)
    }

    /// The inverse: the beat falling on second `s`.
    #[wasm_bindgen(js_name = beatsAt)]
    pub fn beats_at(&self, s: f64) -> f64 {
        self.0.beats_at(s)
    }

    /// The tempo (beats per second) in effect at beat `b`.
    #[wasm_bindgen(js_name = tempoAt)]
    pub fn tempo_at(&self, b: f64) -> f64 {
        self.0.tempo_at(b)
    }

    /// How long the stretch from `b0` to `b1` lasts, in seconds — the only
    /// correct way to turn a length in beats into a length in time, since the
    /// same span lasts differently depending on where it sits.
    #[wasm_bindgen(js_name = spanSecs)]
    pub fn span_secs(&self, b0: f64, b1: f64) -> f64 {
        self.0.span_secs(b0, b1)
    }

    /// How many beats fit in `secs` seconds starting at beat `b0`.
    #[wasm_bindgen(js_name = spanBeats)]
    pub fn span_beats(&self, b0: f64, secs: f64) -> f64 {
        self.0.span_beats(b0, secs)
    }

    /// Appends a constant-tempo change at beat `b`. Returns whether it was
    /// taken; a refused change leaves the map as it was.
    pub fn push(&mut self, b: f64, tempo: f64) -> bool {
        self.0.push(b, tempo).is_ok()
    }

    /// Writes a tempo ramp over `[fromBeats, toBeats]` from `fromTempo` to
    /// `toTempo`, holding `toTempo` after it.
    pub fn ramp(&mut self, from_beats: f64, to_beats: f64, from_tempo: f64, to_tempo: f64) -> bool {
        self.0
            .ramp(from_beats, to_beats, from_tempo, to_tempo)
            .is_ok()
    }

    /// Drops every breakpoint at or after beat `b` (never the first).
    #[wasm_bindgen(js_name = truncateFrom)]
    pub fn truncate_from(&mut self, b: f64) {
        self.0.truncate_from(b);
    }

    /// How many segments the map holds (always at least 1).
    #[wasm_bindgen(getter)]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Always false — a map holds at least one segment by construction.
    #[wasm_bindgen(getter, js_name = isEmpty)]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Segment `i` as `[beats, secs, tempo, curve, endBeats, endTempo]` —
    /// `curve` is 0 for a constant tempo and 1 for a ramp, whose two trailing
    /// fields are 0 when it is not one. `undefined` past the end.
    pub fn segment(&self, i: usize) -> Option<Vec<f64>> {
        self.0.segments().get(i).map(|seg| {
            let (curve, end_beats, end_tempo) = match seg.curve {
                Curve::Step => (0.0, 0.0, 0.0),
                Curve::Linear {
                    end_beats,
                    end_tempo,
                } => (1.0, end_beats, end_tempo),
            };
            vec![seg.beats, seg.secs, seg.tempo, curve, end_beats, end_tempo]
        })
    }

    /// The last segment's affine triple, `[baseBeats, baseSeconds, tempo]` —
    /// what a clock caches so reading *now* stays three float operations with
    /// no search.
    pub fn last(&self) -> Vec<f64> {
        let last = self.0.last();
        vec![last.beats, last.secs, last.tempo]
    }
}

// ---- bundle assembly ----
//
// A message alone has no time; logical time rides on a bundle's timetag. Both
// doors take the same shape — an array of `[addr, [[tag, value], ...]]` — so
// the caller assembles a whole timed emission in one crossing.

/// One `[addr, args]` JS pair → a core message.
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
/// as an NTP timetag) → `Uint8Array`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn osc_encode_bundle(unix_secs: f64, messages: js_sys::Array) -> Result<Vec<u8>, JsError> {
    encode_bundle(osc::unix_to_ntp(unix_secs), messages)
}

/// JS face: a bundle stamped at `secs` **from the start of a render** → the
/// bundle an NRT score is made of. The same packing as [`osc_encode_bundle`]
/// on a different epoch: a score's time is not a wall clock, so nothing is
/// added to it (`clausters_core::osc::pack_timetag`, the rule every client
/// shares — the Python client reaches it through `clausters_core_ntp_timetag`
/// and assembles the bundle itself).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn osc_encode_score_bundle(secs: f64, messages: js_sys::Array) -> Result<Vec<u8>, JsError> {
    encode_bundle(osc::pack_timetag(secs), messages)
}

/// JS face: a bundle with the *immediate* timetag → `Uint8Array`. What rides
/// inside `/sched_at`, whose own absolute sample carries the time.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn osc_encode_immediate_bundle(messages: js_sys::Array) -> Result<Vec<u8>, JsError> {
    encode_bundle(osc::pack_timetag(0.0), messages)
}

// ---- the seeded value stream ----
//
// The random patterns draw from here, so one root seed replays a whole script
// identically in every client language. `spawn` is the sclang-style
// inheritance a routine's own stream is built from; the u64 state never
// crosses to JS, which keeps the surface free of BigInt.

/// A resumable seeded value stream, the JS face of
/// [`clausters_core::rng::Rng`].
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = Rng)]
pub struct JsRng(Rng);

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = Rng)]
impl JsRng {
    /// The stream for `seed` (splitmix64-mixed, never zero) — the same seeding
    /// as the server's `WhiteNoise`.
    #[wasm_bindgen(constructor)]
    pub fn new(seed: f64) -> JsRng {
        JsRng(Rng::from_seed(seed as u64))
    }

    /// Uniform in `[0, 1)` with 53-bit resolution.
    #[wasm_bindgen(js_name = nextF64)]
    pub fn next_f64(&mut self) -> f64 {
        self.0.next_f64()
    }

    /// Uniform in `[lo, hi)` (degenerate to `lo` when `hi <= lo`).
    pub fn uniform(&mut self, lo: f64, hi: f64) -> f64 {
        self.0.uniform(lo, hi)
    }

    /// Uniform integer in `[0, n)`; 0 when `n` is 0.
    #[wasm_bindgen(js_name = nextBelow)]
    pub fn next_below(&mut self, n: f64) -> f64 {
        self.0.next_below(n.max(0.0) as u64) as f64
    }

    /// A child stream seeded from this one's next word: deterministic
    /// derivation, so seeding a root reproduces every stream created under it,
    /// in creation order.
    pub fn spawn(&mut self) -> JsRng {
        JsRng(Rng::from_seed(self.0.next_u64()))
    }
}

// ---- builtins and the pitch space ----

/// JS face: one unary builtin by name (`"midicps"`, `"cpsmidi"`, `"dbamp"`,
/// ...), computed in `f32` exactly as the server's UGens compute it.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn unary(op: &str, x: f64) -> Result<f64, JsError> {
    let op = builtins::UnaryOp::from_name(op)
        .ok_or_else(|| JsError::new(&format!("unknown unary op '{op}'")))?;
    Ok(builtins::apply_unary(op, x as f32) as f64)
}

/// JS face: one binary builtin by name (`"add"`, `"pow"`, `"clip2"`, ...).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn binary(op: &str, a: f64, b: f64) -> Result<f64, JsError> {
    let op = builtins::BinaryOp::from_name(op)
        .ok_or_else(|| JsError::new(&format!("unknown binary op '{op}'")))?;
    Ok(builtins::apply_binary(op, a as f32, b as f32) as f64)
}

/// JS face: one range map by name (`"linlin"`, `"linexp"`, `"lincurve"`, ...),
/// with `clip` naming what an out-of-range input is trimmed to (`"minmax"`,
/// `"min"`, `"max"`, `"none"`). `curve` is read only by the bent pair and the
/// input bounds only by the maps that have an input range.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn map(
    op: &str,
    clip: &str,
    x: f64,
    in_lo: f64,
    in_hi: f64,
    out_lo: f64,
    out_hi: f64,
    curve: f64,
) -> Result<f64, JsError> {
    let op = clausters_core::warp::MapOp::from_name(op)
        .ok_or_else(|| JsError::new(&format!("unknown range map '{op}'")))?;
    let clip = clausters_core::warp::Clip::from_name(clip)
        .ok_or_else(|| JsError::new(&format!("unknown clip mode '{clip}'")))?;
    Ok(clausters_core::warp::apply_map(
        op,
        x as f32,
        in_lo as f32,
        in_hi as f32,
        out_lo as f32,
        out_hi as f32,
        curve as f32,
        clip,
    ) as f64)
}

/// JS face: scale degree → MIDI note number in the pitch space
/// `octave`/`root`, with floored octave wrapping (sclang semantics). An empty
/// `scale` yields middle C.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn degree_to_midinote(degree: f64, octave: f64, root: f64, scale: &[f32]) -> f64 {
    builtins::degree_to_midinote(degree, octave, root, scale)
}

// ---- the sample-clock model ----
//
// How a client paced by its own monotonic clock still schedules on a remote
// server's sample axis: `/clock_query` replies are fed in as anchors, and the model
// regresses local time against the sample counter. The in-page carrier needs
// none of this (the engine shares the page's audio clock); this is the door
// the WebSocket carrier's tracker drives.

/// The local-time ↔ sample regression, the JS face of
/// [`clausters_core::clocksync::SampleClockModel`].
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = SampleClockModel)]
pub struct JsSampleClockModel(SampleClockModel);

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = SampleClockModel)]
impl JsSampleClockModel {
    /// A model seeded with the `nominal_rate`, keeping the last `window`
    /// anchors.
    #[wasm_bindgen(constructor)]
    pub fn new(nominal_rate: f64, window: usize) -> JsSampleClockModel {
        JsSampleClockModel(SampleClockModel::new(nominal_rate, window))
    }

    /// Records one `/clock_query` observation: the local time it was taken at, the
    /// server's counter, and the rate the server reported (0 keeps the
    /// current one).
    #[wasm_bindgen(js_name = addAnchor)]
    pub fn add_anchor(&mut self, t_local: f64, sample: f64, rate: f64) {
        self.0.add_anchor(t_local, sample as i64, rate);
    }

    /// The server's sample at a local time.
    #[wasm_bindgen(js_name = sampleAt)]
    pub fn sample_at(&self, t_local: f64) -> f64 {
        self.0.sample_at(t_local) as f64
    }

    /// The local time at which a server sample falls.
    #[wasm_bindgen(js_name = localTimeOf)]
    pub fn local_time_of(&self, sample: f64) -> f64 {
        self.0.local_time_of(sample as i64)
    }

    /// The measured drift between the two clocks, in parts per million.
    #[wasm_bindgen(getter, js_name = driftPpm)]
    pub fn drift_ppm(&self) -> f64 {
        self.0.drift_ppm()
    }

    /// The measured sample rate (samples per local second).
    #[wasm_bindgen(getter)]
    pub fn rate(&self) -> f64 {
        self.0.rate()
    }

    /// The local-time span the held anchors cover.
    #[wasm_bindgen(getter)]
    pub fn span(&self) -> f64 {
        self.0.span()
    }

    /// How many anchors the model currently holds.
    #[wasm_bindgen(getter)]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[wasm_bindgen(getter, js_name = isEmpty)]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

// ---- the component bundle ----
//
// W4's mount, opened to the page: a persisted bundle is a template, and the
// page turns it into N non-colliding instances. The pass itself is
// `clausters_core::bundle` — pure, natively tested, and the same one the native
// `--standalone` leg runs — so these three are only the JSON boundary. The
// page allocates between `bundle_requirements` and `bundle_resolve`, from its
// own `Server`/`GuiHost` allocators; nothing here allocates or keeps state.

/// JS face: what one instance of a bundle needs allocated.
/// `bundle_requirements(requestJson) -> requirementsJson`, the request holding
/// the manifest and — for a bundle written before the contract, whose widget
/// ids are whatever its author picked — the template its id block is measured
/// from.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn bundle_requirements(request: &str) -> Result<String, JsError> {
    let request: bundle::RequirementsRequest =
        serde_json::from_str(request).map_err(|e| JsError::new(&format!("requirements: {e}")))?;
    serde_json::to_string(&bundle::requirements_request(&request))
        .map_err(|e| JsError::new(&e.to_string()))
}

/// JS face: one mounted instance, from the allocation the page just made.
/// `bundle_resolve(requestJson) -> resolvedJson`, the request carrying the
/// manifest, the template, the allocation and the supplied parameters
/// (`{ attributes, preset }`) in one object.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn bundle_resolve(request: &str) -> Result<String, JsError> {
    let request: bundle::ResolveRequest =
        serde_json::from_str(request).map_err(|e| JsError::new(&format!("resolve: {e}")))?;
    let resolved = bundle::resolve_request(&request).map_err(|e| JsError::new(&e.to_string()))?;
    serde_json::to_string(&resolved).map_err(|e| JsError::new(&e.to_string()))
}

/// JS face: the writers' pre-flight — the mount dry-run over the declared
/// defaults, plus the no-holes check on every def payload.
/// `bundle_validate(requestJson)`, throwing on the first problem.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn bundle_validate(request: &str) -> Result<(), JsError> {
    let request: bundle::ValidateRequest =
        serde_json::from_str(request).map_err(|e| JsError::new(&format!("validate: {e}")))?;
    bundle::validate_request(&request).map_err(|e| JsError::new(&e.to_string()))
}

// ---- the data paths' analysis ----
//
// W10's doors: what a page measures from the data it reads off the server —
// control buses, tap windows, buffer samples. Every one of them is a
// measurement of the *signal*, and it is the same function the GUI host
// measures with, so a figure a script reports and a figure a widget draws are
// one number rather than two implementations of it. Nothing of the *screen*
// belongs here (W26): a display window, a trigger's framing, a decibel curve
// and a row of pixel columns are drawing, and the host is what draws. Nothing
// here keeps state except the peak pyramid, which is a cache by definition.

/// JS face: the **peak and RMS** of one channel of an interleaved buffer, as
/// `[peak, rms]` — what a render reports back about what it produced. The
/// stride walk measures a render without deinterleaving it first, so a page
/// reads the same two numbers the server and the Python client report.
///
/// An empty pair for a channel the buffer does not have.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn channel_stats(samples: &[f32], channels: usize, channel: usize) -> Vec<f32> {
    if channels == 0 || channel >= channels {
        return Vec::new();
    }
    let (peak, rms) = measure::channel_stats(samples, channels, channel);
    vec![peak, rms]
}

/// JS face: the stereo **correlation** (Pearson's r) of two equal-length
/// channels, in `[-1, 1]`. `undefined` when it is undefined — a length
/// mismatch, an empty pair, or a constant channel.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn correlation(left: &[f32], right: &[f32]) -> Option<f32> {
    measure::correlation(left, right)
}

/// JS face: the **Lissajous / goniometer** projection of a stereo pair, as
/// interleaved `[x, y]` pairs (`x` = side, `y` = mid) — one pair per input
/// frame. An empty array when the two channels differ in length.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn lissajous(left: &[f32], right: &[f32]) -> Vec<f32> {
    let mut out = vec![[0.0f32; 2]; left.len()];
    if !measure::lissajous_into(left, right, &mut out) {
        return Vec::new();
    }
    out.into_iter().flatten().collect()
}

/// A built min/max peak pyramid, the JS face of
/// [`clausters_core::peaks::MultiPyramid`] — the summary a waveform view is
/// drawn from, so the drawing costs the width of the window rather than the
/// length of the buffer. Built (or filled from `/buffer_stream` reports) here
/// and handed to the GUI host, which draws it; the readers below answer **what
/// the cache is** — length, channels, bucket, levels — and never what it says,
/// which is a drawing's question.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = Pyramid)]
pub struct JsPyramid(MultiPyramid);

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = Pyramid)]
impl JsPyramid {
    /// Builds one pyramid per channel from interleaved `samples`.
    /// `base_bucket` is the level-0 bucket size (256 is the usual choice:
    /// ~0.8% of the source in cache for a floor of 256 samples per column).
    pub fn build(samples: &[f32], channels: u32, base_bucket: usize) -> JsPyramid {
        JsPyramid(MultiPyramid::build_interleaved(
            samples,
            channels.max(1) as usize,
            base_bucket.max(1),
        ))
    }

    /// **An empty pyramid of a given length** — the picture of a take that has
    /// been allocated and not yet recorded into, ready to be filled by
    /// [`Self::write_buckets`] as the reports arrive.
    ///
    /// Building one out of a buffer of silence instead would allocate the take
    /// (230 MB for ten minutes of stereo) to summarize samples nobody wrote.
    pub fn empty(frames: usize, channels: u32, base_bucket: usize) -> JsPyramid {
        JsPyramid(MultiPyramid::empty(
            frames,
            channels.max(1) as usize,
            base_bucket.max(1),
        ))
    }

    /// Reads back a serialized cache (`toBytes`, or the file the GUI host maps
    /// and the Python client writes). `undefined` when the bytes are not one.
    #[wasm_bindgen(js_name = fromBytes)]
    pub fn from_bytes(data: &[u8]) -> Option<JsPyramid> {
        MultiPyramid::from_bytes(data).map(JsPyramid)
    }

    /// Rewrites the part of the cache a **frame span** touches, from the
    /// interleaved buffer as it now stands — what keeps an editor's overview
    /// true after an edit without re-summarizing the take.
    ///
    /// `samples` is the whole buffer, not the span: a bucket at either edge of
    /// it holds untouched samples too. Returns whether it applied — `false`,
    /// changing nothing, when the buffer is not the one this cache describes,
    /// which is an edit that changed the *length* and therefore a rebuild.
    #[wasm_bindgen(js_name = updateRange)]
    pub fn update_range(&mut self, samples: &[f32], start: usize, frames: usize) -> bool {
        self.0.update_range(samples, start, frames)
    }

    /// Folds a run of **already-summarized buckets** into this pyramid — the
    /// receiving half of `/buffer_stream`, which is how a page follows a
    /// recording it cannot map: the server sends the overview of what was
    /// written (2 kB/s where the audio is 190) and this puts it in the
    /// picture.
    ///
    /// `stats` is the reply's blob read as `f32`s, **bucket-major and
    /// channel-minor**: for each bucket of `bucket` frames in order, for each
    /// channel, `min`, `max` and mean square. `startFrame` is where the report
    /// begins on the buffer's own sample axis. Returns whether it applied —
    /// `false`, changing nothing, when the report is on another grid than this
    /// cache (another bucket size, a start off a bucket boundary, or a run
    /// that does not fit).
    #[wasm_bindgen(js_name = writeBuckets)]
    pub fn write_buckets(&mut self, start_frame: usize, bucket: usize, stats: &[f32]) -> bool {
        self.0.write_buckets(start_frame, bucket, stats)
    }

    /// The cache's bytes, in the format every client reads: the mono layout
    /// for a single channel and the multichannel one above it — the choice
    /// the Python client's door makes, so the same samples serialize to the
    /// same bytes whichever client reduced them. Both are read back by
    /// `fromBytes` and by the GUI host.
    #[wasm_bindgen(js_name = toBytes)]
    pub fn to_bytes(&self) -> Vec<u8> {
        match self.0.channel(0) {
            Some(mono) if self.0.num_channels() == 1 => mono.to_bytes(),
            _ => self.0.to_bytes(),
        }
    }

    /// Samples per channel — the length a view of this cache spans.
    #[wasm_bindgen(getter)]
    pub fn frames(&self) -> usize {
        self.0.frames()
    }

    #[wasm_bindgen(getter)]
    pub fn channels(&self) -> usize {
        self.0.num_channels()
    }

    #[wasm_bindgen(getter, js_name = baseBucket)]
    pub fn base_bucket(&self) -> usize {
        self.0.base_bucket()
    }

    #[wasm_bindgen(getter, js_name = numLevels)]
    pub fn num_levels(&self) -> usize {
        self.0.channel(0).map_or(0, |p| p.num_levels())
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

// ---- the document ----
//
// A class rather than free functions taking the tree, and the reason is a
// measurement rather than a preference. The first binding passed the whole
// document in and took the whole new one back: 205 ms for one placement on a
// 10240-event composition, linear in the document and independent of the edit.
// The tree now stays in Rust and only the intent and the outcome cross, which
// is the same shape `Log` already had and for the same reason.
//
// What has *not* changed is the discipline: the crate is the only thing that
// applies an intent. This is not an accessor handle -- there is no call per
// field of the tree -- it is the same three verbs the by-value binding had,
// with `snapshot` for whoever wants the JSON.

/// One composition, held in Rust — the JS face of
/// [`clausters_document::Document`].
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = Document)]
pub struct JsDocument(clausters_document::Document);

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = Document)]
impl JsDocument {
    /// Open a document from its JSON, or an empty composition from `undefined`.
    #[wasm_bindgen(constructor)]
    pub fn new(json: Option<String>) -> Result<JsDocument, JsError> {
        let document = match json {
            Some(json) => {
                serde_json::from_str(&json).map_err(|e| JsError::new(&format!("document: {e}")))?
            }
            None => clausters_document::Document::new(clausters_document::Node::new(
                clausters_document::NodeId(1),
                clausters_document::Body::Aggregate {
                    grouping: clausters_document::Grouping::Concrete,
                    members: Vec::new(),
                    config: clausters_document::Opaque::none(),
                },
            )),
        };
        Ok(JsDocument(document))
    }

    /// The whole tree as JSON — for saving it, or for a caller that wants it.
    /// The one call that still costs the size of the composition, and it is
    /// asked for rather than paid on every edit.
    pub fn snapshot(&self) -> Result<String, JsError> {
        serde_json::to_string(&self.0).map_err(|e| JsError::new(&e.to_string()))
    }

    /// The monotonic version, bumped by every applied edit. Never zero.
    #[wasm_bindgen(getter)]
    pub fn version(&self) -> u64 {
        self.0.version
    }

    /// Apply an edit. `apply(requestJson) -> outcomeJson`, the request carrying
    /// `{ intent, against?, quant? }` and the result the outcome alone —
    /// the document stays here and `snapshot` is how it leaves.
    ///
    /// One object rather than three arguments because the boundary is JSON
    /// either way, and a request that grows a field then costs no signature.
    pub fn apply(&mut self, request: &str) -> Result<String, JsError> {
        #[derive(serde::Deserialize)]
        struct Request {
            intent: clausters_document::Intent,
            #[serde(default)]
            against: Option<clausters_document::Against>,
            #[serde(default)]
            quant: f64,
        }
        let request: Request =
            serde_json::from_str(request).map_err(|e| JsError::new(&format!("apply: {e}")))?;
        let against = request
            .against
            .unwrap_or_else(clausters_document::Against::unstated);
        let outcome = clausters_document::apply(
            &mut self.0,
            &request.intent,
            &against,
            &clausters_document::Rules {
                quant: request.quant,
            },
        );
        serde_json::to_string(&serde_json::json!({
            "effective": outcome.effective,
            "applied": outcome.applied,
            "reason": outcome.reason,
            "stale": outcome.stale,
        }))
        .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Resolve a selection to the spans of samples underneath it.
    /// `resolve(requestJson) -> resolvedJson`, the request carrying
    /// `{ selection, framesPerBeat, framesPerSecond, inBeats? }` — two ratios
    /// because a placement is in beats and a take's length is in seconds.
    pub fn resolve(&self, request: &str) -> Result<String, JsError> {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Request {
            selection: clausters_document::Selection,
            frames_per_beat: f64,
            frames_per_second: f64,
            #[serde(default)]
            in_beats: bool,
        }
        let request: Request =
            serde_json::from_str(request).map_err(|e| JsError::new(&format!("resolve: {e}")))?;
        let mapping = clausters_document::Mapping {
            frames_per_beat: request.frames_per_beat,
            frames_per_second: request.frames_per_second,
            unit: if request.in_beats {
                clausters_document::Unit::Beats
            } else {
                clausters_document::Unit::Frames
            },
        };
        let resolved: Vec<_> = clausters_document::resolve(&self.0, &request.selection, &mapping)
            .into_iter()
            .map(|r| {
                serde_json::json!({
                    "node": r.node,
                    "source": r.source,
                    "generation": r.generation,
                    "range": r.range,
                    "at": r.at,
                })
            })
            .collect();
        serde_json::to_string(&resolved).map_err(|e| JsError::new(&e.to_string()))
    }
}

// ---- the undo log ----
//
// A `Drop`-backed object, like `Document` beside it and for a related reason:
// a log is state, and the state is the point. The spill store is why — a bulk
// inverse leaves the log deliberately, so passing one by value would carry
// every spilled span on every call, which is the cost spilling exists to
// avoid.

/// The undo history of one document, the JS face of
/// [`clausters_document::Log`].
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = Log)]
pub struct JsLog(clausters_document::Log);

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = Log)]
impl JsLog {
    /// A new log. `budget` is how many entries it keeps before the oldest falls
    /// off and `spillAbove` how many `f32` values a sample payload must reach
    /// before it leaves the log; either as 0 takes the crate's default.
    #[wasm_bindgen(constructor)]
    pub fn new(budget: usize, spill_above: usize) -> JsLog {
        let mut log = clausters_document::Log::new();
        if budget > 0 {
            log = log.budget(budget);
        }
        if spill_above > 0 {
            log = log.spill_above(spill_above);
        }
        JsLog(log)
    }

    /// Apply an edit to `document` **and record it**, in one call: the inverse
    /// has to be read out of the document before the edit lands, so applying
    /// first and recording second would record the wrong thing.
    /// `apply(document, requestJson) -> outcomeJson`, the request carrying
    /// `{ intent, against?, quant?, label? }`.
    pub fn apply(&mut self, document: &mut JsDocument, request: &str) -> Result<String, JsError> {
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
        let request: Request =
            serde_json::from_str(request).map_err(|e| JsError::new(&format!("log.apply: {e}")))?;
        let against = request
            .against
            .unwrap_or_else(clausters_document::Against::unstated);
        let outcome = clausters_document::apply_logged(
            &mut document.0,
            &request.intent,
            &against,
            &clausters_document::Rules {
                quant: request.quant,
            },
            &mut self.0,
            request.label.unwrap_or_else(|| "edit".into()),
        );
        serde_json::to_string(&serde_json::json!({
            "effective": outcome.effective,
            "applied": outcome.applied,
            "reason": outcome.reason,
            "stale": outcome.stale,
        }))
        .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Record an entry the document cannot supply the inverse for — the
    /// destructive case, whose overwritten samples are not in the tree. Applies
    /// nothing. `record(requestJson)` with
    /// `{ forward, backward, label?, coalesce? }`.
    pub fn record(&mut self, request: &str) -> Result<(), JsError> {
        #[derive(serde::Deserialize)]
        struct Request {
            forward: clausters_document::Step,
            backward: clausters_document::Intent,
            #[serde(default)]
            label: Option<String>,
            #[serde(default)]
            coalesce: bool,
        }
        let request: Request =
            serde_json::from_str(request).map_err(|e| JsError::new(&format!("log.record: {e}")))?;
        let mut entry = clausters_document::Entry::new(
            request.label.unwrap_or_else(|| "edit".into()),
            request.forward,
            request.backward,
        );
        if request.coalesce {
            entry = entry.continuing();
        }
        self.0.record(entry);
        Ok(())
    }

    /// Undo the last transaction, applying its inverses to `document`.
    /// Returns `{ undone }`, or `undefined` when there was nothing to undo.
    pub fn undo(&mut self, document: &mut JsDocument) -> Result<Option<String>, JsError> {
        let Some(undone) = self.0.undo() else {
            return Ok(None);
        };
        for intent in &undone {
            // An undo is authoritative: it states what the document was, so it
            // is not checked against a version it predates.
            clausters_document::apply(
                &mut document.0,
                intent,
                &clausters_document::Against::unstated(),
                &clausters_document::Rules::default(),
            );
        }
        serde_json::to_string(&serde_json::json!({ "undone": undone }))
            .map(Some)
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Redo what was last undone, applying what it can to `document`. Returns
    /// `{ remaining }` — the ordinary edits at the front are already applied,
    /// and `remaining` holds the steps from the first one the crate cannot
    /// perform onward, for the owner to re-run. `undefined` when there was
    /// nothing to redo.
    pub fn redo(&mut self, document: &mut JsDocument) -> Result<Option<String>, JsError> {
        let Some(steps) = self.0.redo() else {
            return Ok(None);
        };
        let mut remaining = Vec::new();
        let mut redone = Vec::new();
        let mut stopped = false;
        for step in steps {
            match (&step, stopped) {
                (clausters_document::Step::Edit(intent), false) => {
                    clausters_document::apply(
                        &mut document.0,
                        intent,
                        &clausters_document::Against::unstated(),
                        &clausters_document::Rules::default(),
                    );
                    // Reported as well as applied: a redo is the same shape as
                    // an undo, a list of intents the caller projects.
                    redone.push(intent.clone());
                }
                _ => {
                    stopped = true;
                    remaining.push(step);
                }
            }
        }
        serde_json::to_string(&serde_json::json!({ "redone": redone, "remaining": remaining }))
            .map(Some)
            .map_err(|e| JsError::new(&e.to_string()))
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

    /// What an undo would be called, for a menu item.
    #[wasm_bindgen(getter, js_name = undoLabel)]
    pub fn undo_label(&self) -> Option<String> {
        self.0.undo_label().map(str::to_string)
    }

    /// What a redo would be called.
    #[wasm_bindgen(getter, js_name = redoLabel)]
    pub fn redo_label(&self) -> Option<String> {
        self.0.redo_label().map(str::to_string)
    }

    /// How many entries the log holds.
    #[wasm_bindgen(getter)]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the log holds nothing — `len == 0`, spelled the way a JS
    /// collection is read, as `JsScheduler` and `JsRegistry` already spell it.
    #[wasm_bindgen(getter, js_name = isEmpty)]
    pub fn is_empty(&self) -> bool {
        self.0.len() == 0
    }

    /// Forget everything, releasing what was spilled.
    pub fn clear(&mut self) {
        self.0.clear();
    }
}

/// The patcher's **cord→bus pass**: a directed patch (`{boxes, cords}`) in, the
/// buses and wired members it compiles to out, both as JSON.
///
/// One bus per connected net, its writers summing, and a bad cord — reversed,
/// rate-mismatched, out of range — comes back as `{"error": …}` naming it. The
/// same door the C ABI opens as `clausters_core_patch_compile`: a patcher is a
/// model with one compilation, and a second implementation of it in TypeScript
/// would be a second answer to "what does this cord mean".
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = patchCompile)]
pub fn patch_compile(patch: &str) -> Result<String, JsError> {
    use clausters_core::patch::{Patch, compile};

    let parsed: Patch =
        serde_json::from_str(patch).map_err(|e| JsError::new(&format!("patch: {e}")))?;
    match compile(&parsed) {
        Ok(compiled) => serde_json::to_string(&compiled).map_err(|e| JsError::new(&e.to_string())),
        Err(e) => Err(JsError::new(&e)),
    }
}

// ---- the notation layer ----------------------------------------------------
//
// The engraving logic is the core's and there is one of it: the SVG walk, the
// MEI encoder, and the editable `Score` -- the order an edit is made in, the
// reload that keeps the timemap honest, the undo stack of MEI snapshots. What
// differs in a page is only *where verovio is*: not a linked C++ library but a
// module the page loads, so the engraver arrives as a JS object and this door
// makes it look like the port the core drives.
//
// The methods that object must have are exactly verovio's toolkit calls, in
// this language's spelling: `loadData(data)`, `renderSvg(page)`, `mei()`,
// `edit(actionJson)`, `timemap(optionsJson)`, `midiValues(id)`. The web client
// wraps the published toolkit in one; anything else answering to those six is
// as good, which is what makes this testable without an engraver at all.

/// The engraver as a page has it: a JS object with the six toolkit calls.
///
/// Every crossing is `Reflect::get` plus a call, and every failure — a missing
/// method, a thrown exception, a value of the wrong shape — reads as the same
/// thing a refusal reads as. That is the port's rule (failure is a value), and
/// it is what keeps a broken engraver from taking a page's edit half-applied:
/// the score rolls back either way.
#[cfg(target_arch = "wasm32")]
struct JsEngraver {
    object: js_sys::Object,
}

#[cfg(target_arch = "wasm32")]
impl JsEngraver {
    /// Call `name` with `args`, or `None` when the method is missing or threw.
    fn call(&self, name: &str, args: &[JsValue]) -> Option<JsValue> {
        let method = js_sys::Reflect::get(&self.object, &JsValue::from_str(name)).ok()?;
        let method = method.dyn_ref::<js_sys::Function>()?;
        let out = match args {
            [] => method.call0(&self.object),
            [a] => method.call1(&self.object, a),
            [a, b] => method.call2(&self.object, a, b),
            _ => return None,
        };
        out.ok()
    }

    fn text(&self, name: &str, args: &[JsValue]) -> Option<String> {
        self.call(name, args)?.as_string()
    }
}

#[cfg(target_arch = "wasm32")]
impl clausters_core::notation::Engraver for JsEngraver {
    /// A page has one thread and the engraver is reached from it alone, so
    /// there is nothing to serialize. The guard exists for the native binding,
    /// where libverovio's process-wide state makes it load-bearing.
    type Guard = ();

    fn lock(&self) -> Self::Guard {}

    fn load_data(&self, data: &str) -> bool {
        self.call("loadData", &[JsValue::from_str(data)])
            .map(|v| v.is_truthy())
            .unwrap_or(false)
    }

    fn render_svg(&self, page: i32) -> String {
        self.text("renderSvg", &[JsValue::from_f64(page as f64)])
            .unwrap_or_default()
    }

    fn mei(&self) -> String {
        self.text("mei", &[]).unwrap_or_default()
    }

    fn edit(&self, action: &str) -> bool {
        self.call("edit", &[JsValue::from_str(action)])
            .map(|v| v.is_truthy())
            .unwrap_or(false)
    }

    fn timemap(&self, options: &str) -> String {
        self.text("timemap", &[JsValue::from_str(options)])
            .unwrap_or_default()
    }

    fn midi_values(&self, xml_id: &str) -> Option<String> {
        self.text("midiValues", &[JsValue::from_str(xml_id)])
            .filter(|s| !s.is_empty())
    }
}

/// A loaded score, held open in Rust so it can be edited and re-engraved — the
/// JS face of [`clausters_core::notation::Score`].
///
/// The same object the Python client holds over the C ABI, running the same
/// state machine: a page that transposes a note and one that transposes it in a
/// window take the identical sequence of calls to verovio.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = Score)]
pub struct JsScore(clausters_core::notation::Score<JsEngraver>);

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = Score)]
impl JsScore {
    /// Load `data` (any format the engraver auto-detects) on `engraver`, or
    /// throw when it could not be read.
    ///
    /// Configuring the engraver — its resource path, its options — happens on
    /// the JS side before this, exactly as the native binding configures its
    /// toolkit before handing it over.
    #[wasm_bindgen(constructor)]
    pub fn new(engraver: js_sys::Object, data: &str) -> Result<JsScore, JsError> {
        clausters_core::notation::Score::open(JsEngraver { object: engraver }, data)
            .map(JsScore)
            .ok_or_else(|| JsError::new("the engraver could not load the score data"))
    }

    /// This score engraved into a page: the display list the host draws, the
    /// cursor track a playhead follows, and the notes that sound.
    #[wasm_bindgen(js_name = displayList)]
    pub fn display_list(&mut self, page: i32) -> Result<String, JsError> {
        serde_json::to_string(&self.0.display_list(page)).map_err(|e| JsError::new(&e.to_string()))
    }

    /// The score as MEI, ids and all — what to persist, and what an undo step
    /// is made of.
    pub fn mei(&self) -> String {
        self.0.mei()
    }

    /// Apply one **model** operation as a single undo step, and re-engrave.
    ///
    /// This is the edit path, and the reason it is not the engraver's editor:
    /// there is one implementation of what an edit to a score means, and it is
    /// vocabulary this package already binds. Returns whether it was applied;
    /// a refusal leaves both the page and the model as they were.
    pub fn apply(&mut self, op: &str) -> Result<bool, JsError> {
        let op: clausters_core::notation::Op =
            serde_json::from_str(op).map_err(|e| JsError::new(&format!("op: {e}")))?;
        Ok(self.0.apply(&op))
    }

    /// The open score as the **model**.
    ///
    /// Throws when the document could not be read into one — a state and not a
    /// failure, since the page still draws and still plays and only the model's
    /// verbs are unavailable on it.
    pub fn sheet(&self) -> Result<String, JsError> {
        let sheet = self.0.sheet().ok_or_else(|| {
            JsError::new(
                "this document could not be read into the score model, so the \
                 model's verbs are not available on it; the page still draws \
                 and still plays",
            )
        })?;
        serde_json::to_string(sheet).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Whether there is an edit to step back over.
    #[wasm_bindgen(getter, js_name = canUndo)]
    pub fn can_undo(&self) -> bool {
        self.0.can_undo()
    }

    /// Whether there is an undone edit to step forward into.
    #[wasm_bindgen(getter, js_name = canRedo)]
    pub fn can_redo(&self) -> bool {
        self.0.can_redo()
    }

    /// Step back one edit; `false` when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        self.0.undo()
    }

    /// Step forward again after an undo; `false` when there is nothing to redo.
    pub fn redo(&mut self) -> bool {
        self.0.redo()
    }

    /// Move a note by `steps` diatonic steps along the staff, as one undo step.
    /// The relative form: reach for it only when the delta is what you have.
    pub fn transpose(&mut self, element_id: &str, steps: i32) -> bool {
        self.0.transpose(element_id, steps)
    }

    /// Move a note **to** a diatonic staff position, as one undo step — the
    /// shape an edit travels in, so a resend cannot move the note twice.
    #[wasm_bindgen(js_name = transposeTo)]
    pub fn transpose_to(&mut self, element_id: &str, position: i32, page: i32) -> bool {
        self.0.transpose_to(element_id, position, page)
    }

    /// One raw editor action (`set`, `insert`, `delete`, …) as a single undo
    /// step, `param` being its parameter object as JSON.
    pub fn edit(&mut self, action: &str, param: &str) -> bool {
        self.0.edit(action, param)
    }
}

/// The engraver's options for one page, as the JSON object it is configured
/// with: `scale` (staff size), `pageWidth` (the page units a score wraps into
/// systems at) and an optional JSON object merged over them.
///
/// A page configures its engraver through this rather than through a table of
/// its own, for the same reason the score model is shared: two clients that
/// configure verovio differently draw the same score two ways, and then no
/// display list from one is comparable with one from the other.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = engraveOptions)]
pub fn engrave_options(scale: i32, page_width: i32, extra: Option<String>) -> String {
    clausters_core::notation::engrave_options(scale, page_width, extra.as_deref())
}

/// Walk a verovio SVG into a `score` display list, as JSON.
///
/// The one-shot path: a page that only draws a score engraves once and walks
/// the SVG, with no document held open. A malformed SVG yields an empty display
/// list rather than an error, as the C ABI's twin does.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = svgToDisplayList)]
pub fn svg_to_display_list(svg: &str) -> Result<String, JsError> {
    serde_json::to_string(&clausters_core::notation::svg_to_display_list(svg))
        .map_err(|e| JsError::new(&e.to_string()))
}

/// Lay a **voice** — a JSON array of slots, `{"midis": [60], "ticks": 8}` per
/// note or chord and `{"ticks": 8}` per rest — out into barred, tied MEI.
///
/// `meter` is `"num/den"`, `clef` a shape+line like `"G2"`, and `key` selects
/// the key signature and the sharp-vs-flat spelling. Reducing a client's own
/// sequencing data to that voice stays in the client, where the native types
/// are; this is the language-agnostic step below it, and the seam a richer
/// encoding extends for every client at once.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = voiceToMei)]
pub fn voice_to_mei(voice: &str, meter: &str, clef: &str, key: &str) -> Result<String, JsError> {
    let voice: Vec<clausters_core::notation::Slot> =
        serde_json::from_str(voice).map_err(|e| JsError::new(&format!("voice: {e}")))?;
    Ok(clausters_core::notation::voice_to_mei(
        &voice, meter, clef, key,
    ))
}

/// Lift a **voice** — the v1 wire form, a JSON array of slots — into the score
/// model.
///
/// The bridge a client crosses once: it reduces its own sequencing types to
/// slots, which reads client-native types and stays here, and everything above
/// that is the model. Ticks become exact durations and MIDI numbers become
/// spelled pitches in the world `key` implies.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = voiceToSheet)]
pub fn voice_to_sheet(voice: &str, meter: &str, clef: &str, key: &str) -> Result<String, JsError> {
    let voice: Vec<clausters_core::notation::Slot> =
        serde_json::from_str(voice).map_err(|e| JsError::new(&format!("voice: {e}")))?;
    let sheet = clausters_core::notation::voice_to_sheet(&voice, meter, clef, key);
    serde_json::to_string(&sheet).map_err(|e| JsError::new(&e.to_string()))
}

/// Apply one operation to a score model, both as JSON, returning the new model.
///
/// **One export for every operation there will ever be**: the verb and its
/// parameters are inside `op` (`{"op": "transpose", "semitones": 2}`), so a new
/// operation costs nothing here. What the C ABI answers in an envelope
/// (`{"ok": …}` / `{"error": …}`) this **throws** instead, which is the same
/// behaviour in the shape a page expects — and the reason the refusal reaches
/// the caller either way, since a refused operation has to say why.
///
/// A refused operation changes nothing: the model crossed by value, so the
/// caller still holds what it sent.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = sheetApply)]
pub fn sheet_apply(sheet: &str, op: &str) -> Result<String, JsError> {
    let sheet: clausters_core::notation::Sheet =
        serde_json::from_str(sheet).map_err(|e| JsError::new(&format!("sheet: {e}")))?;
    let op: clausters_core::notation::Op =
        serde_json::from_str(op).map_err(|e| JsError::new(&format!("op: {e}")))?;
    let out = clausters_core::notation::apply(sheet, &op).map_err(|e| JsError::new(&e))?;
    serde_json::to_string(&out).map_err(|e| JsError::new(&e.to_string()))
}

/// Write a score model out as MEI.
///
/// Throws with the emitter's own reason when the model holds something MEI
/// cannot be written for yet — a duration that is not an exact note value, an
/// accidental past a double, or polyphony — each saying which it is, so a
/// caller knows whether it is wrong or early.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = sheetToMei)]
pub fn sheet_to_mei(sheet: &str) -> Result<String, JsError> {
    let sheet: clausters_core::notation::Sheet =
        serde_json::from_str(sheet).map_err(|e| JsError::new(&format!("sheet: {e}")))?;
    clausters_core::notation::sheet_to_mei(&sheet).map_err(|e| JsError::new(&e))
}

/// Every operation this core knows, as JSON — the verb and the parameters each
/// takes.
///
/// The parity surface the binding table cannot provide: operations cross as
/// data, so nothing fails when one client grows a verb the other lacks. This
/// package is contrasted against this list, as the Python client is.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = sheetOps)]
pub fn sheet_ops() -> Result<String, JsError> {
    serde_json::to_string(clausters_core::notation::catalog())
        .map_err(|e| JsError::new(&e.to_string()))
}

/// Read an MEI document into the score model.
///
/// The other return path, and not the one {@link sheetPerform} is: that turns a
/// model into sound, this turns a *document* into a model. A score opened from
/// typed text is a document and nothing else until this reads one, which is why
/// the model's verbs cannot touch it until then. There is one input format rather than
/// four, because the engraver normalizes whatever it loaded to MEI.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = meiToSheet)]
pub fn mei_to_sheet(mei: &str) -> Result<String, JsError> {
    let sheet = clausters_core::notation::mei_to_sheet(mei).map_err(|e| JsError::new(&e))?;
    serde_json::to_string(&sheet).map_err(|e| JsError::new(&e.to_string()))
}

/// Read a score model into the notes it **sounds**, under `interp`.
///
/// Each note carries two lengths — `dur`, what is written, and `sustain`, what
/// is heard — because an honoured articulation makes them different numbers and
/// collapsing them would move every attack after a staccato. It also names the
/// `staff` and `voice` it was written on, which is what a caller binds an
/// instrument to: the notation does not say what plays it.
///
/// `interp` may be `""` or `"{}"` for the default reading, and any field left
/// out keeps its default. What the default *is* comes back from
/// [`interpretation`], so this package writes none of those numbers down.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = sheetPerform)]
pub fn sheet_perform(sheet: &str, interp: &str) -> Result<String, JsError> {
    let sheet: clausters_core::notation::Sheet =
        serde_json::from_str(sheet).map_err(|e| JsError::new(&format!("sheet: {e}")))?;
    let interp: clausters_core::notation::Interpretation = if interp.trim().is_empty() {
        clausters_core::notation::default_interpretation()
    } else {
        serde_json::from_str(interp).map_err(|e| JsError::new(&format!("interpretation: {e}")))?
    };
    let notes = clausters_core::notation::perform(sheet, &interp).map_err(|e| JsError::new(&e))?;
    serde_json::to_string(&notes).map_err(|e| JsError::new(&e.to_string()))
}

/// The default interpretation, as JSON — every number the reading depends on,
/// and the value an override starts from.
///
/// The parity surface for the reading, as `sheetOps` is for the verbs: the
/// interpretation crosses inside a payload, so nothing structural notices when
/// one client's idea of `mf` drifts from the other's.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = interpretation)]
pub fn interpretation() -> Result<String, JsError> {
    serde_json::to_string(&clausters_core::notation::default_interpretation())
        .map_err(|e| JsError::new(&e.to_string()))
}

/// The **model** item an engraved element belongs to, or `-1` when the element
/// was not written from one.
///
/// The page names elements the way the emitter wrote them: `n7` is the item,
/// `n7-2` a piece of it split across a barline, `n7-p1` one pitch of a chord.
/// All three are the same item, which is what lets a gesture anywhere on a note
/// reach the note — and it is the step a client takes between a page's
/// selection and a model verb.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = itemId)]
pub fn item_id(element_id: &str) -> f64 {
    clausters_core::notation::item_id(element_id).map_or(-1.0, |id| id as f64)
}

// ---- MIDI files (W9) --------------------------------------------------------
//
// A page has no filesystem and no virtual OS port, but it does have a score to
// write: `MidiServer` over an NRT interface accumulates `(beat, message)` and
// hands the ticked events here. The writers are `clausters-midi`'s -- the same
// ones `clausters_midi_write_smf`/`_write_clip` open to the Python client -- so
// a `.mid` saved from a tab and one saved from a script are the same bytes, and
// a client that offers the take as a download re-implements nothing.
//
// The JS face takes the events flat, in the shape the C ABI already takes them:
// `ticks` is n absolute ticks and `msgs` is 3n bytes (status, data1, data2).
// Passing an array of objects would be the only other option and it would cost a
// JS-side allocation per event on a path a bounce runs over every note.

/// Reads the flat `(ticks, msgs)` pair into events, checking the lengths agree.
#[cfg(target_arch = "wasm32")]
fn midi_events(ticks: &[u32], msgs: &[u8]) -> Result<Vec<clausters_midi::TimedMessage>, JsError> {
    if msgs.len() != 3 * ticks.len() {
        return Err(JsError::new(&format!(
            "msgs is {} bytes, not 3 per tick ({} ticks)",
            msgs.len(),
            ticks.len()
        )));
    }
    Ok(ticks
        .iter()
        .enumerate()
        .map(|(i, &tick)| clausters_midi::TimedMessage {
            tick,
            bytes: [msgs[3 * i], msgs[3 * i + 1], msgs[3 * i + 2]],
        })
        .collect())
}

/// Type-0 Standard MIDI File bytes from `n` events at `ppq` ticks per quarter
/// note.
///
/// JS face: `midiWriteSmf(Uint32Array, Uint8Array, ppq) -> Uint8Array`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = midiWriteSmf)]
pub fn midi_write_smf(ticks: &[u32], msgs: &[u8], ppq: u16) -> Result<Vec<u8>, JsError> {
    Ok(clausters_midi::write_smf(&midi_events(ticks, msgs)?, ppq))
}

/// MIDI 2.0 Clip File (SMF2CLIP) bytes from the same arguments, carrying note
/// velocities at 16-bit resolution.
///
/// JS face: `midiWriteClip(Uint32Array, Uint8Array, ppq) -> Uint8Array`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = midiWriteClip)]
pub fn midi_write_clip(ticks: &[u32], msgs: &[u8], ppq: u16) -> Result<Vec<u8>, JsError> {
    Ok(clausters_midi::write_clip(&midi_events(ticks, msgs)?, ppq))
}
