//! The rpath that finds libfaust, for this crate's own binaries (its tests).
//!
//! A workspace build unifies the server's features, so the engine this crate
//! takes is built with the `faust` family whenever another member asks for
//! it, and its test binaries then link libfaust. A build script's
//! `rustc-link-arg` reaches only its own crate's artifacts, so the server's
//! rpath does not reach these: the prefix travels as `DEP_FAUST_PREFIX`
//! (published by the server's script through its `links` key) and the same
//! rpath is emitted again, as `DT_RPATH` for the reason the server's script
//! gives. Without the family, or on wasm32, there is nothing to find.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        return;
    }
    let Ok(prefix) = std::env::var("DEP_FAUST_PREFIX") else {
        return;
    };
    println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../_libs");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{prefix}/lib");
}
