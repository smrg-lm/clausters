//! libfaust embedding (Box API + LLVM JIT) — the F fork of the plan.
//!
//! `ffi` is a hand-written FFI over the libfaust C API; `compiler` runs the
//! dedicated compilation thread; `boxes` maps JSON defs to Box API
//! calls (the schema is documented there); `factory` owns compiled
//! factories; `synth` puts JIT instances in the node tree as `SynthNode`s
//!. The FFI started in F0 as a minimal binding, verified
//! against the headers of the exact libfaust build we link (see `ffi`). The
//! decision log lives in LOG.md: distro packages ship without the LLVM
//! backend and without headers, and the existing crates (`faust-build`,
//! `faust-types`) do build-time Faust→Rust codegen, not JIT embedding — so
//! libfaust is built from source and the binding is ours. bindgen remains an
//! option if the surface grows.
//!
//! Threading contract (see the `faust-embedding` skill): everything in `ffi`
//! except `computeCDSPInstance` allocates or locks and must stay off the
//! audio thread; the lib context is global and single-threaded — the compiler's
//! dedicated compiler thread serializes it naturally.

pub mod boxes;
pub mod cache;
pub mod compiler;
pub mod factory;
pub mod ffi;
pub mod json_util;
pub mod signals;
pub mod synth;
pub mod wasm_module;
