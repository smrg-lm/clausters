#!/usr/bin/env python3
"""The high-level client: pattern sequencing with one seam for NRT and live (C9).

A guided tour of the Python client's sequencing layer — the part a musician
actually touches. The idea ported from sc3: a `Pbind` combines per-key value
patterns into a stream of note `Event`s, and an `EventStreamPlayer` plays them
on a `TempoClock`, emitting each at its exact logical beat. The **seam**: the
*same* pattern runs offline (NRT, accumulating a score the embed renderer turns
into samples) or live (RT, over UDP to a running server) depending only on which
`Server` interface the `Session` holds — the routine never changes.

`Session` bundles a `Server` + `TempoClock` with explicit, no-global ergonomics
(`Session.nrt(...)` / `Session.live(...)`), so an offline session for plotting
and a live one can coexist in one script.

Run offline (no server, renders to samples; needs the embed library):

    cargo build --release --features embed,realtime
    python3 examples/sequencing.py [out.wav]

Run live (needs a server in another terminal):

    cargo run --release                          # terminal 1
    python3 examples/sequencing.py --live        # terminal 2
"""

import os
import struct
import sys
import wave

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "clients", "python"))

from clausters import Session
from clausters.seq import Pbind, Pseq, Pwhite

SR = 48000.0


def melody() -> Pbind:
    """A two-bar phrase. Each key is its own value pattern; `Pbind` zips them
    into `Event`s and stops when the shortest finite one is exhausted.

    - `degree` walks a scale (Event maps degree -> a major-scale midinote ->
      freq, all in the shared native core, so it matches the server);
    - `dur` is the beats between notes (also the note length before release);
    - `amp` jitters a little via `Pwhite` for life.
    """
    return Pbind(
        degree=Pseq([0, 1, 2, 3, 4, 3, 2, 1], repeats=1),   # finite -> the Pbind ends
        dur=0.25,
        amp=Pwhite(0.08, 0.18),
    )


def render_offline(path: str | None):
    """Play the phrase into an NRT session and render it to samples."""
    session = Session.nrt(tempo=2.0)        # 2 beats/sec
    session.play(melody())                  # schedule the pattern on its clock
    samples, frames = session.render(sample_rate=SR, channels=2)

    peak = max(abs(s) for s in samples)
    print(f"rendered {frames} frames ({frames / SR:.2f} s) | peak {peak:.3f}")

    if path:
        with wave.open(path, "wb") as w:
            w.setnchannels(2)
            w.setsampwidth(2)
            w.setframerate(int(SR))
            w.writeframes(b"".join(
                struct.pack("<h", int(max(-1.0, min(1.0, s)) * 32767)) for s in samples
            ))
        print(f"wrote {path} - listen with: ffplay -autoexit {path}")


def play_live():
    """The exact same pattern, live over UDP. Only the session differs: a live
    `Server` instead of an NRT one. `run(seconds)` advances the clock in real
    time, then stops; the synths free themselves after each note's sustain."""
    with Session.live(tempo=2.0, latency=0.1) as session:
        session.play(melody())
        session.run(2.0)                    # 8 notes * 0.25 beat / 2 bps = 1.0 s + tail
        print("played live; synths free themselves after their sustain")


def main():
    if "--live" in sys.argv[1:]:
        play_live()
    else:
        out = next((a for a in sys.argv[1:] if not a.startswith("-")), None)
        render_offline(out)


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
