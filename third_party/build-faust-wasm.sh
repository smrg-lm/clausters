#!/usr/bin/env bash
#
# Reproducible vendored build of **libfaust-wasm**: the Faust compiler itself,
# as a WebAssembly library, so a page can compile a FaustDef.
#
# Same source and same pin as the native libfaust this repository builds
# (`faust.pin`, `build-faust.sh`) -- and that is the point of it, not a
# convenience. A def compiled in a tab and the same def compiled in a window
# must be the same DSP, and what decides that is the compiler's version and
# patches. One pin, two builds; the published `@grame/faustwasm` is upstream's
# build of some other version, and using it would put a second compiler in the
# project without saying so.
#
# One artifact triple, staged as static assets by the web client's build.sh:
#
#   libfaust-wasm.js     the Emscripten glue (exports `FaustModule`)
#   libfaust-wasm.wasm   the compiler
#   libfaust-wasm.data   the Faust standard library, for Emscripten's virtual FS
#
# Usage:
#   third_party/build-faust-wasm.sh [--out DIR]
#
# Environment (all optional):
#   FAUST_WASM_OUT       where the triple is written (default:
#                        clients/web/vendor/faust); --out wins
#   FAUST_SRC            source tree (default: third_party/faust)
#   FAUST_SKIP_FETCH=1   build FAUST_SRC as-is, skipping the fetch/checkout
#   EMSDK                the Emscripten SDK to source (default:
#                        ~/.local/lib/emsdk, the user-space install
#                        `clients/web/BUILD.md` documents)
#
# Requirements: the Emscripten SDK (emcc/em++), cmake and make.
#
# **Two things this does that `make wasmlib` alone does not**, both of them
# collisions between the pinned Faust's build files and current tooling, and
# both worth naming because each fails in a way that does not name itself:
#
#   1. `-DCMAKE_LINK_DEPENDS_USE_LINKER=OFF`. CMake 3.31+ asks the linker to
#      write a dependency file (`-Wl,--dependency-file=...`); wasm-ld does not
#      know the flag and stops with "unknown argument".
#   2. The link runs through **em++**, not emcc. Faust's own CMakeLists sets
#      `CMAKE_CXX_COMPILER emcc`, and a current emcc does not pull libc++ in at
#      link time -- so the link fails with a few hundred undefined
#      `std::__2::` symbols, which reads like a broken toolchain and is not one.
#
# **And it patches the bindings**, which is the fifth departure and the largest:
# upstream binds the Box and Signal APIs for the *native* library and not for
# the wasm one, so a page could only ever compile Faust *source*. The backend
# entry points already exist (`createWasmDSPFactoryFromBoxes` and
# `...FromSignals`, in `wasm_dynamic_dsp_aux.cpp`); what was missing was the
# JS-facing surface over them and the C box/signal API being exported at all.
# `faust-wasm-bindings.patch` adds four embind methods, and the link exports the
# C API itself -- the list read out of `src/faust/ffi.rs`, so the two cannot
# drift: whatever the one interpreter calls, the artifact exports.
#
# This matters more than it looks. Without it a def built with the signal or box
# API compiles in a window and fails in a tab, and the same client program means
# two different things depending on where it runs. With it, `faust::boxes` and
# `faust::signals` -- the *one* JSON interpreter, in Rust -- run in the page's
# Worker and drive this compiler through those exports.
#
# And two things it does to the artifact, both so it can be *imported* rather
# than script-tagged:
#
#   3. `-s ENVIRONMENT=web,worker`. The default glue carries Node branches full
#      of `require(...)`, which makes the file ambiguous between CommonJS and
#      ESM -- and this one is loaded from a module Worker. Dropping them also
#      drops a few kilobytes we would never run.
#   4. The CommonJS tail is replaced by `export default FaustModule;`, which is
#      what upstream's own `make wasm` does for its worklet variant.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/.." && pwd)"
# shellcheck source=/dev/null
. "$here/faust.pin"

out="${FAUST_WASM_OUT:-$root/clients/web/vendor/faust}"
src="${FAUST_SRC:-$here/faust}"
emsdk="${EMSDK:-$HOME/.local/lib/emsdk}"

while [ $# -gt 0 ]; do
    case "$1" in
        --out) out="$2"; shift 2 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

if [ ! -d "$src" ]; then
    echo "no Faust source at $src; run third_party/build-faust.sh first" >&2
    exit 1
fi

if [ ! -f "$emsdk/emsdk_env.sh" ]; then
    echo "no Emscripten SDK at $emsdk (set EMSDK=...); see clients/web/BUILD.md" >&2
    exit 1
