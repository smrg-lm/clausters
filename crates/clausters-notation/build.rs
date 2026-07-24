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
        if std::path::Path::new(&format!("{local}/lib/libverovio.so")).exists() {
            local
        } else {
            "/usr/local".into()
        }
    });

    if !std::path::Path::new(&format!("{prefix}/lib/libverovio.so")).exists() {
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

    println!("cargo:rustc-link-search=native={prefix}/lib");
    println!("cargo:rustc-link-lib=dylib=verovio");
    println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../_libs");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{prefix}/lib");
}
