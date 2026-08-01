---
name: release-versioning
description: How to version a Clausters release — the package SemVer versus the two binary ABI counters (ABI_VERSION for embed/IPC, CORE_ABI_VERSION for the core FFI), which of the three answers which question, the pre-1.0 and post-1.0 release rules, and the one-way linkage that makes an ABI bump drag SemVer's breaking tier along. Consult before bumping any version number, before changing the shm segment layout or either C ABI surface, and when deciding whether a change is breaking.
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
