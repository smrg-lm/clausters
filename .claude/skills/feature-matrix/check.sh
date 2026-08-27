#!/usr/bin/env bash
# The fmt + clippy + rustdoc matrix from CLAUDE.md's commit workflow, in one
# command.
#
# CI lints the default build set and the GUI host, but never the def-family
# matrix: a warning that only appears under --no-default-features, or under
# `synth` or `faust` alone, passes CI. Those three configurations are the whole
# reason this script exists; the rest are here so one run answers "is the tree
# committable?" without a second pass.
#
# CI never runs rustdoc either, in any configuration, so the doc build's own
# lints (broken intra-doc links above all) were watched by nothing until they
# were added here.
#
# The GUI host is a separate workspace with a feature set of its own; its matrix
# lives beside it (`clients/gui/check-features.sh`) and runs from here as one
# row, so this stays the single command.
#
# It only ever reads. Nothing here writes to your working tree: this is the gate
# that says whether the code is committable, and a gate that edits the thing it
# is judging cannot be trusted to report on it. `cargo clippy --fix` exists and
# is worth running, but deliberately, by hand, on one configuration at a time,
# with the diff read afterwards — not five times over five different views of
# the code inside a script whose output you skim.
#
# Every configuration runs even if an earlier one fails — a matrix that stops at
# the first error hides how many of the others were also broken.
#
# Usage:
#   check.sh          # everything
#   check.sh --fast   # only the configurations CI does not cover
set -uo pipefail

root="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$root"

fast=0
while [ $# -gt 0 ]; do
    case "$1" in
        --fast) fast=1 ;;
        -h | --help)
            sed -n '2,22p' "$0"
            exit 0
            ;;
        *)
            echo "check.sh: unknown option '$1' (--fast)" >&2
            exit 2
            ;;
    esac
    shift
done

labels=()
statuses=()
failed=0

run() {
    local label="$1"
    shift
    printf '\n\033[1m== %s\033[0m\n' "$label"
    printf '   $ %s\n\n' "$*"
    if "$@"; then
        labels+=("$label")
        statuses+=("ok")
    else
        labels+=("$label")
        statuses+=("FAIL")
        failed=1
    fi
}

clippy() {
    local label="$1"
    shift
    run "$label" cargo clippy "$@" -- -D warnings
}

doc() {
    local label="$1"
    shift
    run "$label" env RUSTDOCFLAGS="-D warnings" "$@"
}

# --- Covered by CI, run unless --fast ----------------------------------------

if [ "$fast" = 0 ]; then
    run "fmt (workspace)" cargo fmt --check
    run "fmt (gui)" cargo fmt --check --manifest-path clients/gui/Cargo.toml
    clippy "clippy: default features" --all-targets
fi

# --- The gap: the def-family matrix ------------------------------------------
#
# `neither` is also the build that must work with no libfaust on the machine.

clippy "clippy: neither def family" --all-targets --no-default-features
clippy "clippy: synth alone" --all-targets --no-default-features --features synth
clippy "clippy: faust alone" --all-targets --no-default-features --features faust

# `verovio` is off by default, so CI's `--workspace --all-targets` never enables
# it and the whole notation layer it pulls in goes unlinted -- the same gap as
# the def families, for the same reason. It needs no libverovio on the machine:
# clippy checks and never links, so a missing library is the build script's
# warning, not a failure. What this covers is the code under the feature's cfgs;
# that the library is actually installed is a different question, and the one
# `third_party/build-verovio.sh` answers.
clippy "clippy: ffi with verovio" -p clausters-ffi --features verovio --all-targets

