#!/usr/bin/env bash
# The web client's single test entry: type-check, the node suites (OSC parity
# with the Python reference + the WS carrier against a real `clausters --ws`),
# and the page-carrier acceptance (client.html under headless Chrome, verdict
# beaconed through the HTTP access log — the real-time posture of every web
# smoke; see docs/decisions.md).
#
# Prerequisites: ./build.sh (stages engine/ gui-host/ core/), npm install,
# and `cargo build` at the workspace root for the WS test's debug server
# (that test skips itself if the binary is missing).
#
# From clients/web/:  ./test.sh
set -euo pipefail
cd "$(dirname "$0")"

CHROME="${CHROME:-$(command -v google-chrome || command -v chromium || command -v chromium-browser)}"
PORT="${PORT:-8142}"

npm run --silent check
npm test

LOG=$(mktemp)
python3 -m http.server "$PORT" --bind 127.0.0.1 >/dev/null 2>"$LOG" &
SERVER=$!
CHROME_PID=""
trap 'kill $SERVER $CHROME_PID 2>/dev/null' EXIT
sleep 0.5

"$CHROME" --headless=new --disable-gpu --no-sandbox \
    --autoplay-policy=no-user-gesture-required \
    --user-data-dir="$(mktemp -d)" \
    "http://127.0.0.1:$PORT/tests/client.html?smoke=1" >/dev/null 2>&1 &
CHROME_PID=$!

verdict=""
for _ in $(seq 1 120); do   # up to 60 s
    verdict=$(grep -o 'smoke-verdict-[^ "]*' "$LOG" | head -1 || true)
    [ -n "$verdict" ] && break
    sleep 0.5
done

if [ -z "$verdict" ]; then
    echo "page-carrier smoke FAILED: no verdict within 60 s" >&2
    exit 1
fi

decoded=$(printf '%s' "${verdict#smoke-verdict-}" | python3 -c \
    'import sys,urllib.parse; print(urllib.parse.unquote(sys.stdin.read()))')
echo "$decoded"
case "$decoded" in PASS*) ;; *) echo "page-carrier smoke FAILED" >&2; exit 1;; esac
