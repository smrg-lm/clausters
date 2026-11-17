# The C library headers a grammar compiles against in a page

`include/` is the header set the `tree-sitter-language` crate (0.1.9) carries
for `wasm32-unknown-unknown`, a target with no C library: declarations only,
whose definitions the `tree-sitter` crate compiles into the module itself
(its `src/wasm-stdlib`), with allocation through Rust's allocator. MIT, as
the crate is; `LICENSE` is its license.

The `tree-sitter` crate finds these headers by itself. The grammar crates
(`tree-sitter-python`, `tree-sitter-javascript`) do not, and nothing a
dependent's build script does reaches another crate's compiler, so the
repository's `.cargo/config.toml` hands them over in
`CFLAGS_wasm32_unknown_unknown`. Native builds do not read that variable.

When `tree-sitter-language` is raised in `Cargo.lock`, copy its `wasm/include`
here again.