fi
# The SDK's env script is written for an interactive shell.
set +u
# shellcheck source=/dev/null
. "$emsdk/emsdk_env.sh" >/dev/null 2>&1
set -u
command -v em++ >/dev/null || { echo "em++ is not on the PATH after sourcing $emsdk" >&2; exit 1; }

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

if [ "${FAUST_SKIP_FETCH:-0}" != "1" ]; then
    head="$(git -C "$src" rev-parse HEAD)"
    if [ "$head" != "$FAUST_SHA" ]; then
        echo "the Faust tree is at $head, not the pinned $FAUST_SHA;" >&2
        echo "run third_party/build-faust.sh (or set FAUST_SKIP_FETCH=1)" >&2
        exit 1
    fi
    # `build/` is tracked at the pin and is what the recipe lives in; a tree
    # someone cleaned has to get it back before anything else works.
    git -C "$src" checkout -- build
fi

echo "== patching the wasm bindings (the Box and Signal APIs)"
git -C "$src" apply --check "$here/faust-wasm-bindings.patch" 2>/dev/null \
    && git -C "$src" apply "$here/faust-wasm-bindings.patch" \
    || echo "   (already applied)"

echo "== configuring (pin $FAUST_SHA)"
build="$src/build"
faustdir="$build/faustdir"
mkdir -p "$faustdir"
cmake -S "$build" -B "$faustdir" \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_LINK_DEPENDS_USE_LINKER=OFF >/dev/null

echo "== building the compiler (this takes a while)"
# The Faust makefile's own wasmlib target: it stages the standard library and
# the mixers into the virtual filesystem the .data file is built from.
# The objects always build; the link is what a current SDK refuses, so it is
# expected to fail here and is redone below with the two changes above. Letting
# make try first keeps the recipe's own work: staging the standard library and
# the mixers into the virtual filesystem the .data file is built from.
make -C "$build" wasmlib || true

# And re-stage it when make did not, which is the case whenever the previous
# run succeeded: `wasmlib` ends in `rm -rf wasm-filesystem`, and cmake reaches
# that line only when it has nothing to relink -- so the recipe deletes the
# directory the relink below needs, and the second run of this script dies in
# `file_packager` with "$../../wasm-filesystem@usr does not exist". These are
# the makefile's own two lines (build/Makefile, target `wasmlib`).
if [ ! -d "$build/wasm-filesystem" ]; then
    mkdir -p "$build/wasm-filesystem/share/faust" "$build/wasm-filesystem/rsrc"
    cp "$src"/libraries/*.lib "$src"/libraries/dx7/*.lib "$src"/libraries/old/*.lib \
       "$build/wasm-filesystem/share/faust"
    cp "$src"/architecture/webaudio/mixer32.wasm \
       "$src"/architecture/webaudio/mixer64.wasm "$build/wasm-filesystem/rsrc"
fi

echo "== linking with em++, for the browser"
# The C box/signal API the one interpreter calls, read out of its own binding so
# the artifact and `faust::ffi` cannot disagree. `_malloc`/`_free` come along
# because the caller writes a signal vector into this module's heap, and the
# runtime methods because every label crosses as bytes.
exports=$(grep -oE '\bpub fn (Cbox[A-Za-z0-9_]*|Csig[A-Za-z0-9_]*|CDSPToBoxes)\b' \
              "$root/src/faust/ffi.rs" \
          | awk '{print "\"_"$3"\""}' | sort -u | paste -sd,)
exports="$exports,\"_malloc\",\"_free\""
runtime='"stringToUTF8","lengthBytesUTF8","UTF8ToString","HEAPU8","HEAP32","getValue","setValue"'

( cd "$faustdir/emcc" \
  && sed -e '1s/^emcc /em++ /' \
         -e "1s| -o libfaust-wasm.js| -s ENVIRONMENT=web,worker -s EXPORTED_FUNCTIONS=[$exports] -s EXPORTED_RUNTIME_METHODS=[$runtime] -o libfaust-wasm.js|" \
         CMakeFiles/wasmlib.dir/link.txt > .relink.sh \
  && bash .relink.sh \
  && rm -f .relink.sh )

[ -f "$faustdir/emcc/libfaust-wasm.wasm" ] || { echo "the link produced nothing" >&2; exit 1; }

mkdir -p "$out"
cp "$faustdir/emcc"/libfaust-wasm.js \
   "$faustdir/emcc"/libfaust-wasm.wasm \
   "$faustdir/emcc"/libfaust-wasm.data "$out/"

# The glue ends in a CommonJS export; make it an ES module instead, so a module
# Worker can import it.
python3 "$here/faust-wasm-esm.py" "$out/libfaust-wasm.js"

echo "== staged into $out"
ls -la "$out"
