//! What the two builds of this crate need from the linker.
//!
//! On wasm32: the engine's function table, exported and growable, so the page
//! can link a Faust module into it, and a linear memory reserved up front
//! rather than grown into. On native: an rpath for the test binary.
//!
//! This crate asks for `clausters` with `synth` + `faust` + `embed`, but
//! `faust` there means the def family, not libfaust — the browser has no
//! libfaust to link. In a workspace build (`cargo test --workspace`)
//! cargo unifies features across members, so the shared `clausters` lib is the
//! one built for the root package, i.e. with `faust` on, and this crate's test
//! binary ends up linking libfaust after all, and needs to find it.
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
        // And a linear memory that is *reserved*, not grown into.
        // `WebAssembly.Memory.grow` detaches the `ArrayBuffer` and every JS
        // view over it, and it is work on the audio thread: in a page the OSC
        // pump runs inside the worklet's `process`, so the allocation a
        // command asks for happens where nothing may allocate. rustc's default
        // is whatever the data segments and the stack need -- 24 pages, 1.5 MB
        // -- and booting the server alone lands at 4.3 MB, so the engine grows
        // dozens of times before it has played a sample.
        //
        // 16 MB covers that boot with room for the defs and the modest buffers
        // a page actually holds; past it growth is a budgeted event rather
        // than a surprise. The ceiling is deliberate too: a tab that asks for
        // more than 256 MB fails at a named limit instead of taking iOS
        // Safari's whole tab down with it near 350 MB. `tests/memory.test.ts`
        // asserts both numbers, and that a booted engine processing blocks
        // does not move off the first one.
        println!("cargo:rustc-cdylib-link-arg=--initial-memory=16777216");
        println!("cargo:rustc-cdylib-link-arg=--max-memory=268435456");
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
