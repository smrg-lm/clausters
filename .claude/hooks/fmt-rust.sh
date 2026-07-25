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

. "$(dirname "$0")/_preflight.sh" || exit 0

# Exit 2 rather than 0 for the warning: on PostToolUse that hands stderr back
# as feedback (the edit is already applied, nothing is lost), which is the one
# channel here that is certain to be read. rustfmt is usually missing for the
# reason cargo is: ~/.cargo/bin is on an interactive PATH but not on the
# non-interactive one hooks run under.
missing=$(hook_missing_tools jq rustfmt)
if [ -n "$missing" ]; then
    hook_warn_once "fmt${missing// /-}" \
        "clausters: the fmt-rust hook is inert — missing on PATH:$missing." \
        "Rust files are NOT being formatted on write, so the tree will drift" \
        "out of \`cargo fmt --check\`. rustfmt ships with rustup (ensure" \
        "~/.cargo/bin is on the PATH of a non-interactive shell); jq is a" \
        "package. See docs/contributing.md, \"Claude Code hooks and settings\"." &&
        exit 2
    exit 0
fi

file=$(jq -r '.tool_input.file_path // empty')

case "$file" in
    *.rs) ;;
    *) exit 0 ;;
esac

[ -f "$file" ] || exit 0

rustfmt --edition 2024 "$file" 2>/dev/null || true
exit 0