# --- The third gap: the featureless build with no libfaust to link ------------
#
# Every row above type-checks; none of them *link*, and on a development machine
# libfaust is installed anyway, so "builds without libfaust" is the one claim
# this script could not make. It is also the claim that broke: the two wasm
# crates depend on `clausters` with `features = ["faust"]` -- the def family,
# whose compiler on wasm32 is libfaust-wasm -- and cargo unifies features across
# the workspace, so `--workspace --no-default-features` built them for the host
# and turned the family back into `-lfaust`. Green here, red on a runner that
# has no libfaust, for three days.
#
# So this row builds what CLAUDE.md and BUILD.md actually promise -- the root
# crate, no features -- with FAUST_PREFIX aimed at nothing, which is the runner.
# It is a build rather than a check because only the link can fail this way --
# and it links into a target directory of its own, so the script keeps its
# promise not to touch the working tree: a featureless `cargo build` would
# otherwise leave a `target/debug/clausters` with no audio backend behind, which
# is the trap the skill's last section is about.
run "build: no features, no libfaust" \
    env FAUST_PREFIX=/nonexistent-prefix \
    cargo build --no-default-features --target-dir target/matrix-nofaust

# --- The other gap: rustdoc --------------------------------------------------
#
# CI never builds the docs, so a broken intra-doc link -- a link to an item that
# was renamed, moved or made private -- lands silently and stays. The doc build
# is its own lint pass: `cargo clippy` says nothing about it.
#
# The doc build walks the def families for the same reason clippy does: a link
# whose target is compiled away by a feature resolves in one configuration and
# not in the next, and only the default one is ever built by habit. A doc
# comment that has to name an item across a feature seam names it in backticks
# instead of linking it -- `dsp::denormals` naming `server::backend`,
# `server::defstore` naming `faust::cache::FaustRecord` -- so every
# configuration documents clean.
#
# The GUI host adds `--document-private-items` because that is how its docs are
# read: it is the internal host crate, most of it private, and its module docs
# name the private function that does the work. Documenting them means a link
# into that machinery is checked rather than quietly rendered as text.
doc "rustdoc: workspace" cargo doc --no-deps --workspace
doc "rustdoc: neither def family" \
    cargo doc --no-deps --workspace --no-default-features
doc "rustdoc: synth alone" \
    cargo doc --no-deps --workspace --no-default-features --features synth
doc "rustdoc: faust alone" \
    cargo doc --no-deps --workspace --no-default-features --features faust

# --- Covered by CI, run unless --fast ----------------------------------------

if [ "$fast" = 0 ]; then
    clippy "clippy: workspace (core, ffi, midi, ...)" --workspace --all-targets
fi

# --- The GUI host's own matrix -----------------------------------------------
#
# That crate has a feature set of its own (its element families, `font-atlas`,
# `standalone`) and CI lints exactly one point of it. Its matrix is its own
# script, run from its own directory, and reported as one row here so a single
# command still answers "is the tree committable?". It runs *there* rather than
# through `--manifest-path` because cargo reads `.cargo/config.toml` from the
# current directory upward, not from the manifest's — and it subsumes the two
# rows this script used to spend on that crate (its clippy and its rustdoc are
# two of its own configurations, over more of them).
#
# `--fast` passes through: there it skips what CI covers and the standalone
# builds that link the server crate.
if [ "$fast" = 1 ]; then
    run "gui host: feature matrix (--fast)" clients/gui/check-features.sh --fast
else
    run "gui host: feature matrix" clients/gui/check-features.sh
fi

# --- Report -------------------------------------------------------------------

printf '\n\033[1m== matrix\033[0m\n'
for i in "${!labels[@]}"; do
    if [ "${statuses[$i]}" = "ok" ]; then
        printf '  \033[32mok  \033[0m %s\n' "${labels[$i]}"
    else
        printf '  \033[31mFAIL\033[0m %s\n' "${labels[$i]}"
    fi
done

if [ "$failed" = 0 ]; then
    printf '\n\033[32mAll %d configurations clean.\033[0m\n' "${#labels[@]}"
else
    printf '\n\033[31mMatrix not clean.\033[0m Zero warnings is the bar, including\n'
    printf 'ones this change did not introduce; clear pre-existing warnings in\n'
    printf 'their own commit (CLAUDE.md, "Commit workflow").\n'
fi

exit "$failed"
