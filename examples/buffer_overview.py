#!/usr/bin/env python3
"""A picture of a take, without the take: `/buffer_peaks`.

Drawing a long recording is a problem of **bandwidth**, not of arithmetic. A
ten-minute stereo take is 230 MB of samples, and a client that cannot map the
server's memory — a browser above all — has to get a picture of it from
somewhere. There are three answers and this script measures two of them
against each other:

1. **Download the samples** (`/buffer_getRange`, `get_samples`) and summarize
   them locally. Exact, and the size of the audio.
2. **Ask for the overview** (`/buffer_peaks`, `peaks`): min, max and mean
   square per bucket of 256 frames, which is the peak pyramid's own base level
   in its own energy form. About a hundredth of the bandwidth, enough to draw
   the whole take at once, and the spans under a zoom read back with
   `get_samples` as they are needed.

(The third is `/buffer_stream`, and it is not an alternative but the other
half: a buffer being *recorded into* has its overview pushed as it is written,
because nothing can ask for what is not there yet. Same blob, same folding.)

What the script does: builds a take of four sections with different levels,
asks for both, prints what each cost, and draws the overview as text — then
draws the same picture out of the downloaded samples and asserts the two are
the same. That is the point worth checking by eye: the cheap route is not an
approximation of the expensive one, it is the same summary computed where the
samples already are.

Needs no audio device (nothing is played; the server boots headless). Run it:

    python3 examples/buffer_overview.py

`docs/schemas.md` (the buffer commands) has the wire form of both replies.
"""

import math
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, os.path.join(ROOT, "clients", "python"))

from clausters import Session  # noqa: E402
from clausters.defs import Buffer  # noqa: E402

SR = 48000
SECONDS = 20
BUCKET = 256
CHANNELS = 2
# Four sections, so the picture has something to be right about.
LEVELS = [0.9, 0.25, 0.0, 0.6]


def samples():
    """A tone whose level steps through `LEVELS`, interleaved stereo with the
    right channel at half the left — so a wrong channel order shows as a
    picture that is symmetric when it should not be."""
    out = []
    section = (SECONDS * SR) // len(LEVELS)
    for i in range(SECONDS * SR):
        level = LEVELS[min(i // section, len(LEVELS) - 1)]
        value = level * math.sin(2 * math.pi * 220.0 * i / SR)
        out.append(value)
        out.append(value * 0.5)
    return out


def trace(stats, channels, columns=64):
    """The overview as text: one column per `columns`, the peak of the buckets
    that fall in it, on the left channel.

    This is the drawing a waveform view does, minus the pixels — a column is a
    range of buckets and what it shows is their extreme, which is why a
    summary can be drawn at any zoom without touching the samples.
    """
    buckets = len(stats) // (channels * 3)
    if buckets == 0:
        return "(nothing to draw)"
    line = []
    for c in range(columns):
        first = (c * buckets) // columns
        last = max(first + 1, ((c + 1) * buckets) // columns)
        peak = 0.0
        for b in range(first, last):
            at = (b * channels) * 3          # left channel, bucket-major
            peak = max(peak, abs(stats[at]), abs(stats[at + 1]))
        line.append(" .:-=+*#@"[min(8, int(peak * 8.999))])
    return "".join(line)


def local_overview(audio, channels, bucket):
    """The same summary computed from the downloaded samples, in the same
    layout the server answers in — so the two can be compared value by value.
    """
    buckets = (len(audio) // channels) // bucket
    stats = []
    for b in range(buckets):
        for ch in range(channels):
            run = [audio[(b * bucket + f) * channels + ch] for f in range(bucket)]
            stats.append(min(run))
            stats.append(max(run))
            stats.append(sum(s * s for s in run) / bucket)
    return stats


def main():
    with Session.live() as session:
        buf = Buffer.alloc(SECONDS * SR, CHANNELS)
        buf.set_samples(samples())
        frames = SECONDS * SR
        print(f"a take of {SECONDS} s, {CHANNELS} channels: "
              f"{frames * CHANNELS * 4 / 1e6:.1f} MB of samples")

        start, bucket, stats = buf.peaks(BUCKET)
        overview_bytes = len(stats) * 4
        print(f"  /buffer_peaks  -> {len(stats) // (CHANNELS * 3)} buckets from frame "
              f"{start} at {bucket}: {overview_bytes / 1e3:.1f} kB")

        audio = buf.get_samples()
        audio_bytes = len(audio) * 4
        print(f"  /buffer_getRange -> {len(audio)} samples: {audio_bytes / 1e6:.1f} MB "
              f"({audio_bytes / overview_bytes:.0f}x the overview)")

        print("\nthe take, drawn from the overview alone (left channel):")
        print("  " + trace(stats, CHANNELS))

        # And the check that makes the ratio above worth anything: the cheap
        # route is not a coarser picture, it is the same one.
        mine = local_overview(audio, CHANNELS, bucket)
        print("the same, computed here from every sample:")
        print("  " + trace(mine, CHANNELS))
        worst = max(abs(a - b) for a, b in zip(stats, mine))
        print(f"\nlargest disagreement between the two: {worst:.2e}")
        assert worst < 1e-6, "the server's summary and ours must be the same summary"

        # A span, on the bucket grid the answer is folded into: a view that
        # zoomed into a section asks for that section's buckets and nothing
        # else -- which is the same command, and the reason it takes a span at
        # all. The second section, so the drawing has something in it.
        third = frames // len(LEVELS)
        _, _, part = buf.peaks(BUCKET, start=third, frames=third)
        print(f"one section of it ({third} frames): "
              f"{len(part) // (CHANNELS * 3)} buckets, "
              f"{len(part) * 4 / 1e3:.1f} kB")
        print("  " + trace(part, CHANNELS))
        # The first column is the loud section, and it should be: the section
        # does not start on a bucket boundary, so the answer starts at the
        # bucket the request falls **inside** rather than at the next one. That
        # is the rounding the command promises, and the reason it is down: a
        # bucket summarized from part of itself would report a peak the samples
        # do not have.
        buf.free()


if __name__ == "__main__":
    main()
