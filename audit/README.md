# Clausters audit tooling

Tools for auditing the project at a fixed commit: the Rust components (audio server, GUI host and every crate), the Python client, the web client and the standalone editor. Each is a separate audit unit with its own scenario, and no unit's result says anything about another's.

The harness never measures the working checkout. It clones the chosen commit into an external work root, builds and measures there, and leaves the repository untouched. The OSC decoder fuzzer lives here too ([fuzz](fuzz/README.md)), since it is run as part of an audit.

## Where results live

Results are **not** kept in the repository. A campaign's runs, logs, measurements and sealed records stay in its external work root, and `archive` packs each run into a tar.gz with a SHA-256 sidecar, kept by whoever ran the audit. `audit/records/` is ignored by git and holds the auditor's compressed copy of each campaign's reviewed record, one zip per campaign, with a `SHA256SUMS` beside them.

What the repository keeps is what was decided: [PLAN.md](PLAN.md) lists each finding of a campaign and the commit or plan entry that settled it, and an application finding that stays open is filed in the owning plan's "Found by use" or "Future directions", like any other.

The first campaign (2026-10-15, commit `04f4b742`) is `audit/records/2026-10-15_04f4b742_01.zip` on the auditor's machine. Beside it, `audit-branch.bundle` keeps the history of the branch the tooling was developed on (`git fetch audit-branch.bundle audit:audit` restores it). The resources the campaign actually ran (commit `2e376873`, an earlier version of that branch's second commit) are not in it: each run's own archive carries them as `resources.tar`. Read [methodology notes](methodology-notes.md) before the next one: that campaign needed corrections the runner does not make yet.

## Prepare

The harness needs Linux and the tool profile in [tools/versions.json](tools/versions.json) (Python, Git, Rust/Cargo, Node and npm at pinned versions). `doctor` fails on a version mismatch; use a separate reviewed profile rather than accepting another toolchain. Nothing is installed automatically.

From the repository root:

```sh
python3 audit/scripts/validate.py
python3 -m unittest discover -s audit/tests -v
python3 audit/scripts/audit.py doctor
python3 audit/scripts/audit.py prepare --work /tmp/clausters-audit --revision <commit>
```

Pick a new external directory with enough disk space (the first campaign peaked near 40 GiB of build targets). `prepare` resolves the commit, makes an independent clone, creates a Python venv without pip and copies the audit resources, so later edits to this directory cannot change the campaign. It does not build or measure anything.

## Run

Use the copies `prepare` saved:

```sh
W=/tmp/clausters-audit
python3 $W/resources/scripts/audit.py run --work $W --scenario $W/resources/scenarios/workspace.json --name workspace-01
python3 $W/resources/scripts/audit.py run --work $W --scenario $W/resources/scenarios/audio.json --name audio-01
python3 $W/resources/scripts/audit.py run --work $W --scenario $W/resources/scenarios/gui.json --name gui-01
python3 $W/resources/scripts/audit.py run --work $W --scenario $W/resources/scenarios/combined.json --name combined-01
```

Do not run performance scenarios while something else compiles or uses the audio device or the GPU. Cargo runs locked and offline against an external target directory; a missing cached dependency is a recorded error, never a download.

The Python, web and editor units each get their own work root. After `prepare`, provision the unit offline as [tools/package-setup.md](tools/package-setup.md) says, then run its scenario. Python is the reference client; the web review compares its surface and its examples against it.

Exit codes: `0` collected and waiting for review, `1` failed checks or a regression, `2` a harness or prerequisite error, `3` required manual evidence still missing. A run never passes just because its automatic commands did.

## Review, seal and archive

Follow the procedures ([crates](procedures/crates.md), [audio](procedures/audio.md), [gui](procedures/gui.md), [combined](procedures/combined.md), [review](procedures/review.md)). Manual evidence is a copy of [templates/manual-evidence.json](templates/manual-evidence.json), filled outside the repository and attached by absolute path; a measurement that was not taken is skipped, never zero. Write the actual review into the run's `findings.md`, then:

```sh
python3 $W/resources/scripts/audit.py attach $W/runs/audio-01 --step audio-device --evidence /tmp/audio-device-evidence.json
python3 $W/resources/scripts/audit.py seal $W/runs/audio-01 --reviewer maintainer
python3 $W/resources/scripts/audit.py verify $W/runs/audio-01
python3 $W/resources/scripts/audit.py archive $W/runs/audio-01 --output /data/audio-01.tar.gz
```

`seal` requires complete coverage, no failures and all manual evidence, and hashes every retained file; a sealed run is never edited, a correction is a new campaign. A failed or partial run can still be kept as evidence with `seal --preserve-incomplete --reason "..."`, which marks it as not accepted. `archive` adds snapshots of the source and the resources, checks a fresh extraction and writes the sidecar.

## Further reading

[Scope](scope.md) and [methodology](methodology.md) say what an audit claims and what it does not. [Formats](formats.md) defines the record files. [Instrumentation](instrumentation/README.md) covers measuring with a patch applied to the clone, and [baselines](baselines/README.md) comparing two sealed runs, which needs a tolerance taken from a same-code noise measurement. [Scenarios](scenarios/README.md), [fixtures](fixtures/README.md), [tools](tools/README.md) and [templates](templates/record/README.md) describe their directories. [PLAN.md](PLAN.md) tracks the tooling's own work and [development.md](development.md) its design decisions.
