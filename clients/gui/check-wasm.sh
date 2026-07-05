#!/usr/bin/env bash
# Browser-readiness build gate (G11): the platform-agnostic GUI host core must
# always compile for the browser target, with the native I/O shell excluded.
#
# This builds only the library (`--lib`, not the native binaries) for
# `wasm32-unknown-unknown`, so a later milestone cannot silently re-couple the
# agnostic core (widget tree, layout, protocol dispatch, the Platform traits) to
# native I/O (sockets, filesystem, the winit driver) — those live behind
# `#[cfg(not(target_arch = "wasm32"))]` and are not compiled here.
#
# One-time setup: `rustup target add wasm32-unknown-unknown`.
# Runnable from anywhere: it cds to its own directory, so `cargo build --lib`
# always resolves to the gui crate (from the repo root it would resolve to the
# root server crate and fail spuriously on wasm-incompatible deps).
set -euo pipefail
cd "$(dirname "$0")"

if ! rustup target list --installed | grep -q '^wasm32-unknown-unknown$'; then
    echo "the wasm32 target is missing; run: rustup target add wasm32-unknown-unknown" >&2
    exit 1
fi

exec cargo build --lib --target wasm32-unknown-unknown "$@"
