#!/usr/bin/env bash
# Build the clausters web package: compile the three wasm crates (the engine,
# the GUI host, the shared core's OSC codec), stage their wasm-bindgen bundles
# into dist/, and emit the TypeScript sources next to them — dist/ is the
# complete, servable package (src/ -> dist/ 1:1; the staged bundles are the
# browser's _bin/_libs).
#
# The glue's .js/.d.ts are also staged into src/ mirror spots (src/core/,
# src/gui-host/, the engine .d.ts) so type-checking and node-from-source
# resolve the same specifiers the emitted modules use.
#
# One-time setup: rustup target add wasm32-unknown-unknown, and
# cargo install wasm-bindgen-cli at Cargo.lock's wasm-bindgen version.
#
# From clients/web/:  ./build.sh   (release; pass `debug` for faster builds).
# Serve and open the demo:  python3 -m http.server  → /examples/demo.html
set -euo pipefail

cd "$(dirname "$0")"
profile="${1:-release}"
flag=""
[ "$profile" = release ] && flag="--release"

# The CLI must be the same version as the `wasm-bindgen` crate the wasm was
# compiled against: the glue they exchange is a private format, and a mismatch
# surfaces as an opaque "different bindgen format" error at the staging step
# below, long after the build. Two lockfiles pin it -- the root workspace's
# (the engine and the core codec) and clients/gui's own (the host) -- and one
# CLI stages all three bundles, so they must agree. See BUILD.md.
pinned_wasm_bindgen() {   # $1 = a Cargo.lock
    sed -n '/^name = "wasm-bindgen"$/{n;s/^version = "\(.*\)"$/\1/p;}' "$1"
}
pin=$(pinned_wasm_bindgen ../../Cargo.lock)
gui_pin=$(pinned_wasm_bindgen ../gui/Cargo.lock)
if [ -z "$pin" ] || [ "$pin" != "$gui_pin" ]; then
    echo "the lockfiles disagree on wasm-bindgen (root '$pin', gui '$gui_pin');" >&2
    echo "no single CLI can stage both -- reconcile them first." >&2
    exit 1
fi

if ! command -v wasm-bindgen >/dev/null; then
    echo "wasm-bindgen is missing; install it with:" >&2
    echo "  cargo install wasm-bindgen-cli --version $pin" >&2
    exit 1
fi

have=$(wasm-bindgen --version | awk '{print $2}')
if [ "$have" != "$pin" ]; then
    echo "wasm-bindgen $have does not match the lockfiles' $pin;" >&2
    echo "the bundles would not stage. Install the pinned CLI with:" >&2
    echo "  cargo install wasm-bindgen-cli --version $pin" >&2
    exit 1
fi

# The engine and the shared core's JS door (workspace crates).
# shellcheck disable=SC2086
(cd ../.. && cargo build -p clausters-web -p clausters-core-web --lib $flag \
    --target wasm32-unknown-unknown)
# The GUI host (its own workspace under clients/gui).
# shellcheck disable=SC2086
(cd ../gui && cargo build --lib $flag --target wasm32-unknown-unknown)

wasm-bindgen --target web --out-dir dist/engine \
    "../../target/wasm32-unknown-unknown/$profile/clausters_web.wasm"
wasm-bindgen --target web --out-dir dist/gui-host \
    "../gui/target/wasm32-unknown-unknown/$profile/clausters_gui.wasm"
wasm-bindgen --target web --out-dir dist/core \
    "../../target/wasm32-unknown-unknown/$profile/clausters_core_web.wasm"

# The src/ stubs: .d.ts for type-checking everywhere; the core and the engine
# also get the glue .js, because node runs those sources directly (the codec in
# src/base/osc.ts, the offline renderer in src/engine/render.ts) and the tests
# need no build step to reach them.
mkdir -p src/core src/gui-host
cp dist/core/clausters_core_web.js dist/core/clausters_core_web.d.ts src/core/
cp dist/gui-host/clausters_gui.d.ts src/gui-host/
cp dist/engine/clausters_web.js dist/engine/clausters_web.d.ts src/engine/

# Type-check + emit the package into dist/ (js + d.ts + maps).
if [ -d node_modules ]; then
    npm run --silent build
else
    echo "note: node_modules missing — run 'npm install' then 'npm run build'" >&2
fi

echo "package staged: dist/ (modules + engine/ gui-host/ core/ wasm bundles)"
echo "demo:  python3 -m http.server  then open http://localhost:8000/examples/demo.html"
