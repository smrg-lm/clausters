#!/usr/bin/env python3
"""Generate clock-vectors.json from the Python client's native core.

The Python client (`clausters._native`) is the reference for everything the
sequencing layer computes: beat/second/sample arithmetic, the bar grid, NTP
timetags, the pitch space, the seeded value stream and the builtins. This
script freezes those values so the TS client -- which reaches the *same*
`clausters-core` through `crates/clausters-core-web` -- can assert parity in
`tests/clock-parity.test.ts`. The JSON is committed; regenerate with:

    python3 gen-clock-vectors.py

Every number is written as the Python float it is: `repr` round-trips a
double exactly, and JSON.parse reads back the same bits.
"""

import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / "python"))
from clausters import _native  # noqa: E402
from clausters.base import builtins  # noqa: E402

# (tempo, base_beats, base_secs, value) -- an affine clock and a position on
# it. The tempo changes pin an instant, hence the non-zero bases.
CLOCKS = [
    (1.0, 0.0, 0.0, 4.0),
    (2.0, 0.0, 0.0, 3.5),
    (0.5, 0.0, 0.0, 1.25),
    (2.0, 8.0, 4.0, 12.0),
    (1.75, 3.25, 1.5, 0.0),
    (120 / 60, 0.0, 0.0, 64.0),
]

# (secs, sample_rate) -- including the tie-to-even boundary.
SAMPLES = [
    (1.0, 48000.0),
    (0.5, 44100.0),
    (0.0000104166666, 48000.0),
    (1.5e-5, 48000.0),   # 0.72 samples
    (2.5 / 48000.0, 48000.0),
    (-0.25, 48000.0),
    (3600.0, 48000.0),
]

# (position, quant) -- the start-quantization grid.
QUANTS = [
    (0.0, 4.0), (1.0, 4.0), (3.999, 4.0), (4.0, 4.0), (7.5, 4.0),
    (2.3, 1.0), (5.0, 0.0), (-1.5, 4.0), (10.0, 3.0),
]

# (degree, octave, root, scale) -- the pitch space Events resolve in.
MAJOR = [0, 2, 4, 5, 7, 9, 11]
MINOR = [0, 2, 3, 5, 7, 8, 10]
DEGREES = [
    (0.0, 5.0, 0.0, MAJOR),
    (7.0, 5.0, 0.0, MAJOR),
    (-1.0, 5.0, 0.0, MAJOR),
    (3.0, 5.0, 2.0, MAJOR),
    (2.0, 4.0, 0.0, MINOR),
    (13.0, 6.0, -3.0, MINOR),
    (3.0, 5.0, 0.0, []),
]

UNARY = ["midicps", "cpsmidi", "midiratio", "ratiomidi", "dbamp", "ampdb",
         "octcps", "cpsoct", "squared", "cubed", "reciprocal", "distort",
         "softclip", "log2", "tanh", "frac", "sign"]
UNARY_INPUTS = [60.0, 69.0, 440.0, 0.5, -0.5, 1.5, 3.0, -12.0]

BINARY = ["add", "sub", "mul", "div", "pow", "min", "max", "round", "trunc",
          "wrap2", "fold2", "clip2", "absdif", "thresh", "hypot"]
BINARY_INPUTS = [(3.0, 2.0), (-1.5, 0.25), (440.0, 1.5), (0.7, 0.3)]

# The unix timestamps stamped into bundle timetags, and the /clock_query anchors a
# sample-locked client schedules through.
UNIX_TIMES = [0.0, 1.0, 1234567890.5, 1753500000.125]
ANCHORS = [
    # (unix, anchor_unix, anchor_sample, rate)
    (100.5, 100.0, 4_800_000, 48000.0),
    (100.0, 100.0, 4_800_000, 48000.0),
    (99.75, 100.0, 4_800_000, 48000.0),
    (7200.25, 7200.0, 345_600_000, 44100.0),
]

SEEDS = [1, 12345, 2**31]


