#!/usr/bin/env bash
# The GUI host's feature matrix: fmt + clippy + rustdoc + the wasm gate + the
# suite, over the configurations the features exist to make possible.
#
# It is the sibling of `.claude/skills/feature-matrix/check.sh`, which does the
# same job for the server's def families, and it exists for the same reason: CI
# lints this crate exactly once, with its **default** features, so a break that
# only appears with a feature off passes CI and every local habit alike. That is
# not hypothetical — `--no-default-features` did not compile at all until the
# commit before this script landed, and nothing had noticed.
#
# What the configurations are chosen to catch:
#
#   * **the floor** (no features) — the build every optional family is optional
#     against, and the one nothing ever runs by habit;
#   * **one family at a time** — a module that quietly reaches into another
#     family compiles fine when both are on and only fails alone;
#   * **the wasm gate** under the same, since the browser bundle is where
#     dropping a family is worth real kilobytes;
#   * **rustdoc**, because a doc link whose target is compiled out resolves in
#     one configuration and not the next (name it in backticks across a feature
#     seam, never as a link);
#   * **the suite**, and that one is the difference from the server's matrix:
#     clippy type-checks test code but never runs it, and a family gated out of
#     the *code* while its tests stay in is a test that compiles and fails.
#
# It only ever reads: nothing here writes to your working tree, for the reason
# the server's matrix states — a gate that edits the thing it is judging cannot
# be trusted to report on it.
#
# Every configuration runs even if an earlier one fails: a matrix that stops at
# the first error hides how many of the others were also broken.
#
# `standalone` links the server crate (and `standalone-faust` needs libfaust at
# build time, like the root matrix's faust rows). `--fast` leaves both out along
# with everything CI already covers, which is the form to run while iterating.
#
# Usage:
#   check-features.sh          # everything
#   check-features.sh --fast   # skip what CI covers, and the heavy standalone builds
set -uo pipefail

cd "$(dirname "$0")"

fast=0
while [ $# -gt 0 ]; do
    case "$1" in
        --fast) fast=1 ;;
        -h | --help)
            sed -n '2,38p' "$0"
            exit 0
            ;;
        *)
            echo "check-features.sh: unknown option '$1' (--fast)" >&2
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

# --- Covered by CI, run unless --fast ----------------------------------------

if [ "$fast" = 0 ]; then
    run "fmt" cargo fmt --check
    clippy "clippy: default features" --all-targets
fi

# --- The gap: one configuration per axis the features open -------------------
#
# The floor first: every optional family out, which is what a program linking
# this crate for its controls alone compiles.

clippy "clippy: no features (the floor)" --all-targets --no-default-features
clippy "clippy: midi alone" --all-targets --no-default-features --features midi
clippy "clippy: notation alone" --all-targets --no-default-features --features notation
clippy "clippy: patcher alone" --all-targets --no-default-features --features patcher
# `font-atlas` doubles the text path (a second pipeline, a real face's advances),
# so it is checked both on the floor and over the default set.
clippy "clippy: font-atlas on the floor" \
    --all-targets --no-default-features --features font-atlas
clippy "clippy: default + font-atlas" --all-targets --features font-atlas

if [ "$fast" = 0 ]; then
    # Links the server crate: the heaviest row here, and the only one whose
    # failure is about a *dependency's* features rather than this crate's.
    clippy "clippy: standalone" --all-targets --features standalone
    clippy "clippy: standalone-faust (needs libfaust)" \
        --all-targets --features standalone-faust
fi

# --- The wasm gate, under the same axes --------------------------------------
#
# `check-wasm.sh` proves the agnostic core never re-couples to native I/O. Here
# it runs three times: the default set, the floor (a family compiled out must
# not take the browser build with it) and the profile `clients/web/build.sh`
# actually ships.

run "wasm: default features" ./check-wasm.sh
run "wasm: the floor" ./check-wasm.sh --no-default-features
run "wasm: the browser bundle's profile" ./check-wasm.sh --features font-atlas

# --- rustdoc -----------------------------------------------------------------
#
# `--document-private-items` because that is how this crate's docs are read: it
# is the internal host, most of it private, and its module docs name the private
# function that does the work. Run on the floor too, since a link into a
# compiled-out family is exactly the drift this catches.

run "rustdoc: default features" \
    env RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items
run "rustdoc: the floor" \
    env RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items \
    --no-default-features

# --- The suite ---------------------------------------------------------------
#
# The row clippy cannot stand in for: a test that exercises a family gated out
# of the code compiles and then fails. Two runs answer it — everything on, and
# everything off — since a test is gated by the family it exercises and there is
# no third answer in between.

run "test: default features" cargo test --quiet
run "test: the floor" cargo test --quiet --no-default-features

# --- Report ------------------------------------------------------------------

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
