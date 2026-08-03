#!/usr/bin/env bash
# The web client's single test entry: type-check, the node suites (OSC, def,
# GuiDef and clock/RNG parity with the Python reference + the WS carriers
# against a real `clausters --ws` server and a real `clausters-gui --ws`
# host), and the page-carrier acceptances (client.html, defs.html, gui.html
# and seq.html under headless Chrome, verdict beaconed through the HTTP access
# log — the real-time posture of every web smoke; see docs/decisions.md).
#
# Prerequisites: ./build.sh (stages engine/ gui-host/ core/), npm install, and
# `cargo build` at the workspace root (the WS server) and in clients/gui (the
# WS GUI host) for the WS suites — those skip themselves if the binary is
# missing.
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
    # --enable-unsafe-swiftshader: the GUI host needs a WebGL2 adapter, and
    # headless has no GPU — SwiftShader is the software one. Harmless for the
    # pages that only make sound.
    # --window-size: a page placing several components needs a viewport tall
    # enough to hold them, since what is out of the viewport deliberately does
    # not draw or stream (components.html asserts both halves of that).
    "$CHROME" --headless=new --disable-gpu --no-sandbox \
        --enable-unsafe-swiftshader \
        --window-size=1280,1600 \
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
run_page gui.html      # the GuiDef builders + GuiHost, gestures and all
run_page seq.html      # the clock and the patterns on the engine's own clock
run_page data.html     # the data paths: buses, taps and bulk, drawn by the script
run_page editor.html   # the editor views host-drawn from a buffer, and a transport
run_page transport.html # the governing transport: a frozen subtree and a held beat
run_page hosts.html    # two host instances in one page, sharing only the event loop
run_page session.html  # two sessions on two engines: the ambient verbs resolve right
run_page notebook.html # the notebook cell's front end: audio through the boot, teardown

# The components acceptance mounts the two example bundles, which are build
# products (git-ignored, written by the Python client). Generate them here so a
# fresh checkout runs the page; skip it — rather than fail — when the client is
# not importable, the same posture as the WS suites above.
if PYTHONPATH=../python python3 -c "import clausters.bundle" 2>/dev/null; then
    for example in graph-controls piano; do
        (cd "examples/$example" && PYTHONPATH=../../../python python3 make_bundle.py >/dev/null)
    done
    run_page components.html  # bundles as components: N canvases in one document
else
    echo "components.html: SKIPPED (the Python client is not importable, so the" \
         "example bundles cannot be written)" >&2
fi
