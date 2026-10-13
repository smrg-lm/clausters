# Native plugin test dependencies

The production host uses Cargo dependencies. Its project-owned CLAP fixtures
live under `crates/clausters-plugin-host/tests/fixtures`; all assertions run
against the Rust shared plane, host or server. External test sources are
prepared in ignored directories, with archive checksums verified:

```sh
bash third_party/prepare-clap.sh

```

CLAP 1.2.6 supplies headers in `third_party/clap` with its upstream license.
Prepare headers before running `cargo test -p clausters-plugin-host` and
`cargo test -p clausters-core --test plugin_shm`. `CLAP_ARCHIVE` can name a
verified local archive for offline preparation. Build products stay under
`target` or a test's temporary directory.
