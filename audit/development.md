# Development of the audit resources

This record describes the design and validation of the audit system itself, as requested by the maintainer. Application audit results stay outside the repository, in the auditor's archives. Pending work belongs in PLAN.md; shipped changes remain identifiable in Git history.

## 2026-10-15: initial organization

Context: another agent is working in the main checkout. The maintainer requested a separate `audit` branch/worktree, a reusable directory with its own instructions, and a record of how the audit resources are developed before any first audit.

Decision: add only audit/ in the isolated worktree, without changing application manifests, dependencies, books, hooks, CI or existing examples. Use Markdown and JSON for the initial resources and Python's standard library for the resource validator. Tool versions remain explicitly unselected until executable scenarios are implemented; no placeholder version is presented as a pin.

Decision: keep raw runs, builds, installed tools and checkouts external. Revised after the first campaign: its committed records added about 1 MB compressed per campaign and nothing a reader of the repository needed, so records are no longer committed. Each run is archived with a SHA-256 sidecar by the auditor, and the repository keeps the disposition of each finding in PLAN.md. Preserve archives rather than replacing the latest result.

Decision: draft workload specifications describe measurements and pending prerequisites but do not claim to execute. Setup, collection, drivers, comparison and report generation are tracked as open work, avoiding successful no-op runner scripts.

Validation: the standard-library resource validator passed; all resource files were checked for unintended non-ASCII characters (the roadmap checkboxes are intentional), local Markdown links resolved, and git diff --check passed. The staged changes were verified to be confined to audit/. No application builds, tests, benchmarks, device access, tool installation or audit campaign were performed.

Limit: reproducibility of actual measurements is not established by this scaffold. It requires completed scenario drivers, pinned tools, validated patch replay and a rehearsal. Record future method changes here with their reason and the checks actually performed; do not append application performance claims without campaign evidence.


## 2026-10-15: operational harness and expanded scope

Rebased the isolated audit branch onto main at 7206783a, preserving the latest two commits that close the node/MIDI work and moving only the audit scaffold commit. The maintainer expanded scope to all component crates, then requested separate Python-reference, web and standalone-editor campaigns. Dynamic Rust and package inventories now retain unit boundaries; per-manifest evidence prevents silent omissions.

Implemented one standard-library CLI for tool verification, independent clone/venv preparation, external targets, environment/command capture, timeouts, explicit partial/failure states, patch export/replay, evidence attachment, sealing, archive retrieval and compatible comparison. Added deterministic WAV generation, a Linux CPU/RSS sampler, operational scenario specifications and manual device/review procedures. No application dependencies, source, hooks, books, CI, examples or bundled artifacts are modified.

Decisions: keep the first performance results descriptive until paired noise and compatible references exist; require real manual evidence for device/GPU/review claims; reject empty test suites, hidden client-test skips and incompatible comparison inputs. Separate unit prerequisites are provisioned offline in isolated environments; the harness never installs system packages. Preserve only synthetic rehearsal results during development, outside Git.

Validation: resource validation and standard-library contract tests exercise known measurements, repeated collection/warmup exclusion, explicit manual coverage, subprocess failure/timeout, deterministic fixture hashes, patch replay including staged/unstaged/new/binary/deleted/ignored-staged files, index preservation, record tampering, comparison regression/incompatibility, and verified archive extraction. Tool profile checks use the workstation's actual version outputs. No Clausters builds/tests, device capture or first audit is performed while developing these resources. The final standard-library suite contains 15 passing tests, including an actual offline installation of a synthetic wheel into a disposable venv and source/resource archive retrieval. The Node parser was checked against both TAP and the installed Node spec reporter. JSON/resource validation, ASCII/local-link checks and git diff --check passed.

Limitations: physical device/GPU capture and source review remain explicit operator tasks, not simulated automatic passes. Cross-machine portability, dependency availability and backend support are checked when preparing actual campaigns. Equal recorded configuration does not establish identical thermal/background conditions. Optional profiling packages are selected and versioned only when used in retained evidence.


## 2026-10-15: first campaign and fuzzer relocation

Ran independent component, Python-reference, web and editor units against 04f4b742 with saved resources at 2e376873. Retained unsuccessful attempts, corrected scenarios, native crash diagnostics, generated-output diffs and supplemental checks rather than replacing results. The record distinguishes findings from prerequisites and incomplete physical-device acceptance. SIGXCPU delivery is reproducible in this agent environment; its origin is unresolved, and the maintainer reports no reproduction under Claude. It is not classified as a confirmed product defect.

The first real campaign exposed assumptions synthetic tests missed: directory-only ignores, diagnostic/release test profiles, target feature unification, ignored wasm prerequisites, independent fuzz locks and JSON-array producer output. Method observations and pending reusable fixes are tracked in methodology-notes.md and PLAN.md. The owner requested CPU and disk limits mid-campaign: later work inherits affinity 0-9 (half of 20 logical CPUs, selected within performance cores), ten jobs/test threads, no incremental compilation and a 150 GiB free floor. Earlier environment captures preserve the policy transition.

At the maintainer's request, moved the existing fuzzer to audit/fuzz without changing its target or seed bytes. Its separate workspace still owns libFuzzer; only its relative shared-core path and its documentation/check-hook routing change. The main application workspace/dependency declarations remain untouched. Historical campaign evidence retains fuzz/ at the measured source commit. Stable lint and formatting do not imply a sanitizer fuzz run; nightly/cargo-fuzz were initially unavailable, then explicitly provisioned in campaign-local roots with the maintainer's authorization. A bounded five-minute AddressSanitizer run completed without a reported crash; retain its exact versions, limits and corpus rather than claiming exhaustive coverage.

Validation and preservation: automatic campaign results, exact negative evidence and limitations are retained by unit; evidence-only sealing/verification and archive extraction protect run integrity. No source fix or performance acceptance follows from collecting this first baseline. Relocation checks and disk-cleanup receipts belong beside the campaign's method evidence, so another agent can continue without private session memory.
