# Audit tooling plan

## Initial organization

- ✅ Isolate development on branch `audit` in a separate worktree.
- ✅ Define scope, methodology, storage boundaries and evidence templates.
- ✅ Provide scenario specifications and a dependency-free validator.

## Ready for the first audit

- ✅ Pin and verify the harness tool profile, create external environments, and document explicit offline package provisioning.
- ✅ Capture environment, source/resource revisions, package/crate inventories and isolated checkout/build preparation.
- ✅ Define executable component, Python, web and editor scenarios with repetitions, deterministic fixtures, correctness assertions and explicit manual device/review procedures.
- ✅ Capture commands/logs/statuses with timeouts, warmup separation and explicit skip/error handling; block stale-output measurements after failed prerequisites.
- ✅ Export complete instrumentation changes without mutating the original index; verify clean-base replay including new/binary/deleted/staged files and retain the patch with results.
- ✅ Validate raw metric parsing, summaries, required coverage, manual evidence, integrity and compatible comparisons using known inputs and negative cases.
- ✅ Rehearse preparation, isolated execution, manual attachment, sealing and archive retrieval on synthetic repositories without auditing application behavior.
- ✅ Keep all component crates in dynamic coverage; audit Python as the reference client, web parity and the standalone editor as separate units.

## First campaign: where the application findings went

The archived record keeps the findings as they were reported; this is what the maintainer decided after review, with the application commits on `main`.

- ✅ F04 (ambiguous rustdoc links), F05 (VST3 fixture formatting), F06 (plugin tests naming a path that never existed, and run by no CI job): fixed in `b0eca25c`.
- ✅ F03 (`Toolkit` public and safe while its soundness depends on the caller): the type is now crate-private, and `Score::engraver`, a second way past the lock, is removed, in `39c46e16`.
- ✅ F02 (the GUI host's window front queued frames without a ceiling): the front admits frames through one shared `clausters_net::Inflight`, in `7af3ff3d`.
- ✅ E01 (SIGXCPU in the embedded engine): read as the embed running without the binary's SIGXCPU guard (A19); the decision whether the embed offers it is filed in the root `PLAN.md`, Found by use, in `f472cee0`.
- ✅ F01 (no per-block bound on the command drain): downgraded to a measurement to take before any policy, filed in the root `PLAN.md`, Future directions, in `f472cee0`.
- ✅ F07 (the editor's evaluation queue has no bound): not filed. The queue is fed by hand from a local REPL, the editor has no plan of its own, and no use has shown a cost.

## Second campaign: real-device audio and interference

Campaign `2026-10-15_2558210e_02` (record `audit/records/2026-10-15_2558210e_02.zip`, `findings.md` inside) measured the audio callback on the device at quanta 1024 and 128 and the audio/GUI interference at 128, with a histogram patch on the server's callback and the GUI host's frame.

- ✅ The soft `RLIMIT_RTTIME` was one buffer, so one late callback demoted the audio thread until restart: fixed in `4ee5b7d2`, verified by twelve runs at quantum 128.
- ✅ Interference at 60 nodes: none systematic on the audio callback (p99 within 20 us per placement mode); the GUI host held 30 fps in every condition. Not filed.
- ✅ The callback's cost depends on the core it lands on (efficiency cores 53 % against 32 % for the same load); filed in the root `PLAN.md`, Found by use. A 40-minute capture of every callback over 90 % of its deadline found no overrun in 900 000 callbacks and only near misses on efficiency cores, pure DSP time, so the earlier isolated overruns are folded into that entry rather than filed apart.

## Definition of done

The resources are ready when the validator and harness contract tests pass, prerequisites and execution/review commands are documented, and resources stay isolated from application dependencies. Actual application auditing is a separate action. A complete campaign requires the mandatory manual/device/target evidence for its unit; automatic collection alone cannot establish it. Full project coverage requires records for all four independent units and all component scenarios.

## Found by use

- ✅ A generic argument formatter interpreted JSON braces as substitution fields. Limit expansion to the documented tokens so JSON-emitting drivers remain valid.
- ✅ The benchmark's peak_block field contains microseconds, including diagnostic rows that reuse the field. Preserve that unit and keep ungated rows descriptive.
- ✅ Existing native/web builders resolve workspace target paths literally. Isolated preparations provide target symlinks without touching the maintainer's checkout.
- ✅ A18: preserve raw capture bytes with scoped whitespace attributes and check staged evidence diffs. Moot since records left the repository.

From the first campaign's method notes:

- ⬜ A01: remove the unnecessary editor target symlink and test real directory-only ignore patterns during preparation.
- ⬜ A02/A03: publish debug correctness scenarios and target-aware workspace selection, with profile/feature validation.
- ⬜ A04/A05: preflight web vendor assets and retain generated-source diffs as build outputs for repeat runs.
- ⬜ A06/A07: collect signal names, last-test identity and counts from failed suites; support subprocess isolation for native tests.
- ⬜ A08/A09: promote campaign CPU-affinity/jobs and disk-floor/nonincremental controls into the reusable CLI and validate them.
- ⬜ A10/A12: preserve per-crate/target evidence and renderer identity; distinguish software acceptance from real GPU/presentation measurements.
- ⬜ A13: model step dependencies so independent checks still run after unrelated failures, with uniform auxiliary command receipts.
- ⬜ A14/A15: retain independent fuzz lock provenance and accept the actual JSON-array benchmark format; add producer-contract checks.
- ⬜ A16/A17: isolate targets between independent workspaces and formalize optional pinned fuzz-tool provisioning/seed provenance.
- ⬜ A22: `prepare` copies ignored files into the campaign resources (`audit/records/`, the fuzzer's build directory), which made `resources.tar` 380 MB; copy what git tracks under `audit/` instead.
- ⬜ A23: a GUI measurement must check that the window drew. An unattended screen blanks, the host then receives no frame callbacks and draws 2-3 frames a minute, which reads as no interference; the second campaign's first two sets were lost to it. Check a minimum frame rate per run and say in the procedure that the screen must stay on.
- ⬜ A24: the second campaign's per-callback histogram patch and slow-callback capture (faults, context switches, commands, core per event), its representative-load rule and its frame-rate check live in a scratch driver kept in the record; promote them into the harness (an instrumentation patch with a JSONL writer, a sweep step kind) so the next device campaign does not rewrite them.
- ✅ A19/A20/A21: search the project's own records before calling a cause unresolved or an input absent, and rank severity by evidence and cost; written into procedures/review.md.

## Future directions

- ⬜ Evaluate optional CI integration after actual campaign evidence establishes stable noise and duration.
- ⬜ Automate more device/GPU captures where backend support justifies it; preserve the required manual evidence path and explicit unsupported coverage.
