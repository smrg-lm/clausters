#!/usr/bin/env bash
# Install clausters[editor] from this checkout the way PyPI would: as wheels,
# resolved by name, into a virtual environment of its own.
#
# The `editor` extra names `clausters-editor` by version, so pip looks for it
# on an index. This builds both wheels into one directory and installs with
# `--no-index --find-links` on it: that directory is the index, and pip
# resolves `clausters[editor]` exactly as it would against PyPI -- plain wheel
# installs, nothing editable, nothing pointing back into the repository.
#
# Usage:
#   scripts/install-local.sh                 # build both wheels, install into .venv-local/
#   scripts/install-local.sh --venv DIR      # into another environment (created if missing)
#   scripts/install-local.sh --skip-build    # wheels from the artifacts already built
#   scripts/install-local.sh --no-editor     # plain `clausters`, without the extra
#
# Then: .venv-local/bin/clausters-editor (or activate it).
# --skip-build stages what is already there: clients/python/clausters/_bin and
# _libs (scripts/refresh-bin.sh refreshes them) and the editor's
# src-tauri/target/release binary.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
venv="$root/.venv-local"
wheels="$root/dist/local"
skip=0
spec="clausters[editor]"

while [ $# -gt 0 ]; do
    case "$1" in
        --venv) [ $# -ge 2 ] || { echo "--venv needs a folder" >&2; exit 2; }; venv="$2"; shift ;;
        --skip-build) skip=1 ;;
        --no-editor) spec="clausters" ;;
        -h | --help) sed -n '2,21s/^# \{0,1\}//p' "$0"; exit 0 ;;
        *) echo "install-local.sh: unknown option $1 (see --help)" >&2; exit 2 ;;
    esac
    shift
done

# The client's staging shells out to patchelf, which is often a wheel in the
# development venv: put those on PATH, as refresh-bin.sh does.
for dev in "$root/.venv" "$root/clients/python/.venv"; do
    [ -d "$dev/bin" ] && PATH="$dev/bin:$PATH"
done
export PATH

[ -x "$venv/bin/python" ] || python3 -m venv "$venv"
pip=("$venv/bin/python" -m pip)

if [ "$skip" = 1 ]; then
    export CLAUSTERS_SKIP_NATIVE_BUILD=1 CLAUSTERS_EDITOR_SKIP_BUILD=1
fi

# Only this build's wheels in the index, so an older version cannot win.
rm -rf "$wheels"
mkdir -p "$wheels"
"${pip[@]}" wheel --no-deps -w "$wheels" "$root/clients/python"
if [ "$spec" != clausters ]; then
    "${pip[@]}" wheel --no-deps -w "$wheels" "$root/clausters-editor/wheel"
fi

# --force-reinstall: the version does not change between two local builds, and
# without it pip keeps what is installed.
"${pip[@]}" install --no-index --find-links "$wheels" --force-reinstall "$spec"

echo
echo "Installed $spec into $venv:"
ls "$wheels"
