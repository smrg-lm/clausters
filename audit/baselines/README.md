# References and comparison

No application baseline exists yet. Accept a sealed reviewed campaign first and keep its archive and SHA-256 sidecar somewhere durable. A baseline descriptor should point at that record, its source commit, scenario/environment identity, accepted metric set, reviewer and reason. Preserve earlier descriptors rather than silently replacing references.

Compare sealed records with an explicit relative tolerance based on a same-code noise experiment:

```sh
python3 audit/scripts/audit.py compare /data/base-record /data/candidate-record --tolerance 0.10
```

0.10 is an invocation example, not a project-wide acceptance threshold. The comparison checks evidence hashes, complete review status, exact scenario/environment/build compatibility, metric identities/units/directions and repetition counts. It reports median relative changes and fails on a directional regression beyond the supplied tolerance. Descriptive metrics never gate. With zero base, a worsening directional value is reported as regression and relative change is null. Compatibility is deliberately strict; review a new profile rather than forcing unrelated runs to compare.

Known environmental limitations still require operator judgment: equal configuration does not prove equal temperatures/background load. Run paired/alternating measurements on the same machine and inspect captures. A comparison threshold is not a confidence interval. The initial harness computes nearest-rank summaries rather than claiming statistical significance from three observations.
