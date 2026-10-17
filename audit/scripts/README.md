# Audit commands

Use audit.py --help and each subcommand's --help. README.md contains a complete workflow. There is one CLI rather than multiple wrappers with duplicated path/state handling. validate.py checks resources without application execution; tests/test_harness.py validates behavior against synthetic inputs.

Device collection also has focused drivers: `instrument.py` exports a checked measurement patch, `device_driver.py` captures one fresh-server trial, and `paired.py` collects six alternating conditions. `build_controls.py` builds the clean prepared source with separate targets and receipts; `telemetry.py` reads optional Linux counters without changing the renderer. `scenarios/device-paired.json` composes them through the main harness. `baseline.py` derives a reviewed noise descriptor only from compatible sealed records and verified archives. See the combined procedure, instrumentation and baseline references for prerequisites and limits.

prepare verifies the tool profile, creates an external detached independent clone, a no-pip venv and a resource snapshot. run uses explicit command arrays with isolated target output, per-command timeouts and logs; manual steps remain skipped until evidence is attached. Concurrent use of one work root is rejected by an exclusive lock. After an unclean runner termination, inspect the recorded PID and partial outputs before manually removing a stale .audit-lock; do not blindly resume a campaign name.

export-patch uses a temporary Git index to include staged, unstaged, new nonignored files, deletions and binary additions without changing the original index. replay-patch tests a clean clone and byte-identical re-export. run --patch uses another isolated clone and retains the verified patch in the record. attach records manual evidence; seal checks coverage and creates hashes; verify checks record integrity; archive tests retrieval by fresh extraction; compare accepts only sealed compatible evidence and explicit noise tolerance.

Exit codes are 0 for successful collection/utility, 1 for failed checks or comparison regression, 2 for usage/prerequisite/harness errors and 3 for partial collection awaiting required manual evidence. Status in manifest.json remains the authoritative state; successful collection is not accepted review.

Paths supplied for explicit record/archive/patch output are operator-controlled. Do not point them at another checkout. No tool silently resets a checkout, changes system configuration, edits application manifests, refreshes bundled artifacts or installs a package.


For Linux process-only observation, run sample_process.py --pid PID --seconds 60 --interval 0.1 --output /data/new-samples.jsonl. It reads /proc without injecting into the process, identifies PID reuse and records CPU seconds/RSS plus CPU percent relative to one core. It does not include child processes, GPU execution, presentation or device deadlines. The output must be new; attach it as raw evidence rather than substituting it for unavailable frame/callback metrics.


Failed or partial audits are valid historical evidence too. To retain them after review, use `seal RECORD --reviewer NAME --preserve-incomplete --reason "WHY COVERAGE/FAILURES ARE RETAINED"`, then verify/archive normally. This explicit mode keeps status failed/partial and review_outcome=evidence-only, sets quality_accepted=false, and never makes the record an accepted performance baseline. It does not bypass raw-data, patch or hash integrity checks. Default sealing still requires complete passing coverage.


`prepare --cpus 0-9 --jobs 10 --disk-floor-gib 150` records a campaign policy. CPU IDs must belong to the runner's allowed affinity; jobs must be positive and disk floors finite/nonnegative. Defaults keep the inherited CPU allocation, disable incremental compilation and impose no extra disk floor. `--incremental` is an explicit opt-in. `taskset` applies the declared affinity to every command and its descendants; Cargo jobs and Rust test threads inherit the job count. Setup/build/run receipts retain the effective policy and disk samples. The same policy belongs in comparison compatibility.

Preparation copies only Git-tracked `audit/` resources. Ignored archives, fuzz targets and caches never enter resources.tar. Server, GUI, editor and fuzz have independent external targets; only the server and GUI get literal-path symlinks because their builders require them. The Cargo router also handles nested builder calls and `--manifest-path`; the editor's directory-only target ignore never sees an unnecessary symlink.

Native Python tests run verbosely in one subprocess per collected file. Collection failure is a prerequisite failure; a native signal in one file is retained and other files still run. Signal names, last visible test and observed suite counts survive failed commands; counts do not change a failed verdict into a pass. Isolation is for the default discovery suite used in python.json, not arbitrary positional pytest selections.

The benchmark parser accepts the producer's pretty-printed JSON array (nullable values and descriptive diagnostic rows) as well as legacy JSONL. `peak_block` remains microseconds. Generated tracked declarations named in `generated_outputs` are saved as binary-capable patches and restored from the recorded index only in the disposable measurement checkout, so a repeat run starts clean. Other changes remain visible and block a later run. `retain_outputs` copies generated locks/artifacts into per-step evidence; a newly retained Cargo.lock becomes immutable for later steps.

`web-build` preflights all six Faust/Verovio runtime assets before compiling. Its receipt retains content hashes and the source pin hashes. These identify the input bytes; they do not establish how an external cached binary was built. Supply pinned source/build receipts separately before claiming reproducible vendor provenance.

`setup-fuzz --work WORK --cargo-fuzz WORK/tools/cargo/bin/cargo-fuzz` verifies explicitly provisioned cargo-fuzz 0.13.2 and nightly-2026-10-15, then records executable identity, rustc version and versioned seed hashes. It installs nothing and remains optional for stable lint. Follow ../fuzz/README.md for isolated provisioning and retain all installation receipts.
