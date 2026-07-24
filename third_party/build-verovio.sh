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
# Two targets, because two consumers need two different artifacts out of the one
# pinned tree:
#
#   --library   the shared libverovio.so, the headers and the SMuFL/CSS
#   (default)   resource data, installed into a prefix. What a native producer
#               links against. The resource path is baked in at configure time
#               from the prefix, so the prefix is self-contained.
#   --python    the `verovio` Python module (SWIG), installed into the active
#               interpreter. What the Python client engraves with -- and
#               building it from the pin is what makes the score editor work at
#               all, since the released wheel's is dead (see verovio.pin).
#
# Both may be given; with neither, --library runs. They are separate compiles of
# the same sources (one shared library, one static library plus the SWIG module),
# so asking for both costs two builds.
#
# Usage:
#   third_party/build-verovio.sh [--prefix DIR] [--library] [--python]
#
# Environment (all optional):
#   VEROVIO_PREFIX          install prefix (default: $HOME/.local); --prefix wins
#   VEROVIO_SRC             source tree (default: third_party/verovio)
#   VEROVIO_BUILD           cmake build dir (default: <src>/build-clausters)
#   VEROVIO_ORIGIN          git remote to fetch from (default: whatever
#                           verovio.pin sets; an env value still wins)
#   PYTHON                  interpreter --python installs into (default: python3)
#   VEROVIO_BUILD_PYTHON    interpreter that *compiles* the module (default:
#                           $PYTHON). Worth setting when $PYTHON has no
#                           development headers: the extension is built against
#                           the stable ABI, so a wheel built by any CPython
#                           >= 3.10 installs into any other.
#   VEROVIO_WHEELDIR        where --python leaves the built wheel
#                           (default: <src>/dist-clausters)
#   VEROVIO_SKIP_FETCH=1    build VEROVIO_SRC as-is, skipping the fetch/checkout
#                           of the pin (for iterating on an existing checkout)
#   VEROVIO_ALLOW_CHECKOUT=1  allow checking out the pinned SHA over an existing,
#                           different HEAD (otherwise a working clone is
#                           protected)
#
# Requirements (system packages): cmake, make, a C++20 compiler. Nothing else --
# verovio vendors its own dependencies and has no submodules. The --python
# target also needs SWIG and the Python development headers; pip pulls SWIG from
# PyPI into the isolated build environment, and VEROVIO_BUILD_PYTHON above
# covers the headers without sudo.

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
python="${PYTHON:-python3}"
build_library=0
build_python=0

while [ $# -gt 0 ]; do
  case "$1" in
    --prefix) prefix="$2"; shift 2 ;;
    --prefix=*) prefix="${1#*=}"; shift ;;
    --library) build_library=1; shift ;;
    --python) build_python=1; shift ;;
    -h|--help) sed -n '2,54p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
# No target asked for: the library, which is what a prefix install means here.
if [ "$build_library" = 0 ] && [ "$build_python" = 0 ]; then
  build_library=1
fi

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
if [ "$build_library" = 1 ]; then
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
fi

# --- The Python module -------------------------------------------------------
# pip drives the same cmake project through scikit-build-core (pyproject.toml
# sets BUILD_AS_PYTHON), pulling SWIG into the isolated build environment.
#
# Build and install are two steps rather than one `pip install <src>` because
# they can need two different interpreters. Compiling needs the development
# headers, which a distro python often ships in a separate -dev package (and
# installing that is the one thing here that would want sudo). But the extension
# is built against the *stable ABI* (Py_LIMITED_API 3.10, so the wheel is tagged
# cp310-abi3), which means the wheel one interpreter builds is installable into
# any other >= 3.10. So: build with an interpreter that has headers, install
# into the one you actually use.
if [ "$build_python" = 1 ]; then
  builder="${VEROVIO_BUILD_PYTHON:-$python}"
  wheeldir="${VEROVIO_WHEELDIR:-$src/dist-clausters}"

  # Fail here, with the fix, rather than 200 lines into a cmake FindPython error.
  if ! "$builder" -c 'import os, sys, sysconfig
sys.exit(0 if os.path.exists(os.path.join(sysconfig.get_paths()["include"], "Python.h")) else 1)'; then
    echo "error: $builder has no development headers (Python.h), so the" >&2
    echo "       extension cannot be compiled with it." >&2
    echo "       Either install the matching -dev package, or -- no sudo needed --" >&2
    echo "       build with another interpreter and let the stable ABI carry the" >&2
    echo "       wheel across; any CPython >= 3.10 that ships headers will do:" >&2
    echo "         uv python install 3.12" >&2
    echo "         VEROVIO_BUILD_PYTHON=\"\$(uv python find 3.12)\" $0 --python" >&2
    exit 1
  fi

  echo ">> building the verovio Python module from $src with $("$builder" -V)"
  # scikit-build-core forwards SKBUILD_CMAKE_DEFINE to the same cmake project
  # the --library target configures directly, so one option list drives both.
  defines="$(IFS=';'; printf '%s' "${vrv_options[*]#-D}")"
  SKBUILD_CMAKE_DEFINE="$defines" \
    "$builder" -m pip wheel --no-deps --wheel-dir "$wheeldir" "$src"
  wheel="$(ls -t "$wheeldir"/verovio-*.whl | head -1)"
  echo ">> installing $(basename "$wheel") into $("$python" -V)"
  "$python" -m pip install --force-reinstall "$wheel"

  echo ">> done. Sanity check:"
  "$python" -u - <<'EOF'
import re
import verovio

tk = verovio.toolkit()
print("verovio", tk.getVersion())
tk.loadData("@clef:G-2\n@timesig:4/4\n@data:4CDEF/")

# The editor is the reason this build exists: on the released wheel every
# edit() returns False, because the guard around m_editorToolkit never opens
# and the pointer stays null (see verovio.pin). So a *successful* edit is the
# check -- drag the first note and confirm the toolkit accepted it.
#
# Note it is deliberately not `undo` on a fresh toolkit: with a working editor
# that dereferences an empty undo stack and SIGSEGVs, so the very probe that
# proves the released wheel is broken crashes the fixed one. Always gate
# undo/redo on editInfo()'s canUndo/canRedo.
note = re.search(r'<g id="([^"]+)" class="note"', tk.renderToSVG(1)).group(1)
ok = tk.edit({"action": "drag", "param": {"elementId": note, "x": 0, "y": 20}})
print("editor alive:", ok, "--", tk.editInfo())
raise SystemExit(0 if ok else 1)
EOF
fi
