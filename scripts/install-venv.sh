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
#   scripts/install-venv.sh --reset      # delete the local state clausters keeps
#                                        # (asks first), then install as above
#
# Options:
#   --no-editor   leave the editor out (and the client's `editor` extra)
#   --no-dev      leave the `dev` group out
#   --python EXE  the interpreter that creates the venv (default python3)
#   -y, --yes     --reset does not ask for confirmation
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
#
# --reset starts from a clean user: it deletes the user config of the server
# and the client (config.toml), the server's data -- its data directory (sent
# defs, MIDI bindings, boot.json, the GUI's saved state), any other one a
# data_dir in that config.toml names, the ephemeral defs' cache in the temp
# directory and the shared-memory segments of servers no longer running -- and
# the editor's configuration, its environments and its cache. Each is taken
# where it resolves now, CLAUSTERS_CONFIG, CLAUSTERS_DATA_DIR and the XDG
# variables included. The .venv, the build outputs, the libraries built into
# ~/.local (libfaust, libverovio) and a project's clausters.toml stay.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
venv="$root/.venv"

info() { printf '\033[1m==>\033[0m %s\n' "$*"; }
die() { printf 'install-venv.sh: %s\n' "$*" >&2; exit 1; }

refresh=0
reset=0
yes=0
editor=1
dev=1
python=python3
while [[ $# -gt 0 ]]; do
  case $1 in
    --refresh) refresh=1; shift ;;
    --reset) reset=1; shift ;;
    -y | --yes) yes=1; shift ;;
    --no-editor) editor=0; shift ;;
    --no-dev) dev=0; shift ;;
    --python) [[ $# -ge 2 ]] || die "--python needs an interpreter"; python=$2; shift 2 ;;
    -h | --help) sed -n '2,43s/^# \{0,1\}//p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) die "unknown option: $1 (see --help)" ;;
  esac
done

# The local state, where the server, the client and the editor resolve it
# (crates/clausters-core/src/config.rs, src/server/defstore.rs, the shared
# memory segments of src/shm, and Tauri's app_config_dir/app_data_dir/
# app_cache_dir for the editor's identifier).
reset_state() {
  local config_home=${XDG_CONFIG_HOME:-$HOME/.config}
  local data_home=${XDG_DATA_HOME:-$HOME/.local/share}
  local cache_home=${XDG_CACHE_HOME:-$HOME/.cache}
  local editor_id
  editor_id=$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["identifier"])' \
    "$root/clausters-editor/src-tauri/tauri.conf.json")
  [[ -n $editor_id ]] || die "could not read the editor's identifier"
  local config=${CLAUSTERS_CONFIG:-$config_home/clausters/config.toml}
  local targets=(
    "${CLAUSTERS_CONFIG:-$config_home/clausters}"
    "${CLAUSTERS_DATA_DIR:-$data_home/clausters}"
    "${TMPDIR:-/tmp}/clausters-tmpdefs"
    "$config_home/$editor_id"
    "$data_home/$editor_id"
    "$cache_home/$editor_id"
  )
  # A data directory the user config moves the data to, in any of its sections:
  # read before the config itself goes.
  if [[ -f $config ]]; then
    local dir
    while IFS= read -r dir; do
      [[ $dir == /* ]] && targets+=("$dir")
    done < <(python3 -c '
import sys, tomllib
def walk(t):
    for k, v in t.items():
        if isinstance(v, dict): yield from walk(v)
        elif k == "data_dir" and isinstance(v, str): yield v
print("\n".join(walk(tomllib.load(open(sys.argv[1], "rb")))))' "$config")
  fi
  # A shared-memory segment (clients/python/clausters/launch.py names them
  # clausters_<client pid>_<n>) that no process maps any more is stale; one a
  # running server still maps stays.
  local seg
  for seg in /dev/shm/clausters_*; do
    [[ -e $seg ]] || continue
    grep -qsF -- "$seg" /proc/[0-9]*/maps || targets+=("$seg")
  done
  local present=() t
  for t in "${targets[@]}"; do
    [[ -e $t ]] && present+=("$t")
  done
  if [[ ${#present[@]} == 0 ]]; then
    info "No local state to delete"
    return
  fi
  echo "These will be deleted:"
  printf '  %s\n' "${present[@]}"
  if [[ $yes == 0 ]]; then
    local answer
    read -r -p "Continue? [y/N] " answer
    [[ $answer =~ ^[yY]$ ]] || die "nothing deleted"
  fi
  rm -rf -- "${present[@]}"
  info "Local state deleted"
}

[[ $reset == 0 ]] || reset_state

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
