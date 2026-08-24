---
name: release-versioning
description: How to version a Clausters release — the package SemVer versus the two binary ABI counters (ABI_VERSION for embed/IPC, CORE_ABI_VERSION for the core FFI), which of the three answers which question, the pre-1.0 and post-1.0 release rules, and the one-way linkage that makes an ABI bump drag SemVer's breaking tier along — and the procedure for cutting one: the one place the version is written and the script that spreads it, rehearsing the release gate with `gh workflow run release.yml` before the tag exists, tagging, and what is one-way after it. Consult before cutting or tagging a release, before bumping any version number, before changing the shm segment layout or either C ABI surface, and when deciding whether a change is breaking.
---

# Versioning: SemVer of the package vs. the binary ABI counters

Three version numbers exist and answer **different** questions — keep them
distinct:

- **The package SemVer** — `version` in `Cargo.toml` (and the Python wheel). The
  *source/package* contract: what `cargo`/`pip` resolves and installs.
- **The embed / IPC ABI** — `ABI_VERSION` in `src/server/ipc.rs`, exposed by
  `clausters_abi_version()`. The shm segment layout + the embed C ABI.
- **The core FFI ABI** — `CORE_ABI_VERSION` in `crates/clausters-ffi`, exposed by
  `clausters_core_abi_version()`. The language-agnostic C surface (ctypes/N-API/
  wasm).

The two integer counters — not SemVer — are the **source of truth for binary
compatibility**: they are monotonic, bumped only when their own boundary changes
incompatibly, and checked **at runtime** on attach/load. SemVer governs the
package, never the wire.

## Release rules

1. **Pre-1.0 (while the major is `0`)**, the **minor** is the breaking tier —
   this is standard SemVer, the minor acts as the major. *Any* incompatible
   change (source API **or** binary boundary) bumps the minor; purely additive or
   corrective changes bump the patch.
2. Bump `ABI_VERSION` / `CORE_ABI_VERSION` **only** when that specific boundary
   changes incompatibly — independently of SemVer.
3. **Linkage (one-way):** if a release bumps either ABI counter, that release
   **must** bump the breaking tier of SemVer (minor pre-1.0, major post-1.0). The
   reverse does not hold — a minor bump can ship without touching either counter.
4. **At `1.0.0`** the semantics become the standard post-1.0 ones (major breaks,
   minor adds, patch fixes); the ABI counters keep their role unchanged.
5. **"Published" means *tagged*, everywhere in this document — and the two
   kinds of number do opposite things with that.** This rule used to say a
   counter "states the distance from the last published boundary" and told you
   to **amend** an unshipped bump rather than move past it. That was wrong for
   the ABI counters, it contradicted the release procedure below (step 2 asks
   only whether a counter *moved* since the last tag, never whether it moved by
   one), and its ambiguity is what made it unenforceable: read "published" as
   "in the tree" and every bump is legal, read it as "tagged" and every bump is
   a violation, so whichever reading suited the moment was always available.
   The split now, with the reason each way:

   - **The ABI counters bump whenever their boundary changes — per commit is
     correct, and gaps are free.** Both are compared by **equality and nothing
     else** (`header.abi_version != ABI_VERSION` in `clausters-core/src/shm.rs`,
     `got != CORE_ABI_VERSION` in `clients/python/clausters/_native.py`).
     Nothing subtracts them, orders them or counts them, so "distance" was a
     quantity with no consumer. What per-commit bumping *does* buy is real: in
     this source checkout the staged `_bin`/`_libs` copy wins over `target/`,
     and a counter that moved makes a stale staging fail with *"speaks ABI v21,
     this binding v22"* instead of an `AttributeError` at some later call site.
     Under the amend rule both sides would read the same number all cycle and
     that failure would surface as a missing symbol.
   - **The SemVer tier moves once per unreleased cycle.** Here the old rule was
     right and stays: a version is consumer-facing and ordering-sensitive, and
     bumping the breaking tier twice before a tag invents a release that never
     existed. If the tier has already moved since the last tag, a further
     breaking change rides the bump that is already there.

   So a counter is *not* the last tag's plus one, and it is not meant to be;
   check that it **differs**, which is rule 3's trigger and all any consumer
   reads.

Rationale (why the decouple) is in `docs/decisions.md`.

## Cutting a release

**The version is bumped during development, not at release time.** The number
in the tree is the development version, and the tag publishes *it* — a release
does not open by choosing a number. So the tag to cut is whatever
`Cargo.toml` already says (`v0.8.0` for `version = "0.8.0"`), and the question
"which bump" belongs to the commit that made the breaking change, not to this
procedure. If the rules above say the tier is wrong for what accumulated since
the last tag, fix it as its own commit first, then release.

1. **One version, one command.** The number is decided in the root
   `Cargo.toml`'s `[workspace.package].version`, which the eight Cargo crates
   **inherit** (`version.workspace = true`). The five files that cannot inherit
   it — `clients/gui/Cargo.toml` (an independent workspace) with its lockfile,
   `clients/python/pyproject.toml`, `clients/web/package.json` and its
   `package-lock.json` — are written by **`scripts/set-version.sh <x.y.z>`**,
   which also regenerates both lockfiles; run with no argument it prints what
   every file says. Nothing here is on trust any more:
   `tests/versions.rs` fails when any of them disagrees, and when a Cargo crate
   writes its own number instead of inheriting. (Before that: `clients/gui` had
   drifted, and `package-lock.json` was found two minors behind.)
2. **Both ABI counters and the SemVer tier against the last tag** —
   `.claude/skills/release-versioning/versions.sh`, which prints all three and
   exits non-zero if it cannot find one. Read it by rule 5: a counter that
   **differs** is what rule 3 triggers on — by how much is not a question and a
   gap is not a defect — while the version is the one that must have moved its
   breaking tier **exactly once** since the tag. Two steps there (0.8.1 → 0.9.0
   → 0.10.0) invented a release that never existed; settle on one before
   tagging rather than tagging both.

   The script **searches** for each constant instead of naming its path, and
   that is the point rather than tidiness: `ABI_VERSION` has already moved file
   once (`src/server/ipc.rs` → `crates/clausters-core/src/shm.rs`), and the
   hardcoded `git show <tag>:src/server/ipc.rs | grep ABI_VERSION` this step
   used to be went on returning an empty string afterwards — which reads
   exactly like "did not move". A check whose failure mode is silence is not a
   check, which is the same defect rule 5 had in prose.
3. **Rehearse the gate before the tag exists.** `gh workflow run release.yml
   --ref main` runs `release.yml`'s `verify` job — the full fmt/clippy/rustdoc
   feature matrix plus `cargo test` on the default set and on `+embed` — with
   every build and publish job skipped by `if: github.event_name == 'push'`.
   Watch it green (`gh run watch <id> --exit-status`, ~6 min warm) on the exact
   commit about to be tagged. On a tag, `verify` runs *after* the tag exists, so
   a red one leaves a tag to delete and re-cut; the rehearsal moves that
   discovery earlier and costs nothing but runner time.
4. **Tag and push.** `git tag vX.Y.Z && git push origin vX.Y.Z`. From here it is
   one-way: `publish-npm` and `publish-pypi` cannot be taken back, and a version
   is never re-published. If something is wrong after the tag, the fix is the
   next version, not a retag.
5. **Watch the run to the end.** The GitHub release page is the last job, so a
   run that stops earlier means one registry has the version and the other does
   not — say which, in the report, rather than calling the release done.
