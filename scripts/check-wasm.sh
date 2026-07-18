#!/usr/bin/env bash
# Browser-readiness build gate for the engine (the B track): the server crate
# must always compile for `wasm32-unknown-unknown` in its lean, browser-viable
# feature sets, so later work cannot silently re-couple the engine core to
# native-only machinery (sockets, threads, cpal, libfaust). The native shell
# lives behind `#[cfg(not(target_arch = "wasm32"))]` / native-only features and
# is not compiled here.
#
# Checks `--lib` only (the binary is native by definition), three feature sets:
# the bare engine core, the SynthDef family, and the browser build proper
# (`synth,embed` — what `crates/clausters-web` links), plus the wasm shell
# crate itself.
#
# One-time setup: `rustup target add wasm32-unknown-unknown`.
set -euo pipefail
cd "$(dirname "$0")/.."

if ! rustup target list --installed | grep -q '^wasm32-unknown-unknown$'; then
    echo "the wasm32 target is missing; run: rustup target add wasm32-unknown-unknown" >&2
    exit 1
fi

TARGET=wasm32-unknown-unknown

cargo check --lib --target "$TARGET" --no-default-features "$@"
cargo check --lib --target "$TARGET" --no-default-features --features synth "$@"
cargo check --lib --target "$TARGET" --no-default-features --features synth,embed "$@"
cargo check -p clausters-web --target "$TARGET" "$@"
