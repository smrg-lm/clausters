# Instrumentation export and replay

Measurement changes are made only in the prepared external source clone. Export the complete change before running. Use absolute paths for patch files and the full base commit from prepared.json:

```sh
python3 audit/scripts/audit.py export-patch --source /tmp/clausters-first-audit/source --base SOURCE_COMMIT --output /tmp/measurement.patch
python3 audit/scripts/audit.py replay-patch --repo /tmp/clausters-first-audit/source --base SOURCE_COMMIT --patch /tmp/measurement.patch
```

Export uses a disposable Git index. It includes the checkout's final staged/unstaged contents, new nonignored files, binary additions and deletions, while preserving the real index. Ignored generated files are not included by default. An intentionally force-added file in the real index is included without requiring an instrumentation commit; separately retain/hash any external input as evidence. Do not rely on an ignored untracked dependency. Inspect the patch for completeness, credentials and unintended files.

For a run, prepare a fresh clean work root at the same source commit; do not reset or clean a checkout that may contain ongoing work. The new clean preparation receives a copy of the resources. Run with its saved runner:

```sh
python3 /tmp/clausters-instrumented/resources/scripts/audit.py run --work /tmp/clausters-instrumented --scenario /tmp/clausters-instrumented/resources/scenarios/audio.json --name instrumented-01 --patch /tmp/measurement.patch
```

The runner verifies clean-base application and byte-identical re-export, creates a separate campaign-local instrumented clone, and saves instrumentation.patch, SHA-256 and replay state with the record. Record purpose, boundaries, buffering/lost samples and overhead in an attached instrumentation note. An optional retained instrumentation commit supplements the patch; its hash is not sufficient alone.

The saved patch can add measurement-only dependencies and their lockfile changes in the disposable clone. Application branch dependencies remain unchanged. Keep optimizations in separately identified candidates. Capture an uninstrumented control and state whether metrics are diagnostic or acceptance measurements. Generic metric drivers emit the JSONL format in ../formats.md; no RT logging or per-sample allocation is permitted.

## Linux device histogram and slow-callback patch

`instrument.py` promotes the second campaign's instrumentation into a reproducible patch generator. It checks every anchor against the selected source before writing, formats the result with rustfmt, exports new modules as well as edits, and verifies clean-base replay. Run it only in a clean disposable clone, separate from the preparation used for collection:

```sh
python3 audit/scripts/instrument.py --source /data/device-patch/source --output /data/device-timing.patch
python3 /data/device-run/resources/scripts/audit.py run --work /data/device-run --scenario /data/device-run/resources/scenarios/device.json --name sweep-01 --patch /data/device-timing.patch
```

Prepare the Python unit's external interpreter first. The driver passes absolute server/GUI binary overrides and uses separate data directories and loopback ports. It does not use a bundled binary from another checkout. The scenario builds the instrumented release binaries in independent server/GUI targets. Source drift fails with the missing anchor instead of silently applying partial instrumentation. This patch is Linux-native measurement code; do not apply it to a browser or other platform build. It includes no application fixes.

The shared template `timing.rs.in` records callback duration and interval in 8192 preallocated atomic buckets of 10 us, retaining the exact cumulative maximum and deadline count. The final bucket includes overflow. The GUI counts only `Drawn::Whole` and `Drawn::Live` after the rendering call succeeds; an attempted frame or unavailable drawable does not count. CPU frame duration is not GPU execution time or physical presentation latency. Cumulative snapshots are asynchronous diagnostic observations, not an atomic transaction across every counter; quantiles must come from bucket deltas, excluding warmup, and the overflowing bucket cannot give a finite upper bound.

`CLAUSTERS_AUDIT_TIMING` and `CLAUSTERS_AUDIT_GUI_TIMING` name snapshot JSON files; writer threads also append cumulative JSONL snapshots beside them (`.jsonl` extension). Disk I/O, formatting and allocation happen on those writer threads. The histogram path has no per-callback resource-usage syscall. Retain an uninstrumented control to establish overhead.

For a separate diagnostic pass, pass `--slow-us 2400` to the driver (the example threshold is 90% of 128/48000 s, not a universal threshold). This enables `getrusage(RUSAGE_THREAD)` and `sched_getcpu` instrumentation. It deliberately pays syscall overhead and cannot serve as the timing control. It records faults, context switches, commands applied, DSP blocks, longest block, CPU time, interval, frames and start/end CPU. The first 1024 slow events are retained in fixed slots; a release/acquire publication flag prevents the writer reading a half-written event. `total` counts all slow events, so `total - len(events)` identifies overflow/unpublished events. Per-CPU counts cover CPU IDs 0-63; higher IDs remain in event records but not that array. Raw snapshots and JSONL are retained with each sweep level. Watch disk space: cumulative JSONL retains repeated snapshots. The preparation's disk floor is checked before each sweep level.

## Optional GPU engine counters

`device_driver.py` samples the GUI process's DRM fdinfo at the capture endpoints and during collection. It retains the raw descriptor text, driver/device/client identities and per-engine busy nanoseconds. Shared and duplicated descriptors count once. The collector follows the [Linux DRM client usage contract](https://origin.kernel.org/doc/html/v6.14/gpu/drm-usage-stats.html): temporary counter decreases wait for catch-up; changing coverage or counters that do not catch up leave the interval unsupported. No optional counter becomes an observed zero.

These counters measure the client's accumulated GPU engine activity, including asynchronous work. They do not associate that activity with individual frames, measure a render pass with timestamp queries or establish presentation latency. Unsupported backends retain a reason. GPU metrics are initially descriptive; the device scenario does not enable extra wgpu features or change the renderer's supported backend set.

The generated patch also writes the actual wgpu adapter, driver, backend, surface size and present mode at native window creation when `CLAUSTERS_AUDIT_RENDERER` names an output file. This happens outside the audio callback. A control binary built without the patch cannot supply that additional identity file; its DRM counters remain independently attributable to its process. The driver captures the window through the existing host API after the timed interval so readback does not perturb the measured frames. This PNG is a host render readback, not proof that the compositor displayed it; screen visibility and the user's visual/audio verdict remain manual evidence.
