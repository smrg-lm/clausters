#!/usr/bin/env bash
#
# Reproducible vendored build of libfaust (LLVM backend) for Clausters.
#
# The Faust source is NOT committed (it is heavy and git-ignored under
# third_party/faust); reproducibility comes from this script plus third_party/
# faust.pin, which pin the exact commit and the LLVM version. One recipe, shared
# by local development, CI and the release wheel, so the bundled libfaust /
# libLLVM are deterministic instead of "whatever the build host had".
#
# What it does: fetch the pinned commit (+ submodules) into the source tree,
# build the dynamic libfaust.so against the pinned system libLLVM, install into
# a prefix, and stage that libLLVM beside libfaust.so in <prefix>/lib -- so a
# consumer resolves both through the DT_RPATH build.rs writes (<prefix>/lib),
# with no LLVM runtime installed. This is the same self-contained layout the
# wheel ships and the CI jobs restore.
#
# Usage:
#   third_party/build-faust.sh [--prefix DIR]
#
# Environment (all optional):
#   FAUST_PREFIX          install prefix (default: $HOME/.local); --prefix wins
#   FAUST_SRC             source tree (default: third_party/faust)
#   FAUST_ORIGIN          git remote to fetch from
#                         (default: https://github.com/grame-cncm/faust)
#   LLVM_CONFIG           llvm-config binary
#                         (default: llvm-config-<FAUST_LLVM_VERSION> from the pin)
#   FAUST_SKIP_FETCH=1    build FAUST_SRC as-is, skipping the fetch/checkout of
#                         the pin (for iterating on an existing checkout)
#   FAUST_ALLOW_CHECKOUT=1  allow checking out the pinned SHA over an existing,
#                         different HEAD (otherwise a working clone is protected)
#
# Requirements (system packages; the only ones that may need sudo -- see
# BUILD-FAUST.md): cmake, make, g++, llvm-<version>-dev, zlib1g-dev.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=/dev/null
. "$here/faust.pin"

prefix="${FAUST_PREFIX:-$HOME/.local}"
src="${FAUST_SRC:-$here/faust}"
origin="${FAUST_ORIGIN:-https://github.com/grame-cncm/faust}"
llvm_config="${LLVM_CONFIG:-llvm-config-$FAUST_LLVM_VERSION}"

while [ $# -gt 0 ]; do
  case "$1" in
    --prefix) prefix="$2"; shift 2 ;;
    --prefix=*) prefix="${1#*=}"; shift ;;
    -h|--help) sed -n '2,40p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

if ! command -v "$llvm_config" >/dev/null 2>&1; then
  echo "error: '$llvm_config' not found. Install llvm-$FAUST_LLVM_VERSION-dev, or" >&2
  echo "       set LLVM_CONFIG to your llvm-config binary." >&2
  exit 1
fi

# --- Fetch the pinned commit (unless told to build the tree as-is) -----------
if [ "${FAUST_SKIP_FETCH:-0}" != 1 ]; then
  if [ ! -e "$src/.git" ]; then
    git init -q "$src"
    git -C "$src" remote add origin "$origin"
  fi
  git -C "$src" remote set-url origin "$origin" 2>/dev/null \
    || git -C "$src" remote add origin "$origin"
  if ! git -C "$src" cat-file -e "${FAUST_SHA}^{commit}" 2>/dev/null; then
    git -C "$src" fetch --depth 1 origin "$FAUST_SHA"
  fi
  # --verify -q: empty (not the literal "HEAD") on a fresh/unborn checkout.
  head="$(git -C "$src" rev-parse --verify -q HEAD || true)"
  if [ "$head" != "$FAUST_SHA" ]; then
    if [ -n "$head" ] && [ "${FAUST_ALLOW_CHECKOUT:-0}" != 1 ]; then
      echo "error: $src is at $head, not the pinned $FAUST_SHA." >&2
      echo "       It looks like a working clone; refusing to move it." >&2
      echo "       Re-run with FAUST_ALLOW_CHECKOUT=1 to check out the pin," >&2
      echo "       or FAUST_SKIP_FETCH=1 to build the current checkout as-is." >&2
      exit 1
    fi
    git -C "$src" checkout -q --detach "$FAUST_SHA"
  fi
  git -C "$src" submodule update --init --recursive --depth 1
fi

# --- Build + install ---------------------------------------------------------
# `make most` builds the CLI compiler and libfaust.a; INCLUDE_DYNAMIC=ON adds
# the shared libfaust.so that build.rs links. INCLUDE_STATIC=off skips the
# static libfaustwithllvm.a (it embeds LLVM's component libs and needs Polly).
# LINK_LLVM_STATIC=off links the monolithic system libLLVM.so (small .so, no
# Polly/zstd). See BUILD-FAUST.md for the rationale.
echo ">> building libfaust from $src against $($llvm_config --version) ($llvm_config)"
CMAKE_BUILD_PARALLEL_LEVEL="$(nproc)" make -C "$src" most \
  CMAKEOPT="-DINCLUDE_DYNAMIC=ON -DINCLUDE_STATIC=off -DLINK_LLVM_STATIC=off -DLLVM_CONFIG=$llvm_config"
echo ">> installing into $prefix"
make -C "$src" install PREFIX="$prefix"

# --- Stage the libLLVM libfaust.so is NEEDED-linked against ------------------
# Copy it into <prefix>/lib so a consumer that only has the prefix (no llvm-dev)
# resolves it through Clausters' DT_RPATH. cp -L follows the ldd-resolved
# symlink to the real file and keeps its SONAME basename.
libfaust_so="$prefix/lib/libfaust.so"
libllvm="$(ldd "$libfaust_so" | awk 'tolower($0) ~ /llvm/ {print $3}')"
if [ -z "$libllvm" ] || [ ! -f "$libllvm" ]; then
  echo "error: could not locate libLLVM via ldd of $libfaust_so:" >&2
  ldd "$libfaust_so" >&2
  exit 1
fi
cp -Lv "$libllvm" "$prefix/lib/$(basename "$libllvm")"

echo ">> done. Sanity check:"
"$prefix/bin/faust" --version
ldd "$libfaust_so" | grep -i llvm || true
