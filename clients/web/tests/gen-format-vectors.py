#!/usr/bin/env python3
"""Generate format-vectors.json: numbers as the Python client prints them.

Every record line both clients print (`info-vectors.json`) has numbers in it,
and a number is rounded by a rule: Python's format strings round the float's
exact binary value and take an exact tie to the even digit. The web client's
`src/base/format.ts` is that rule written again, in integers, because
JavaScript's own `toFixed` and `toPrecision` take a tie the other way -- `4.25`
printed `4.3` there and `4.2` here.

So the reference is the language itself, and this freezes what it says for a
spread chosen to find a wrong rule rather than to pass a right one:

- **exact ties**, at every count of decimals the formatters use and at `%g`'s
  six significant digits, in both parities of the digit before them;
- **values that only look like ties** once scaled (`0.05`, `0.15`, `1.005`),
  which a rounding done in floating point gets wrong;
- **the edges of `%g`**: where it starts writing an exponent, a rounding that
  carries into the next power of ten, zero and its negative, the largest and
  the smallest float;
- what is not a finite number;
- a seeded spread across magnitudes, and the same spread as **float32** --
  what a figure off the wire actually is.

A value travels as JSON, which writes a float as the shortest decimal that
reads back to the same bits; the three that JSON cannot say are written as
strings.

The JSON is committed; regenerate with:

    python3 gen-format-vectors.py

(from clients/web/tests/; it needs nothing but Python.)
"""

import json
import pathlib
import random
import struct

#: The counts of decimals the formatters ask for, and one more.
DIGITS = (0, 1, 2, 3)


def float32(value: float) -> float:
    """`value` as a single-precision float, which is what the wire carries."""
    return struct.unpack(">f", struct.pack(">f", value))[0]


def values() -> list:
    out = [
        # Exact ties, the digit before them even and odd.
        0.5, 1.5, 2.5, 3.5, 0.25, 0.75, 4.25, 4.75, 0.125, 0.375, 0.0625,
        0.3125, 100000.5, 100001.5, 262144.5, 123456.5, 1234565.0, 12345650.0,
        # Looks like a tie once scaled by ten, and is not one.
        0.05, 0.15, 0.35, 0.45, 1.005, 2.675, 0.0005, 1.0000005, 0.1234565,
        # The edges of the general format.
        0.0, -0.0, 1.0, 10.0, 99999.0, 999999.0, 999999.4, 999999.5, 1e6,
        1234567.0, 1e21, 1e22, 0.0001, 0.00012345678, 0.00009999995, 0.00001,
        0.000015, 9.9999995, 0.99999949, 0.9999995, 1e-7, 5e-324,
        1.7976931348623157e308, 2.2250738585072014e-308,
        # What the formatters are actually handed.
        48000.0, 44100.0, 96000.0, 0.1, 0.2, 1 / 3, 2 / 3, 440.0, 20000.0,
        47999.83, 0.002, 1.4, 12.34567,
    ]
    out += [-v for v in (0.25, 4.25, 0.05, 2.5, 100000.5, 0.04, 1e-5, 1234567.0)]
    rng = random.Random(20261007)
    for _ in range(300):
        v = rng.uniform(-1.0, 1.0) * 10.0 ** rng.randint(-9, 9)
        out += [v, float32(v)]
    # Halves and quarters at random scales: ties on purpose.
    for _ in range(120):
        out.append(rng.randint(-200000, 200000) / 2 ** rng.randint(1, 6))
    return out


def written(value: float):
    """A float as JSON can say it."""
    if value != value:
        return "nan"
    if value in (float("inf"), float("-inf")):
        return "inf" if value > 0 else "-inf"
    return value


def main() -> None:
    cases = []
    for value in values() + [float("nan"), float("inf"), float("-inf")]:
        cases.append({
            "value": written(value),
            # A negative zero does not survive JSON on every reader.
            "negativeZero": value == 0 and str(value).startswith("-"),
            "general": f"{value:g}",
            "fixed": {str(d): f"{value:.{d}f}" for d in DIGITS},
        })
    path = pathlib.Path(__file__).with_name("format-vectors.json")
    path.write_text(json.dumps(cases, indent=0) + "\n")
    print(f"wrote {path.name}: {len(cases)} values")


if __name__ == "__main__":
    main()
