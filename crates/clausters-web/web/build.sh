#!/usr/bin/env bash
# Build the wasm engine bundle: compile the shell for wasm32, then run
# wasm-bindgen to emit the JS loader + .wasm next to the harness pages.
#
# One-time setup:
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version <the wasm-bindgen version in Cargo.lock>
#
# From crates/clausters-web/:  ./web/build.sh   (release; pass `debug` for a
# faster, larger build).
set -euo pipefail

cd "$(dirname "$0")/.."   # crates/clausters-web
profile="${1:-release}"
flag=""
[ "$profile" = release ] && flag="--release"

if ! command -v wasm-bindgen >/dev/null; then
    echo "wasm-bindgen is missing; install it with:" >&2
    echo "  cargo install wasm-bindgen-cli --version <Cargo.lock wasm-bindgen version>" >&2
    exit 1
fi

# shellcheck disable=SC2086
cargo build --lib $flag --target wasm32-unknown-unknown
wasm-bindgen --target web --no-typescript \
    --out-dir web \
    "../../target/wasm32-unknown-unknown/$profile/clausters_web.wasm"

echo "bundle written to crates/clausters-web/web/ (clausters_web.js + clausters_web_bg.wasm)"
