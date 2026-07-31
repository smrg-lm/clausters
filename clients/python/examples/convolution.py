#!/usr/bin/env python3
"""Convolution reverb: a prepared kernel through the partitioned `conv` UGen.

Runs from the *installed* package, offline, like ``spectral.py``::

    python -m venv .venv && . .venv/bin/activate
    pip install ./clients/python
    python clients/python/examples/convolution.py out.wav

The pipeline is the whole point:

1. The impulse response — here a synthetic one, exponentially decaying noise
   (~0.7 s), written to a WAV and loaded with ``Buffer.read`` — lives in an
   ordinary buffer.
2. ``kernel.gen("prepare_partconv", fft_size, ir.bufnum)`` partitions
   it and computes every partition's spectrum **once, off the audio thread**,
   into a second buffer sized with `partconv_frames`. The audio thread never
   transforms a kernel.
3. ``conv(sig, kernel.bufnum, ...)`` convolves against the ready spectra with
   a **flat** per-block cost: the partition products are spread across the
   hop, so a long reverb tail does not spike the block where the FFT lands.

The convolver has an intrinsic latency of ``fft_size / 2`` samples (the
partition length) — at 1024 that is ~10.7 ms, inaudible as the reverb's
predelay here. Left channel: the dry pluck. Right channel: the convolved
tail (100% wet, so the reverb is obvious).
"""

import math
import random
import struct
import sys
import tempfile
import wave
from pathlib import Path

from clausters import Session
from clausters.base import Routine
from clausters.defs import Buffer
from clausters.defs import (
    DoneAction,
    Env,
    Synth,
    SynthDef,
    control,
    conv,
    env_gen,
    out,
    partconv_frames,
    sine,
)

SR = 48000.0
FFT_SIZE = 1024
IR_SECONDS = 0.7


def write_ir(path: str) -> int:
    """A synthetic reverb impulse response: exponentially decaying noise,
    seeded for reproducibility. Returns its frame count."""
    rng = random.Random(2026)
    frames = int(SR * IR_SECONDS)
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(int(SR))
        w.writeframes(b"".join(
            struct.pack("<h", int(rng.uniform(-1.0, 1.0)
                                  * math.exp(-3.0 * k / frames) * 0.4 * 32767))
            for k in range(frames)
        ))
    return frames


def pluck(kernel_bufnum: int, partitions: int, name: str = "pluck") -> SynthDef:
    """A plucked tone, dry on bus 0 and fully wet (convolved) on bus 1."""
    freq = control("freq", 440.0)
    env = env_gen(Env.perc(attack=0.002, release=0.25),
                  done_action=DoneAction.NONE)
    sig = sine(freq) * env * 0.5
    wet = conv(sig, float(kernel_bufnum), fft_size=FFT_SIZE, partitions=partitions)
    # A convolution's gain is the IR's energy (sqrt of its summed squares —
    # here ~13x for 0.7 s of decaying noise), so the wet side takes a small
    # make-up gain; scale the IR itself instead when its level matters.
    return SynthDef(name, out(0.0, sig * 0.6), out(1.0, wet * 0.04))


def main():
    out_path = next((a for a in sys.argv[1:] if not a.startswith("-")), "convolution.wav")

    ir_path = str(Path(tempfile.gettempdir()) / "clausters_ir.wav")
    ir_frames = write_ir(ir_path)

    session = Session.nrt(tempo=2.0)
    server = session.server

    # 1. The raw IR, 2. the prepared kernel (spectra, computed off-RT).
    ir = Buffer.read(ir_path, server=server)
    partitions = -(-ir_frames // (FFT_SIZE // 2))
    kernel = Buffer.alloc(partconv_frames(ir_frames, FFT_SIZE), server=server)
    kernel.gen("prepare_partconv", FFT_SIZE, ir.bufnum)

    pluck(kernel.bufnum, partitions).send(server)

    def sequence():
        for midi, dur in [(69, 1.5), (64, 1.5), (71, 1.5), (69, 3.0)]:
            freq = 440.0 * 2.0 ** ((midi - 69.0) / 12.0)
            voice = Synth.new("pluck", {"freq": freq}, server=server)
            yield dur
            server.send_bundle(("/n_free", voice.id))

    Routine(sequence).play(session.clock)
    stats = session.render(sample_rate=SR, channels=2, path=out_path)

    peak = max(stats.peak, default=0.0)
    print(f"rendered {stats.frames} frames ({stats.duration:.2f} s) | peak {peak:.3f} "
          f"| IR {IR_SECONDS:.1f} s = {partitions} partitions")

    print(f"wrote {out_path} - listen with: pw-play {out_path}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError) as e:
        sys.exit(str(e))
