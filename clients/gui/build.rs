//! **Where this binary looks for libfaust**, and nothing else.
//!
//! The host links no native library of its own. It grows one with the
//! `standalone-faust` feature, which pulls the server's `faust` family: the
//! server crate's own build script emits the `-l faust` and the search path
//! for it, and those *do* reach a binary that depends on the crate. What does
//! not reach it is the rpath, because a link argument is the linking package's
//! and stops there -- so a host built here came out with `NEEDED
//! libfaust.so.2` and no `RPATH` at all, found libfaust nowhere, and the wheel
//! that bundles the library beside it could not run it.
//!
//! So the recipe is stated twice, once per linking package, and it is the same
//! one the root `build.rs` explains: `$ORIGIN` and `$ORIGIN/../_libs` for the
//! staged copy a wheel ships (the binary sits in `clausters/_bin/`, the
//! libraries in `clausters/_libs/`), the build prefix for a developer's own,
//! and `DT_RPATH` rather than `RUNPATH` because only that one is inherited by
//! transitive dependencies -- which is what lets libfaust find libz and
//! libzstd beside itself.
//!
//! Every other build of this crate emits nothing, which is the ordinary one: a
//! host that is a client of a server process links no libfaust and needs no
//! path to one.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=FAUST_PREFIX");

    // The feature that links the embedded server *with* the Faust family.
    // `standalone` alone pulls `synth`, which needs no native library.
    if std::env::var("CARGO_FEATURE_STANDALONE_FAUST").is_err() {
        return;
    }
    // On wasm the family's backend is a module the page compiles and links
    // itself, so there is no library to find and an rpath is not even a valid
    // argument for that linker.
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        return;
    }

    let prefix = std::env::var("FAUST_PREFIX").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_default();
        let local = format!("{home}/.local");
        if std::path::Path::new(&format!("{local}/lib")).exists() {
            local
        } else {
            "/usr/local".into()
        }
    });

    // Only the rpath: the library itself and where to find it at *link* time
    // come from the server crate's own script, which is where the dependency
    // is declared.
    println!("cargo:rustc-link-arg-bins=-Wl,--disable-new-dtags");
    println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN");
    println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN/../_libs");
    println!("cargo:rustc-link-arg-bins=-Wl,-rpath,{prefix}/lib");
}
