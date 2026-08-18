#!/usr/bin/env bash
# The three books, built the way CI and Read the Docs build them, in one
# command.
#
# Why this exists: the doc build is the only check in this repository with no
# local counterpart. cargo refuses to compile a caller of a signature that
# moved, clippy and the def-family matrix have `feature-matrix/check.sh`, the
# Python call sites have pyright -- but a dangling `{@link}` in a TSDoc comment,
# a page missing from a `SUMMARY.md` or a docstring pydoc-markdown chokes on is
# seen by nothing until the push, and the answer arrives minutes later in a job
# nobody is watching. Every red `docs` job so far has been of exactly that kind.
#
# The three legs mirror .github/workflows/ci.yml's `docs` job step for step, and
# the tool versions are pinned to the same ones there and in the three
# .readthedocs.yaml (mdBook 0.4.40, TypeDoc 0.28 with TypeScript 5.9,
# pydoc-markdown on Python 3.12) -- that equivalence is what makes a green run
# here mean anything. Install them per docs/contributing.md, "Editing this book"
# and clients/web/BUILD.md.
#
# Unlike the feature matrix this one *writes*: a doc build generates pages. All
# of it lands in git-ignored output (`book/`, `clients/*/docs/book/`,
# `clients/python/docs/src/api.md`, `clients/web/docs/src/api/`), so the working
# tree is untouched, but do not expect a read-only gate.
#
# Every leg runs even if an earlier one fails -- a run that stops at the first
# error hides how many of the others were also broken.
#
# Usage:
#   check-docs.sh                 # all three books
#   check-docs.sh server|python|web [...]   # only those
set -uo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

want_server=0
want_python=0
want_web=0
if [ $# -eq 0 ]; then
    want_server=1
    want_python=1
    want_web=1
else
    for arg in "$@"; do
        case "$arg" in
            server) want_server=1 ;;
            python) want_python=1 ;;
            web) want_web=1 ;;
            *)
                echo "usage: check-docs.sh [server|python|web ...]" >&2
                exit 2
                ;;
        esac
    done
fi

# The tools are checked before anything runs, and only the ones the selected
# books need: a missing generator must read as "the check did not run", never as
# a book that failed to build.
missing=""
command -v mdbook >/dev/null 2>&1 || missing="$missing mdbook"
if [ "$want_python" = 1 ]; then
    command -v pydoc-markdown >/dev/null 2>&1 || missing="$missing pydoc-markdown"
fi
if [ "$want_web" = 1 ]; then
    command -v typedoc >/dev/null 2>&1 || missing="$missing typedoc"
fi
if [ -n "$missing" ]; then
    echo "check-docs: not available:$missing -- nothing was checked." >&2
    echo "See docs/contributing.md (\"Editing this book\") for the pinned installs." >&2
    exit 2
fi

failed=""
run() {
    local label="$1" dir="$2"
    shift 2
    echo
    echo "=== $label: $* (in ${dir#"$root"/})"
    if ! (cd "$dir" && "$@"); then
        failed="$failed $label"
    fi
}

if [ "$want_server" = 1 ]; then
    run "server" "$root" mdbook build .
fi
if [ "$want_python" = 1 ]; then
    # pydoc-markdown regenerates docs/src/api.md from the package docstrings;
    # the book then fails on anything the page turned into.
    run "python(api)" "$root/clients/python" pydoc-markdown
    run "python(book)" "$root/clients/python" mdbook build docs
fi
if [ "$want_web" = 1 ]; then
    # TypeDoc has treatWarningsAsErrors on (typedoc.json), so a link to a symbol
    # that was renamed fails here exactly as a broken intra-doc link fails
    # rustdoc. It parses statically -- no wasm build, no npm install needed.
    run "web(api)" "$root/clients/web" typedoc
    run "web(book)" "$root/clients/web" mdbook build docs
fi

echo
if [ -n "$failed" ]; then
    echo "check-docs: FAILED --$failed" >&2
    exit 1
fi
echo "check-docs: all books built."
