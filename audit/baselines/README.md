# References and comparison

No application baseline exists yet. Accept a sealed reviewed campaign first and keep its archive and SHA-256 sidecar somewhere durable. A baseline descriptor should point at that record, its source commit, scenario/environment identity, accepted metric set, reviewer and reason. Preserve earlier descriptors rather than silently replacing references.

Compare sealed records with an explicit relative tolerance based on a same-code noise experiment:

```sh
python3 audit/scripts/audit.py compare /data/base-record /data/candidate-record --tolerance 0.10
```

0.10 is an invocation example, not a project-wide acceptance threshold. The comparison checks evidence hashes, complete review status, exact scenario/environment/build compatibility, metric identities/units/directions and repetition counts. It reports median relative changes and fails on a directional regression beyond the supplied tolerance. Descriptive metrics never gate. With zero base, a worsening directional value is reported as regression and relative change is null. Compatibility is deliberately strict; review a new profile rather than forcing unrelated runs to compare.

Known environmental limitations still require operator judgment: equal configuration does not prove equal temperatures/background load. Run paired/alternating measurements on the same machine and inspect captures. A comparison threshold is not a confidence interval. The initial harness computes nearest-rank summaries rather than claiming statistical significance from three observations.

## Calibrating a device reference

Collect at least two independent `device-paired` records on the same code with the same instrumentation and workload, each containing at least three measured repetitions per metric. Complete their manual review before sealing; collection alone cannot accept the picture, sound or physical target. Archive each run with the harness and keep the archive and retrieval receipt outside the repository. Then create a new descriptor:

```sh
python3 audit/scripts/baseline.py /data/device-base /data/device-noise --archives /data/device-base.tar.gz /data/device-noise.tar.gz --output /data/device-reference.json --reviewer maintainer --reason "Reviewed device capture and same-code noise"
```

The command checks both sealed records and their compatibility, requires the same source and instrumentation patch, and verifies that each archive identifies its selected record. For each directional metric it retains the largest relative median difference between records and the relative within-record range. The scalar comparison tolerance is the largest observed noise multiplied by an explicit review margin (`--margin`, default 1.25). The margin is a policy choice recorded in the descriptor, not a confidence claim. Review the resulting tolerance before using it: an excessively noisy metric or contaminated trial requires a new experiment rather than silently accepting a permissive threshold.

The initial paired scenario gates audio CPU and callback durations plus late/deadline counts. GUI timings, FPS, GPU engine activity and process CPU remain descriptive. An unchanging zero deadline counter is valid; a counter changing between zero and nonzero cannot be calibrated by relative tolerance and requires an absolute acceptance policy. Keep the descriptor's scope, accepted metric set, reviewer, source identity and verified archive hashes with the reference. Never overwrite a previous descriptor or edit a sealed record to make calibration pass.
