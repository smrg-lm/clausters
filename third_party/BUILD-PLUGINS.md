# Native plugin test dependencies

The production host uses Cargo dependencies. Its project-owned CLAP fixtures
live under `crates/clausters-plugin-host/tests/fixtures`; all assertions run
against the Rust shared plane, host or server. External test sources are
prepared in ignored directories, with archive checksums verified:

```sh
bash third_party/prepare-clap.sh
bash third_party/prepare-vst3-fixture.sh
cargo fetch --locked --manifest-path crates/clausters-plugin-host/tests/fixtures/vst3/Cargo.toml

```

CLAP 1.2.6 supplies headers in `third_party/clap` with its upstream license.
The VST3 fixture starts from the `vst3` 0.3.0 gain example in ignored
`third_party/vst3-fixture`, retaining its MIT license. The versioned
`vst3-fixture.patch` adds our acceptance checks. Its manifest and lockfile
remain under `crates/clausters-plugin-host/tests/fixtures/vst3`.
`CLAP_ARCHIVE` and `VST3_ARCHIVE` can name verified local archives for offline
preparation. The fixture's dependencies must be fetched before an offline test.

Run the same combined functional suite as CI and the release gate:

```sh
cargo test -p clausters-core --test plugin_shm
cargo test -p clausters-plugin-host --no-default-features --features clap,vst3
cargo build -p clausters-plugin-host --no-default-features --features clap,vst3
cargo test --no-default-features --features plugin-clap,plugin-vst3 --test plugin_events --test plugin_family --test plugin_offline --test plugin_scan --test plugin_state --test plugin_vst3
```

CI also runs each adapter alone. Use `clap` / `plugin-clap`, or `vst3` /
`plugin-vst3`, for the corresponding host/server feature pair. Shared-plane
checks cover deadlines, accepted releases, slot ownership and capacity. Host
checks cover native DSP, transport, state, thread separation, allocation and
syscall guards, failure and cleanup. Server checks cover OSC, event ownership,
recovery and live/offline equality. There are no machine-dependent timing
thresholds. Build products stay under `target` or a test's temporary directory.
