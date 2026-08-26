//! What the two builds of this crate need from the linker.
//!
//! On wasm32: the engine's function table, exported and growable, so the page
//! can link a Faust module into it. On native: an rpath for the test binary.
//!
//! This crate asks for `clausters` with `synth` + `embed` and no Faust — the
//! browser has no libfaust. But in a workspace build (`cargo test --workspace`)
//! cargo unifies features across members, so the shared `clausters` lib is the
//! one built for the root package, i.e. with `faust` on, and this crate's test
//! binary ends up linking libfaust after all.
//!
//! Link args do not cross package boundaries: the `cargo:rustc-link-arg` lines
//! in the root `build.rs` apply to the root package's own targets, not to
//! dependents. Without the rpath repeated here the test binary builds fine and
//! then fails at startup with `libfaust.so.2: cannot open shared object file`.
//!
//! Only the rpath is repeated — the link-search and link-lib come from the root
//! script, through the dependency. Keep the prefix search in step with it.

fn main() {
    println!("cargo:rerun-if-env-changed=FAUST_PREFIX");
    // wasm32 is the real target of this crate and links no native libfaust.
    // What it does need is a table the *page* can write into: a Faust def is a
    // second wasm module instantiated against this engine's memory, and its
    // `compute` is reached by appending it to this module's
    // `__indirect_function_table` and calling through the slot. The table is
    // internal by default and fixed-size, so both flags are load-bearing —
    // without the first the host cannot see it, without the second it cannot
    // grow it. See `clausters::faust::synth` (the wasm backend).
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        println!("cargo:rustc-cdylib-link-arg=--export-table");
        println!("cargo:rustc-cdylib-link-arg=--growable-table");
        return;
    }

    let prefix = std::env::var("FAUST_PREFIX").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_default();
        let local = format!("{home}/.local");
        if std::path::Path::new(&format!("{local}/lib/libfaust.so")).exists()
            || std::path::Path::new(&format!("{local}/lib/libfaust.a")).exists()
        {
            local
        } else {
            "/usr/local".into()
        }
    });

    // Harmless when the workspace is built without `faust`: an rpath entry
    // pointing at a directory with no libfaust in it is simply not used.
    println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../_libs");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{prefix}/lib");
}
