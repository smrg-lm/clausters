#!/usr/bin/env bash
# The standalone-in-a-tab acceptance: a native-format bundle (the exact files
# `clausters-gui --standalone` reads) boots entirely in a browser tab — the
# engine in an AudioWorklet, the GUI host on a canvas, no server process —
# with its meter live over /bus_stream. clients/web/examples/standalone.html
# (?smoke=1) does the asserting; the verdict is beaconed as a fetch of
# /smoke-verdict-… and read from the HTTP access log (real-time audio: no
# --virtual-time-budget, same posture as scripts/smoke-web.sh).
#
# The demo bundle is written by clients/web/tools/demo-bundle.sh in the
# persisted formats (a drone SynthDef with a 0.5 Hz LFO on control bus 0, a
# GuiDef whose meter/scope read it, plus the generated bundle.json — the one
# browser-only file). Requires wasm-bindgen-cli and Chrome/Chromium.
set -euo pipefail
cd "$(dirname "$0")/.."

CHROME="${CHROME:-$(command -v google-chrome || command -v chromium || command -v chromium-browser)}"
PORT="${PORT:-8140}"

clients/web/build.sh release
clients/web/tools/demo-bundle.sh

cd clients/web
LOG=$(mktemp)
python3 -m http.server "$PORT" --bind 127.0.0.1 >/dev/null 2>"$LOG" &
SERVER=$!
CHROME_PID=""
trap 'kill $SERVER $CHROME_PID 2>/dev/null' EXIT
sleep 0.5

"$CHROME" --headless=new --disable-gpu --no-sandbox \
    --autoplay-policy=no-user-gesture-required \
    --user-data-dir="$(mktemp -d)" \
    "http://127.0.0.1:$PORT/examples/standalone.html?smoke=1" \
    >/dev/null 2>&1 &
CHROME_PID=$!

verdict=""
for _ in $(seq 1 180); do   # up to 90 s (two wasm bundles compile/instantiate)
    verdict=$(grep -o 'smoke-verdict-[^ "]*' "$LOG" | head -1 || true)
    [ -n "$verdict" ] && break
    sleep 0.5
done

if [ -z "$verdict" ]; then
    echo "standalone smoke FAILED: no verdict within 90 s" >&2
    exit 1
fi

decoded=$(printf '%s' "${verdict#smoke-verdict-}" | python3 -c \
    'import sys,urllib.parse; print(urllib.parse.unquote(sys.stdin.read()))')
echo "$decoded"
case "$decoded" in PASS*) ;; *) echo "standalone smoke FAILED" >&2; exit 1;; esac
