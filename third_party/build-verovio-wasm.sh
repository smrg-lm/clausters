#!/usr/bin/env bash
#
# Reproducible vendored build of verovio **for the browser**, beside the native
# one.
#
# Same source, same pin, same trim: this script is `build-verovio.sh` compiled
# by Emscripten instead of by the system compiler, and the point of it is that
# sentence. A page and a window must engrave with one verovio -- not one
# *version*, one **build** -- because what a build decides is which input
# formats exist, and a format one client reads and the other refuses is a
# difference in surface between the two clients, which this project treats as a
# defect rather than as packaging.
#
# That is also why the published npm artifact is not used: it is upstream's own
# build of the same tag, with a different importer set. It is *not* about size:
# measured, the published module is 7.0 MB raw / 2.2 MB gzipped and this build
# is 6.6 MB / 2.2 MB -- base64 costs a third on disk and gzip gives almost all
# of it back, so the two are the same download. Ours is the same importers as
# the native library, and that is the whole of the argument.
#
# One artifact pair, staged as a static asset by the web client's build.sh:
#
#   verovio.js    the Emscripten glue, an ES module exporting `createVerovioModule`
#   verovio.wasm  the engraver itself
#
# The SMuFL resource data is **embedded in the module** (`--embed-file data/`),
# not fetched: a page has no resource directory to point at, and the native
# side's equivalent -- the `share/verovio` tree staged next to the library -- is
# what a wheel carries for the same reason.
#
# Usage:
#   third_party/build-verovio-wasm.sh [--out DIR]
#
# Environment (all optional):
#   VEROVIO_WASM_OUT        where the pair is written (default:
#                           clients/web/vendor/verovio); --out wins
#   VEROVIO_SRC             source tree (default: third_party/verovio)
#   VEROVIO_WASM_BUILD      cmake build dir (default: <src>/build-clausters-wasm)
#   VEROVIO_ORIGIN          git remote to fetch from (default: whatever
#                           verovio.pin sets; an env value still wins)
#   VEROVIO_SKIP_FETCH=1    build VEROVIO_SRC as-is, skipping the fetch/checkout
#   VEROVIO_ALLOW_CHECKOUT=1  allow checking out the pinned SHA over an existing,
#                           different HEAD
#   EMSDK                   the Emscripten SDK to source (default:
#                           ~/.local/lib/emsdk, the user-space install
#                           `clients/web/BUILD.md` documents)
#
# Requirements: the Emscripten SDK (emcc/emcmake/emmake), cmake and make. The
# SDK is the one heavy addition the repository's tooling rule admits, and it is
# user-space: nothing in `src/` or the test loop touches it, and build.sh only
# stages what it produced.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# verovio.pin carries VEROVIO_ORIGIN too, and sourcing it would clobber an
# override from the environment -- so remember that one first and let it win.
env_origin="${VEROVIO_ORIGIN:-}"
# shellcheck source=/dev/null
. "$here/verovio.pin"

src="${VEROVIO_SRC:-$here/verovio}"
origin="${env_origin:-${VEROVIO_ORIGIN:-https://github.com/rism-digital/verovio}}"
out="${VEROVIO_WASM_OUT:-$here/../clients/web/vendor/verovio}"
emsdk="${EMSDK:-$HOME/.local/lib/emsdk}"

while [ $# -gt 0 ]; do
  case "$1" in
    --out) out="$2"; shift 2 ;;
    --out=*) out="${1#*=}"; shift ;;
    -h|--help) sed -n '2,50p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

build="${VEROVIO_WASM_BUILD:-$src/build-clausters-wasm}"

# --- What we build out of verovio, and what we leave out ---------------------
# **The same three options build-verovio.sh passes**, and they are copied rather
# than reasoned out again on purpose: the two builds differ in their compiler
# and in nothing else. Kept: MEI (the canonical format -- the edit cycle
# round-trips through it), MusicXML and its compressed form, Plaine & Easie and
# ABC. Dropped: Humdrum (which vendors humlib, 148k lines), GABC and DARMS.
#
# The fonts are *not* trimmed, for the same reason the importers are trimmed to
# the same list: the native prefix installs the whole SMuFL set, so cutting it
# here would leave a page unable to engrave in a font a window engraves in.
vrv_options=(
  -DNO_HUMDRUM_SUPPORT=ON
  -DNO_GABC_SUPPORT=ON
  -DNO_DARMS_SUPPORT=ON
)

# --- Fetch the pinned commit (unless told to build the tree as-is) -----------
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

