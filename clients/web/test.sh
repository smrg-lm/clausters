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
CHROME_PGID=""

# Tear the browser down by its **process group**, not by the one pid bash is
# holding. A browser is a tree — zygote, GPU process, renderers — and the
# children do not all carry the flags a `pkill -f` could match, so killing the
# parent alone can leave a page running with its audio engine, its wasm host
# and its frame tick, forever and unattended. One of those per page is how a
# suite that "was interrupted" ends up eating the machine.
#
# Every step of it ends in `|| true`, and that is not decoration: the browser
# is being killed on purpose, so `wait` reports it terminated by a signal, and
# under `set -e` that status aborts the suite after the first page — with the
# EXIT trap dying at the same line before it can take the HTTP server down.
reap_chrome() {
    [ -n "$CHROME_PGID" ] && { kill -TERM -- "-$CHROME_PGID" 2>/dev/null || true; }
    if [ -n "$CHROME_PID" ]; then
        kill "$CHROME_PID" 2>/dev/null || true
        wait "$CHROME_PID" 2>/dev/null || true
    fi
    # Whatever ignored the TERM (a renderer mid-frame) goes now: the next page
    # must not start beside it.
    [ -n "$CHROME_PGID" ] && { kill -KILL -- "-$CHROME_PGID" 2>/dev/null || true; }
    CHROME_PID=""
    CHROME_PGID=""
    return 0
}

# On the signals too, not only on a clean exit: bash does not run an EXIT trap
# when it is terminated by an untrapped signal, so a `kill` of this script (or
# a Ctrl-C at the wrong moment) used to leave the HTTP server and a whole
# browser behind — the exact leak above, with nobody watching for it.
cleanup() {
    reap_chrome
    kill "$SERVER" 2>/dev/null || true
    return 0
}
trap 'cleanup' EXIT
trap 'cleanup; exit 130' INT
trap 'cleanup; exit 143' TERM HUP
sleep 0.5

# A server that never bound (the port already in use, most often another run of
# this suite) would otherwise hand every page a connection refused, and each
# would spend its full minute waiting for a verdict that cannot come.
if ! kill -0 "$SERVER" 2>/dev/null; then
    echo "the HTTP server did not start on port $PORT -- is it already in use?" >&2
    sed -n '1,5p' "$LOG" >&2
    exit 1
fi

# Runs one acceptance page under headless Chrome and reads its verdict out of
# the HTTP access log (the page beacons it as a fetch). Each page gets its own
# browser and profile, so one engine per tab and no shared state between them.
# A headless Chrome is not cheap: one page brings up about ten processes and
# some 480 MB of PSS (the RSS *sum* reads over a gigabyte, but Chrome shares
# most of it between processes — measure with smaps_rollup, not with `ps`).
# That is affordable exactly once at a time, which is what this function
# guarantees: it takes the browser's whole **process group** down and waits for
# it to be gone before returning, so two never overlap. Without the wait a slow
# teardown left one browser resident while the next started; without the group,
# a child that outlived its parent kept a page — its audio engine, its wasm
# host, its frame tick — running unattended.
run_page() {   # $1 = page under tests/, $2 = optional WxH viewport
    local page="$1" size="${2:-1280,1600}" mark verdict decoded profile
    mark=$(wc -c <"$LOG")
    profile=$(mktemp -d)
    # --enable-unsafe-swiftshader: the GUI host needs a WebGL2 adapter, and
    # headless has no GPU — SwiftShader is the software one. Harmless for the
    # pages that only make sound.
    # --window-size: the viewport every page is written against. It is not a
    # knob to tune down — the pages that synthesize gestures address widgets in
    # canvas coordinates, so a smaller window moves the target out from under
    # them (gui.html fails outright at 800x600). It costs little anyway: the
    # framebuffers are a few MB against Chrome's own half a gigabyte.
    # The rest is containment — no crash reporting, no background networking, a
    # bounded renderer count, a JS heap ceiling — none of which any page here
    # has reason to exceed.
    # `setsid`: the browser leads a session of its own, so every process it
    # forks lands in one group that `reap_chrome` can take down whole.
    setsid "$CHROME" --headless=new --disable-gpu --no-sandbox \
        --enable-unsafe-swiftshader \
        --window-size="$size" \
        --autoplay-policy=no-user-gesture-required \
        --disable-dev-shm-usage \
        --disable-background-networking \
        --disable-breakpad \
        --no-first-run \
        --renderer-process-limit=2 \
        --js-flags=--max-old-space-size=512 \
        --user-data-dir="$profile" \
        "http://127.0.0.1:$PORT/tests/$page?smoke=1" >/dev/null 2>&1 &
    CHROME_PID=$!
    # The group to reap: `setsid` makes the browser its own leader, and its
    # children inherit it. Falling back to the pid keeps this working if the
    # browser is a wrapper that exits before `ps` sees it.
    CHROME_PGID=$(ps -o pgid= -p "$CHROME_PID" 2>/dev/null | tr -d ' ')
    [ -n "$CHROME_PGID" ] || CHROME_PGID="$CHROME_PID"

    verdict=""
    gone=""
    for _ in $(seq 1 120); do   # up to 60 s
        verdict=$(tail -c "+$((mark + 1))" "$LOG" \
            | grep -o 'smoke-verdict-[^ "]*' | head -1 || true)
        [ -n "$verdict" ] && break
        # A browser that died (a crash, an out-of-memory kill) will never
        # beacon: say so now instead of holding the suite for a minute.
        if ! kill -0 "$CHROME_PID" 2>/dev/null; then gone=1; break; fi
        sleep 0.5
    done
    # Terminate and **wait**: `kill` only asks, and the next page must not
    # start while this browser is still winding down. The profile goes with it
    # — a directory per page, left behind, was the other half of the mess.
    reap_chrome
    rm -rf "$profile"

    if [ -z "$verdict" ]; then
        if [ -n "$gone" ]; then
            echo "$page FAILED: the browser exited before it beaconed a verdict" >&2
        else
            echo "$page FAILED: no verdict within 60 s" >&2
        fi
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
run_page automation.html # the automation lane, and the bpf editor that draws it
run_page transport.html # the governing transport: a frozen subtree and a held beat
run_page hosts.html    # two host instances in one page, sharing only the event loop
run_page session.html  # two sessions on two engines: the ambient verbs resolve right
run_page plot.html     # the plot verb: its six kinds, each in its own window
run_page notebook.html # the notebook cell's front end: audio through the boot, teardown

# The components acceptance mounts the two example bundles, which are build
# products (git-ignored, written by the Python client). Generate them here so a
# fresh checkout runs the page; skip it — rather than fail — when the client is
# not importable, the same posture as the WS suites above.
#
# The client's dependencies live in the repo's venv, so a bare `python3` is the
# interpreter least likely to import it: prefer the venv when it is there, or
# these two pages are skipped on a checkout that can perfectly well run them.
PY="${PYTHON:-}"
if [ -z "$PY" ]; then
    if [ -x ../../.venv/bin/python ]; then PY="$(cd ../.. && pwd)/.venv/bin/python"
    else PY=python3; fi
fi
if PYTHONPATH=../python "$PY" -c "import clausters.bundle" 2>/dev/null; then
    for example in graph-controls piano; do
        (cd "examples/$example" && PYTHONPATH=../../../python "$PY" make_bundle.py >/dev/null)
    done
    run_page components.html  # bundles as components: N canvases in one document
else
    echo "components.html: SKIPPED ($PY cannot import the Python client, so the" \
         "example bundles cannot be written)" >&2
fi
