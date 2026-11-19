//! **Where the editor's binary looks for libverovio**, which the `score`
//! feature links through the host's engraver. A link argument is the linking
//! package's and stops there, so the rpath the host's own `build.rs` writes
//! for its binary never reaches this one: the same recipe, stated again
//! (`$ORIGIN`, `$ORIGIN/../_libs` for a staged copy, and the prefix
//! `clausters-verovio` found, published as `DEP_VEROVIO_PREFIX`).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_FEATURE_SCORE").is_err() {
        return;
    }
    let Ok(prefix) = std::env::var("DEP_VEROVIO_PREFIX") else {
        return;
    };
    println!("cargo:rustc-link-arg-bins=-Wl,--disable-new-dtags");
    println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN");
    println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN/../_libs");
    println!("cargo:rustc-link-arg-bins=-Wl,-rpath,{prefix}/lib");
}