# --- The Emscripten SDK ------------------------------------------------------
if ! command -v emcc >/dev/null 2>&1; then
  if [ -f "$emsdk/emsdk_env.sh" ]; then
    # shellcheck source=/dev/null
    . "$emsdk/emsdk_env.sh" >/dev/null 2>&1
  fi
fi
command -v em++ >/dev/null 2>&1 || {
  echo "error: em++ not found. Install the SDK (clients/web/BUILD.md) or set EMSDK." >&2
  exit 1
}

# The SDK is pinned too (third_party/emsdk.pin): this artifact is shipped and
# nothing in it comes from our sources, so the toolchain is half of what it is.
# shellcheck source=/dev/null
. "$here/emsdk.pin"
emcc_version="$(em++ --version 2>/dev/null | sed -n '1s/.*clang-like replacement[^)]*) \([0-9][0-9.]*\).*/\1/p')"
if [ "$emcc_version" != "$EMSDK_VERSION" ]; then
    echo "emcc is ${emcc_version:-unknown}, not the pinned $EMSDK_VERSION" >&2
    echo "(cd $emsdk && ./emsdk install $EMSDK_VERSION && ./emsdk activate $EMSDK_VERSION)" >&2
    echo "or set EMSDK_SKIP_PIN_CHECK=1 to build with the toolchain you have" >&2
    [ "${EMSDK_SKIP_PIN_CHECK:-0}" = "1" ] || exit 1
fi

# --- The resources that go inside the module ---------------------------------
# `--embed-file` takes a *directory as it stands*, so the SMuFL data is staged
# into the build directory first. This is what upstream's own buildToolkit does
# (its `syncSvgResources`), minus the font exclusions it offers and we decline.
data="$build/data"
echo ">> staging the SMuFL resources"
rm -rf "$data"
mkdir -p "$data"
cp -r "$src/data/." "$data/"
find "$data" -name '.DS_Store' -type f -delete

# The commit header the sources expect (upstream's build generates it too). It
# is run from a subdirectory on purpose: the script's first act is `cd ..`.
(cd "$src/tools" && ./get_git_commit.sh >/dev/null)

# --- Compile -----------------------------------------------------------------
# The flags are upstream's release ones for a wasm toolkit, with one deliberate
# omission: **no `-s SINGLE_FILE=1`**. That flag base64s the engraver into the
# glue, and while gzip makes the download the same either way, a real `.wasm` is
# compiled from bytes rather than decoded from text first, and it is cached and
# served as what it is.
cflags="-O3 -DNDEBUG -std=c++20 -s STRICT=1 -fwasm-exceptions"
lflags=(
  -s WASM=1
  -s INITIAL_MEMORY=128MB
  -s STACK_SIZE=64MB
  -s ALLOW_MEMORY_GROWTH
  -s INCOMING_MODULE_JS_API=onRuntimeInitialized
  -fwasm-exceptions
  -s MODULARIZE=1
  -s EXPORT_ES6=1
  -s "EXPORT_NAME='createVerovioModule'"
  -s "EXPORTED_RUNTIME_METHODS=[\"cwrap\",\"HEAPU8\"]"
)

echo ">> configuring (emcmake) in $build"
mkdir -p "$build"
emcmake cmake -S "$src/cmake" -B "$build" \
  -DBUILD_AS_WASM=ON \
  -DCMAKE_CXX_FLAGS="$cflags" \
  "${vrv_options[@]}"

echo ">> building libverovio.a"
emmake make -C "$build" -j "$(nproc)"

# The exported surface is upstream's own list, and it is the *same C wrapper*
# the native library exposes (`tools/c_wrapper.h`) -- which is the whole reason
# one `Engraver` port serves both: a page calls `_vrvToolkit_edit` through
# `cwrap`, a process calls `vrvToolkit_edit` through the C ABI.
# `em++` and not `emcc`: the archive is C++, and the C driver links it without
# the C++ runtime -- which fails at the end of a ten-minute build, on undefined
# symbols that say nothing about the cause. Upstream's own script uses `em++`
# for the same reason.
echo ">> linking the module"
mkdir -p "$out"
em++ "$build/libverovio.a" \
  $cflags \
  "${lflags[@]}" \
  --embed-file "$data@/data" \
  -s "EXPORTED_FUNCTIONS=@$src/emscripten/exports.txt" \
  -o "$out/verovio.js"

echo ">> done. Sanity check:"
ls -l "$out/verovio.js" "$out/verovio.wasm"
