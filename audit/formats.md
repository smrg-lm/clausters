# Evidence formats, version 1

All JSON is UTF-8; numeric values must be finite. Paths inside records are relative. Actual commands are argument arrays, never implicitly interpreted by a shell. The runner records expanded commands, cwd, start times, exit codes, elapsed seconds and separate stdout/stderr logs per step/repetition. A negative repetition index is warmup and contributes no metric samples.

## Scenario

`format_version: 1`, safe `id`, `status: ready`, a scope/acceptance explanation and `steps`. Each step has a unique safe ID, kind (`command` or `manual`) and required flag. Commands specify argv, relative cwd, positive repetitions/timeout_seconds, nonnegative warmup count and parser (`exit`, `rust-tests`, `bench`, `metrics`, `pytest`, `node-tests`). Manual steps specify instructions. Built-in substitutions are `{source}`, `{target}`, `{work}`, `{audit}`, `{python}`, `{client_python}`; literal JSON braces are preserved.

`rust-tests` requires a successful nonempty Rust test result. `bench` reads the existing example's JSONL; x_real_time is a ratio, peak_block is microseconds (including the example's diagnostic rows that reuse this field). Rows marked ungated by the example remain descriptive. `metrics` accepts one object per stdout line with name, unit, value and optional direction; use it for a reviewed measurement driver. Arbitrary stdout is not interpreted as measurements by exit-only steps.

## Manifest and environment

manifest.json records source_commit, audit_resources_commit, lockfile hashes, tools, times, expanded command entries, selected build environment, instrumentation provenance and a compatibility descriptor. Status moves from running to failed, partial or needs_review; only explicit successful sealing makes it complete. An interrupted collection that never finalizes remains running and is not accepted.

environment.json captures platform/CPU/kernel, available memory/load/power/affinity information, exact tool outputs and executable hashes. Dynamic load and memory are captures, not equality keys. Device/driver/display/presentation and actual RT-thread settings are supplied with manual evidence when relevant. Compatibility includes manual procedure/configuration/limitations, so different device configurations cannot silently compare as equal.

## Raw measurements and summaries

results/measurements.jsonl has one object per observed sample: step, repetition, name, unit, value and direction (`lower`, `higher`, `descriptive`). No missing sample is replaced with zero. Command_wall_seconds is always descriptive and includes process startup; builds/tests are not frame or callback benchmarks.

results/summary.json groups by step/name/unit/direction, with count, median, min, max and nearest-rank p95 over repetitions. With three repetitions p95 equals the maximum; it says nothing about per-frame tails. Per-frame or per-callback quantiles must be separate named metrics from the retained device capture. results/checks.json records step, repetition for command checks, required, status and reason.

## Manual evidence

A manual evidence JSON has status, operator, procedure, configuration, limitations, reason, optional metrics and artifact input paths. Each metric has name/unit/value/direction; repeated measurements use repeated entries with the same identity. A passing manual check requires at least one copied artifact. Artifacts are copied into evidence/ with hashes; the attachment itself is retained there. Record all optional profiler versions and capture commands in configuration/procedure or an attached configuration file. Do not mark a check passed merely because an artifact exists: the reviewer is responsible for the procedure's assertions.

## Integrity and review

Sealing checks required coverage, check statuses, raw-to-summary agreement and instrumentation replay/hash state. hashes.json covers all other retained files and is excluded from its own list. Verification rejects changed, added, missing or symlinked files. Hashes establish identity and detect accidental changes; they are not cryptographic signatures or an independent quality certification.

Archives are gzip tar files with a record/ root and inputs/source.tar and inputs/resources.tar snapshots, plus a .json sidecar identifying the archive SHA-256 and verified extraction. Preserve every archive at a documented durable location; local /tmp is suitable for rehearsal, not long-term evidence retention.


## Package and crate coverage

The prepared manifest inventories every tracked Rust package by package name, manifest path, content hash and audit unit. The editor is a separate unit; component crate-review evidence must contain crate_coverage keyed by every components manifest, each with status, reason and evidence. Missing entries or skipped/failed entries cannot pass that review. Python, web and editor records use their own scenario/unit identities. Test parsers for pytest/node retain passing and skipped counts; skipped tests make the required check skipped and the campaign partial. No package-wide claim is inferred from a nonempty subset of tests.

Unit setup receipts, logs and Python wheel hashes are copied into run evidence. The source's project lockfiles and package declarations are identified separately from tool versions. A setup command is not evidence that package tests ran.


The source snapshot is a Git archive of the evaluated commit, preserving tracked source without relying only on continued availability of the commit on a branch. The resources snapshot preserves the exact runner/method. Both are hashed during preparation and checked again on archive extraction. Dependency caches, compilers and system libraries are not bundled: their provisioning and captured identities remain explicit prerequisites. To reconstruct without Git history, extract the source snapshot into a new external directory and apply the retained patch with git apply; the original commit identity remains provenance, not a newly recreated commit hash.


Failed or partial audits are valid historical evidence too. To retain them after review, use `seal RECORD --reviewer NAME --preserve-incomplete --reason "WHY COVERAGE/FAILURES ARE RETAINED"`, then verify/archive normally. This explicit mode keeps status failed/partial and review_outcome=evidence-only, sets quality_accepted=false, and never makes the record an accepted performance baseline. It does not bypass raw-data, patch or hash integrity checks. Default sealing still requires complete passing coverage.


Each executed command records the resolved executable hash when available. Effective lockfile/declaration hashes are captured after applying any declared instrumentation and checked after each command; an unexpected change becomes an error and blocks downstream measurements. Builders that do not expose --locked cannot silently change the recorded dependency inputs.
