#!/usr/bin/env bash
# Installs, updates or uninstalls clausters-editor for the current user (no sudo).
#
#   ./install.sh install     builds and copies the binary, the documentation, the licenses
#                            and the menu entry
#   ./install.sh update      the same as install (rebuilds and replaces)
#   ./install.sh uninstall   removes what was installed; with --purge also deletes the app's
#                            configuration, virtual environments and cache
#
# Options:
#   --prefix DIR   destination (default ~/.local): DIR/bin, DIR/share/clausters-editor, DIR/share
#   --no-build     does not rebuild; uses the binary already built
#   --purge        (uninstall) deletes the app's data
#   -y, --yes      does not ask for confirmation
set -euo pipefail

# The crate's folder, and the repository's root, where cargo builds it.
PROJECT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(cd "$PROJECT_DIR/../.." && pwd)

# The program's name: its binary, its folders and its menu entry.
APP=clausters-editor
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
# The binary finds its book at ../share/clausters-editor/docs.
SHARE="$PREFIX/share/$APP"
DESKTOP="$PREFIX/share/applications/$APP.desktop"
ICON="$PREFIX/share/icons/hicolor/128x128/apps/$APP.png"
BUILT="$ROOT/target/release/$APP"

# Data the app creates, under its name (src/native/paths.rs).
DATA_DIRS=(
  "${XDG_CONFIG_HOME:-$HOME/.config}/$APP"
  "${XDG_DATA_HOME:-$HOME/.local/share}/$APP"
  "${XDG_CACHE_HOME:-$HOME/.cache}/$APP"
  # the webview application's, if the editor never started to move them
  "${XDG_CONFIG_HOME:-$HOME/.config}/io.github.smrg-lm.clausters.editor"
  "${XDG_DATA_HOME:-$HOME/.local/share}/io.github.smrg-lm.clausters.editor"
  "${XDG_CACHE_HOME:-$HOME/.cache}/io.github.smrg-lm.clausters.editor"
)

build() {
  command -v cargo >/dev/null || die "cargo (Rust) not found"
  info "Building (cargo build -p $APP --release)..."
  (cd "$ROOT" && cargo build -p "$APP" --release)
}

install_app() {
  if [[ $BUILD == 1 ]]; then build; fi
  [[ -x $BUILT ]] || die "$BUILT does not exist; build without --no-build"

  info "Installing into $PREFIX"
  install -Dm755 "$BUILT" "$BIN"
  # the webview application's book, which the new binary does not read
  rm -rf "$PREFIX/lib/$APP"

  # The whole folder is replaced so that documents deleted from the project do not linger.
  rm -rf "$SHARE"
  mkdir -p "$SHARE"
  cp -r "$PROJECT_DIR/docs" "$SHARE/docs"
  cp -r "$PROJECT_DIR/licenses" "$SHARE/licenses"

  install -Dm644 "$PROJECT_DIR/icons/128x128.png" "$ICON"
  mkdir -p "$(dirname "$DESKTOP")"
  cat >"$DESKTOP" <<EOF
[Desktop Entry]
Type=Application
Name=$APP
Comment=Python editor with an interactive session
Exec=$BIN %F
MimeType=text/x-python;text/x-python3;
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
  # and where the webview application kept its book, if it was installed
  rm -rf "$SHARE" "$PREFIX/lib/$APP"
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
