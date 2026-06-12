# Parallel groups (`/g_parallel` and `--workers`)

A graph that no longer fits in one core can run its independent parts on
several: Clausters can process the children of a marked group in parallel
**stages** derived from the same bus analysis that powers the auto-sorted
groups ([`auto-order.md`](auto-order.md)). The result is the analogue of
supernova's `ParGroup`, except the independence is **inferred and
verified** by the engine instead of promised by the user — a wrong
declaration cannot corrupt audio, it just serializes.

## Usage

```sh
clausters --workers 3                       # real-time server, 3 DSP threads
clausters --nrt score.osc out.wav --workers 3   # faster offline renders too
```

```text
/g_parallel groupID mode    # 1 = process children in stages, 0 = strict order
```

Without `--workers` (the default) everything stays sequential; `/g_parallel`
is then a no-op flag, accepted and remembered. The flag is schedulable in
timed bundles and valid in NRT scores, like `/g_sortMode`.

The layout that benefits is *independent chains*: subgroups (voices,
mixer channels) under one parallel group, each chain writing its own buses.
Each child of the parallel group — synth or whole subgroup — is one unit;
units that touch disjoint buses run concurrently.

## How stages are formed

Per processing block the engine scans the group's children **in order**,
batching them greedily into a stage while each next unit:

- writes nothing the stage already reads or writes, and
- reads nothing the stage writes.

A conflicting unit closes the stage and opens the next one, so anything
that *must* be ordered (two writers summing into the same bus, a reader
after its writer) is automatically serialized — same-bus mixing keeps the
child order, exactly like sequential execution. A **dynamic** unit (a bus
index computed by a signal: the engine cannot know what it touches) always
runs alone. Nested parallel groups dispatched to a worker run sequentially
inside it (parallelism does not nest in v1).

The masks come from the same per-def analysis as M12 (`In`/`Out`/
`ReplaceOut` and the Faust reserved `in`/`out` buses, constants and
controls; `/n_set` on a control used as a bus index re-ships the masks),
but the copy the scheduler trusts lives **in the engine**, shipped with
each synth — stage safety never depends on network-side state.

## Determinism

Parallel rendering is **bit-identical** to sequential rendering. Stage
members touch pairwise disjoint buses and read nothing the stage writes, so
their results are independent of worker interleaving; stages and the
serialized conflicts preserve child order. `tests/parallel.rs` pins this
down for live engines and NRT renders, including across an `/n_set` that
retargets a bus. Goldens, the RT/NRT sample-identity guarantee and the
`--workers` flag therefore compose: workers only change wall-clock time.

## Workers and the real-time path

`--workers N` spawns N DSP threads at startup. The audio thread (the
conductor) publishes each stage with a few atomic stores, the workers and
the conductor race through it with an atomic work-stealing cursor, and the
conductor spin-waits (bounded, no locks) for completion. Idle workers spin
briefly, then yield, then park; waking a parked worker costs one `unpark`
syscall when audio resumes — while audio runs hot they never park. The
conductor path allocates nothing (`tests/rt_safety.rs` covers it,
parallel dispatch included).

Measure before reaching for it: `cargo run --release --example bench` ends
with a `/g_parallel` section (8 chains × 125 sines on disjoint buses)
comparing worker counts — on the development machine it peaks around 3×
with 3 workers and degrades past the physical core count. A graph that
already fits in one core gains nothing; a stage with fewer units than
workers caps the speedup at the unit count.
