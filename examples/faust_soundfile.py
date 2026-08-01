#!/usr/bin/env python3
"""Read a server buffer from *inside* a Faust def, with `soundfile`.

Faust's `soundfile("<bufnum>", n)` primitive binds to the server buffer whose
index is its (integer) label, so a Faust DSP can read sample memory directly --
no `PlayBuf`, no audio bus in between. This example loads a short WAV into a
buffer with ``/buffer_allocRead`` and loops it from a Faust def that reads it through
`soundfile`, all driven by the Python client's own resources (buffer allocator,
def sending with the ``/done`` barrier, live ``/node_set`` automation).

Two things worth knowing:

  * **`soundfile` lives in Faust source, not in the signal-tree builder.** The
    `clausters.defs.signals` API (the lowercase `Signal` callables) has no
    `soundfile` op, so the def is built with :meth:`FaustDef.from_source` -- a
    plain Faust program whose ``process`` reads the buffer. The label baked into
    the source *is* the buffer index the client allocated.

  * **The bind is a snapshot taken at ``/synth_new``.** When a synth instantiates,
    its `soundfile` is filled from the buffer's *current* contents (deinterleaved
    into the instance's own memory). Reloading the buffer afterwards does not
    touch a voice already playing -- re-``/synth_new`` to pick up the new samples.
    The example shows this: it reloads the buffer mid-play and spawns a second
    voice, which reads the new motif while the first keeps playing the old one.

This is a **live RT** demo (live playback, ``/node_set`` automation, overlapping
voices), so it needs a server built with the Faust feature. (``soundfile`` reads
the buffer mirror at ``/synth_new``; that mirror is wired both on the live server and
in the offline NRT renderer, so a `soundfile` def renders offline too -- this
example just shows the live side.) Build the server, then run this; it starts and
stops its own::

    cargo build --release --features faust
    python3 examples/faust_soundfile.py

Point it at a prebuilt binary with ``CLAUSTERS_BIN=/path/to/clausters``.
"""

import math
import os
import struct
import subprocess
import sys
import tempfile
import time
import wave

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "clients", "python"))

from clausters.defs import FaustDef, Server, ServerOptions
from clausters.defs.buffer import Buffer
from clausters.errors import CommandError
from clausters.defs import Synth

REPO = os.path.join(os.path.dirname(__file__), "..")
BIN = os.environ.get("CLAUSTERS_BIN", os.path.join(REPO, "target", "release", "clausters"))


# --------------------------------------------------------------------------
# Server lifecycle (same pattern as examples/live_patch.py).
# --------------------------------------------------------------------------


def launch(options: ServerOptions, *extra_args: str) -> subprocess.Popen:
    if not os.path.exists(BIN):
        sys.exit(f"server binary not found at {BIN}\n"
                 "build it with: cargo build --release --features faust\n"
                 "(or set CLAUSTERS_BIN)")
    return subprocess.Popen([BIN, *options.args(), *extra_args])


