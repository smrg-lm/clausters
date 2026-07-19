#!/usr/bin/env bash
# Refresh the Python package's bundled native binaries, then (optionally) run
# an example — the one command to type before any manual/visual test.
#
# In this source checkout the package is installed editable, so the *bundled*
# binaries (clausters/_bin/, _libs/) win over the workspace target/ and go
# stale the moment a crate is rebuilt. This wraps the staging machinery that
# already knows the whole recipe (clients/python/build_native.py: server +
# FFI + GUI host + the faust libs) and then runs whatever you give it with
# the repo's .venv Python, no activation needed.
#
# Usage:
#   scripts/refresh-bin.sh                    # just refresh the bundled bins
#   scripts/refresh-bin.sh gui_shell          # refresh + run that example
#   scripts/refresh-bin.sh path/to/script.py  # refresh + run any script
#   scripts/refresh-bin.sh --debug gui_shell  # debug-profile build
#   scripts/refresh-bin.sh --skip gui_shell   # skip the rebuild, just run
set -eu

root="$(cd "$(dirname "$0")/.." && pwd)"
python="$root/.venv/bin/python"
if [ -x "$python" ]; then
    # The venv's tools (patchelf, used by the lib staging) without activation.
    PATH="$root/.venv/bin:$PATH"
    export PATH
else
    python="$(command -v python3)"
fi

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

if [ "$refresh" = 1 ]; then
    # shellcheck disable=SC2086  # an empty flag must vanish, not quote to ""
    "$python" "$root/clients/python/build_native.py" $profile_flag
fi

[ $# -gt 0 ] || exit 0

# A bare name resolves as a Python example (gui_shell ->
# clients/python/examples/gui_shell.py); a path runs as given.
target="$1"
shift
if [ ! -e "$target" ]; then
    candidate="$root/clients/python/examples/${target%.py}.py"
    [ -e "$candidate" ] && target="$candidate"
fi
exec "$python" "$target" "$@"
