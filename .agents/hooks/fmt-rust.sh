#!/usr/bin/env bash
# PostToolUse(Edit|Write|apply_patch): format the Rust files just written.
#
# One script for both agents. Claude Code's Edit/Write hands over
# `tool_input.file_path`; Codex's apply_patch hands over the patch text, whose
# `*** Add File:` / `*** Update File:` / `*** Move to:` lines name the files,
# relative to the session's `cwd`.
#
# AGENTS.md makes rustfmt the source of truth ("do not hand-format Rust
# against rustfmt") and requires a `cargo fmt --check`-clean tree at every
# commit. Formatting one file is sub-second, so doing it here removes the
# whole class of "the commit needed a formatting pass" churn.
#
# `rustfmt` directly rather than `cargo fmt`: the tree has four workspaces
# (root, clients/gui, fuzz, clausters-editor/src-tauri). The editor uses edition
# 2021; the other workspaces use 2024. Select the edition by the resolved path.
#
# Formatting failures return feedback after the edit, which remains applied.
# A file that does not parse yet can be picked up by the next fmt pass.
set -uo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd -P)"

# This hook reads its input with jq and shells out to rustfmt, neither of which
# it ships. Miss either and it would not error -- it would parse nothing, match
# nothing and exit 0: the protection off, looking exactly like the protection
# on. So say so, then get out of the way. A machine that cannot run the check is
# not a reason to stop the work.
#
# Exit 2 rather than 0 for the warning: on PostToolUse that hands stderr back
# as feedback (the edit is already applied, nothing is lost), which is the one
# channel here that is certain to be read. rustfmt is usually missing because
# ~/.cargo/bin is on an interactive PATH but not on the non-interactive one
# hooks run under.
#
# Once every twelve hours, not every edit: a broken setup has to surface the
# same day someone clones, but a message on every single edit is noise, and
# noise is how a hook ends up disabled.
missing=""
for tool in jq rustfmt realpath; do
    command -v "$tool" >/dev/null 2>&1 || missing="$missing $tool"
done
if [ -n "$missing" ]; then
    stamp="${TMPDIR:-/tmp}/.clausters-hook-warn-$(id -u)-fmt${missing// /-}"
    if [ ! -f "$stamp" ] || [ -n "$(find "$stamp" -mmin +720 2>/dev/null)" ]; then
        # stderr silenced *before* the write, so a redirection that fails
        # anyway (a read-only TMPDIR) does not print a shell error of its own.
        : 2>/dev/null >"$stamp"
        {
            echo "clausters: the fmt-rust hook is inert -- missing on PATH:$missing."
            echo "Rust files are NOT being formatted on write, so the tree will drift"
            echo "out of \`cargo fmt --check\`. rustfmt ships with rustup (ensure"
            echo "~/.cargo/bin is on the PATH of a non-interactive shell); install"
            echo "jq and realpath too. See docs/contributing.md, \"Coding-agent instructions, skills, hooks and settings\"."
        } >&2
        exit 2
    fi
    exit 0
fi

input=$(cat)
cwd=$(jq -r '.cwd // empty' <<<"$input")
files=$(jq -r '
    (.tool_input.file_path // empty),
    ((.tool_input.command // .tool_input.input // .tool_input.patch // empty)
     | if type == "array" then join("\n") else . end
     | split("\n")[]
     | capture("^\\*\\*\\* (Add File|Update File|Move to): (?<p>.*)$")?.p)
' <<<"$input" 2>/dev/null)

# Only existing `.rs` files whose resolved paths are inside this checkout.
# Resolve symlinks and `..` before checking the boundary and the workspace.
failed=0
while IFS= read -r file; do
    case "$file" in
        *.rs) ;;
        *) continue ;;
    esac
    case "$file" in
        /*) ;;
        *) file="${cwd:-$PWD}/$file" ;;
    esac
    [ -f "$file" ] || continue
    if ! file=$(realpath -e -- "$file"); then
        echo "clausters: fmt-rust could not resolve a Rust file path." >&2
        failed=2
        continue
    fi
    case "$file" in
        "$root"/*) ;;
        *) continue ;;
    esac
    case "$file" in
        "$root"/clausters-editor/*) edition=2021 ;;
        *) edition=2024 ;;
    esac
    if ! rustfmt --edition "$edition" "$file"; then
        echo "clausters: fmt-rust failed for $file; the edit remains applied. Fix the reported error and run cargo fmt in its workspace." >&2
        failed=2
    fi
done <<<"$files"
exit "$failed"
