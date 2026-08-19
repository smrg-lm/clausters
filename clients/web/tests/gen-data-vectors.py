#!/usr/bin/env python3
"""Generate data-vectors.json from the Python client's native core.

W10's analysis is shared with the Python client through `clausters-ffi`: the
peak pyramid, the stereo correlation and the Lissajous projection all exist on
both sides, so their values are frozen here from the reference client and
asserted in `tests/data.test.ts` against the TS client, which reaches the
*same* `clausters-core` through `crates/clausters-core-web`.

The peak cache is compared **as bytes**: both clients serialize the identical
cache, which is also the file the GUI host maps, so one hexadecimal digest
holds the whole format -- layout, level count and every min/max.

What is deliberately absent: the oscilloscope's trigger and the spectrum's
decibel curve. Python has no door on either (they are not in the C ABI), so
there is no second implementation to disagree with; `data.test.ts` asserts
their behaviour instead.

    python3 gen-data-vectors.py
"""

import hashlib
import json
import math
import pathlib
import sys
from array import array

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / "python"))
from clausters import _native  # noqa: E402


def sine(n, period, phase=0.0, amp=1.0):
    return [amp * math.sin(2.0 * math.pi * i / period + phase) for i in range(n)]


def signals():
    """The buffers every vector below is computed over, named so the TS side
    rebuilds them from the same recipe rather than from a wall of numbers."""
    return {
        "sine440": sine(4096, 109.09),          # ~440 Hz at 48 kHz
        "ramp": [i / 2048.0 - 1.0 for i in range(4096)],
        "quiet": sine(1024, 64.0, amp=0.001),
        "stereo_wide": None,                     # built from the two below
    }


def main():
    out = {}
    sines = signals()

    # ---- the peak pyramid: the cache bytes, mono and stereo ----
    peaks = []
    for name, samples, channels, base in [
        ("sine440", sines["sine440"], 1, 256),
        ("sine440", sines["sine440"], 1, 64),
        ("ramp", sines["ramp"], 1, 256),
        ("stereo", None, 2, 256),
        ("short", None, 1, 256),
    ]:
        if name == "stereo":
            left = sines["sine440"]
            right = sine(4096, 109.09, phase=0.7, amp=0.5)
            samples = [v for pair in zip(left, right) for v in pair]
        elif name == "short":
            samples = sines["quiet"][:100]
        cache = _native.peaks_cache(array("f", samples), base_bucket=base,
                                    channels=channels)
        peaks.append({
            "signal": name,
            "channels": channels,
            "baseBucket": base,
            "bytes": len(cache),
            "sha256": hashlib.sha256(cache).hexdigest(),
        })
    out["peaks"] = peaks

    # ---- the streamed overview: a cache filled from reports, not samples ----
    # `/buffer_stream` sends what the writer measured, and the receiver folds
    # it in. Both clients bind the same core door, so the cache a report leaves
    # must be the same bytes here and there -- and the same bytes a build over
    # the samples would leave, which is what makes a page's recording view
    # agree with the host's rather than merely resemble it.
    streams = []
    for name, samples, channels, base, start, count in [
        ("sine440", sines["sine440"], 1, 256, 0, 8),
        ("stereo", None, 2, 256, 1024, 4),
    ]:
        if name == "stereo":
            left = sines["sine440"]
            right = sine(4096, 109.09, phase=0.7, amp=0.5)
            samples = [v for pair in zip(left, right) for v in pair]
        frames = len(samples) // channels
        # The report, as the server measures it: bucket-major, channel-minor.
        stats = []
        for b in range(count):
            for ch in range(channels):
                at = start + b * base
                chunk = [samples[(at + i) * channels + ch] for i in range(base)]
                stats.append(min(chunk))
                stats.append(max(chunk))
                stats.append(sum(v * v for v in chunk) / len(chunk))
        silent = _native.peaks_cache(array("f", [0.0] * (frames * channels)),
                                     base_bucket=base, channels=channels)
        filled = _native.peaks_cache_write_buckets(silent, start, base, array("f", stats))
        streams.append({
            "signal": name,
            "channels": channels,
            "baseBucket": base,
            "startFrame": start,
            "stats": stats,
            "bytes": len(filled),
            "sha256": hashlib.sha256(filled).hexdigest(),
        })
    out["peaksStream"] = streams

    # ---- the stereo-field measurements ----
    pairs = []
    left = sines["sine440"][:1024]
    cases = [
        ("identical", left, left),
        ("inverted", left, [-v for v in left]),
        ("quarter_turn", left, sine(1024, 109.09, phase=math.pi / 2)),
        ("half_amplitude", left, [0.5 * v for v in left]),
        ("silence", left, [0.0] * 1024),
    ]
    # A length mismatch is left out on purpose: the Python client refuses it
    # before the FFI (a ValueError) while the wasm door reports it as an
    # absent value, so there is no shared number to freeze -- `data.test.ts`
    # asserts the TS side's own answer.
    for name, a, b in cases:
        r = _native.correlation(array("f", a), array("f", b))
        points = _native.lissajous(array("f", a), array("f", b))
        pairs.append({
            "case": name,
            "correlation": r,
            # The whole projection is long and dull; its first points and its
            # length pin the layout and the formula together.
            "points": len(points),
            "head": [list(p) for p in points[:8]],
        })
    out["stereoField"] = pairs

    path = pathlib.Path(__file__).with_name("data-vectors.json")
    path.write_text(json.dumps(out, indent=2) + "\n")
    print(f"wrote {path} ({len(out['peaks'])} caches, "
          f"{len(out['peaksStream'])} streamed, {len(out['stereoField'])} pairs)")


if __name__ == "__main__":
    main()
