#!/usr/bin/env python3
"""A live patch wired by hand: server config, groups, two def kinds, buses
and a buffer — all through the Python client's own resources.

This is the *low-level* half of a pair (the high-level half is
``examples/persistent_graphdef.py``, which packages the same ideas as a stored
GraphDef). Here every connection is explicit, so you can see what the client
actually owns:

  * **server configuration** with :class:`ServerOptions` (bus counts + sample
    rate), which both **launches** a matching server (``options.args()`` ->
    ``subprocess``) and **sizes the client's allocators**, and is checked
    against the running server with :meth:`Server.query_info`;
  * a **group** per role (sources, then an output stage that runs after them);
  * a **FaustDef** (a sine voice) *and* a **SynthDef** (a buffer player) as the
    two sound sources, plus a SynthDef mixer;
  * **audio buses** connecting the sources to the mixer, and a **control bus**
    driving the voice's pitch (so one write retunes it with no command);
  * **reading from a buffer**: a one-shot pluck WAV is generated, loaded with
    ``/b_allocRead`` and looped by the ``PlayBuf`` SynthDef.

The signal flow::

    fsine (FaustDef)  --> [busVoice] --\
                                        >-- mixer (SynthDef) --> hw 0/1
    bufplayer (PlayBuf) -> [busSample]-/
       ^ reads a loaded buffer
    freqBus (control) --map--> fsine.freq

Real audio hardware is required (a live RT server). Build the server with the
Faust feature first, then run this — it starts and stops its own server:

    cargo build --release --features faust
    python3 examples/live_patch.py

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

from clausters.defs import (
    FaustDef,
    Server,
    ServerOptions,
    SynthDef,
    control,
    in_,
    out,
    play_buf,
    signals as S,
)
from clausters.errors import CommandError
from clausters.defs.node import AddAction

REPO = os.path.join(os.path.dirname(__file__), "..")
BIN = os.environ.get("CLAUSTERS_BIN", os.path.join(REPO, "target", "release", "clausters"))


# --------------------------------------------------------------------------
# Server lifecycle: ServerOptions both launches the server and sizes the client.
# --------------------------------------------------------------------------


def launch(options: ServerOptions, *extra_args: str) -> subprocess.Popen:
    """Start a server whose configuration matches ``options`` and return the
    process. ``options.args()`` is the exact CLI for the flags; we add any
    run-specific flags (here ``--no-persist``: this demo keeps nothing on
    disk)."""
    if not os.path.exists(BIN):
        sys.exit(f"server binary not found at {BIN}\n"
                 "build it with: cargo build --release --features faust\n"
                 "(or set CLAUSTERS_BIN)")
    return subprocess.Popen([BIN, *options.args(), *extra_args])


def wait_until_ready(server: Server, timeout: float = 8.0) -> "object":
    """Poll ``/server_info`` until the freshly launched server answers, then
    return what it reports so we can confirm it matches our options."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            return server.query_info(timeout=0.3)
        except Exception:
            time.sleep(0.2)
    raise RuntimeError("server did not come up in time")


# --------------------------------------------------------------------------
# A buffer to read: synthesize a short pluck WAV and load it server-side.
# --------------------------------------------------------------------------


def write_pluck_wav(path: str, sr: float, freq: float = 330.0, dur: float = 0.7) -> int:
    """Write a mono decaying-sine 'pluck' to ``path`` with stdlib ``wave``.
    Returns the frame count."""
    n = int(sr * dur)
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(int(sr))
        frames = bytearray()
        for i in range(n):
            t = i / sr
            sample = 0.6 * math.exp(-4.0 * t) * math.sin(2 * math.pi * freq * t)
            frames += struct.pack("<h", int(max(-1.0, min(1.0, sample)) * 32767))
        w.writeframes(bytes(frames))
    return n


def alloc_read(server: Server, path: str, frames: int):
    """``/b_allocRead`` through the client's resources: take a buffer index from
    the Server's allocator, then send the command and wait for ``/done``. (The
    high-level :meth:`Server.alloc_buffer` only does the empty ``/b_alloc``; for
    loading a file we drive ``/b_allocRead`` over :meth:`Server.request`.)"""
    from clausters.defs.buffer import Buffer

    bufnum = server.buffers.alloc()
    addr, args = server.request("/b_allocRead", bufnum, path,
                                timeout=5.0, expect=("/done", "/fail"))
    if addr == "/fail":
        server.buffers.free(bufnum)
        raise CommandError(f"/b_allocRead failed: {args}")
    return Buffer(bufnum, frames, 1)