def wait_until_ready(server: Server, timeout: float = 8.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            return server.query_info(timeout=0.3)
        except Exception:
            time.sleep(0.2)
    raise RuntimeError("server did not come up in time")


# --------------------------------------------------------------------------
# Buffers: a short recognizable motif, loaded server-side.
# `soundfile` does *not* resample, so write the WAV at the server's rate.
# --------------------------------------------------------------------------


def write_motif_wav(path: str, sr: float, freqs, note_dur: float = 0.18) -> int:
    """Concatenate decaying-sine notes (one per frequency) into a mono WAV.
    Returns the total frame count."""
    per = int(sr * note_dur)
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(int(sr))
        frames = bytearray()
        for f in freqs:
            for i in range(per):
                t = i / sr
                s = 0.7 * math.exp(-5.0 * t) * math.sin(2 * math.pi * f * t)
                frames += struct.pack("<h", int(max(-1.0, min(1.0, s)) * 32767))
        w.writeframes(bytes(frames))
    return per * len(freqs)


def _alloc_read(server: Server, bufnum: int, path: str):
    """``/buffer_allocRead`` for a known index, blocking on ``/done``. (This example
    reloads the *same* index mid-play, which no constructor does: `Buffer.read`
    allocates a fresh one, so the reload goes over ``Server.request``.)"""
    addr, args = server.request("/buffer_allocRead", bufnum, path,
                                timeout=5.0, expect=("/done", "/fail"))
    if addr == "/fail":
        raise CommandError(f"/buffer_allocRead {bufnum} failed: {args}")


def load_buffer(server: Server, path: str, frames: int) -> Buffer:
    """Take a buffer index from the client's allocator and load ``path`` into it."""
    bufnum = server.buffers.alloc()
    try:
        _alloc_read(server, bufnum, path)
    except CommandError:
        server.buffers.free(bufnum)
        raise
    return Buffer(bufnum, frames, 1, server=server)


# --------------------------------------------------------------------------
# The instrument: a Faust def that reads the buffer via `soundfile`.
# --------------------------------------------------------------------------


def soundfile_player(name: str, bufnum: int) -> FaustDef:
    """A looping buffer player written in Faust source. ``gain`` and ``speed``
    are live controls; the buffer is read with `soundfile`, whose label is the
    allocated ``bufnum``."""
    src = f"""
gain  = hslider("gain", 0.35, 0.0, 1.0, 0.001);
speed = hslider("speed", 1.0, 0.25, 4.0, 0.001);

// soundfile("<bufnum>", 1): bind to a server buffer, read 1 channel. Inputs are
// (part, frame index); outputs are [length, sampleRate, channel0].
sf(part, index) = (part, index) : soundfile("{bufnum}", 1);

length = sf(0, 0) : (_, !, !);              // part-0 frame count (1st output)

// A looping read phase advancing `speed` frames per output sample, wrapped to
// the buffer length (one feedback sample via `~`).
wrap(p) = p - length * (p >= length);
phase = (+(speed) : wrap) ~ _;

// Channel 0 at the (truncated) phase, scaled, sent to both outputs.
process = (sf(0, int(phase)) : (!, !, _)) * gain <: _, _;
"""
    return FaustDef.from_source(name, src)


# --------------------------------------------------------------------------


def run(server: Server, sr: float):
    motif_up = [440.0, 554.37, 659.25]        # A major, ascending
    motif_down = [659.25, 554.37, 440.0]      # the same notes, descending

    with tempfile.TemporaryDirectory() as tmp:
        wav_up = os.path.join(tmp, "up.wav")
        wav_down = os.path.join(tmp, "down.wav")
        frames = write_motif_wav(wav_up, sr, motif_up)
        write_motif_wav(wav_down, sr, motif_down)

        buf = load_buffer(server, wav_up, frames)
        print(f"loaded buffer {buf.bufnum}: {frames} frames @ {sr:.0f} Hz")

        # The def's soundfile label is the buffer we just allocated. Async send;
        # wait=True (default) blocks on /done (the Faust JIT compile).
        player = soundfile_player("sfplay", buf.bufnum)
        player.send(server)

        # Voice 1 reads the ascending motif (snapshot taken now, at /synth_new) and
        # loops it. Routed to "out" 0 -> hardware outputs 0/1.
        voice1 = Synth.new("sfplay", {"out": 0.0, "gain": 0.35, "speed": 1.0},
                           server=server)
        print("voice 1: looping the ascending motif; sweeping speed")
        for speed in (1.0, 1.5, 0.75, 1.25):
            voice1.set({"speed": speed})
            print(f"  speed -> {speed}")
            time.sleep(0.6)

        # Reload the SAME buffer with the descending motif. Voice 1 is unaffected
        # -- it plays from its own snapshot. Only a freshly instantiated voice
        # sees the new contents.
        _alloc_read(server, buf.bufnum, wav_down)
        print("reloaded buffer with the descending motif (voice 1 keeps the old one)")
        voice2 = Synth.new("sfplay", {"out": 0.0, "gain": 0.3, "speed": 1.0},
                           server=server)
        print("voice 2: reads the new (descending) motif -- both voices play together")
        time.sleep(1.8)

        voice1.free()
        voice2.free()
        buf.free()
        print("freed both voices and the buffer")


def main():
    options = ServerOptions(audio_buses=64, control_buses=128, sample_rate=48000)
    proc = launch(options, "--no-persist")
    server = Server(options=options)
    try:
        info = wait_until_ready(server)
        print(f"server up @ {info.actual_sample_rate:.0f} Hz")
        run(server, info.actual_sample_rate)
    finally:
        try:
            server.quit()
        except Exception:
            pass
        server.close()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.terminate()


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError, CommandError) as e:
        sys.exit(str(e))
