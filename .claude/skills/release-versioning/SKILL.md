---
name: release-versioning
description: How to version a Clausters release — the package SemVer versus the two binary ABI counters (ABI_VERSION for embed/IPC, CORE_ABI_VERSION for the core FFI), which of the three answers which question, the pre-1.0 and post-1.0 release rules, and the one-way linkage that makes an ABI bump drag SemVer's breaking tier along — and the procedure for cutting one: the five manifests the version lives in, rehearsing the release gate with `gh workflow run release.yml` before the tag exists, tagging, and what is one-way after it. Consult before cutting or tagging a release, before bumping any version number, before changing the shm segment layout or either C ABI surface, and when deciding whether a change is breaking.
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
5. **A counter moves once per release, not once per commit.** If the same
   boundary changes again before that number has shipped (no tag yet), **amend**
   the existing bump and its comment instead of bumping past it — a counter
   states the distance from the last *published* boundary, not the history of
   how the release got there. The same holds for the SemVer tier rule 3 drags
   along: one breaking tier per release, however many breaking changes it took.

Rationale (why the decouple) is in `docs/decisions.md`.

## Cutting a release

**The version is bumped during development, not at release time.** The number
in the tree is the development version, and the tag publishes *it* — a release
does not open by choosing a number. So the tag to cut is whatever
`Cargo.toml` already says (`v0.8.0` for `version = "0.8.0"`), and the question
"which bump" belongs to the commit that made the breaking change, not to this
procedure. If the rules above say the tier is wrong for what accumulated since
the last tag, fix it as its own commit first, then release.

1. **One version, five places.** The root `Cargo.toml`, every crate under
   `crates/`, `clients/python/pyproject.toml`, `clients/web/package.json` and
   `clients/gui/Cargo.toml` (plus its own lockfile — it is an independent
   workspace, so `cargo update -p clausters-gui --offline` after editing).
   **Only one pair is checked by anything**: `npm run check-package` refuses a
   `package.json` that disagrees with the crate. The other three are on trust,
   and `clients/gui` in particular has drifted before precisely because no root
   build reads its manifest.
2. **Both ABI counters against the last tag** —
   `git show <last-tag>:src/server/ipc.rs | grep ABI_VERSION` and the same for
   `crates/clausters-ffi/src/lib.rs`'s `CORE_ABI_VERSION`. If either moved, rule
   3 above requires the breaking tier to have moved too; check, don't assume.
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
