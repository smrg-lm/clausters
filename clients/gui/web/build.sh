#!/usr/bin/env bash
# Build the browser host bundle (G12): compile the agnostic core to wasm, then
# run wasm-bindgen to emit the JS loader + .wasm next to index.html.
#
# One-time setup:
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version <the wasm-bindgen version in Cargo.lock>
#
# Then, from clients/gui/:   ./web/build.sh        (release; pass `debug` for a
# faster, larger build). Serve and open it (WebGPU needs a secure context, and
# localhost counts):
#   (cd web && python3 -m http.server)   # then open http://localhost:8000/
set -euo pipefail

cd "$(dirname "$0")/.."   # clients/gui
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
    "target/wasm32-unknown-unknown/$profile/clausters_gui.wasm"

# Stage the in-page engine (the AudioWorklet backend) next to the host, so
# standalone.html can boot a bundle with no server process: build the engine's
# wasm bundle and copy it (plus its worklet/loader/osc modules) into
# web/engine/. Skipped if the engine crate is missing (a gui-only checkout).
ENGINE_WEB=../../crates/clausters-web/web
if [ -x "$ENGINE_WEB/build.sh" ]; then
    "$ENGINE_WEB/build.sh" "$profile"
    mkdir -p web/engine
    cp "$ENGINE_WEB"/clausters_web.js "$ENGINE_WEB"/clausters_web_bg.wasm \
       "$ENGINE_WEB"/worklet.js "$ENGINE_WEB"/worklet-shim.js \
       "$ENGINE_WEB"/loader.js "$ENGINE_WEB"/osc.js \
       web/engine/
    echo "engine staged in web/engine/ (the in-page AudioWorklet backend)"
fi

echo "bundle written to clausters/gui/web/ (clausters_gui.js + clausters_gui_bg.wasm)"
echo "serve it:  (cd web && python3 -m http.server)  then open http://localhost:8000/"
