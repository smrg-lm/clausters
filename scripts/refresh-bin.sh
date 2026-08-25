#!/usr/bin/env bash
# Refresh the Python package's bundled native binaries, then (optionally) run
# an example — the one command to type before any manual/visual test.
#
# A source checkout resolves the *bundled* binaries (clausters/_bin/, _libs/)
# before the workspace target/, so they go stale the moment a crate is rebuilt
# and a manual test silently exercises pre-change binaries. This wraps the
# staging machinery that already knows the whole recipe
# (clients/python/build_native.py: server + FFI + GUI host + the faust and
# verovio libs) and then runs whatever you give it, no activation needed.
#
# Usage:
#   scripts/refresh-bin.sh                    # just refresh the bundled bins
#   scripts/refresh-bin.sh shell              # refresh + run that example
#   scripts/refresh-bin.sh path/to/script.py  # refresh + run any script
#   scripts/refresh-bin.sh --debug shell      # debug-profile build
#   scripts/refresh-bin.sh --skip shell       # skip the rebuild, just run
set -eu

root="$(cd "$(dirname "$0")/.." && pwd)"

# The interpreter: the workspace-root .venv first -- that is the documented
# setup (clients/python/README.md: a venv at the repo root, then
# `pip install -e ./clients/python`) -- then one next to the package, then plain
# python3. Any of the three runs an example either way, because the examples put
# clients/python on sys.path themselves rather than relying on an install.
#
# Every venv found goes on PATH even when it did not win the interpreter slot,
# because the staging shells out to tools that may live in either: patchelf,
# which rewrites the run path of the vendored libs, is often a per-venv wheel.
python=""
venv_bins=""
for venv in "$root/.venv" "$root/clients/python/.venv"; do
    [ -x "$venv/bin/python" ] || continue
    [ -n "$python" ] || python="$venv/bin/python"
    venv_bins="${venv_bins:+$venv_bins:}$venv/bin"
done
if [ -n "$venv_bins" ]; then
    PATH="$venv_bins:$PATH"
    export PATH
fi
[ -n "$python" ] || python="$(command -v python3)"

profile_flag=""
refresh=1
while [ $# -gt 0 ]; do
    case "$1" in
        --debug | --release) profile_flag="$1" ;;
        --skip) refresh=0 ;;
        *) break ;;
    esac
    shift
done

# The GUI host is built **with `standalone`** here, unlike the wheel's own
# build, and that is the difference between refreshing for a manual test and
# packaging one. `standalone` links the server crate into the host so a saved
# session can be opened with no language client behind it -- which is the whole
# subject of `session.py` and of every H-track example. Off, the host still
# runs and still draws, and a take just comes back **empty** with a warning, so
# the failure is quiet and looks like the example is broken. Since this script
# is what CLAUDE.md tells everyone to run before any manual test, it must not
# be what silently removes the mode under test. Override by exporting
# CLAUSTERS_GUI_FEATURES yourself.
: "${CLAUSTERS_GUI_FEATURES:=standalone}"
export CLAUSTERS_GUI_FEATURES

if [ "$refresh" = 1 ]; then
    # shellcheck disable=SC2086  # an empty flag must vanish, not quote to ""
    "$python" "$root/clients/python/build_native.py" $profile_flag
fi

[ $# -gt 0 ] || exit 0

# A bare name resolves as a Python example, searched through the example
# folders (shell -> clients/python/examples/panels/shell.py); a path runs as
# given. The folders are what the `gui_` prefix became, so a name is unique
# across them and the search needs no order.
target="$1"
shift
if [ ! -e "$target" ]; then
    candidate=$(find "$root/clients/python/examples" -name "${target%.py}.py" -print -quit)
    [ -n "$candidate" ] && target="$candidate"
fi
case "$target" in
    *.py) ;;
    *)
        echo "refresh-bin.sh: '$target' is not a Python script (expected a" >&2
        echo "  .py path or an example name, e.g. 'shell')" >&2
        exit 2
        ;;
esac
exec "$python" "$target" "$@"
