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
# build the dynamic libfaust.so against the pinned LLVM -- statically, and only
# the components the JIT reaches, which is what keeps it one self-contained
# library instead of a pair with a 137 MiB libLLVM (see "What the shipped
# libfaust.so carries" below) -- and install it into a prefix. A consumer
# resolves it through the DT_RPATH build.rs writes (<prefix>/lib), with no LLVM
# runtime installed. This is the same layout the wheel ships and the CI jobs
# restore.
#
# Usage:
#   third_party/build-faust.sh [--prefix DIR]
#
# Environment (all optional):
#   FAUST_PREFIX          install prefix (default: $HOME/.local); --prefix wins
#   FAUST_SRC             source tree (default: third_party/faust)
#   FAUST_ORIGIN          git remote to fetch from (default: whatever
#                         faust.pin sets, currently our fork -- it carries one
#                         patch not yet upstream; an env value still wins)
#   LLVM_CONFIG           llvm-config binary
#                         (default: llvm-config-<FAUST_LLVM_VERSION> from the pin)
#   FAUST_SKIP_FETCH=1    build FAUST_SRC as-is, skipping the fetch/checkout of
#                         the pin (for iterating on an existing checkout)
#   FAUST_ALLOW_CHECKOUT=1  allow checking out the pinned SHA over an existing,
#                         different HEAD (otherwise a working clone is protected)
#
# Requirements (system packages; the only ones that may need sudo -- see
# BUILD-FAUST.md): cmake, make, g++, llvm-<version>-dev, zlib1g-dev. The static
# link also reaches LLVM's own system libraries; where only a runtime package is
# installed the recipe links the versioned file, so no extra -dev is required.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# faust.pin now carries FAUST_ORIGIN too, and sourcing it would clobber an
# override from the environment -- so remember that one first and let it win.
env_origin="${FAUST_ORIGIN:-}"
# shellcheck source=/dev/null
. "$here/faust.pin"

prefix="${FAUST_PREFIX:-$HOME/.local}"
src="${FAUST_SRC:-$here/faust}"
origin="${env_origin:-${FAUST_ORIGIN:-https://github.com/grame-cncm/faust}}"
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

# --- What the shipped libfaust.so carries -------------------------------------
# libfaust links LLVM *statically*, and only the components the JIT reaches.
# That is the difference between a 146 MiB pair -- a 9 MiB libfaust.so plus the
# distro's 137 MiB monolithic libLLVM.so, which the wheel had to bundle whole --
# and a single 43 MiB libfaust.so with nothing beside it. Three independent
# trims, each measured; the numbers and the reasoning are in docs/decisions.md.
#
#   1. LINK_LLVM_STATIC=on. The distro libLLVM is one shared object for the
#      entire toolchain (every backend, BOLT, the DWARF linkers, Exegesis), and
#      a NEEDED link takes all of it whether or not anything calls it. Against
#      the static components the linker takes only the archive members it
#      references. It also empties most of the NEEDED closure: libxml2, libedit,
#      libtinfo, libffi, libbsd and libmd drop out, so the wheel stops vendoring
#      those too. Asking llvm-config for a component list rather than for
#      `--libs` is also what keeps Polly out of the link, which is the reason
#      this was off before.
#   2. One target backend instead of twenty. The JIT compiles for the machine it
#      runs on, so a per-platform artifact wants its own architecture and no
#      other. -ULLVM_BUILD_UNIVERSAL is what makes that possible: upstream's
#      build/CMakeLists.txt defines LLVM_BUILD_UNIVERSAL on *every* platform
#      (line 141, outside the `if (UNIVERSAL)` that line 169 guards, which is
#      therefore dead), and that is what compiles the InitializeAllTargets()
#      call in llvm_dynamic_dsp_aux.cpp and pins a reference to all twenty.
#      Undefined, initJIT is left with InitializeNativeTarget() alone. What it
#      gives up is emitting machine code for a *different* triple
#      (writeDSPFactoryToMachineFile), which Clausters never asks for.
#      There is no silent failure mode here: if the flag ever stopped taking,
#      the link fails on an undefined LLVMInitialize<other-arch>TargetInfo.
#   3. One Faust backend in the shared library. The server only ever JITs, so
#      the .so carries the LLVM backend and the other seventeen stay in the CLI
#      compiler (COMPILER, below) -- which is where `faust -lang c` reads them,
#      so the developer-facing instrument is unchanged.
#
# Plus the section flags: a shared object exports everything by default, which
# left ~26k LLVM symbols as roots that no collector could touch. Hiding the
# archives' symbols (--exclude-libs,ALL) brings the exported set to ~1.4k and
# lets --gc-sections do its work; the Faust API is compiled into the target
# rather than pulled from an archive, so it stays exported.
case "$(uname -m)" in
  x86_64|amd64)  llvm_target=x86 ;;
  aarch64|arm64) llvm_target=aarch64 ;;
  *) echo "error: unknown machine '$(uname -m)'; add its LLVM target here." >&2; exit 1 ;;
