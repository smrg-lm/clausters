#!/usr/bin/env sh
# Build the Clausters Python client documentation book.
#
# Step 1 generates the API reference pages (src/api/, one per module) from the
# package docstrings with pydoc-markdown (gen_api.py) -- a static AST parse, so
# no native cdylib is needed. Step 2 builds the mdBook. Both outputs (src/api/
# and book/) are git-ignored.
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
python3 docs/gen_api.py   # pydoc-markdown, one page per module -> docs/src/api/
mdbook build docs         # -> docs/book/
echo "Built: $here/book/index.html"
