#!/usr/bin/env python3
"""A persistent GraphDef: package a wired patch, store it on disk, and prove it
reloads after a server restart.

This is the *high-level* half of a pair (the low-level half is
``examples/live_patch.py``, which wires the same idea by hand). Here the whole
patch is one stored unit:

  * **server configuration** with :class:`ServerOptions`, launched with a
    ``--data-dir`` pointing at a temp subdirectory *inside this example's
    folder* (so persistence is visible and self-contained);
  * a **GraphDef** combining a **FaustDef** member (a sine voice) and two
    **SynthDef** members (a buffer player and a mixer), wired by **internal
    buses** private to each instance, with a named **surface** (``freq``,
    ``rate``) driving the inner controls;
  * **reading from a buffer** inside the graph (the ``PlayBuf`` member);
  * **persistence**: phase 1 sends the member defs and the GraphDef (the server
    writes them under the data dir); phase 2 launches a *fresh* server on the
    *same* data dir and instantiates the GraphDef **without sending anything** —
    it only plays because the defs were reloaded from disk at boot.

The data directory is **kept on disk** (``examples/out/defs_store/``) so you can
open the persisted defs and explore them; the server groups them under a
``defs/`` subdir (``defs/synthdefs/``, ``defs/faustdefs/``, ``defs/graphdefs/``)
and keeps other persistent files like ``midi.json`` at the top level. Delete it
yourself when done (``rm -rf examples/out``) -- ``out/`` is the git-ignored
directory every generator in this tree writes to.

Real audio hardware is required. Build the server with the Faust feature first:

    cargo build --release --features faust
    python3 examples/persistent_graphdef.py

Point it at a prebuilt binary with ``CLAUSTERS_BIN=/path/to/clausters``.
"""

import math
import os
import struct
import subprocess
import sys
import time
import wave

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "clients", "python"))