esac

llvm_components="mcjit executionengine $llvm_target ipo passes irreader linker bitwriter bitreader analysis target core support"
llvm_libs="$("$llvm_config" --link-static --libs $llvm_components | tr ' ' ';')"

# LLVM's own system dependencies. `-lfoo` needs the development symlink; where
# only the runtime package is installed, link the versioned file directly --
# the SONAME recorded in DT_NEEDED is the same either way, so this costs
# nothing and saves asking for a -dev package per LLVM release.
llvm_syslibs=""
for l in $("$llvm_config" --link-static --system-libs); do
  name="${l#-l}"
  if [ "$name" = "$l" ]; then llvm_syslibs="$llvm_syslibs;$l"; continue; fi
  path="$(${CC:-cc} -print-file-name="lib$name.so")"
  if [ "$path" = "lib$name.so" ]; then
    path="$(ls -1 /usr/lib/*/"lib$name.so".* 2>/dev/null | head -n1)"
    if [ -z "$path" ]; then
      echo "error: LLVM needs lib$name and neither lib$name.so nor a versioned" >&2
      echo "       lib$name.so.N is installed. Install lib$name-dev." >&2
      exit 1
    fi
  fi
  llvm_syslibs="$llvm_syslibs;$path"
done

# Every Faust backend but LLVM's, kept in the CLI compiler and out of the .so.
backends=""
for b in AS C CODEBOX CPP CMAJOR CSHARP DLANG FIR INTERP INTERP_COMP JAVA JAX \
         JULIA JSFX OLDCPP RUST SDF3 TEMPLATE WASM; do
  backends="$backends -D${b}_BACKEND=COMPILER"
done

# --- Build + install ---------------------------------------------------------
# `make most` builds the CLI compiler; INCLUDE_DYNAMIC=ON adds the shared
# libfaust.so that build.rs links. INCLUDE_STATIC=off skips libfaustwithllvm.a
# (it embeds LLVM's component libs and needs Polly).
#
# FAUSTDIR is *not* the default `faustdir`: that one belongs to
# build-faust-wasm.sh, which reconfigures it without a backend file and so
# inherits whatever this build last cached there. Two recipes, two directories.
echo ">> building libfaust from $src against $($llvm_config --version) ($llvm_config)"
CMAKE_BUILD_PARALLEL_LEVEL="$(nproc)" make -C "$src" most \
  FAUSTDIR=faustdir-native \
  USE_LLVM_CONFIG=off \
  LLVM_PACKAGE_VERSION="$("$llvm_config" --version)" \
  LLVM_INCLUDE_DIRS="$("$llvm_config" --includedir)" \
  LLVM_LIB_DIR="$("$llvm_config" --libdir)" \
  LLVM_LD_FLAGS="$("$llvm_config" --ldflags)" \
  LLVM_LIBS="$llvm_libs$llvm_syslibs" \
  LLVM_DEFINITIONS="-ULLVM_BUILD_UNIVERSAL" \
  CMAKEOPT="-DINCLUDE_DYNAMIC=ON -DINCLUDE_STATIC=off -DLINK_LLVM_STATIC=on $backends \
            -DCMAKE_CXX_FLAGS=-ffunction-sections\ -fdata-sections \
            -DCMAKE_SHARED_LINKER_FLAGS=-Wl,--gc-sections\ -Wl,--exclude-libs,ALL"
echo ">> installing into $prefix"
make -C "$src" install PREFIX="$prefix" FAUSTDIR=faustdir-native

# --- Check that nothing LLVM-shaped is left to bundle ------------------------
# The whole point of the link above: the prefix is libfaust.so and its handful
# of system libraries, with no libLLVM to stage beside it. A libLLVM in NEEDED
# means one of the trims silently stopped applying.
libfaust_so="$prefix/lib/libfaust.so"
if ldd "$libfaust_so" | grep -qi llvm; then
  echo "error: $libfaust_so is still NEEDED-linked against a shared libLLVM:" >&2
  ldd "$libfaust_so" | grep -i llvm >&2
  echo "       the static link did not take -- see the comment above." >&2
  exit 1
fi
rm -f "$prefix"/lib/libLLVM.so.* 2>/dev/null || true

echo ">> done. Sanity check:"
"$prefix/bin/faust" --version
ls -l "$(readlink -f "$libfaust_so")"
ldd "$libfaust_so"
