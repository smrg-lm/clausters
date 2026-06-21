#!/usr/bin/env sh
# Build the Clausters Python client documentation book.
#
# Step 1 generates the API reference page (src/api.md) from the package
# docstrings with pydoc-markdown -- a static AST parse, so no native cdylib is
# needed. Step 2 builds the mdBook. Both outputs (src/api.md and book/) are
# git-ignored.
#
# Requires: mdbook, pydoc-markdown (`pip install pydoc-markdown`).
set -e
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$here/.."             # clients/python -- where pydoc-markdown.yml and clausters/ live
pydoc-markdown            # reads pydoc-markdown.yml -> writes docs/src/api.md
mdbook build docs         # -> docs/book/
echo "Built: $here/book/index.html"
