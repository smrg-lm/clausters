#!/usr/bin/env bash
# PreToolUse(Bash): refuse a Python launch that would run stale native binaries.
#
# The footgun this guards is the one CLAUDE.md documents under "Testing via the
# Python launcher": in this source checkout the package is installed editable,
# so the artifacts *bundled* in clients/python/clausters/{_bin,_libs} win over
# the workspace target/ (env override -> bundled -> target/). They go stale the
# moment a crate is edited, and nothing fails loudly — the test simply exercises
# pre-change binaries and reports a result about code that is no longer there.
#
# The check is on *sources*, not on target/: "is any Rust source newer than the
# last staging pass?". Comparing against target/ instead would fire after every
# `cargo test` (which rebuilds the debug cdylib while the staged copy is
# release), and that noise would get the hook disabled within a day.
#
# The reference is the staged artifacts' **ctime**, not their mtime, and the
# **newest** of them rather than the oldest. Both details matter:
#
#   - build_native.py stages with shutil.copy2, which *preserves mtime*. An
#     artifact cargo did not have to rebuild is copied carrying a build time
#     from days ago, so an mtime comparison reports stale forever. Its ctime,
#     though, is set by the copy itself — it is the moment of the refresh.
#   - `stage()` skips an artifact whose target/ source is absent (a cleaned
#     tree, a crate that build did not produce), so the oldest ctime can be a
#     leftover from an earlier pass. The newest is the one that dates the last
#     pass, and every pass copies everything it can find.
#
# Timestamps, not content: any git operation that rewrites a file — a checkout,
# a branch switch, a stash pop — bumps its mtime and trips this even though the
# bytes may be identical. That direction is the safe one (a branch switch really
# can change what gets built), and the cost is one `scripts/refresh-bin.sh`.
#
# Exits 2 (blocking, stderr goes back to the agent) so the fix — one
# `scripts/refresh-bin.sh` — happens before the run rather than after a
# confusing result. To make it advisory instead, change the final `exit 2` to
# `exit 0` and the message still shows up in the transcript.
set -uo pipefail
set -f  # the command line is split into words below; never glob them

root="$(cd "$(dirname "$0")/../.." && pwd)"

. "$root/.claude/hooks/_preflight.sh" || exit 0

# Fail open, but say so. A PreToolUse hook that exits 2 blocks the command, and
# a missing jq is not a reason to stop someone's shell — it is a reason to tell
# them the guard is off.
missing=$(hook_missing_tools jq stat find)
hook_has_gnu_file_tools || missing="$missing GNU-stat/find"
if [ -n "${missing# }" ]; then
    hook_warn_once "stale${missing// /-}" \
        "clausters: the check-stale-bin hook is inert — missing:$missing." \
        "Python launches are NOT being checked against the bundled native" \
        "binaries, so a test can silently exercise pre-change code (CLAUDE.md," \
        "\"Testing via the Python launcher\"). The check needs jq and the GNU" \
        "coreutils/findutils; run \`scripts/refresh-bin.sh\` by hand meanwhile."
    exit 0
fi

cmd=$(jq -r '.tool_input.command // empty')
[ -n "$cmd" ] || exit 0

# --- Does this command even reach the bundled artifacts? ---------------------

# The escape hatch, the refresh itself, and the documented override env vars
# (which bypass the bundled copy entirely) are all none of our business.
case "$cmd" in
    *CLAUSTERS_SKIP_STALE_CHECK* | *refresh-bin.sh* | *build_native.py* | \
    *CLAUSTERS_GUI_BIN=* | *CLAUSTERS_BIN=* | *CLAUSTERS_LIB=* | *CLAUSTERS_FFI_LIB=*)
        exit 0
        ;;
esac

# Is an interpreter actually being *launched*? Matching "python" anywhere in the
# string is too coarse — `cd clients/python && cat foo.py` and `grep -rn python
# foo.py` both contain it — so look only at command position: split the line into
# pipeline segments and, in each, skip the leading env assignments, flags and
# wrappers (`timeout 20 python3 ...`) to find the word actually being run.
#
# The splitting is not quote-aware, so a literal `&& python ...` *inside* a
# quoted string reads as a real segment and can trip the check. Quote-aware
# shell parsing in a hook is not worth the code: the error is in the safe
# direction, it names the offending file, and CLAUSTERS_SKIP_STALE_CHECK=1
# clears it in one prefix.
launches_python() {
    local segment word
    while IFS= read -r segment; do
        for word in $segment; do
            case "$word" in
                *=* | -* | [0-9]*) continue ;;  # assignment, flag, timeout's duration
                timeout | env | exec | nohup | time | stdbuf | xvfb-run | command | sudo)
                    continue
                    ;;
            esac
            case "$word" in
                python | python3 | pytest | uv | */python | */python3 | */pytest | */uv)
                    return 0
                    ;;
            esac
            break  # the segment's real command is not an interpreter
        done
    done < <(tr ';|&\n' '\n\n\n\n' <<<"$1")
    return 1
}

launches_python "$cmd" || exit 0

# ...and is it about this package? `python3 -c 'print(1)'` is not.
grep -Eq '\.py([[:space:]]|$)|pytest|clausters' <<<"$cmd" || exit 0

# --- Are the staged artifacts older than the Rust sources? -------------------

# The four artifacts the launcher actually resolves. Newest ctime among them =
# when build_native.py last staged anything (see the header for why ctime).
staged=$(stat -c '%Z %n' \
    "$root/clients/python/clausters/_bin/clausters" \
    "$root/clients/python/clausters/_bin/clausters-gui" \
    "$root/clients/python/clausters/_libs/libclausters.so" \
    "$root/clients/python/clausters/_libs/libclausters_ffi.so" \
    2>/dev/null | sort -rn | head -1 | cut -d' ' -f2-)

# Nothing bundled at all -> the resolution order falls through to target/,
# which is by definition current. Not our problem.
[ -n "$staged" ] || exit 0

# -newermc: the found file's *mtime* against the reference's *ctime*.
newer=$(find \
    "$root/src" \
    "$root/crates" \
    "$root/clients/gui/src" \
    "$root/Cargo.toml" "$root/Cargo.lock" "$root/build.rs" \
    "$root/clients/gui/Cargo.toml" \
    \( -name '*.rs' -o -name 'Cargo.toml' -o -name 'Cargo.lock' \) \
    -newermc "$staged" -print -quit 2>/dev/null)

[ -n "$newer" ] || exit 0

cat >&2 <<EOF
Stale bundled binaries: ${newer#"$root"/} was edited after the last refresh
($(stat -c '%z' "$staged" | cut -d. -f1)).

The Python package is installed editable, so the bundled copy in
clients/python/clausters/{_bin,_libs} wins over the workspace target/ — this
run would silently exercise pre-change native code (CLAUDE.md, "Testing via
the Python launcher: refresh the bundled binaries first").

Refresh first:
  scripts/refresh-bin.sh              # rebuild + stage server, FFI and GUI host
  scripts/refresh-bin.sh <example>    # ...and run that example

Or point the override env vars at the workspace build (CLAUSTERS_GUI_BIN,
CLAUSTERS_BIN, CLAUSTERS_LIB, CLAUSTERS_FFI_LIB), or prefix the command with
CLAUSTERS_SKIP_STALE_CHECK=1 if the staleness is deliberate.
EOF
exit 2
