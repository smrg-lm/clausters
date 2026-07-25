#!/usr/bin/env bash
# PostToolUse(Edit|Write): format the Rust file that was just written.
#
# CLAUDE.md makes rustfmt the source of truth ("do not hand-format Rust
# against rustfmt") and requires a `cargo fmt --check`-clean tree at every
# commit. Formatting one file is sub-second, so doing it here removes the
# whole class of "the commit needed a formatting pass" churn.
#
# `rustfmt` directly rather than `cargo fmt`: the tree has three workspaces
# (root, clients/gui, fuzz) and cargo would have to be pointed at the right
# manifest per file. Every crate is edition 2024 and there is no rustfmt.toml,
# so a bare `rustfmt --edition 2024` is exactly what `cargo fmt` would do.
#
# Always exits 0 — a file that does not parse yet (mid-refactor) must not
# block the edit; the next fmt pass picks it up.
set -uo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"

# This hook reads its input with jq and shells out to rustfmt, neither of which
# it ships. Miss either and it would not error — it would parse nothing, match
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
for tool in jq rustfmt; do
    command -v "$tool" >/dev/null 2>&1 || missing="$missing $tool"
done
if [ -n "$missing" ]; then
    stamp="${TMPDIR:-/tmp}/.clausters-hook-warn-$(id -u)-fmt${missing// /-}"
    if [ ! -f "$stamp" ] || [ -n "$(find "$stamp" -mmin +720 2>/dev/null)" ]; then
        # stderr silenced *before* the write, so a redirection that fails
        # anyway (a read-only TMPDIR) does not print a shell error of its own.
        : 2>/dev/null >"$stamp"
        {
            echo "clausters: the fmt-rust hook is inert — missing on PATH:$missing."
            echo "Rust files are NOT being formatted on write, so the tree will drift"
            echo "out of \`cargo fmt --check\`. rustfmt ships with rustup (ensure"
            echo "~/.cargo/bin is on the PATH of a non-interactive shell); jq is a"
            echo "package. See docs/contributing.md, \"Claude Code hooks and settings\"."
        } >&2
        exit 2
    fi
    exit 0
fi

file=$(jq -r '.tool_input.file_path // empty')

case "$file" in
    *.rs) ;;
    *) exit 0 ;;
esac

# ...and inside this checkout. A session can edit Rust anywhere on the machine,
# and the `--edition 2024` below is a fact about *this* tree only. On the
# command line it overrides the edition another project declares, so an older
# crate would be parsed by 2024's rules — `gen`, an ordinary identifier there,
# is a reserved keyword here. Its style config is not the issue: rustfmt finds
# that project's rustfmt.toml on its own, walking up from the file. The edition
# is the part this hook would impose, and reformatting someone else's tree
# against its own policy is not its business. The path arrives absolute, so a
# prefix test is enough.
case "$file" in
    "$root"/*) ;;
    *) exit 0 ;;
esac

[ -f "$file" ] || exit 0

rustfmt --edition 2024 "$file" 2>/dev/null || true
exit 0
