#!/usr/bin/env sh
# Build the Clausters web client documentation book.
#
# Step 1 generates the API reference pages (src/api/) from the package's TSDoc
# comments with TypeDoc -- a static parse of src/index.ts, so no wasm bundle
# and no build of the package are needed. Step 2 builds the mdBook. Both
# outputs (src/api/ and book/) are git-ignored.
#
# Requires (user space, no sudo):
#   mdbook   -- cargo install mdbook --version 0.4.40 (the version CI and all
#               three .readthedocs.yaml builds use)
#   typedoc  -- npm install -g typedoc@0.28 typedoc-plugin-markdown@4 \
#                              typescript@5.9
#               with npm's prefix under ~/.local (the node recipe in
#               ../BUILD.md), then symlink ~/.local/lib/node/bin/typedoc into
#               ~/.local/bin like node and npm. TypeDoc parses with its own
#               TypeScript 5.9; the package itself compiles with the v7 in
#               node_modules, and the two never meet.
set -e
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$here/.."             # clients/web -- where typedoc.json and src/ live
typedoc                   # reads typedoc.json -> writes docs/src/api/
mdbook build docs         # -> docs/book/
echo "Built: $here/book/index.html"
