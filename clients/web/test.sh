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
    PAGE="$page"
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
    # What this page asked the server for, for the assertions below: a page is
    # judged by its verdict *and*, where it matters, by what it fetched.
    PAGE_LOG=$(tail -c "+$((mark + 1))" "$LOG")

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

# What the last page fetched, and what it did not.
#
# The Faust compiler is megabytes of Emscripten and is imported on the *first*
# `/def_send faust`, so a page that mounts a prebuilt SynthDef-only bundle must
# never ask for it. That is a property of the request stream, not of anything a
# page can see about itself — the import happens in the NRT worker, whose
# fetches are in no window's resource timeline — so it is read where the server
# logs it.
#
# The two go together on purpose: a negative on its own would pass just as well
# if the asset moved or its name changed, so the page that *does* compile
# asserts the same string positively. One of them failing says which of the two
# things broke.
fetched() {   # $1 = path fragment, $2 = what it is
    if ! printf '%s' "$PAGE_LOG" | grep -q "$1"; then
        echo "$PAGE FAILED: it fetched no $2 ($1)" >&2
        exit 1
    fi
    echo "$PAGE: fetched $2"
}

not_fetched() {   # $1 = path fragment, $2 = what it is
    if printf '%s' "$PAGE_LOG" | grep -q "$1"; then
        echo "$PAGE FAILED: it fetched $2 ($1), which it has no def to compile with" >&2
        exit 1
    fi
    echo "$PAGE: fetched no $2"
}

# `tests/wheel.html` is deliberately absent from this list and is not a gap: it
# measures what one wheel notch reports in the browser it is opened in, so it
# needs a hand turning a wheel and has no verdict to beacon. Open it when
# `gestures::Wheel::BROWSER` has to be calibrated; the page says how to read it.

run_page client.html   # the carrier seam itself
run_page defs.html     # the def model + Server over that carrier
run_page gui.html      # the GuiDef builders + GuiHost, gestures and all
run_page seq.html      # the clock and the patterns on the engine's own clock
run_page data.html     # the data paths: buses, taps and bulk, drawn by the script
run_page editor.html   # the editor views host-drawn from a buffer, and a transport
run_page recording.html # a take followed from the page over /buffer_stream
run_page recording-host.html # the host: shape only while it fills, spans on zoom
run_page automation.html # the automation lane, and the bpf editor that draws it
run_page transport.html # the governing transport: a frozen subtree and a held beat
run_page hosts.html    # two host instances in one page, sharing only the event loop
run_page session.html  # two sessions on two engines: the ambient verbs resolve right
run_page plot.html     # the plot verb: its six kinds, each in its own window
run_page scope.html    # the scope verb: its three views on live buses
run_page responders.html # OscFunc over the engine's own notifications
run_page midi.html     # a pattern to a MIDI port on the engine's grid, and a MidiFunc back
run_page catalogue.html # the filled-out UGen families, measured on the output
run_page ring-peers.html # a host meter and a script bus stream, both over one ring
run_page authored.html # a bundle written in the page and mounted with no disk
run_page nrt.html      # /buffer_allocRead out of the page's filesystem, in the NRT worker
run_page disk.html     # diskOut into the page's filesystem, diskIn streaming it back
run_page faust-artifact.html # what the vendored Faust compiler emits, asserted
run_page faust.html    # a FaustDef compiled in the page, sounding, and set by name
fetched 'vendor/faust/libfaust-wasm' "Faust compiler"   # the positive half of not_fetched

# The components and lifecycle acceptances mount the example bundles, which are
# build products: git-ignored, written into examples/out/ by the examples' own
# node scripts. Generate them here so a fresh checkout runs the page — no skip
# branch any more, because nothing outside this package is needed to write them
# (they were Python until the TypeScript writer landed, and these two pages
# quietly skipped themselves on a checkout that could perfectly well run them).
for example in panels/graph-controls panels/piano; do
    (cd "examples/$example" && node make_bundle.mjs >/dev/null)
done
run_page components.html  # bundles as components: N canvases in one document
# The bundles it mounts are SynthDef-only, so nothing in this page ever
# reaches `/def_send faust` and the compiler stays on the shelf.
not_fetched 'vendor/faust/libfaust' "Faust compiler"
run_page lifecycle.html   # and the unmount: a hundred of them come and go
