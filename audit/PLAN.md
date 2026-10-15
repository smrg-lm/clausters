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
- ✅ A19/A20/A21: search the project's own records before calling a cause unresolved or an input absent, and rank severity by evidence and cost; written into procedures/review.md.

## Future directions

- ⬜ Evaluate optional CI integration after actual campaign evidence establishes stable noise and duration.
- ⬜ Automate more device/GPU captures where backend support justifies it; preserve the required manual evidence path and explicit unsupported coverage.
