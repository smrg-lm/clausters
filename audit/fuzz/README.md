# OSC decoder fuzzing

This is the existing network-edge fuzzer, relocated from root fuzz/ into the audit resources. It remains an independent Cargo workspace: the application workspace, dependency declarations and normal builds do not include libFuzzer. The target and versioned seed bytes are unchanged.

## Stable checks

From the repository root, select an external target directory and a CPU/job limit:

```sh
export CARGO_TARGET_DIR=/tmp/clausters-fuzz-target
export CARGO_BUILD_JOBS=10
export CARGO_INCREMENTAL=0
cargo generate-lockfile --offline --manifest-path audit/fuzz/Cargo.toml
cargo fmt --manifest-path audit/fuzz/Cargo.toml --check
cargo clippy --manifest-path audit/fuzz/Cargo.toml --locked --offline --all-targets -- -D warnings
```

Cargo.lock is intentionally local/ignored because this workspace predates the audit harness. For a campaign, preserve the generated lock in its evidence before running with --locked. Do not regenerate it between repetitions. Ten jobs is this workstation's campaign limit, not a portable default; choose at most the owner's allowed CPU budget and bind the process tree when measuring.

## Fuzz run

Running with sanitizer instrumentation requires separately provisioned nightly Rust and cargo-fuzz; stable lint success is not fuzz coverage. No automatic installation occurs. From the repository root:

```sh
cd audit/fuzz
cargo +nightly fuzz run decode_packet corpus/decode_packet seeds/decode_packet -- -max_total_time=300 -rss_limit_mb=2048 -max_len=65536
```

cargo-fuzz is the build/run tool around the existing target and libFuzzer engine; it does not replace the target implementation or its seeds. Keep those sources.

The target calls the shared OSC decode entry point. A crash, timeout or sanitizer report needs retained input and reproduction before attribution. This bounded example covers packets up to 64 KiB; it does not cover every allowed stream frame size. An audit must explicitly choose input length, duration, memory and CPU bounds for its transport scope. The seed corpus is versioned; growing corpus, artifacts, coverage, targets and the local lock are ignored. Watch free space during long campaigns and stop before its declared floor.

Record source/resource commits, tool versions, generated lock, argv, environment/affinity, wall time, corpus hashes and outcomes under the campaign directory. Copy crash artifacts and minimized reproducers there before cleaning local outputs. First-campaign source 04f4b742 used the original fuzz/ path; its immutable records retain that historical path. That campaign later installed cargo-fuzz 0.13.2 and an isolated nightly 1.101.0 (2026-10-15 channel) and ran the unchanged target for 301 seconds under AddressSanitizer: 5190816 executions without a reported crash. Exact results/corpus and limits are retained in its record. This is sampled coverage, not a proof for arbitrary input. For replay, install nightly-2026-10-15 and cargo-fuzz --version 0.13.2 --locked in a new external tool root; do not resolve a future floating nightly as if it were the same toolchain.
