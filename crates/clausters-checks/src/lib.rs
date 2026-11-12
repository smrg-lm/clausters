//! The repository's checks on itself.
//!
//! The crate has no code of its own: its tests read the tree as text and fail
//! on what no compiler looks at. One file under `tests/` per rule:
//!
//! - `ascii` -- a source file is ASCII.
//! - `versions` -- the package version is written in one place.
//! - `levels` -- a crate depends only on crates of a lower level.
//! - `bindings` -- every symbol of the C ABI and of the wasm core has a row in
//!   `docs/bindings.md`.
//!
//! Run them with `cargo test -p clausters-checks`. They link nothing of the
//! project, so they need no audio library, no libfaust and no feature.

use std::path::{Path, PathBuf};

/// The repository's root: two directories above this crate's manifest.
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root")
}
