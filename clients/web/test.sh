#!/usr/bin/env bash
# The web client's single test entry: type-check, the node suites (OSC and def
# parity with the Python reference + the WS carrier against a real
# `clausters --ws`), and the page-carrier acceptances (client.html and
# defs.html under headless Chrome, verdict beaconed through the HTTP access
# log — the real-time posture of every web smoke; see docs/decisions.md).
#
# Prerequisites: ./build.sh (stages engine/ gui-host/ core/), npm install,
# and `cargo build` at the workspace root for the WS tests' debug server
# (those tests skip themselves if the binary is missing).
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

# Runs one acceptance page under headless Chrome and reads its verdict out of
# the HTTP access log (the page beacons it as a fetch). Each page gets its own
# browser and profile, so one engine per tab and no shared state between them.
run_page() {   # $1 = page under tests/
    local page="$1" mark verdict decoded
    mark=$(wc -c <"$LOG")
    "$CHROME" --headless=new --disable-gpu --no-sandbox \
        --autoplay-policy=no-user-gesture-required \
        --user-data-dir="$(mktemp -d)" \
        "http://127.0.0.1:$PORT/tests/$page?smoke=1" >/dev/null 2>&1 &
    CHROME_PID=$!

    verdict=""
    for _ in $(seq 1 120); do   # up to 60 s
        verdict=$(tail -c "+$((mark + 1))" "$LOG" \
            | grep -o 'smoke-verdict-[^ "]*' | head -1 || true)
        [ -n "$verdict" ] && break
        sleep 0.5
    done
    kill "$CHROME_PID" 2>/dev/null || true
    CHROME_PID=""

    if [ -z "$verdict" ]; then
        echo "$page FAILED: no verdict within 60 s" >&2
        exit 1
    fi
    decoded=$(printf '%s' "${verdict#smoke-verdict-}" | python3 -c \
        'import sys,urllib.parse; print(urllib.parse.unquote(sys.stdin.read()))')
    echo "$page: $decoded"
    case "$decoded" in PASS*) ;; *) echo "$page FAILED" >&2; exit 1;; esac
}

run_page client.html   # the carrier seam itself
run_page defs.html     # the def model + Server over that carrier
