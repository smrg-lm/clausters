//! Linker configuration for the `verovio` feature (off by default). Without it
//! this script does nothing.
//!
//! The cdylib this crate produces is what a binding loads, so it — not just the
//! `clausters-notation` rlib underneath it — needs the rpath that finds
//! libverovio at run time. A build script's `rustc-link-arg` only reaches its own
//! crate's artifacts, so the prefix travels here as `DEP_VEROVIO_PREFIX`
//! (published by `clausters-notation`'s script through its `links` key) and the
//! same rpath is emitted again.
//!
//! `$ORIGIN` and `$ORIGIN/../_libs` come before the build-time prefix, which is
//! what lets the Python wheel ship libverovio beside the cdylibs; `DT_RPATH`
//! (`--disable-new-dtags`) rather than `DT_RUNPATH`, because only that one is
//! inherited by transitive dependencies.

fn main() {
    if std::env::var_os("CARGO_FEATURE_VEROVIO").is_none() {
        return;
    }
    let Ok(prefix) = std::env::var("DEP_VEROVIO_PREFIX") else {
        return;
    };
    println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../_libs");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{prefix}/lib");
}
