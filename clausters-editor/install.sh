#!/usr/bin/env bash
# Installs, updates or uninstalls clausters-editor for the current user (no sudo).
#
#   ./install.sh install     builds and copies the binary, the documentation and the menu entry
#   ./install.sh update      the same as install (rebuilds and replaces)
#   ./install.sh uninstall   removes what was installed; with --purge also deletes the app's
#                            configuration, virtual environments and cache
#
# Options:
#   --prefix DIR   destination (default ~/.local): DIR/bin, DIR/lib/clausters-editor, DIR/share
#   --no-build     does not rebuild; uses the binary already built
#   --purge        (uninstall) deletes the app's data
#   -y, --yes      does not ask for confirmation
set -euo pipefail

PROJECT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

# The name and the identifier come from tauri.conf.json so they are not written twice.
read -r APP IDENTIFIER < <(python3 -c '
import json, sys
c = json.load(open(sys.argv[1]))
print(c["productName"], c["identifier"])' "$PROJECT_DIR/src-tauri/tauri.conf.json")
[[ -n ${APP:-} && -n ${IDENTIFIER:-} ]] || { echo "Error: could not read tauri.conf.json" >&2; exit 1; }
PREFIX="$HOME/.local"
BUILD=1
PURGE=0
YES=0

usage() {
  sed -n '2,15s/^# \{0,1\}//p' "${BASH_SOURCE[0]}"
  exit "${1:-0}"
}

info() { printf '\033[1m==>\033[0m %s\n' "$*"; }
die() { printf 'Error: %s\n' "$*" >&2; exit 1; }

confirm() {
  [[ $YES == 1 ]] && return 0
  local answer
  read -r -p "$1 [y/N] " answer
  [[ $answer =~ ^[yY]$ ]]
}

# --- Arguments ---
[[ $# -gt 0 ]] || usage 1
COMMAND=$1
shift
while [[ $# -gt 0 ]]; do
  case $1 in
    --prefix) [[ $# -ge 2 ]] || die "--prefix needs a folder"; PREFIX=$2; shift 2 ;;
    --prefix=*) PREFIX=${1#*=}; shift ;;
    --no-build) BUILD=0; shift ;;
    --purge) PURGE=1; shift ;;
    -y | --yes) YES=1; shift ;;
    -h | --help) usage ;;
    *) die "unknown option: $1 (see --help)" ;;
  esac
done
[[ -n $PREFIX ]] || die "--prefix cannot be empty"
PREFIX=$(realpath -m -- "$PREFIX")
[[ $PREFIX != / ]] || die "--prefix cannot be /"

BIN="$PREFIX/bin/$APP"
LIB="$PREFIX/lib/$APP" # Tauri looks for resources in ../lib/<app>/ relative to the binary
DESKTOP="$PREFIX/share/applications/$APP.desktop"
ICON="$PREFIX/share/icons/hicolor/128x128/apps/$APP.png"
BUILT="$PROJECT_DIR/src-tauri/target/release/$APP"

# Data the app creates (the same XDG paths Tauri uses).
DATA_DIRS=(
  "${XDG_CONFIG_HOME:-$HOME/.config}/$IDENTIFIER"
  "${XDG_DATA_HOME:-$HOME/.local/share}/$IDENTIFIER"
  "${XDG_CACHE_HOME:-$HOME/.cache}/$IDENTIFIER"
)

build() {
  command -v npm >/dev/null || die "npm not found"
  command -v cargo >/dev/null || die "cargo (Rust) not found"
  info "Building (npm run tauri build -- --no-bundle)..."
  cd "$PROJECT_DIR"
  [[ -d node_modules ]] || npm ci
  npm run tauri build -- --no-bundle
}

install_app() {
  if [[ $BUILD == 1 ]]; then build; fi
  [[ -x $BUILT ]] || die "$BUILT does not exist; build without --no-build"

  info "Installing into $PREFIX"
  install -Dm755 "$BUILT" "$BIN"

  # The whole folder is replaced so that documents deleted from the project do not linger.
  rm -rf "$LIB/docs"
  mkdir -p "$LIB"
  cp -r "$PROJECT_DIR/docs" "$LIB/docs"

  install -Dm644 "$PROJECT_DIR/src-tauri/icons/128x128.png" "$ICON"
  mkdir -p "$(dirname "$DESKTOP")"
  cat >"$DESKTOP" <<EOF
[Desktop Entry]
Type=Application
Name=$APP
Comment=Python editor with an interactive session
Exec=$BIN
Icon=$APP
Terminal=false
Categories=Development;IDE;
EOF
  command -v update-desktop-database >/dev/null && update-desktop-database -q "$(dirname "$DESKTOP")" || true

  info "Done: $BIN"
  case ":$PATH:" in
    *":$PREFIX/bin:"*) ;;
    *) echo "Note: $PREFIX/bin is not in PATH; open it from the menu or by its full path." ;;
  esac
}

uninstall_app() {
  info "Removing the installation from $PREFIX"
  rm -f "$BIN" "$DESKTOP" "$ICON"
  rm -rf "$LIB"
  command -v update-desktop-database >/dev/null && update-desktop-database -q "$(dirname "$DESKTOP")" || true

  if [[ $PURGE == 1 ]]; then
    echo "The configuration, the virtual environments and the cache will be deleted:"
    printf '  %s\n' "${DATA_DIRS[@]}"
    if confirm "Continue?"; then
      rm -rf "${DATA_DIRS[@]}"
      info "Data deleted"
    else
      echo "Data kept."
    fi
  else
    echo "Configuration and environments kept (use --purge to delete them)."
  fi
  info "Uninstalled"
}

case $COMMAND in
  install | update) install_app ;;
  uninstall) uninstall_app ;;
  -h | --help | help) usage ;;
  *) die "unknown command: $COMMAND (install, update or uninstall)" ;;
esac
