//! The registry: node ids, buses, buffers.
//!
//! The client-side allocators are all one model (`clausters_core::registry`),
//! so they cross to JS as one class rather than three. The JS names are
//! camelCase because wasm-bindgen renames methods for the language it lands
//! in; the semantics are the Rust ones verbatim -- exhaustion is `undefined`
//! (never a wrap), and a refused release reports instead of corrupting the map.

#[cfg(target_arch = "wasm32")]
use clausters_core::registry;
#[cfg(target_arch = "wasm32")]
use clausters_core::registry::NodeIdPartition;
#[cfg(target_arch = "wasm32")]
use clausters_core::registry::Registry;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

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
    /// or not currently allocated -- a double release).
    pub fn release(&mut self, first: f64, width: u32) -> bool {
        self.0.release(first as i64, width as usize).is_ok()
    }

    /// Whether `id` falls inside this registry's space (allocated or not) --
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

// ---- the id spaces a client allocates from ----
//
// `clausters_core::ids::IdSpaces`: the five spaces sized from the server and
// sliced by a share. The page's allocators and its component pools all stand on
// this, so the policy -- the outputs at the bottom of the audio space, the
// GraphDef windows at the top of both bus spaces, the slice a share takes -- is
// the Rust one and not a restatement of it.

/// A client's id spaces, the JS face of [`clausters_core::ids::IdSpaces`].
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = IdSpaces)]
pub struct JsIdSpaces(pub(crate) clausters_core::ids::IdSpaces);

#[cfg(target_arch = "wasm32")]
fn js_space(name: &str) -> Result<clausters_core::ids::Space, JsError> {
    clausters_core::ids::Space::parse(name)
        .ok_or_else(|| JsError::new(&format!("no id space called {name:?}")))
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = IdSpaces)]
impl JsIdSpaces {
    /// The spaces of a live client of a server of this shape, taking share
    /// `index` of `of`.
    // The arguments are the server's shape, flat, as the C ABI's
    // `clausters_ids_new` takes them: a struct would be a JS object built per
    // call for a constructor that runs once a connection.
    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(constructor)]
    pub fn new(
        max_nodes: u32,
        audio_buses: u32,
        outputs: u32,
        control_buses: u32,
        buffers: u32,
        transports: u32,
        event_buses: u32,
        index: u32,
        of: u32,
    ) -> Result<JsIdSpaces, JsError> {
        let share = clausters_core::ids::IdShare::new(index, of)
            .map_err(|e| JsError::new(&e.to_string()))?;
        Ok(JsIdSpaces(clausters_core::ids::IdSpaces::new(
            js_shape(
                max_nodes,
                audio_buses,
                outputs,
                control_buses,
                buffers,
                transports,
                event_buses,
            ),
            share,
        )))
    }

    /// The spaces of an offline score: node ids never run out.
    #[allow(clippy::too_many_arguments)]
    pub fn score(
        max_nodes: u32,
        audio_buses: u32,
        outputs: u32,
        control_buses: u32,
        buffers: u32,
        transports: u32,
        event_buses: u32,
    ) -> JsIdSpaces {
        JsIdSpaces(clausters_core::ids::IdSpaces::score(js_shape(
            max_nodes,
            audio_buses,
            outputs,
            control_buses,
            buffers,
            transports,
            event_buses,
        )))
    }

    /// A run of `width` ids of `space` (`"nodes"`, `"audio"`, `"control"`,
    /// `"buffers"`, `"transports"`, `"events"`); throws when the space is exhausted.
    pub fn alloc(&mut self, space: &str, width: u32) -> Result<f64, JsError> {
        let space = js_space(space)?;
        self.0
            .alloc(space, width as usize)
            .map(|id| id as f64)
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Returns a run of `space` to the pool; throws on a double free.
    pub fn release(&mut self, space: &str, first: f64, width: u32) -> Result<(), JsError> {
        let space = js_space(space)?;
        self.0
            .release(space, first as i64, width as usize)
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Takes share `index` of `of` of every space, keeping what is allocated;
    /// throws, changing nothing, when something held lies outside the slice.
    pub fn narrow(&mut self, index: u32, of: u32) -> Result<(), JsError> {
        let share = clausters_core::ids::IdShare::new(index, of)
            .map_err(|e| JsError::new(&e.to_string()))?;
        self.0
            .narrow(share)
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// A node the server reports gone, taken back if it was this client's.
    #[wasm_bindgen(js_name = nodeEnded)]
    pub fn node_ended(&mut self, node: f64) -> bool {
        self.0.node_ended(node as i64)
    }

    /// Whether `id` falls inside this client's slice of `space`.
    pub fn contains(&self, space: &str, id: f64) -> Result<bool, JsError> {
        Ok(self.0.contains(js_space(space)?, id as i64))
    }

    /// How many ids of `space` are allocated now.
    #[wasm_bindgen(js_name = inUse)]
    pub fn in_use(&self, space: &str) -> Result<u32, JsError> {
        Ok(self.0.in_use(js_space(space)?) as u32)
    }
}

#[cfg(target_arch = "wasm32")]
fn js_shape(
    max_nodes: u32,
    audio_buses: u32,
    outputs: u32,
    control_buses: u32,
    buffers: u32,
    transports: u32,
    event_buses: u32,
) -> clausters_core::ids::ServerShape {
    clausters_core::ids::ServerShape {
        max_nodes: max_nodes as usize,
        audio_buses: audio_buses as usize,
        outputs: outputs as usize,
        control_buses: control_buses as usize,
        buffers: buffers as usize,
        transports: transports as usize,
        event_buses: event_buses as usize,
    }
}

/// The `[base, span]` of share `index` of `of` within `span` ids at `base`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = shareOf)]
pub fn share_of(base: f64, span: u32, index: u32, of: u32) -> Result<Vec<f64>, JsError> {
    let share =
        clausters_core::ids::IdShare::new(index, of).map_err(|e| JsError::new(&e.to_string()))?;
    let (first, width) = clausters_core::ids::share_of(base as i64, span as usize, share);
    Ok(vec![first as f64, width as f64])
}

/// JS face: the boot-derived node-id partition for a node table of
/// `max_nodes` slots -- `{clientBase, clientCapacity, autoBase, autoCapacity,
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

/// JS face: the `[audio, control]` widths GraphDef instances reserve at the
/// top of bus spaces of these sizes.
///
/// A share of what the server was configured with rather than a fixed number,
/// so ask with the counts *that* server reports.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn graph_bus_reserved(audio_buses: usize, control_buses: usize) -> Vec<u32> {
    vec![
        registry::graph_audio_reserved(audio_buses) as u32,
        registry::graph_control_reserved(control_buses) as u32,
    ]
}
