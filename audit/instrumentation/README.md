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
