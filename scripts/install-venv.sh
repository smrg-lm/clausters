#!/usr/bin/env bash
# Create or update the repository's .venv with everything Python: the client
# (`clausters`, editable, with its native binaries and its offline book staged
# inside it), the editor (`clausters-editor`, a plain wheel) and the client's
# `dev` group (pytest).
#
# Usage:
#   scripts/install-venv.sh              # create .venv if missing, install everything
#   scripts/install-venv.sh --refresh    # an existing .venv after a code change:
#                                        # stage the binaries, the book and the editor
#                                        # again, without resolving any dependency
#
# Options:
#   --no-editor   leave the editor out (and the client's `editor` extra)
#   --no-dev      leave the `dev` group out
#   --python EXE  the interpreter that creates the venv (default python3)
#
# The full install is clients/python/README.md, "From the repository, step by
# step", step 3: one pip command that builds, in release, the cdylibs, the
# server, the GUI host and the editor, and stages each into its package. The
# system dependencies, libfaust and libverovio (steps 1 and 2) are not done
# here; a build that misses one stops and names it, and the CLAUSTERS_SKIP_*
# variables (clients/python/build_native.py) build without that piece.
#
# --refresh is the README's step 5. The client is editable, so its Python needs
# nothing, but what is staged inside it is a copy: scripts/refresh-bin.sh stages
# the binaries again and build_docs.py the book. The editor is not editable, so
# its wheel is built and installed again.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
venv="$root/.venv"

info() { printf '\033[1m==>\033[0m %s\n' "$*"; }
die() { printf 'install-venv.sh: %s\n' "$*" >&2; exit 1; }

refresh=0
editor=1
dev=1
python=python3
while [[ $# -gt 0 ]]; do
  case $1 in
    --refresh) refresh=1; shift ;;
    --no-editor) editor=0; shift ;;
    --no-dev) dev=0; shift ;;
    --python) [[ $# -ge 2 ]] || die "--python needs an interpreter"; python=$2; shift 2 ;;
    -h | --help) sed -n '2,29s/^# \{0,1\}//p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) die "unknown option: $1 (see --help)" ;;
  esac
done

# Every command runs from the root, where the build hooks find the cargo
# workspace and the editor; and with the venv's bin on PATH, as an activated
# shell would have it (the staging shells out to tools such as patchelf).
cd "$root"
if [[ ! -x $venv/bin/python ]]; then
  [[ $refresh == 0 ]] || die "no .venv to refresh; run without --refresh first"
  info "Creating $venv"
  "$python" -m venv "$venv"
fi
PATH="$venv/bin:$PATH"
export PATH
pip=("$venv/bin/python" -m pip)

if [[ $refresh == 1 ]]; then
  "$venv/bin/python" -c "import clausters" 2>/dev/null \
    || die "clausters is not installed in .venv; run without --refresh first"
  info "Staging the native binaries (scripts/refresh-bin.sh)"
  "$root/scripts/refresh-bin.sh"
  info "Staging the offline book (clients/python/build_docs.py)"
  "$venv/bin/python" "$root/clients/python/build_docs.py"
  if [[ $editor == 1 ]]; then
    info "Building and installing the editor"
    # A local directory is always reinstalled, even at the same version.
    "${pip[@]}" install --no-deps ./clausters-editor/wheel
  fi
  info "Done"
  exit 0
fi

# `--group` takes pip 25.1 or later.
info "Updating pip"
"${pip[@]}" install --quiet --upgrade pip

args=()
if [[ $editor == 1 ]]; then
  args+=(-e "./clients/python[editor]" ./clausters-editor/wheel)
else
  args+=(-e ./clients/python)
fi
[[ $dev == 0 ]] || args+=(--group ./clients/python/pyproject.toml:dev)

info "Installing (builds every native piece in release: minutes the first time)"
"${pip[@]}" install "${args[@]}"
info "Done: $venv"
