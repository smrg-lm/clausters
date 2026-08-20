#!/usr/bin/env bash
# The measurement the in-page shared-memory path waits on
# (clients/gui/PLAN.md, Future directions -- "The in-page shared-memory path"):
# what a frame of `/bus_stream` costs on a page holding forty canvases.
#
# Runs tools/bus-stream-profile.html under headless Chrome, exactly as
# test.sh runs an acceptance page -- one browser, its own profile, the report
# read out of the HTTP access log -- and prints the report line. This is a
# profile and not a test: it asserts nothing about the numbers, it produces
# them, so it is deliberately not in test.sh.
#
# Prerequisites: ./build.sh (dist/ is git-ignored and stale by default).
#
# From clients/web/:  ./tools/profile-bus-stream.sh [canvases] [seconds] [ceiling]
#
# The headless caveat that belongs beside every number it prints: Chrome's
# software WebGL (SwiftShader) makes the *drawing* far more expensive than a
# real GPU does, so the frame rate and the lag are a floor, not a forecast.
# The stream's own cost -- the bytes, and the time inside `server_reply` -- is
# CPU work either way and carries over.
set -euo pipefail
cd "$(dirname "$0")/.."

CANVASES="${1:-40}"
SECONDS_PER_PHASE="${2:-6}"
# Optional third argument: boot the in-page engine with a lowered /bus_stream
# ceiling, to exercise what a page does when its document outgrows the server's
# limit (it subscribes what fits and says what it left out).
CEILING="${3:-}"
CHROME="${CHROME:-$(command -v google-chrome || command -v chromium || command -v chromium-browser)}"
PORT="${PORT:-8143}"

[ -f dist/index.js ] || { echo "dist/ is not built: run ./build.sh first" >&2; exit 1; }

LOG=$(mktemp)
python3 -m http.server "$PORT" --bind 127.0.0.1 >/dev/null 2>"$LOG" &
SERVER=$!
CHROME_PID=""
CHROME_PGID=""

# The browser goes down by its process group, for the reason test.sh explains
# at length: a page left running holds an audio engine and a frame tick
# forever.
reap_chrome() {
    [ -n "$CHROME_PGID" ] && { kill -TERM -- "-$CHROME_PGID" 2>/dev/null || true; }
    if [ -n "$CHROME_PID" ]; then
        kill "$CHROME_PID" 2>/dev/null || true
        wait "$CHROME_PID" 2>/dev/null || true
    fi
    [ -n "$CHROME_PGID" ] && { kill -KILL -- "-$CHROME_PGID" 2>/dev/null || true; }
    CHROME_PID=""
    CHROME_PGID=""
    return 0
}
cleanup() {
    reap_chrome
    kill "$SERVER" 2>/dev/null || true
    return 0
}
trap 'cleanup' EXIT
trap 'cleanup; exit 130' INT
trap 'cleanup; exit 143' TERM HUP
sleep 0.5

if ! kill -0 "$SERVER" 2>/dev/null; then
    echo "the HTTP server did not start on port $PORT -- is it already in use?" >&2
    exit 1
fi

profile=$(mktemp -d)
url="http://127.0.0.1:$PORT/tools/bus-stream-profile.html?canvases=$CANVASES&seconds=$SECONDS_PER_PHASE"
[ -n "$CEILING" ] && url="$url&maxbuses=$CEILING"
setsid "$CHROME" --headless=new --disable-gpu --no-sandbox \
    --enable-unsafe-swiftshader \
    --window-size=1600,1200 \
    --autoplay-policy=no-user-gesture-required \
    --disable-dev-shm-usage \
    --disable-background-networking \
    --disable-breakpad \
    --no-first-run \
    --js-flags=--max-old-space-size=1024 \
    --user-data-dir="$profile" \
    "$url" >/dev/null 2>&1 &
CHROME_PID=$!
CHROME_PGID=$(ps -o pgid= -p "$CHROME_PID" 2>/dev/null | tr -d ' ')
[ -n "$CHROME_PGID" ] || CHROME_PGID="$CHROME_PID"

# Three phases plus the settle, with room for a slow build-up of canvases.
budget=$(( SECONDS_PER_PHASE * 3 + 120 ))
report=""
gone=""
for _ in $(seq 1 $(( budget * 2 ))); do
    report=$(grep -o 'smoke-verdict-[^ "]*' "$LOG" | head -1 || true)
    [ -n "$report" ] && break
    if ! kill -0 "$CHROME_PID" 2>/dev/null; then gone=1; break; fi
    sleep 0.5
done
reap_chrome
rm -rf "$profile"

if [ -z "$report" ]; then
    if [ -n "$gone" ]; then
        echo "the browser exited before it reported" >&2
    else
        echo "no report within ${budget}s" >&2
    fi
    exit 1
fi
printf '%s\n' "${report#smoke-verdict-}" | python3 -c \
    'import sys,urllib.parse; print(urllib.parse.unquote(sys.stdin.read()))'
