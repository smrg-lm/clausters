#!/usr/bin/env bash
# The web acceptance smokes: every page that beacons a verdict, run by one
# script over one list.
#
# Each case below is a page that asserts something the browser can only prove
# by running it -- the engine live in an AudioWorklet, a bundle booting as a
# custom element, a GuiDef's own voices reaching the engine -- and says so by
# fetching /smoke-verdict-<PASS|FAIL ...>, which this script reads out of the
# HTTP server's access log. That posture (rather than a CDP evaluate) is
# deliberate: the audio clock is real time and a virtual-time budget races
# timers ahead of it (docs/decisions.md).
#
#     scripts/smoke-web.sh                 every case
#     scripts/smoke-web.sh piano worklet   just those, in the order given
#     scripts/smoke-web.sh --list          what the cases are
#
# Prerequisites: Chrome/Chromium, the wasm toolchain clients/web/build.sh needs
# (rustup's wasm32-unknown-unknown and the pinned wasm-bindgen CLI -- see
# BUILD.md), and, for the two authored-bundle cases, the Python client
# importable (the repo's .venv, or PYTHON=... pointing at an interpreter that
# can import `clausters.bundle`) -- the core's C ABI it validates through is
# built here if the checkout has none.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$(pwd)

CHROME="${CHROME:-$(command -v google-chrome || command -v chromium || command -v chromium-browser || true)}"
PORT="${PORT:-8138}"

# --- The list ----------------------------------------------------------------
#
# name | what it needs built | page (relative to clients/web, the served root)
#
# `demo-bundle` is tools/demo-bundle.sh -- the native persisted formats a
# `clausters-gui --standalone` reads. `authored:<dir>` is an example's own
# make_bundle.py, written with the Python client. Everything needs the package
# staged into dist/, which is built once for the whole run.
CASES=(
    "worklet|-|tests/smoke.html"
    "standalone|demo-bundle|examples/panels/standalone.html?smoke=1"
    "components|demo-bundle|examples/components/demo.html?smoke=1"
    "graph-controls|authored:examples/panels/graph-controls|examples/panels/graph-controls/index.html?smoke=1"
    "piano|authored:examples/panels/piano|examples/panels/piano/index.html?smoke=1"
)

case "${1:-}" in
    --list|-l)
        for c in "${CASES[@]}"; do
            IFS='|' read -r name _ page <<<"$c"
            printf '  %-15s %s\n' "$name" "$page"
        done
        exit 0
        ;;
    -h|--help)
        sed -n '2,25p' "$0" | sed 's/^# \{0,1\}//'
        exit 0
        ;;
esac

# The selection: the arguments, in the order given, or the whole list.
selected=()
if [ "$#" -gt 0 ]; then
    for want in "$@"; do
        hit=""
        for c in "${CASES[@]}"; do
            [ "${c%%|*}" = "$want" ] && { selected+=("$c"); hit=1; break; }
        done
        [ -n "$hit" ] || { echo "no such case: $want (try --list)" >&2; exit 2; }
    done
else
    selected=("${CASES[@]}")
fi

[ -n "$CHROME" ] || { echo "no Chrome/Chromium on PATH (set CHROME=...)" >&2; exit 1; }

# --- What the selection needs built ------------------------------------------

clients/web/build.sh release

need_demo_bundle=""
authored=()
for c in "${selected[@]}"; do
    IFS='|' read -r _ needs _ <<<"$c"
    case "$needs" in
        demo-bundle) need_demo_bundle=1 ;;
        authored:*)  authored+=("${needs#authored:}") ;;
    esac
done

[ -n "$need_demo_bundle" ] && clients/web/tools/demo-bundle.sh

# The authored bundles are build products (git-ignored) written by the Python
# client. Unlike clients/web/test.sh, which skips its two component pages when
# the client is not importable, a missing bundle here is a failure: these cases
# exist because their assertions were written and never fired, and a smoke that
# skips itself is how that happened.
if [ "${#authored[@]}" -gt 0 ]; then
    PY="${PYTHON:-}"
    if [ -z "$PY" ]; then
        if [ -x "$ROOT/.venv/bin/python" ]; then PY="$ROOT/.venv/bin/python"; else PY=python3; fi
    fi
    if ! PYTHONPATH="$ROOT/clients/python" "$PY" -c "import clausters.bundle" 2>/dev/null; then
        echo "$PY cannot import the Python client, so the authored bundles" \
             "cannot be written (set PYTHON=... or run the cases that need none)" >&2
        exit 1
    fi
    # Authoring is not pure Python: `Bundle.write` validates the manifest and
    # the defs through the core's C ABI, so the client needs libclausters_ffi.
    # In a source checkout that is the workspace build (the wheel bundles its
    # own copy, and an env override beats both), and cargo no-ops when it is
    # already there. The crate's default feature set is empty -- no libfaust, no
    # libverovio -- so this costs a build of the core and nothing else.
    if ! PYTHONPATH="$ROOT/clients/python" "$PY" -c \
        "from clausters import _native; _native.lib()" 2>/dev/null; then
        echo "staging libclausters_ffi (the bundle writer validates through it)"
        cargo build -p clausters-ffi --release
    fi
    for dir in "${authored[@]}"; do
        (cd "clients/web/$dir" && PYTHONPATH="$ROOT/clients/python" "$PY" make_bundle.py >/dev/null)
    done