def jnum(value):
    """A float as JSON can carry it: the number itself, or a tag for the
    non-finite results (JSON has no NaN/Infinity literal)."""
    value = float(value)
    if value != value:
        return "nan"
    if value == float("inf"):
        return "inf"
    if value == float("-inf"):
        return "-inf"
    return value


def rng_run(seed):
    """The first draws of a seeded stream, and of a stream spawned from it --
    the derivation a routine's own generator is built by."""
    stream = _native.RngStream(seed)
    floats = [stream.next_f64() for _ in range(6)]
    below = [stream.next_below(n) for n in (2, 7, 16, 1, 0)]
    child = stream.spawn()
    child_floats = [child.next_f64() for _ in range(4)]
    after_spawn = [stream.next_f64() for _ in range(2)]
    return {
        "seed": seed,
        "floats": floats,
        "below": below,
        "childFloats": child_floats,
        "afterSpawn": after_spawn,
        "uniform": [stream.uniform(-1.0, 1.0) for _ in range(3)],
    }


def main():
    vectors = {
        "beatsToSecs": [
            {"tempo": t, "baseBeats": bb, "baseSecs": bs, "beats": v,
             "secs": _native.beats_to_secs(t, bb, bs, v)}
            for (t, bb, bs, v) in CLOCKS
        ],
        "secsToBeats": [
            {"tempo": t, "baseBeats": bb, "baseSecs": bs, "secs": v,
             "beats": _native.secs_to_beats(t, bb, bs, v)}
            for (t, bb, bs, v) in CLOCKS
        ],
        "secsToSamples": [
            {"secs": s, "rate": r, "samples": _native.secs_to_samples(s, r)}
            for (s, r) in SAMPLES
        ],
        "samplesToSecs": [
            {"samples": n, "rate": r, "secs": _native.samples_to_secs(n, r)}
            for (n, r) in [(48000, 48000.0), (22050, 44100.0), (1, 48000.0),
                           (0, 48000.0), (-48000, 48000.0)]
        ],
        "quantDelay": [
            {"pos": p, "quant": q, "delay": _native.quant_delay(p, q)}
            for (p, q) in QUANTS
        ],
        "bar": [
            {"beats": p, "quant": q, "bar": _native.bar(p, q),
             "beatInBar": _native.beat_in_bar(p, q)}
            for (p, q) in QUANTS
        ],
        "unixToNtp": [
            # A u64 of wire bits: carried as a decimal string, read back as a
            # BigInt (a JS number would drop the low bits).
            {"unix": u, "ntp": str(_native.unix_to_ntp(u))}
            for u in UNIX_TIMES
        ],
        "unixToSample": [
            {"unix": u, "anchorUnix": au, "anchorSample": asm, "rate": r,
             "sample": _native.unix_to_sample(u, au, asm, r)}
            for (u, au, asm, r) in ANCHORS
        ],
        "degreeToMidinote": [
            {"degree": d, "octave": o, "root": r, "scale": s,
             "midinote": _native.degree_to_midinote(d, o, r, s)}
            for (d, o, r, s) in DEGREES
        ],
        # The op *names* are the core's own (`UnaryOp::name`), which is what
        # the TS door takes; the Python client spells its functions the same.
        # A non-finite result is real behaviour worth pinning (log of a
        # negative, cpsmidi of zero), so it rides as a tag JSON can carry.
        "unary": [
            {"op": op, "x": x, "y": jnum(getattr(builtins, op)(x))}
            for op in UNARY for x in UNARY_INPUTS
        ],
        "binary": [
            {"op": op, "a": a, "b": b, "y": jnum(getattr(builtins, op)(a, b))}
            for op in BINARY for (a, b) in BINARY_INPUTS
        ],
        "rng": [rng_run(seed) for seed in SEEDS],
    }
    out = pathlib.Path(__file__).with_name("clock-vectors.json")
    out.write_text(json.dumps(vectors, indent=1) + "\n")
    total = sum(len(v) for v in vectors.values())
    print(f"wrote {out.name}: {total} vectors")


if __name__ == "__main__":
    main()
