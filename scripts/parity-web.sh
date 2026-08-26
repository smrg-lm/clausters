#!/usr/bin/env bash
# Wasm render parity: the wasm build of the engine must render a score to
# within a tolerance of the native NRT render of the same bytes.
#
# Pipeline: generate the fixtures natively (gen_parity: score.bin + native.f32
# and score-faust.bin + native-faust.f32 into clients/web/tests/, asserting
# each scene is denormal-free -- the FTZ parity policy), build/stage the web
# package, serve clients/web/ and read the verdict of tests/parity.html under
# headless Chrome. Requires wasm-bindgen-cli (Cargo.lock version) and a
# Chrome/Chromium binary.
#
# The verdict is beaconed by the page and read out of the HTTP access log,
# rather than dumped from the DOM: the Faust scene compiles its def in a
# Worker, and Chrome's virtual time does not wait for one. It is the same
# posture every other page in the web suite reports with.
set -euo pipefail
cd "$(dirname "$0")/.."

CHROME="${CHROME:-$(command -v google-chrome || command -v chromium || command -v chromium-browser)}"
PORT="${PORT:-8137}"

cargo run -p clausters-web --example gen_parity
clients/web/build.sh release

cd clients/web
LOG=$(mktemp)
python3 -m http.server "$PORT" --bind 127.0.0.1 >/dev/null 2>"$LOG" &
SERVER=$!
PROFILE=$(mktemp -d)
CHROME_PGID=""
cleanup() {
    [ -n "$CHROME_PGID" ] && { kill -TERM -- "-$CHROME_PGID" 2>/dev/null || true; }
    [ -n "$CHROME_PGID" ] && { kill -KILL -- "-$CHROME_PGID" 2>/dev/null || true; }
    kill "$SERVER" 2>/dev/null || true
    rm -rf "$PROFILE"
    return 0
}
trap 'cleanup' EXIT
trap 'cleanup; exit 130' INT
trap 'cleanup; exit 143' TERM HUP
sleep 0.5

setsid "$CHROME" --headless=new --disable-gpu --no-sandbox \
    --disable-dev-shm-usage --disable-background-networking \
    --disable-breakpad --no-first-run --user-data-dir="$PROFILE" \
    "http://127.0.0.1:$PORT/tests/parity.html" >/dev/null 2>&1 &
CHROME_PID=$!
CHROME_PGID=$(ps -o pgid= -p "$CHROME_PID" 2>/dev/null | tr -d ' ')
[ -n "$CHROME_PGID" ] || CHROME_PGID="$CHROME_PID"

verdict=""
for _ in $(seq 1 120); do   # up to 60 s
    verdict=$(grep -o 'parity-verdict-[^ "]*' "$LOG" | head -1 || true)
    [ -n "$verdict" ] && break
    kill -0 "$CHROME_PID" 2>/dev/null || break
    sleep 0.5
done

[ -n "$verdict" ] || { echo "parity FAILED: no verdict within 60 s" >&2; exit 1; }
decoded=$(printf '%s' "${verdict#parity-verdict-}" | python3 -c \
    'import sys,urllib.parse; print(urllib.parse.unquote(sys.stdin.read()))')
echo "$decoded"
case "$decoded" in PASS*) ;; *) echo "parity FAILED" >&2; exit 1;; esac
