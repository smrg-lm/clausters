#!/usr/bin/env sh
# Build the Clausters Python client documentation book.
#
# Step 1 generates the API reference page (src/api.md) from the package
# docstrings with pydoc-markdown -- a static AST parse, so no native cdylib is
# needed. Step 2 builds the mdBook. Both outputs (src/api.md and book/) are
# git-ignored.
#
# Requires (user space, no sudo):
#   mdbook          -- cargo install mdbook --version 0.4.40 (the version CI
#                      and both .readthedocs.yaml builds use)
#   pydoc-markdown  -- uv tool install --python 3.12 pydoc-markdown
#                      (global uv CLI in ~/.local/bin; pin 3.12 -- its deps lag
#                      the newest CPython. Or run via `uvx pydoc-markdown`, or
#                      `pip install pydoc-markdown` where pip is not externally
#                      managed.)
set -e
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$here/.."             # clients/python -- where pydoc-markdown.yml and clausters/ live
pydoc-markdown            # reads pydoc-markdown.yml -> writes docs/src/api.md
mdbook build docs         # -> docs/book/
echo "Built: $here/book/index.html"
