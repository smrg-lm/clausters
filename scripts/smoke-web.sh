#!/usr/bin/env bash
# The B2 acceptance smoke: the engine live in an AudioWorklet under headless
# Chrome — /status round trip over the MessagePort, engine clock advance, and
# an /s_new sine measured at an AnalyserNode (web/smoke.html does the
# asserting).
#
# Audio pacing is real time (no --virtual-time-budget: it races timers ahead
# of the audio clock), so the page beacons its verdict as a fetch of
# /smoke-verdict-… and this script reads it from the HTTP server's access
# log. Requires wasm-bindgen-cli (Cargo.lock version) and Chrome/Chromium.
set -euo pipefail
cd "$(dirname "$0")/.."

CHROME="${CHROME:-$(command -v google-chrome || command -v chromium || command -v chromium-browser)}"
PORT="${PORT:-8138}"

crates/clausters-web/web/build.sh release

cd crates/clausters-web/web
LOG=$(mktemp)
python3 -m http.server "$PORT" --bind 127.0.0.1 >/dev/null 2>"$LOG" &
SERVER=$!
CHROME_PID=""
trap 'kill $SERVER $CHROME_PID 2>/dev/null' EXIT
sleep 0.5

"$CHROME" --headless=new --disable-gpu --no-sandbox \
    --autoplay-policy=no-user-gesture-required \
    --user-data-dir="$(mktemp -d)" \
    "http://127.0.0.1:$PORT/smoke.html" >/dev/null 2>&1 &
CHROME_PID=$!

verdict=""
for _ in $(seq 1 120); do   # up to 60 s
    verdict=$(grep -o 'smoke-verdict-[^ "]*' "$LOG" | head -1 || true)
    [ -n "$verdict" ] && break
    sleep 0.5
done

if [ -z "$verdict" ]; then
    echo "smoke FAILED: no verdict within 60 s" >&2
    exit 1
fi

# Undo the beacon's URL encoding for the report.
decoded=$(printf '%s' "${verdict#smoke-verdict-}" | python3 -c \
    'import sys,urllib.parse; print(urllib.parse.unquote(sys.stdin.read()))')
echo "$decoded"
case "$decoded" in PASS*) ;; *) echo "smoke FAILED" >&2; exit 1;; esac
