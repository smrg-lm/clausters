# Audit tooling plan: the closed tracks

The tracks of [`audit/PLAN.md`](../../../audit/PLAN.md) whose every milestone shipped, moved here
whole on 2026-11-12 so that the plan holds what is open. They are the record
of what each track decided and built; a track is not reopened here, and
work that changes what one built is written in the plan.

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
- ✅ E01 (SIGXCPU in the embedded engine): read as the embed running without the binary's SIGXCPU guard (A19); the decision whether the embed offers it is filed in the `crates/clausters-server/PLAN.md`, Found by use, in `f472cee0`.
- ✅ F01 (no per-block bound on the command drain): downgraded to a measurement to take before any policy, filed in the `crates/clausters-server/PLAN.md`, Future directions, in `f472cee0`.
- ✅ F07 (the editor's evaluation queue has no bound): not filed. The queue is fed by hand from a local REPL, the editor has no plan of its own, and no use has shown a cost.

## Second campaign: real-device audio and interference

Campaign `2026-10-15_2558210e_02` (record `audit/records/2026-10-15_2558210e_02.zip`, `findings.md` inside) measured the audio callback on the device at quanta 1024 and 128 and the audio/GUI interference at 128, with a histogram patch on the server's callback and the GUI host's frame.

- ✅ The soft `RLIMIT_RTTIME` was one buffer, so one late callback demoted the audio thread until restart: fixed in `4ee5b7d2`, verified by twelve runs at quantum 128.
- ✅ Interference at 60 nodes: none systematic on the audio callback (p99 within 20 us per placement mode); the GUI host held 30 fps in every condition. Not filed.
- ✅ The callback's cost depends on the core it lands on (efficiency cores 53 % against 32 % for the same load); filed in the `crates/clausters-server/PLAN.md`, Found by use. A 40-minute capture of every callback over 90 % of its deadline found no overrun in 900 000 callbacks and only near misses on efficiency cores, pure DSP time, so the earlier isolated overruns are folded into that entry rather than filed apart.
