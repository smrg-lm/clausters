//! The FaustDef family: one def kind, two backends.
//!
//! A `FaustDef` is a compiled Faust program the node tree can instantiate, and
//! the `faust` feature means exactly that the family exists — **not** that
//! libfaust is linked in. Which compiler produces the def and how an instance
//! is computed depend on the target:
//!
//! - **Native** — libfaust with the LLVM JIT, embedded through the
//!   hand-written [`ffi`] binding. [`compiler`] runs the dedicated compilation
//!   thread, [`boxes`] and [`signals`] map the JSON def formats to the Box and
//!   Signal APIs, [`factory`] owns a compiled factory and [`cache`] persists
//!   its bitcode. The decision log records why libfaust is built from source
//!   and the binding is ours: distro packages ship without the LLVM backend
//!   and without headers, and the existing crates (`faust-build`,
//!   `faust-types`) do build-time Faust→Rust codegen, not JIT embedding.
//! - **wasm32** — a page has no LLVM, so the compiler is `libfaust-wasm`
//!   running in the engine's Worker and the def arrives as a **second wasm
//!   module linked into the engine's own linear memory**, its `compute` reached
//!   through the engine's `__indirect_function_table`. `compiler_web` is the
//!   queue the host drains and answers; `synth_web` is the node. See
//!   `docs/decisions.md`, "The page's Faust is a second wasm module linked into
//!   the engine's own memory".
//!
//! Both backends publish the same `faust::compiler` and `faust::synth` paths,
//! so everything above them — the def table, `/def_send faust`, `/node_set`,
//! the bus mapping, the done actions — is one piece of code.
//!
//! Threading contract on the native side (see the `faust-embedding` skill):
//! everything in `ffi` except `computeCDSPInstance` allocates or locks and must
//! stay off the audio thread; the lib context is global and single-threaded —
//! the dedicated compiler thread serializes it naturally. In a page the same
//! separation is the Worker's, and the audio thread only ever calls the
//! module's `compute`.

pub mod json_ui;
pub mod wasm_module;

#[cfg(not(target_arch = "wasm32"))]
pub mod json_util;

#[cfg(not(target_arch = "wasm32"))]
pub mod boxes;
#[cfg(not(target_arch = "wasm32"))]
pub mod cache;
#[cfg(not(target_arch = "wasm32"))]
pub mod factory;
#[cfg(not(target_arch = "wasm32"))]
pub mod ffi;
#[cfg(not(target_arch = "wasm32"))]
pub mod signals;

#[cfg(not(target_arch = "wasm32"))]
pub mod compiler;
#[cfg(target_arch = "wasm32")]
#[path = "compiler_web.rs"]
pub mod compiler;

#[cfg(not(target_arch = "wasm32"))]
pub mod synth;
#[cfg(target_arch = "wasm32")]
#[path = "synth_web.rs"]
pub mod synth;

/// One named parameter of a Faust def, as declared by its UI elements. The
/// same shape whichever backend compiled the def: `/node_set` writes the value
/// into the instance's zone, and the control index is this parameter's
/// position in declaration order.
pub struct ParamSpec {
    pub name: String,
    pub init: f32,
    pub min: f32,
    pub max: f32,
    pub step: f32,
}

/// What `/def_send faust` carries: one of the three def formats. Shared by
/// both backends — the wire is the same in a window and in a tab.
pub enum CompilePayload {
    /// Raw Faust source code.
    Source(String),
    /// JSON box graph (see [`boxes`] for the schema).
    Json(String),
    /// JSON signal tree, root `{"signals": …}` (see [`signals`]).
    Signal(String),
}

impl CompilePayload {
    /// Classifies a `/def_send faust` def string: raw Faust source unless it
    /// starts with `{`, then a signal tree if the JSON object has a top-level
    /// `"signals"` key, otherwise a box tree. The sniff is unambiguous —
    /// Faust source never starts with `{`, and a box def's root is a single
    /// box node (`{"op": …}`), never an object keyed by `"signals"`.
    pub fn classify(def: String) -> Self {
        if !def.trim_start().starts_with('{') {
            return Self::Source(def);
        }
        let is_signal = serde_json::from_str::<serde_json::Value>(&def)
            .ok()
            .and_then(|v| v.as_object().map(|o| o.contains_key("signals")))
            .unwrap_or(false);
        if is_signal {
            Self::Signal(def)
        } else {
            Self::Json(def)
        }
    }

    /// The format's wire name, as the host's compile job carries it.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Source(_) => "source",
            Self::Json(_) => "boxes",
            Self::Signal(_) => "signals",
        }
    }

    /// The def text itself, whichever format it is in.
    pub fn text(&self) -> &str {
        match self {
            Self::Source(s) | Self::Json(s) | Self::Signal(s) => s,
        }
    }
}