fi

# --- The harness -------------------------------------------------------------

cd clients/web
LOG=$(mktemp)
python3 -m http.server "$PORT" --bind 127.0.0.1 >/dev/null 2>"$LOG" &
SERVER=$!
CHROME_PID=""
CHROME_PGID=""

# Tear the browser down by its **process group**, not by the one pid bash is
# holding: a browser is a tree, and a child that outlives its parent keeps a
# page -- its audio engine, its wasm host, its frame tick -- running unattended.
# The `|| true`s are not decoration: the browser is being killed on purpose, so
# `wait` reports a signal, which under `set -e` would abort the run after the
# first case with the EXIT trap dying before it can take the HTTP server down.
# (The same reaping clients/web/test.sh does, for the same reasons.)
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
    sed -n '1,5p' "$LOG" >&2
    exit 1
fi

failures=()

run_case() {   # $1 = case name, $2 = page under clients/web
    local name="$1" page="$2" mark verdict decoded profile gone=""
    mark=$(wc -c <"$LOG")
    profile=$(mktemp -d)
    # --enable-unsafe-swiftshader: the pages that mount a GUI host need a WebGL2
    # adapter and headless has no GPU; SwiftShader is the software one, and it
    # is harmless for the pages that only make sound. `setsid` gives the browser
    # a session of its own so reap_chrome can take the whole tree down.
    setsid "$CHROME" --headless=new --disable-gpu --no-sandbox \
        --enable-unsafe-swiftshader \
        --window-size=1280,1600 \
        --autoplay-policy=no-user-gesture-required \
        --disable-dev-shm-usage \
        --disable-background-networking \
        --disable-breakpad \
        --no-first-run \
        --renderer-process-limit=2 \
        --user-data-dir="$profile" \
        "http://127.0.0.1:$PORT/$page" >/dev/null 2>&1 &
    CHROME_PID=$!
    CHROME_PGID=$(ps -o pgid= -p "$CHROME_PID" 2>/dev/null | tr -d ' ')
    [ -n "$CHROME_PGID" ] || CHROME_PGID="$CHROME_PID"

    verdict=""
    for _ in $(seq 1 180); do   # up to 90 s: two wasm bundles compile and instantiate
        verdict=$(tail -c "+$((mark + 1))" "$LOG" \
            | grep -o 'smoke-verdict-[^ "]*' | head -1 || true)
        [ -n "$verdict" ] && break
        # A browser that died (a crash, an out-of-memory kill) will never
        # beacon: say so now rather than holding the run for its full minute.
        if ! kill -0 "$CHROME_PID" 2>/dev/null; then gone=1; break; fi
        sleep 0.5
    done
    reap_chrome
    rm -rf "$profile"

    if [ -z "$verdict" ]; then
        if [ -n "$gone" ]; then
            echo "$name FAILED: the browser exited before it beaconed a verdict" >&2
        else
            echo "$name FAILED: no verdict within 90 s" >&2
        fi
        failures+=("$name")
        return 0
    fi
    decoded=$(printf '%s' "${verdict#smoke-verdict-}" | python3 -c \
        'import sys,urllib.parse; print(urllib.parse.unquote(sys.stdin.read()))')
    echo "$name: $decoded"
    case "$decoded" in PASS*) ;; *) failures+=("$name") ;; esac
    return 0
}

# Every case runs even after one fails: a run that stops at the first failure
# tells you about one page, and these are exactly the assertions that go
# unnoticed one at a time.
for c in "${selected[@]}"; do
    IFS='|' read -r name _ page <<<"$c"
    run_case "$name" "$page"
done

if [ "${#failures[@]}" -gt 0 ]; then
    echo "" >&2
    echo "web smokes FAILED: ${failures[*]}" >&2
    exit 1
fi
echo ""
echo "all ${#selected[@]} web smokes passed"
