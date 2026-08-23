#!/usr/bin/env bash
# The three version numbers, as they were at the last tag and as they are now.
#
# Read it by rule 5 in SKILL.md: an ABI counter that *differs* is what triggers
# the breaking-tier linkage -- by how much is not a question and a gap is not a
# defect -- while the version must have moved its breaking tier exactly once.
#
# It searches for each constant rather than naming a path, because ABI_VERSION
# has already moved file once (src/server/ipc.rs -> crates/clausters-core/src/
# shm.rs) and a hardcoded path turns "moved house" into a silent "did not move".
# A constant it cannot find is an error, never an empty column.
set -uo pipefail
cd "$(git rev-parse --show-toplevel)"

tag=$(git tag --sort=-v:refname | head -1)
[ -n "$tag" ] || { echo "no tag to compare against"; exit 1; }
status=0

# Where a constant lives, in a given tree-ish ("" for the working tree).
where() {
    local at=$1 name=$2
    if [ -z "$at" ]; then
        grep -rl --include=*.rs "pub const $name: u32" . 2>/dev/null | grep -v '/target/' | head -1
    else
        git grep -l "pub const $name: u32" "$at" -- '*.rs' 2>/dev/null | head -1 | cut -d: -f2-
    fi
}

value() {  # tree-ish (or ""), path, pattern
    if [ -z "$1" ]; then cat "$2"; else git show "$1:$2"; fi \
        | grep -m1 -- "$3" | sed 's/.*= *//; s/[;"]//g'
}

row() {  # label, constant-or-pattern, kind
    local label=$1 name=$2 kind=$3 was now wp np
    if [ "$kind" = const ]; then
        wp=$(where "$tag" "$name"); np=$(where "" "$name")
        [ -n "$wp" ] && was=$(value "$tag" "$wp" "pub const $name")
        [ -n "$np" ] && now=$(value "" "${np#./}" "pub const $name")
    else
        was=$(value "$tag" Cargo.toml '^version'); now=$(value "" Cargo.toml '^version')
    fi
    if [ -z "${was:-}" ] || [ -z "${now:-}" ]; then
        printf '%-17s %-9s -> %-9s  ERROR: not found (%s)\n' \
            "$label" "${was:-?}" "${now:-?}" "moved, renamed or deleted"
        status=1
    else
        [ "$was" = "$now" ] && mark="unchanged" || mark="MOVED"
        printf '%-17s %-9s -> %-9s  %s\n' "$label" "$was" "$now" "$mark"
    fi
}

echo "since $tag:"
row ABI_VERSION      ABI_VERSION      const
row CORE_ABI_VERSION CORE_ABI_VERSION const
row version          -                semver
exit $status
