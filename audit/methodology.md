# Methodology

## Freeze the inputs

Identify the full application commit and the full audit-resource commit separately. Record the exact scenario snapshots, commands, working directories, compiler/tool versions, feature sets, lockfile hashes, seeds, environment overrides and input hashes. A branch name is not a reproducible source identifier. Resolve external tools and fixtures before accepting a run; preserve downloads by hash and source where redistribution is allowed.

Use a clean, independent checkout of the selected application commit. A worktree shares Git objects and refs but has its own files, index and HEAD. Do not switch branches, stage or commit in another agent's worktree. Do not prune worktrees, rewrite shared refs or run global cleanup during concurrent work. A detached worktree is suitable for a measurement checkout. Create it only under an explicitly chosen work root.

Set an external `CARGO_TARGET_DIR`, including when entering the GUI's independent workspace. Isolate browser profiles, ports, temporary files and client artifacts as well. Python launchers prefer bundled native artifacts: point their documented override variables to the measured build rather than refreshing the active checkout's bundled copies. Preserve the resolved paths and hashes in the manifest.

## Describe the environment

Record CPU, core count, memory, OS/kernel, power policy, affinity and actual scheduling policy, audio backend/device/sample rate/callback sizes, GPU/driver/backend, display size/scale/refresh rate and browser version as applicable. Mark unavailable facts explicitly. Do not change system-wide power, scheduling, audio or driver settings automatically.

File isolation does not isolate CPU, GPU, caches, storage or audio devices. Coordinate exclusive performance windows with other agents and record concurrent load. Prefer paired or alternating base/candidate measurements on the same machine. Set warmup, measurement duration, repetition count and order before the run. Record temperatures or throttling when available. Do not infer portability from one machine.

## Measure audio

Separate offline DSP throughput from real-device behavior. Measure block and callback durations separately, using the actual callback frame counts and sample rate for deadlines. Record quantiles, sample counts, maximum observed, deadline misses and backend xruns when available. An observed maximum is not a proven worst-case bound. A throughput ratio does not prove glitch-free playback.

Review allocation and deallocation, locks, I/O, queue bounds, command bursts and deferred reclamation on RT paths. Pair timing with DSP/scheduling correctness and saturation/recovery checks. End-to-end audio latency requires a separately documented loopback setup; processing time is not that latency.

## Measure the GUI

Measure idle and active behavior separately. Distinguish CPU frame preparation, GPU execution and presentation. Record frame-time distributions, missed frame targets, input-to-visible latency where measurable, redraws, memory and upload/message volumes. GPU timestamp queries are optional and backend-dependent; missing support is a limitation, not a zero duration.

Sweep total data and visible data independently, with fixed physical viewport sizes where appropriate. Check image/interaction correctness along with performance. Preserve screenshots and their dimensions/backend when visual evidence is collected. Offscreen measurements cannot establish interactive presentation behavior.

## Instrument without changing the evaluated checkout

Apply measurement changes only in the isolated measurement checkout. Save the full base commit and an applicable `instrumentation.patch`, including new text files; use a binary-capable patch or separately hashed assets for binary additions. A plain unstaged diff omits staged changes and untracked files. Inspect the patch contents for completeness and unintended data.

Record patch SHA-256, purpose, measurement points, build implications and overhead. An optional instrumentation commit hash is supplementary: it is insufficient if its objects are not retained. Validate patch application on a clean base before accepting the record. Never mix an optimization with the measurement patch; evaluate each candidate as separately identified code.

Capture an uninstrumented control when instrumenting timing paths. Do not log, allocate or block in an audio callback to collect evidence. Prefer preallocated bounded capture drained outside RT, and record dropped measurement samples. Instrumented diagnostic timings and minimally instrumented acceptance timings must be labeled separately.

## Capture and interpret evidence

Preserve per-repetition raw data before aggregating. State units, measurement boundaries, timing source, aggregation method, noise and exclusions. Separate `passed`, `failed`, `skipped` and `error`; unavailable measurements never pass. A missing threshold means a descriptive result, not acceptance. Do not silently discard failed repetitions or compare incompatible environments.

Every finding needs reproduction steps, evidence, impact and confidence. Keep profiler captures separate from acceptance runs when profiling perturbs timing. Link each summary value to its source data and calculation. Report limitations and untested features/backends explicitly.

## Preserve a record

Use the record templates. Set the manifest status to `complete` only after review; record deviations and incomplete coverage explicitly. Hash every retained evidence file and external archive with SHA-256 using relative paths within its archive. Store external artifact locations and access requirements without credentials. Test retrieval and patch application before describing a record as reproducible.

Keep records outside the repository: the run directories in the external work root and their verified archives, with SHA-256 sidecars, wherever the auditor keeps data. The repository keeps only the decisions taken on the findings (`PLAN.md`) and the application plan entries they produced. Hashes establish identity, not continued availability: report missing archives as a reproducibility gap. Do not overwrite an accepted record; create a new record and link any correction. A baseline is an explicit pointer to a compatible accepted record, not a copy of selected numbers.

## Operational boundary

The CLI implements isolated preparation, pinned tool checks, environment/command capture, automatic scenario execution, manual evidence attachment, patch export/replay, integrity/review sealing, verified archive extraction and compatible-record comparison. The fixture generator and Linux process sampler are independent utilities. Tests rehearse the complete evidence lifecycle with synthetic repositories and known measurements; these are not application audit results.

Device/GPU capture, code review and target-specific configurations are explicit required manual procedures. An unavailable critical measurement leaves the record partial. The first actual audit must reserve the machine and choose real devices/configurations. Package dependency provisioning is explicit and offline during setup/execution; no application manifest, lockfile or active checkout is changed. Components, Python reference client, web client and standalone editor are separate audit units with distinct records and baselines.
