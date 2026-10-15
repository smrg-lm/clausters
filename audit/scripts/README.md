# Audit commands

Use audit.py --help and each subcommand's --help. README.md contains a complete workflow. There is one CLI rather than multiple wrappers with duplicated path/state handling. validate.py checks resources without application execution; tests/test_harness.py validates behavior against synthetic inputs.

prepare verifies the tool profile, creates an external detached independent clone, a no-pip venv and a resource snapshot. run uses explicit command arrays with isolated target output, per-command timeouts and logs; manual steps remain skipped until evidence is attached. Concurrent use of one work root is rejected by an exclusive lock. After an unclean runner termination, inspect the recorded PID and partial outputs before manually removing a stale .audit-lock; do not blindly resume a campaign name.

export-patch uses a temporary Git index to include staged, unstaged, new nonignored files, deletions and binary additions without changing the original index. replay-patch tests a clean clone and byte-identical re-export. run --patch uses another isolated clone and retains the verified patch in the record. attach records manual evidence; seal checks coverage and creates hashes; verify checks record integrity; archive tests retrieval by fresh extraction; compare accepts only sealed compatible evidence and explicit noise tolerance.

Exit codes are 0 for successful collection/utility, 1 for failed checks or comparison regression, 2 for usage/prerequisite/harness errors and 3 for partial collection awaiting required manual evidence. Status in manifest.json remains the authoritative state; successful collection is not accepted review.

Paths supplied for explicit record/archive/patch output are operator-controlled. Do not point them at another checkout. No tool silently resets a checkout, changes system configuration, edits application manifests, refreshes bundled artifacts or installs a package.


For Linux process-only observation, run sample_process.py --pid PID --seconds 60 --interval 0.1 --output /data/new-samples.jsonl. It reads /proc without injecting into the process, identifies PID reuse and records CPU seconds/RSS plus CPU percent relative to one core. It does not include child processes, GPU execution, presentation or device deadlines. The output must be new; attach it as raw evidence rather than substituting it for unavailable frame/callback metrics.


Failed or partial audits are valid historical evidence too. To retain them after review, use `seal RECORD --reviewer NAME --preserve-incomplete --reason "WHY COVERAGE/FAILURES ARE RETAINED"`, then verify/archive normally. This explicit mode keeps status failed/partial and review_outcome=evidence-only, sets quality_accepted=false, and never makes the record an accepted performance baseline. It does not bypass raw-data, patch or hash integrity checks. Default sealing still requires complete passing coverage.
