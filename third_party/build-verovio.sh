#!/usr/bin/env bash
#
# Reproducible vendored build of verovio for Clausters.
#
# The verovio source is NOT committed (it is heavy and git-ignored under
# third_party/verovio); reproducibility comes from this script plus third_party/
# verovio.pin, which pins the exact commit. One recipe, shared by local
# development and CI, so what we build is deterministic instead of "whatever the
# build host had" -- the same arrangement as build-faust.sh next to it.
#
# One artifact: the shared libverovio.so, its headers and the SMuFL/CSS resource
# data, installed into a prefix -- exactly the arrangement build-faust.sh uses
# for libfaust, down to the default prefix, and consumed the same way
# (clients/python/build_native.py stages the library and its resources into the
# wheel, as it already does for libfaust and libLLVM).
#
# Deliberately *only* the library. verovio also ships a SWIG Python module, and
# building that would mean a second compile of the same sources, a second copy
# in site-packages, and a distribution literally named `verovio` that pip can
# replace with the published one -- whose score editor is dead (see verovio.pin).
# Every consumer goes through this one .so instead: the Python client binds its C
# API (tools/c_wrapper.h) with ctypes, and a wasm build would reuse the same
# surface.
#
# The resource path is baked into the binary at configure time from the prefix,
# so the prefix is self-contained; a copy staged elsewhere is told where its data
# is at runtime.
#
# Usage:
#   third_party/build-verovio.sh [--prefix DIR]
#
# Environment (all optional):
#   VEROVIO_PREFIX          install prefix (default: $HOME/.local); --prefix wins
#   VEROVIO_SRC             source tree (default: third_party/verovio)
#   VEROVIO_BUILD           cmake build dir (default: <src>/build-clausters)
#   VEROVIO_ORIGIN          git remote to fetch from (default: whatever
#                           verovio.pin sets; an env value still wins)
#   VEROVIO_SKIP_FETCH=1    build VEROVIO_SRC as-is, skipping the fetch/checkout
#                           of the pin (for iterating on an existing checkout)
#   VEROVIO_ALLOW_CHECKOUT=1  allow checking out the pinned SHA over an existing,
#                           different HEAD (otherwise a working clone is
#                           protected)
#
# Requirements (system packages): cmake, make, a C++20 compiler. That is the
# whole list -- verovio vendors its dependencies in-tree and has no submodules,
# and nothing here is language-specific, so there is no SWIG and no Python
# development headers to hunt for.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# verovio.pin carries VEROVIO_ORIGIN too, and sourcing it would clobber an
# override from the environment -- so remember that one first and let it win.
env_origin="${VEROVIO_ORIGIN:-}"
# shellcheck source=/dev/null
. "$here/verovio.pin"

prefix="${VEROVIO_PREFIX:-$HOME/.local}"
src="${VEROVIO_SRC:-$here/verovio}"
origin="${env_origin:-${VEROVIO_ORIGIN:-https://github.com/rism-digital/verovio}}"

while [ $# -gt 0 ]; do
  case "$1" in
    --prefix) prefix="$2"; shift 2 ;;
    --prefix=*) prefix="${1#*=}"; shift ;;
    -h|--help) sed -n '2,46p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

build="${VEROVIO_BUILD:-$src/build-clausters}"

# --- What we build out of verovio, and what we leave out ---------------------
# The importers are independent cmake options, so the build carries only the
# input formats the client actually offers. Kept: MEI (the canonical format --
# the edit cycle round-trips through it), MusicXML and its compressed form (what
# a user brings from a notation editor), and Plaine & Easie plus ABC (the two
# compact ones a score is typed in by hand). Dropped: Humdrum, GABC and DARMS.
#
# Only Humdrum is worth a size argument -- it vendors humlib, 148k lines of it,
# and dropping it takes the built wheel from 8.2 MB to 5.2 MB. The other three
# are noise either way (PAE and ABC measure ~10 KB apart), so they are out
# because nothing here reads them, not to save anything.
#
# NO_EDIT_SUPPORT stays off, obviously: the editor is why this build exists. It
# is worth knowing that in 6.2.1 the two were entangled -- the editor was guarded
# by Humdrum being *off*, so this trim alone would have revived it. Past the pin
# they are independent, so we get small and editable without the coupling.
vrv_options=(
  -DNO_HUMDRUM_SUPPORT=ON
  -DNO_GABC_SUPPORT=ON
  -DNO_DARMS_SUPPORT=ON
)

# --- Fetch the pinned commit (unless told to build the tree as-is) -----------
# No submodules to update: verovio vendors its dependencies in-tree.
if [ "${VEROVIO_SKIP_FETCH:-0}" != 1 ]; then
  if [ ! -e "$src/.git" ]; then
    git init -q "$src"
    git -C "$src" remote add origin "$origin"
  fi
  git -C "$src" remote set-url origin "$origin" 2>/dev/null \
    || git -C "$src" remote add origin "$origin"
  if ! git -C "$src" cat-file -e "${VEROVIO_SHA}^{commit}" 2>/dev/null; then
    git -C "$src" fetch --depth 1 origin "$VEROVIO_SHA"
  fi
  # --verify -q: empty (not the literal "HEAD") on a fresh/unborn checkout.
  head="$(git -C "$src" rev-parse --verify -q HEAD || true)"
  if [ "$head" != "$VEROVIO_SHA" ]; then
    if [ -n "$head" ] && [ "${VEROVIO_ALLOW_CHECKOUT:-0}" != 1 ]; then
      echo "error: $src is at $head, not the pinned $VEROVIO_SHA." >&2
      echo "       It looks like a working clone; refusing to move it." >&2
      echo "       Re-run with VEROVIO_ALLOW_CHECKOUT=1 to check out the pin," >&2
      echo "       or VEROVIO_SKIP_FETCH=1 to build the current checkout as-is." >&2
      exit 1
    fi
    git -C "$src" checkout -q --detach "$VEROVIO_SHA"
  fi
fi

# --- The shared library ------------------------------------------------------
# BUILD_AS_LIBRARY switches the cmake project from the command-line tool to a
# shared libverovio.so (it adds tools/c_wrapper.cpp) and turns on the install
# rules for the headers and the pkg-config file. CMAKE_INSTALL_PREFIX has to be
# right at *configure* time, not just at install: the resource directory is
# baked into the binary then (add_compile_definitions RESOURCE_DIR=...), and a
# toolkit that cannot find its SMuFL data engraves nothing.
echo ">> building libverovio from $src"
cmake -S "$src/cmake" -B "$build" \
  -DCMAKE_BUILD_TYPE=Release \
  -DBUILD_AS_LIBRARY=ON \
  -DCMAKE_INSTALL_PREFIX="$prefix" \
  "${vrv_options[@]}"
cmake --build "$build" --parallel "$(nproc)"
echo ">> installing into $prefix"
cmake --install "$build"

echo ">> done. Sanity check:"
ls -l "$prefix/lib/libverovio."*
PKG_CONFIG_PATH="$prefix/lib/pkgconfig" pkg-config --modversion verovio \
  || echo "(pkg-config not available; skipped the version check)"
