//! Linker configuration for the `verovio` feature (off by default). Without it
//! this script does nothing and the crate is empty, so a plain build links no
//! libverovio.
//!
//! libverovio is the engraving library `third_party/build-verovio.sh` installs
//! from a pinned source; it is located through the `VEROVIO_PREFIX` environment
//! variable (the same one `clients/python/build_native.py` stages from), falling
//! back to `~/.local`, then `/usr/local`. This mirrors how the server's build.rs
//! links libfaust.
//!
//! The rpath keeps the artifacts **relocatable**: `$ORIGIN` and
//! `$ORIGIN/../_libs` come before the build-time prefix, which is what lets the
//! Python wheel ship libverovio beside the cdylibs. It is emitted as `DT_RPATH`
//! (`--disable-new-dtags`) rather than `DT_RUNPATH` for the same reason libfaust
//! is: only `DT_RPATH` is inherited by transitive dependencies.

fn main() {
    println!("cargo:rerun-if-env-changed=VEROVIO_PREFIX");
    if std::env::var_os("CARGO_FEATURE_VEROVIO").is_none() {
        return;
    }

    let prefix = std::env::var("VEROVIO_PREFIX").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_default();
        let local = format!("{home}/.local");
        if has_libverovio(&local) {
            local
        } else {
            "/usr/local".into()
        }
    });

    if !has_libverovio(&prefix) {
        println!(
            "cargo:warning=no libverovio under {prefix}/lib: the `verovio` feature is on but \
             libverovio is not built (third_party/build-verovio.sh has the recipe). Point \
             VEROVIO_PREFIX at it, or drop the feature for a build with no engraver."
        );
    }

    // The build prefix's SMuFL data, as a run-time fallback resource path: verovio
    // bakes in a configure-time path that need not match where we link it from, so
    // `default_resource_path` uses this when `CLAUSTERS_VEROVIO` is unset. The
    // staged wheel overrides it at run time; this is the dev-checkout convenience.
    println!("cargo:rustc-env=CLAUSTERS_VEROVIO_RESOURCES={prefix}/share/verovio");

    // Published as `DEP_VEROVIO_PREFIX` to dependents (see the `links` key): the
    // rpath below only applies to this crate's own artifacts, so clausters-ffi's
    // cdylib emits its own from the same prefix rather than re-deriving it.
    println!("cargo:prefix={prefix}");

    println!("cargo:rustc-link-search=native={prefix}/lib");
    println!("cargo:rustc-link-lib=dylib=verovio");
    println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../_libs");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{prefix}/lib");
}

/// Whether libverovio is installed under `<prefix>/lib` — telling cargo, on the
/// way, to re-run this script when that directory changes.
///
/// The `rerun-if-changed` is what keeps the answer from going stale. Emitting
/// any `rerun-if-*` turns off cargo's default "re-run when a file in the package
/// changes", so `VEROVIO_PREFIX` would otherwise be the *only* trigger — and a
/// cached "not found" then survives the very install that fixes it: you build
/// libverovio into the prefix, cargo replays a resolution made before it
/// existed, and the link fails against a prefix that has none. The library
/// appearing in the directory is the event that matters, so that is what we
/// watch. (The server's build.rs does the same for libfaust.)
fn has_libverovio(prefix: &str) -> bool {
    println!("cargo:rerun-if-changed={prefix}/lib");
    std::path::Path::new(&format!("{prefix}/lib/libverovio.so")).exists()
}
