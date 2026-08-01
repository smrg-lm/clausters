#!/usr/bin/env bash
# The web-components acceptance: a standalone bundle as one HTML element
# (<clausters-bundle>) over the per-page singletons — element up with the
# canvas adopted into its shadow DOM, and the raw server() surface sharing the
# element's namespace (/server_status counts the bundle's synth, the meter bus
# streams moving values). clients/web/examples/demo.html?smoke=1 does the
# asserting; the verdict is beaconed as a fetch and read from the HTTP access
# log (the same real-time posture as the other web smokes).
set -euo pipefail
cd "$(dirname "$0")/.."

CHROME="${CHROME:-$(command -v google-chrome || command -v chromium || command -v chromium-browser)}"
PORT="${PORT:-8141}"

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
    "http://127.0.0.1:$PORT/examples/demo.html?smoke=1" >/dev/null 2>&1 &
CHROME_PID=$!

verdict=""
for _ in $(seq 1 180); do   # up to 90 s
    verdict=$(grep -o 'smoke-verdict-[^ "]*' "$LOG" | head -1 || true)
    [ -n "$verdict" ] && break
    sleep 0.5
done

if [ -z "$verdict" ]; then
    echo "components smoke FAILED: no verdict within 90 s" >&2
    exit 1
fi

decoded=$(printf '%s' "${verdict#smoke-verdict-}" | python3 -c \
    'import sys,urllib.parse; print(urllib.parse.unquote(sys.stdin.read()))')
echo "$decoded"
case "$decoded" in PASS*) ;; *) echo "components smoke FAILED" >&2; exit 1;; esac