from clausters.defs import (
    FaustDef,
    GraphDef,
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
from clausters.defs import Group
from clausters.defs import Buffer

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.join(HERE, "..")
BIN = os.environ.get("CLAUSTERS_BIN", os.path.join(REPO, "target", "release", "clausters"))

OPTIONS = ServerOptions(audio_buses=64, control_buses=512, sample_rate=48000)

# Kept on disk after the run so the persisted defs can be explored. The server
# nests the def kinds under a `defs/` subdir of this directory.
DATA_DIR = os.path.join(HERE, "out", "defs_store")


# --------------------------------------------------------------------------
# Server lifecycle (ServerOptions launches; query_info confirms).
# --------------------------------------------------------------------------


def launch(data_dir: str) -> subprocess.Popen:
    """Start a server with our options *and* on-disk persistence at
    ``data_dir`` (the flag the high-level options do not emit, since the data
    dir is a per-run choice)."""
    if not os.path.exists(BIN):
        sys.exit(f"server binary not found at {BIN}\n"
                 "build it with: cargo build --release --features faust\n"
                 "(or set CLAUSTERS_BIN)")
    return subprocess.Popen([BIN, *OPTIONS.args(), "--data-dir", data_dir])


def connect(timeout: float = 8.0) -> Server:
    """Connect a client sized from OPTIONS and wait until the server answers."""
    server = Server(options=OPTIONS)
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            server.query_info(timeout=0.3)
            return server
        except Exception:
            time.sleep(0.2)
    server.close()
    raise RuntimeError("server did not come up in time")


def shutdown(server: Server, proc: subprocess.Popen):
    try:
        server.quit()
    except Exception:
        pass
    server.close()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.terminate()


# --------------------------------------------------------------------------
# A buffer to read (regenerated each phase; buffers are runtime resources, not
# persisted with the defs).
# --------------------------------------------------------------------------


def load_pluck(server: Server, path: str, freq: float = 330.0, dur: float = 0.7):
    """Write a decaying-sine pluck WAV and load it with ``/buffer_allocRead``.
    Returns the :class:`Buffer`. A fresh client's allocator starts at 0, so the
    bufnum the GraphDef bakes in (0) matches in both phases."""
    from clausters.defs.buffer import Buffer

    sr = OPTIONS.sample_rate
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

    bufnum = server.buffers.alloc()
    addr, args = server.request("/buffer_allocRead", bufnum, path,
                                timeout=5.0, expect=("/done", "/fail"))
    if addr == "/fail":
        server.buffers.free(bufnum)
        raise CommandError(f"/buffer_allocRead failed: {args}")
    return Buffer(bufnum, n, 1)


# --------------------------------------------------------------------------
# The defs: three member defs and the GraphDef that wires them.
# --------------------------------------------------------------------------


def member_defs():
    """The FaustDef + SynthDefs the GraphDef references by name."""
    freq = S.hslider("freq", 220.0, 20.0, 20000.0, 0.01)
    phasor = S.rec(lambda s: (s + freq / S.sr()) % 1.0)
    fsine = FaustDef.from_signals("fsine", S.sin(phasor * S.TAU) * 0.2)

    bufnum, rate, amp, obus = (control("bufnum"), control("rate", 1.0),
                               control("amp", 0.5), control("out"))
    bufplayer = SynthDef("bufplayer",
                         out(obus, play_buf(bufnum, rate=rate, loop=1.0) * amp))

    in_a, in_b = control("inA"), control("inB")
    mixed = in_(in_a) + in_(in_b)
    mixer = SynthDef("mixer", out(0.0, mixed), out(1.0, mixed))
    return fsine, bufplayer, mixer


def patch_graphdef(bufnum: int) -> GraphDef:
    """One stored unit: the sine voice and the buffer player each write a
    private internal bus, the mixer reads both and goes to the hardware. The
    surface exposes `freq` and `rate`; `bufnum` is baked in as a member control
    (a buffer is a runtime resource the def just references by index)."""
    g = GraphDef("patch")
    bus_voice = g.bus("voice")                       # private audio buses,
    bus_sample = g.bus("sample")                     # one set per instance
    voice = g.add("fsine", out=bus_voice, freq=220.0)
    player = g.add("bufplayer", out=bus_sample, bufnum=bufnum, rate=1.0, amp=0.5)
    g.add("mixer", inA=bus_voice, inB=bus_sample)    # mixer -> hardware 0/1
    g.port("freq", voice["freq"], default=220.0)
    g.port("rate", player["rate"], default=1.0)
    return g


def play_instance(server: Server):
    """Instantiate the stored GraphDef and drive it through its surface."""
    inst = Group.graph("patch", {"freq": 220.0, "rate": 1.0}, server=server)
    print("  instance playing; driving the surface ports")
    for freq, rate in ((220.0, 1.0), (330.0, 0.75), (247.0, 1.5)):
        inst.set({"freq": freq, "rate": rate})
        print(f"    freq -> {freq:6.1f} Hz | rate -> {rate}")
        time.sleep(0.6)
    inst.free()                          # frees the group + private buses


# --------------------------------------------------------------------------
# The two phases.
# --------------------------------------------------------------------------


def phase_create(data_dir: str):
    """Send the defs + GraphDef (the server persists them), then play once."""
    print("phase 1: create and persist")
    proc = launch(data_dir)
    server = connect()
    try:
        fsine, bufplayer, mixer = member_defs()
        fsine.send(server)               # -> faustdefs/fsine.json (+ bitcode)
        bufplayer.send(server)           # -> synthdefs/bufplayer.json
        mixer.send(server)               # -> synthdefs/mixer.json
        buf = load_pluck(server, os.path.join(data_dir, "pluck.wav"))
        patch_graphdef(buf.bufnum).send(server)   # -> graphdefs/patch.json
        play_instance(server)
        buf.free()
    finally:
        shutdown(server, proc)

    print("  persisted files under defs/:")
    for sub in ("synthdefs", "faustdefs", "graphdefs"):
        d = os.path.join(data_dir, "defs", sub)
        names = sorted(os.listdir(d)) if os.path.isdir(d) else []
        print(f"    defs/{sub}/: {', '.join(names) or '(empty)'}")


def phase_reload(data_dir: str):
    """Fresh server on the same data dir: instantiate the GraphDef WITHOUT
    sending any def. It can only play if the defs were reloaded from disk."""
    print("phase 2: reload from disk (no defs sent)")
    proc = launch(data_dir)
    server = connect()
    try:
        num_defs = server.status()[4]            # synth+faust defs reloaded at boot
        print(f"  server reports {num_defs} synth/faust defs after boot "
              "(built-in 'default' + fsine + bufplayer + mixer)")
        load_pluck(server, os.path.join(data_dir, "pluck.wav"))   # bufnum 0 again
        play_instance(server)                    # uses the GraphDef reloaded from disk
        print("  the GraphDef instantiated without being re-sent: persistence works")
    finally:
        shutdown(server, proc)


def main():
    # A data dir INSIDE this example's folder, KEPT on disk so you can explore
    # the persisted defs afterwards (delete it yourself when done).
    os.makedirs(DATA_DIR, exist_ok=True)
    print(f"data dir (kept on disk): {DATA_DIR}")
    phase_create(DATA_DIR)
    phase_reload(DATA_DIR)
    print("\nexplore the persisted definitions on disk:")
    for sub in ("synthdefs", "faustdefs", "graphdefs"):
        d = os.path.join(DATA_DIR, "defs", sub)
        for name in sorted(os.listdir(d)) if os.path.isdir(d) else []:
            print(f"  {os.path.join(d, name)}")
    print(f"\ndelete it when done: rm -rf {DATA_DIR}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError, CommandError) as e:
        sys.exit(str(e))
