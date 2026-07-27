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
//! the builtins and the pitch space, and the sample-clock model. Always the
//! same shape: the logic lives in `clausters-core`, natively tested; this
//! shell only converts values at the JS boundary, and only on the wasm target.
//!
//! The JS argument convention mirrors the tagged pairs the interim page codec
//! used: encode takes `[tag, value]` pairs (`"i"`/`"h"`/`"f"`/`"d"`/`"s"`/
//! `"b"`), preserving the int/float distinction explicitly; decode returns
//! plain values (`{addr, args}` per message, bundles flattened), which is
//! what reply consumers want.

use clausters_core::osc::{OscMessage, OscPacket, OscType, decode_packet, encode};
#[cfg(target_arch = "wasm32")]
use clausters_core::{
    builtins, bundle,
    clocksync::SampleClockModel,
    osc,
    registry::{self, NodeIdPartition, Registry},
    rng::Rng,
    tempoclock::{self, Scheduler},
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
    /// the filter for foreign `/n_end` ids.
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

/// A Unix timestamp → the server's absolute sample, through a `/clock` anchor
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

/// JS face: a bundle with the *immediate* timetag → `Uint8Array`. What rides
/// inside `/sched`, whose own absolute sample carries the time.
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
// server's sample axis: `/clock` replies are fed in as anchors, and the model
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

    /// Records one `/clock` observation: the local time it was taken at, the
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
/// `bundle_requirements(manifestJson) -> requirementsJson`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn bundle_requirements(manifest: &str) -> Result<String, JsError> {
    let manifest: bundle::Manifest =
        serde_json::from_str(manifest).map_err(|e| JsError::new(&format!("bundle.json: {e}")))?;
    serde_json::to_string(&bundle::requirements(&manifest))
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
