#!/usr/bin/env python3
"""Bounce a phrase offline, then play it back from the file the server wrote.

Runs from the *installed* package; it boots its own server, so nothing has to
be running first::

    python -m venv .venv && . .venv/bin/activate
    pip install ./clients/python
    python clients/python/examples/render_then_load.py

This is the **change of state** the `render` verb is named for: a generator
thing (a pattern — an algorithm that describes sound) becomes a generated one
(a file — random-access audio you can slice, loop and read backwards). The
point of the example is that the file needs no conversion in between, because
the same process wrote it and reads it:

1. An NRT session plays the phrase and renders it with ``path=``. **The
   server writes the WAV**: the score goes to the ``clausters --nrt``
   renderer, which streams straight to disk, so the samples never cross into
   Python. What comes back is the stats — frames, events, per-channel peak
   and RMS — measured while it streamed.
2. A live session boots, and ``read_buffer`` (``/buffer_allocRead``) loads that
   same file into a server buffer. Float32 WAV, read by the decoder the
   server already ships; nothing is converted to int16 on the way.
3. A `play_buf` synth plays the buffer — the take, sounding as a sample.
4. For comparison, `read_soundfile` brings the samples into the client
   through *that same decoder*, and the numbers match the render's own stats.

The two ends of step 4 are worth noticing: the stdlib `wave` module cannot
read a float32 WAV at all, which is exactly why the client borrows the
server's reader instead of growing one.
"""

import os
import sys
import tempfile

from clausters import Session
from clausters.defs import SynthDef, control, out
from clausters.defs.ugens import play_buf
from clausters.render import channels, read_soundfile
from clausters.seq import Pbind, Pseq, Pwhite
from clausters.defs import Synth
from clausters.defs import Buffer

SR = 48_000.0


def phrase():
    """A short arpeggio — the thing being bounced."""
    return Pbind(instrument="default", dur=0.25, legato=0.9, amp=0.22,
                 freq=Pseq([220.0, 277.2, 330.0, 440.0, 330.0, 277.2], 2),
                 pan=Pwhite(-0.6, 0.6))


def sampler(name: str = "take") -> SynthDef:
    """Plays the buffer's two channels straight out.

    `play_buf` is mono -- one call per channel, selected by `chan` -- which is
    also a reminder that a server buffer is interleaved: the UGen walks the
    stride, exactly as `clausters.render.channels` does on the client side.
    """
    bufnum = control("bufnum", 0.0)
    rate = control("rate", 1.0)
    amp = control("amp", 1.0)
    left = play_buf(bufnum, 0.0, rate)
    right = play_buf(bufnum, 1.0, rate)
    return SynthDef(name, out(0.0, left * amp), out(1.0, right * amp))


def main():
    tmp = tempfile.mkdtemp(prefix="clausters-take-")
    wav = os.path.join(tmp, "phrase.wav")

    # ---- 1. bounce it: the server writes the file, we keep the stats -------
    offline = Session.nrt(tempo=2.0)
    offline.play(phrase())
    stats = offline.render(sample_rate=SR, channels=2, path=wav)
    print(f"rendered {stats.events} score events -> {stats.frames} frames "
          f"({stats.duration:.2f} s) at {stats.sample_rate:.0f} Hz")
    print(f"  peak {tuple(round(p, 3) for p in stats.peak)}  "
          f"rms {tuple(round(r, 3) for r in stats.rms)}")
    print(f"  samples in this process: {stats.samples}  (they went to the file)")
    print(f"  wrote {wav} ({os.path.getsize(wav)} bytes, float32)")

    # ---- 2. the client can read it back, through the server's decoder -----
    audio = read_soundfile(wav)
    left, right = channels(audio.samples, audio.channels)
    print(f"\nread back: {audio.frames} frames x {audio.channels} ch, "
          f"{len(left)} + {len(right)} samples deinterleaved")
    same = all(abs(a - b) < 1e-9 for a, b in zip(audio.peak, stats.peak))
    print(f"  peak matches the render's own measurement: {same}")

    # ---- 3. load it into a running server and play it ---------------------
    with Session.live(tempo=2.0, latency=0.15) as live:
        buf = Buffer.read(wav, server=live.server)          # /buffer_allocRead, by content
        info = buf.info()
        print(f"\nbuffer {buf.bufnum} on the server: {info.frames} frames, "
              f"{info.channels} ch, {info.sample_rate:.0f} Hz")

        sampler().send(live.server)
        node = Synth("take", {"bufnum": float(buf.bufnum)}, server=live.server)
        print("playing the take back from the buffer...")
        live.run(stats.duration + 0.5)
        node.free()

    print("\ndone - the bounce, the file and the playback were one artifact.")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError) as e:
        sys.exit(str(e))
