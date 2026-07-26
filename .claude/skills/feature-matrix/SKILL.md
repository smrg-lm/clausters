---
name: feature-matrix
description: Run the fmt + clippy + rustdoc checks CI does not cover — the def-family feature matrix (synth/faust/neither), the workspace, the GUI host and the doc build's own lints (broken intra-doc links). Consult before committing any change to feature-gated code or to doc comments, or whenever clippy or rustdoc needs to be verified clean across the whole matrix.
---

# The feature matrix CI does not run

CI (`.github/workflows/ci.yml`) lints `cargo clippy --workspace --all-targets`
and the GUI host — the **default** build set, plus the GUI. It never builds the
def-family matrix. So a warning (or an error) that appears only under
`--no-default-features`, or only with `synth` alone, or only with `faust` alone,
**passes CI and lands on `main`**.

That matters because `synth` and `faust` are peers, either can ship alone, and
the code is full of `#[cfg(feature = ...)]` seams where an import, a helper or a
match arm goes unused as soon as one family is switched off.

CI never builds the **docs** either, in any configuration. `cargo doc` is its
own lint pass — clippy says nothing about a `[`link`]` to an item that was
renamed, moved or made private — so those warnings accumulated unwatched until
the doc build was added here.

Two standing rules from CLAUDE.md apply to whatever this reports:

- **Zero warnings, not "no new ones."** Fix warnings the change did not
  introduce too. CI pins no toolchain (`dtolnay/rust-toolchain@stable`, no
  `rust-toolchain.toml`), so every rustc release can turn a green tree red with
  no code change — finding warnings you did not cause is normal.
- **Pre-existing warnings get their own commit**, separate from the feature, so
  the feature's diff stays readable.

A warning that is genuinely wrong gets a scoped `#[allow(...)]` **with a comment
saying why** — never a silent pass.

## What already runs on its own

Two hooks cover the parts that can be automated, so this skill is for the part
that cannot:

- `.claude/hooks/fmt-rust.sh` formats every `.rs` file as it is written.
- `.githooks/pre-commit` blocks a commit whose fmt or clippy is dirty — but only
  for the **default** feature set of the workspaces the working tree touches,
  because five extra builds at commit time is not a cost worth paying on every
  commit. It needs `git config core.hooksPath .githooks` once per clone; if that
  was never run, nothing is being checked at commit time at all.

The def-family matrix is the remaining gap, and it is deliberately manual: run
it when the change touches feature-gated code, and before a release.

## Running it

```sh
.claude/skills/feature-matrix/check.sh
```

It runs every configuration even when an earlier one fails, then prints a
pass/fail table and exits non-zero if anything failed. Expect a few minutes on a
cold `target/` — each feature combination is a distinct build.

One option: `--fast` skips `cargo fmt` and the two default-feature clippy
configurations (the ones CI already covers), leaving what CI never sees — the
def-family matrix, the `verovio` build and both doc builds. Use when you have
just run the ordinary clippy by hand.

**The script only reads.** It never writes to the working tree — it is the gate
that decides whether the code is committable, and a gate that edits what it is
judging cannot be trusted to report on it. `cargo clippy --fix` (rustfix applying
the suggestions marked machine-applicable) is a genuinely useful tool, but run it
by hand, on one feature configuration, and read `git diff` afterwards: a
mechanical fix is not always the change you meant, and what is obviously right
under one set of `cfg`s is not always right under another.

## What it covers

| # | Configuration | In CI? |
|---|---------------|--------|
| 1 | `cargo fmt --check` | yes |
| 2 | `cargo fmt --check --manifest-path clients/gui/Cargo.toml` | yes |
| 3 | `cargo clippy --all-targets` (default features) | yes |
| 4 | `cargo clippy --all-targets --no-default-features` | **no** |
| 5 | `cargo clippy --all-targets --no-default-features --features synth` | **no** |
| 6 | `cargo clippy --all-targets --no-default-features --features faust` | **no** |
| 7 | `cargo clippy -p clausters-ffi --features verovio --all-targets` | **no** |
| 8 | `cargo doc --no-deps --workspace` | **no** |
| 9 | `cargo doc --no-deps --workspace --no-default-features` | **no** |
| 10 | `cargo doc --no-deps --workspace --no-default-features --features synth` | **no** |
| 11 | `cargo doc --no-deps --workspace --no-default-features --features faust` | **no** |
| 12 | `cargo doc --no-deps --document-private-items` in `clients/gui` | **no** |
| 13 | `cargo clippy --workspace --all-targets` | yes |
| 14 | `cargo clippy --all-targets` in `clients/gui` | yes |

Every clippy line runs with `-- -D warnings` and every doc line with
`RUSTDOCFLAGS=-D warnings`, matching CI's bar, so a warning is a failure rather
than something to scroll past.

Configuration 4 is also the build that must stay green **without libfaust
installed at all** — the core has to compile and test with no LLVM-backed
libfaust on the machine.

Configuration 7 is the same kind of gap one crate over: `verovio` is off by
default, so CI's `--workspace` run never enables it and the notation layer it
pulls in is linted by nothing. It needs no libverovio present — clippy checks
and never links — so it lints the code under the feature's `cfg`s, not the
library's presence.

Configurations 8–12 are the doc build, and it walks the def families for the
same reason clippy does: a link whose target is compiled away by a feature
resolves in one configuration and not in the next, and only the default one
gets built by habit.

That has one consequence for how a doc comment is written. **A doc comment that
names an item across a feature seam names it in backticks, not as a link** —
`dsp::denormals` naming `server::backend`, `server::defstore` naming
`faust::cache::FaustRecord`, `embed`'s module docs naming the `realtime`-gated
C exports. Linking it would resolve in the build where the target exists and
warn in every build where it does not.

The GUI host is documented with `--document-private-items` because that is how
its docs are read: it is the internal host crate, most of it private, and its
module docs name the private function that does the work (`frame`'s `render`,
`widget`'s `build`/`apply`). Its crate root turns
`rustdoc::private_intra_doc_links` off for the same reason — `broken_intra_doc_links`,
the one that catches a link to something that does not exist at all, stays on.

## What it does not cover

- **`fuzz/`** — a separate workspace, outside both CI and CLAUDE.md's list. Lint
  it by hand, from inside it (`cd fuzz && cargo clippy --all-targets`), when you
  touch a fuzz target. It lints on stable — only *running* the fuzzer needs
  nightly. The commit hook does lint it when a commit touches `fuzz/`, so this
  is for the passes between commits.
- **`--features embed`** and `midi-jack` — CI does exercise the embed test run
  (`cargo test --workspace --features embed`); `midi-jack` needs
  `libjack-jackd2-dev` and is left to the machines that have it.
- **Tests.** This is a lint gate, not a test run. `cargo test --workspace` is a
  separate step in the commit workflow.
