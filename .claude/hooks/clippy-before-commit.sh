#!/usr/bin/env bash
# PreToolUse(Bash): no commit touching Rust without fmt + clippy clean.
#
# CLAUDE.md's commit workflow: "Before generating any commit that touches Rust,
# run `cargo fmt` ... the tree must be `cargo fmt --check`-clean" and "Clippy
# must come back clean, always — zero warnings, not 'no new ones'". This is the
# boundary where that is actually enforceable.
#
# Deliberately *not* a PostToolUse hook beside fmt-rust.sh: clippy is a compile,
# not a reformat. Per edit it would cost tens of seconds each time and would
# fail constantly on half-finished refactors, which is how a hook gets turned
# off. Once per commit is rare, and it is the moment the rule exists for.
#
# Only the workspaces the commit actually touches are linted (root, clients/gui,
# fuzz — three separate workspaces), so a Python- or docs-only commit pays
# nothing. The def-family matrix is NOT run here (five more builds); when the
# diff touches feature-gated code the message points at the feature-matrix
# skill instead.
#
# Escape hatch: CLAUSTERS_SKIP_CLIPPY=1 git commit ...
set -uo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"

. "$root/.claude/hooks/_preflight.sh" || exit 0

# cargo matters more here than jq does. Without it the hook would still detect
# the commit, run `cargo` into a "command not found", read that as "clippy
# failed" and block the commit with a message about warnings that do not exist
# — failing closed on a wrong diagnosis. Better to stand down and say why.
missing=$(hook_missing_tools jq git cargo)
if [ -n "$missing" ]; then
    hook_warn_once "clippy${missing// /-}" \
        "clausters: the clippy-before-commit hook is inert — missing on PATH:$missing." \
        "Commits touching Rust are NOT being checked for fmt/clippy warnings." \
        "cargo is usually missing because ~/.cargo/bin is on an interactive" \
        "PATH but not on the non-interactive one hooks run under. See" \
        "docs/contributing.md, \"Claude Code hooks and settings\"."
    exit 0
fi

cmd=$(jq -r '.tool_input.command // empty')
[ -n "$cmd" ] || exit 0

case "$cmd" in
    *CLAUSTERS_SKIP_CLIPPY*) exit 0 ;;
esac

# A `git ... commit` invocation (`git commit`, `git -C dir commit`, `... && git
# commit`), but not `git log --grep=commit`: `commit` has to be a word of its
# own, which the trailing group enforces by only ever starting after whitespace.
grep -Eq '(^|[;|&(])[[:space:]]*[[:alnum:]_./-]*git[[:space:]]+([^[:space:]]+[[:space:]]+)*commit([[:space:]]|$)' \
    <<<"$cmd" || exit 0

# --- Which workspaces does the working tree touch? ---------------------------

changed=$(git -C "$root" status --porcelain --untracked-files=all 2>/dev/null |
    sed 's/^...//; s/.* -> //')

want_root=0
want_gui=0
want_fuzz=0
while IFS= read -r path; do
    case "$path" in
        *.rs | Cargo.toml | Cargo.lock | */Cargo.toml | */Cargo.lock) ;;
        *) continue ;;
    esac
    case "$path" in
        clients/gui/*) want_gui=1 ;;
        fuzz/*) want_fuzz=1 ;;
        clients/*) ;; # other clients hold no Rust the root workspace builds
        *) want_root=1 ;;
    esac
done <<<"$changed"

[ "$want_root$want_gui$want_fuzz" = "000" ] && exit 0

# --- Lint them ---------------------------------------------------------------

out=$(mktemp)
trap 'rm -f "$out"' EXIT
failed=""

check() {
    local label="$1" dir="$2"
    shift 2
    if ! (cd "$dir" && "$@") >>"$out" 2>&1; then
        failed="$failed $label"
    fi
}

if [ "$want_root" = 1 ]; then
    check "fmt" "$root" cargo fmt --check
    check "clippy" "$root" cargo clippy --workspace --all-targets -- -D warnings
fi
if [ "$want_gui" = 1 ]; then
    check "fmt(gui)" "$root/clients/gui" cargo fmt --check
    check "clippy(gui)" "$root/clients/gui" cargo clippy --all-targets -- -D warnings
fi
if [ "$want_fuzz" = 1 ]; then
    check "fmt(fuzz)" "$root/fuzz" cargo fmt --check
    check "clippy(fuzz)" "$root/fuzz" cargo clippy --all-targets -- -D warnings
fi

[ -n "$failed" ] || exit 0

{
    echo "Commit blocked —$failed failed. CLAUDE.md: the tree must be"
    echo "\`cargo fmt --check\`-clean and clippy must come back clean, always"
    echo "(zero warnings, not \"no new ones\" — fix the ones this change did not"
    echo "introduce too, in their own commit so the feature's diff stays readable)."
    echo
    tail -60 "$out"
    echo
    if git -C "$root" diff -U0 HEAD 2>/dev/null | grep -q '^+.*cfg(feature'; then
        echo "This diff touches feature-gated code, and CI never builds the"
        echo "def-family matrix — run the feature-matrix skill before committing."
    fi
    echo "A warning that is genuinely wrong gets a scoped #[allow(...)] with a"
    echo "comment saying why. To bypass: CLAUSTERS_SKIP_CLIPPY=1 git commit ..."
} >&2
exit 2