# --------------------------------------------------------------------------
# The instrument definitions: one FaustDef, two SynthDefs.
# --------------------------------------------------------------------------


def build_defs():
    """A Faust sine voice, a UGen buffer player, and a UGen 2-input mixer."""
    # FaustDef: a sine oscillator. `freq` is its control; the server adds the
    # reserved `out` bus selector. `S.sr()` (Faust's ma.SR) keeps it in tune at
    # whatever rate the engine runs.
    freq = S.hslider("freq", 220.0, 20.0, 20000.0, 0.01)
    phasor = S.rec(lambda s: (s + freq / S.sr()) % 1.0)   # 1-sample feedback ramp
    fsine = FaustDef.from_signals("fsine", S.sin(phasor * S.TAU) * 0.2)

    # SynthDef: loop a buffer into the bus named by `out`, scaled by `amp`.
    bufnum, rate, amp, obus = (control("bufnum"), control("rate", 1.0),
                               control("amp", 0.5), control("out"))
    bufplayer = SynthDef("bufplayer",
                         out(obus, play_buf(bufnum, rate=rate, loop=1.0) * amp))

    # SynthDef: sum two input buses to the hardware outputs 0/1.
    in_a, in_b = control("inA"), control("inB")
    mixed = in_(in_a) + in_(in_b)
    mixer = SynthDef("mixer", out(0.0, mixed), out(1.0, mixed))
    return fsine, bufplayer, mixer


# --------------------------------------------------------------------------


def run(server: Server, buf):
    """Wire and play the patch live, then tear it down."""
    fsine, bufplayer, mixer = build_defs()
    # Async sends; wait=True (default) blocks on /done. The Faust def JIT-
    # compiles on the server, so this is the slow one.
    server.add_faustdef(fsine)
    server.add_synthdef(bufplayer)
    server.add_synthdef(mixer)

    # Two groups give a defined execution order: everything in `sources` runs
    # before `output`, so the mixer always reads buses the sources already wrote
    # this block.
    sources = server.group()
    output = server.group()              # added after `sources` -> runs later

    # Buses connecting the nodes. Sizes come from ServerOptions (the allocators
    # never hand out a bus the server lacks).
    bus_voice = server.audio_bus()
    bus_sample = server.audio_bus()
    freq_bus = server.control_bus()
    server.set_bus(freq_bus, 220.0)      # initial pitch lives on the control bus

    # The Faust voice writes to bus_voice; its freq is *mapped* to the control
    # bus, so retuning is a single /c_set with no per-note command.
    voice = server.synth("fsine", {"out": bus_voice.index},
                         target=sources.id, action=AddAction.TAIL)
    server.map(voice, "freq", freq_bus)

    # The buffer player loops the pluck into bus_sample.
    player = server.synth("bufplayer",
                          {"bufnum": buf.bufnum, "out": bus_sample.index,
                           "rate": 1.0, "amp": 0.5},
                          target=sources.id, action=AddAction.TAIL)

    # The mixer reads both source buses and sends the sum to the speakers.
    server.synth("mixer", {"inA": bus_voice.index, "inB": bus_sample.index},
                 target=output.id, action=AddAction.TAIL)

    print("playing: Faust sine + looped buffer, mixed to the outputs")
    # Move the voice's pitch by writing the control bus only; sweep the buffer
    # playback rate by setting the player's control directly.
    for pitch, rate in ((220.0, 1.0), (277.0, 0.75), (330.0, 1.5), (220.0, 1.0)):
        server.set_bus(freq_bus, pitch)
        server.set(player, {"rate": rate})
        print(f"  freq bus -> {pitch:6.1f} Hz | buffer rate -> {rate}")
        time.sleep(0.6)

    server.free(voice, player)
    server.free(sources, output)         # frees the groups (and their contents)
    print("freed all nodes")


def main():
    # The single source of truth for the server's shape: it launches a matching
    # server and sizes this client's allocators.
    options = ServerOptions(audio_buses=64, control_buses=512, sample_rate=48000)
    proc = launch(options, "--no-persist")
    server = Server(options=options)
    try:
        info = wait_until_ready(server)
        print(f"server up: {info.audio_buses} audio / {info.control_buses} control "
              f"buses @ {info.actual_sample_rate:.0f} Hz")

        with tempfile.TemporaryDirectory() as tmp:
            wav = os.path.join(tmp, "pluck.wav")
            frames = write_pluck_wav(wav, info.actual_sample_rate)
            buf = alloc_read(server, wav, frames)
            print(f"loaded buffer {buf.bufnum}: {frames} frames")
            run(server, buf)
            server.free_buffer(buf)
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
