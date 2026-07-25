#!/usr/bin/env bash
# The fmt + clippy matrix from CLAUDE.md's commit workflow, in one command.
#
# CI lints the default build set and the GUI host, but never the def-family
# matrix: a warning that only appears under --no-default-features, or under
# `synth` or `faust` alone, passes CI. Those three configurations are the whole
# reason this script exists; the rest are here so one run answers "is the tree
# committable?" without a second pass.
#
# Every configuration runs even if an earlier one fails — a matrix that stops at
# the first error hides how many of the others were also broken.
#
# Usage:
#   check.sh          # everything
#   check.sh --fast   # only the three configurations CI does not cover
#   check.sh --fix    # clippy --fix --allow-dirty instead of reporting
set -uo pipefail

root="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$root"

fast=0
fix_args=()
while [ $# -gt 0 ]; do
    case "$1" in
        --fast) fast=1 ;;
        --fix) fix_args=(--fix --allow-dirty) ;;
        -h | --help)
            sed -n '2,18p' "$0"
            exit 0
            ;;
        *)
            echo "check.sh: unknown option '$1' (--fast, --fix)" >&2
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
    run "$label" cargo clippy "${fix_args[@]}" "$@" -- -D warnings
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

# --- Covered by CI, run unless --fast ----------------------------------------

if [ "$fast" = 0 ]; then
    clippy "clippy: workspace (core, ffi, midi, ...)" --workspace --all-targets
    # A separate workspace, so it needs its own invocation, not --manifest-path
    # (which would resolve against the root workspace's lockfile).
    run "clippy: gui host" \
        env -C clients/gui cargo clippy "${fix_args[@]}" --all-targets -- -D warnings
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
