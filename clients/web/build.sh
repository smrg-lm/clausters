#!/usr/bin/env bash
# Stage the clausters web package: build the two wasm bundles (the engine and
# the GUI host) and copy them into engine/ and gui-host/, so this directory is
# a complete, servable ES-module package (no bundler, no node toolchain — the
# future TS track adds those; see PLAN.md).
#
# One-time setup: rustup target add wasm32-unknown-unknown, and
# cargo install wasm-bindgen-cli at Cargo.lock's wasm-bindgen version.
#
# From clients/web/:  ./build.sh   (release; pass `debug` for faster builds).
# Serve and open the demo:  python3 -m http.server  → /demo.html
set -euo pipefail

cd "$(dirname "$0")"
profile="${1:-release}"

# The GUI host's build also builds the engine and stages it in its web/engine/.
../gui/web/build.sh "$profile"

mkdir -p engine gui-host
cp ../gui/web/engine/* engine/
cp ../gui/web/clausters_gui.js ../gui/web/clausters_gui_bg.wasm gui-host/

echo "package staged: engine/ + gui-host/ next to the ES modules"
echo "demo:  python3 -m http.server  then open http://localhost:8000/demo.html"
