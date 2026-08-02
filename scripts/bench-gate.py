#!/usr/bin/env python3
"""Compare two `bench --json` runs and fail on a regression.

    cargo run --release --example bench -- --json > base.json   # the merge base
    cargo run --release --example bench -- --json > head.json   # this branch
    scripts/bench-gate.py base.json head.json

**The comparison is only meaningful when both numbers come off the same
machine, back to back.** A shared CI runner swings 10-20% between runs of
identical code, so an absolute threshold against a committed number would fire
on the weather; the ratio of two measurements that shared the machine, the
thermal state and the noise does not.

Two columns carry the claim, and they answer different questions:

* `x_real_time` -- throughput. How many copies of that graph fit in one
  callback budget.
* `peak_block` -- the worst single block, which an average hides. Only the
  spectral rows have one: an FFT chain concentrates its work on the block where
  the hop closes, and that spike is what makes a callback late.

The thresholds are deliberately loose, and they differ per metric because the
two metrics have different noise. Measured here over three runs of an identical
build (210 throughput comparisons, 30 peak comparisons):

    x_real_time             median 0.5%   p95 1.8%   worst  5.6%
    peak_block, aligned                                worst 21.0%  (n >= 32)
    peak_block, staggered                              worst 34.6%  (n >= 32)

So throughput is gated at 10% and the peak block at 50%, each roughly twice its
observed worst case -- what it takes to catch a cliff (a lost inline, an
allocation that crept into the block path) without firing on the weather. A 3%
drift is invisible to this on purpose; on a shared runner it is
indistinguishable from nothing.

A row that says `"gated": false` is printed and never fails the build. Four
kinds are, and each for a measured reason: the Faust rows (their cost includes
whatever the LLVM JIT decided that run), the worker sweep (it reads the
machine's core count), the spectral peak at one voice (a single hop's spike --
two runs of the same build differed by 250%), and the **staggered** spectral
peak at any size, which measures whether two chains' hops happened to collide
this run. The aligned peak is the deliberate worst arrangement, so it is a
property of the code and is gated.
"""

import argparse
import json
import pathlib
import sys

#: A gated row fails the build when it is this much worse than the base, per
#: metric. Generous on purpose -- tighten only if the observed spread on the
#: runner turns out narrower than the numbers in the docstring above.
THRESHOLDS = {"x_real_time": 0.10, "peak_block": 0.50}


def load(path):
    rows = json.loads(pathlib.Path(path).read_text())
    return {row["name"]: row for row in rows}


def regressions(base, head):
    """(name, metric, before, after, change) for every row that moved, worst
    first. `change` is negative for a regression in both metrics, so one sign
    convention reads for throughput (higher is better) and for the peak block
    (lower is better)."""
    out = []
    for name, after in head.items():
        before = base.get(name)
        if before is None:
            continue  # a row this branch adds: nothing to compare it against
        for metric, better_is_higher in (("x_real_time", True), ("peak_block", False)):
            a, b = before.get(metric), after.get(metric)
            if a is None or b is None or a == 0:
                continue
            change = (b - a) / a if better_is_higher else (a - b) / a
            out.append((name, metric, a, b, change, after.get("gated", True)))
    out.sort(key=lambda r: r[4])
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("base", help="bench --json from the merge base")
    ap.add_argument("head", help="bench --json from this branch")
    ap.add_argument("--scale", type=float, default=1.0,
                    help="multiply both thresholds (1.0 = as measured)")
    args = ap.parse_args()

    limits = {m: t * args.scale for m, t in THRESHOLDS.items()}
    base, head = load(args.base), load(args.head)
    missing = sorted(set(base) - set(head))
    rows = regressions(base, head)
    failures = [r for r in rows if r[5] and r[4] < -limits[r[1]]]

    def line(name, metric, a, b, change, gated):
        mark = "  " if gated else " *"
        unit = "x" if metric == "x_real_time" else "us"
        return (f"{mark} {name:<28} {metric:<12} {a:>10.2f}{unit} -> {b:>10.2f}{unit}"
                f"  {change * 100:+6.1f}%")

    print(f"{len(rows)} measurements compared; thresholds "
          + ", ".join(f"{m} {v * 100:.0f}%" for m, v in sorted(limits.items())))
    if missing:
        # Not a failure: a row can legitimately disappear (a section removed, a
        # feature not compiled in). It is reported because the alternative is a
        # gate that silently stops covering things.
        print(f"\nrows in the base and not in the head ({len(missing)}): "
              + ", ".join(missing))

    worst = [r for r in rows if r[4] < -0.02][:12]
    if worst:
        print("\nmoved against the base (worst first; * = reported, not gated):")
        for r in worst:
            print(line(*r))

    best = [r for r in rows if r[4] > 0.02][-5:]
    if best:
        print("\nmoved in favour:")
        for r in reversed(best):
            print(line(*r))

    if failures:
        print(f"\nFAIL: {len(failures)} gated measurement(s) past their threshold")
        for r in failures:
            print(line(*r))
        return 1
    print("\nOK: no gated measurement regressed past its threshold")
    return 0


if __name__ == "__main__":
    sys.exit(main())
