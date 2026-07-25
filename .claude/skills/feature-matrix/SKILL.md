---
name: feature-matrix
description: Run the fmt + clippy checks CI does not cover — the def-family feature matrix (synth/faust/neither), the workspace and the GUI host. Consult before committing any change to feature-gated code, or whenever clippy needs to be verified clean across the whole matrix.
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

One option: `--fast` skips `cargo fmt` and the two default-feature
configurations (the ones CI already covers), leaving only the three CI never
sees. Use when you have just run the ordinary clippy by hand.

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
| 7 | `cargo clippy --workspace --all-targets` | yes |
| 8 | `cargo clippy --all-targets` in `clients/gui` | yes |

Every clippy line runs with `-- -D warnings`, matching CI, so a warning is a
failure rather than something to scroll past.

Configuration 4 is also the build that must stay green **without libfaust
installed at all** — the core has to compile and test with no LLVM-backed
libfaust on the machine.

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
