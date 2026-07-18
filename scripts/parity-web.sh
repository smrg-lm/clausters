#!/usr/bin/env bash
# Wasm render parity (the B0 acceptance): the wasm build of the engine must
# render a score bit-identical to the native NRT render of the same bytes.
#
# Pipeline: generate the fixtures natively (gen_parity: score.bin + native.f32,
# asserting the scene is denormal-free — the FTZ parity policy), build the wasm
# bundle, serve crates/clausters-web/web/ and read the verdict of parity.html
# under headless Chrome. Requires wasm-bindgen-cli (Cargo.lock version) and a
# Chrome/Chromium binary.
set -euo pipefail
cd "$(dirname "$0")/.."

CHROME="${CHROME:-$(command -v google-chrome || command -v chromium || command -v chromium-browser)}"
PORT="${PORT:-8137}"

cargo run -p clausters-web --example gen_parity
crates/clausters-web/web/build.sh release

cd crates/clausters-web/web
python3 -m http.server "$PORT" --bind 127.0.0.1 >/dev/null 2>&1 &
SERVER=$!
trap 'kill $SERVER 2>/dev/null' EXIT
sleep 0.5

dom=$("$CHROME" --headless=new --disable-gpu --no-sandbox \
    --virtual-time-budget=20000 --dump-dom \
    "http://127.0.0.1:$PORT/parity.html" 2>/dev/null)

echo "$dom" | grep -o 'PASS[^<]*\|FAIL[^<]*' | head -1
echo "$dom" | grep -q 'PASS:' || { echo "parity FAILED" >&2; exit 1; }
