#!/usr/bin/env python3
"""Recording into a buffer while another node plays it: the looper.

A buffer's contents are **mutable**, and only its shape is fixed — frames,
channels and sample rate are settled at allocation and never change, while
every sample can be written at any time, by a UGen or by a `/buffer_*`
command, with anything else reading it. Nothing has to be declared: there is
no writable *kind* of buffer, because they all are.

That is what makes one buffer a place two nodes meet, which is the whole
subject here. In five acts, each audible on its own:

1. **Record.** `record_buf` writes a phrase into an empty buffer while a
   looping `play_buf` reads the same buffer. One buffer, two nodes, no copy
   between them.
2. **Hold.** `rec_level` 0 with `pre_level` 1 writes each frame back
   unchanged: the recording stops without the loop noticing.
3. **Overdub.** `pre_level` 1 makes each frame `new + old` instead of `new`,
   so a second pass at another pitch **adds** to the loop. Then `pre_level`
   below 1 fades the older layers a little more each pass — that pair is what
   makes the UGen a looper rather than a tape head.
4. **A one-shot that frees itself.** A non-looping `play_buf` cued to the
   second half of the buffer with `done_action` 2: it plays once and its node
   leaves the tree, which the synth count printed before and after says out
   loud. A rising `trigger` re-cues it, so one player is a re-usable voice.
5. **A delay through a shared buffer.** `buf_comb_c` runs its line inside the
   pool buffer rather than in synth-private memory, so the delay's own
   contents are a buffer like any other — readable, savable, and here written
   to a WAV at the end.

**Nothing here clicks, and the two reasons are the interesting part.** The
recorded phrase is a windowed note followed by a silence, retriggered by an
`impulse` at exactly the loop's own rate: the material is therefore zero at
the loop's seam, and every change of what is being written is made *during*
that silence, where a change writes nothing and can leave no step behind.
And the reader is started **after** the writer and runs at the same rate, so
it trails it by a fixed distance forever and never crosses the write head —
what it reads is always a frame the writer is done with.

Needs an audio device (it boots its own server and plays through the sound
card). Run it:

    python3 examples/buffer_writing.py

`docs/schemas.md` has the reference rows for `RecordBuf`, `BufWr`, `PlayBuf`
and the `BufDelay*` family.
"""

import os
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, os.path.join(ROOT, "clients", "python"))

from clausters import Session  # noqa: E402
from clausters.defs import Buffer, Group, Synth, SynthDef  # noqa: E402
from clausters.defs import (  # noqa: E402
    DoneAction, Env, buf_comb_c, control, env_gen, impulse, out, play_buf,
    record_buf, sine,
)
from clausters.defs.ugens import detect_silence  # noqa: E402

SR = 48000
LOOP = 2.0                  # the buffer, in seconds
NOTE = 1.6                  # the phrase inside it; the rest is silence
TAIL = LOOP - NOTE          # where every change below is made


def recorder_def():
    """Writes a phrase into the buffer, one frame per sample.

    The phrase is a windowed note (`env_gen` over `Env`, zero at both ends)
    retriggered by an `impulse` at the loop's own rate. Both the impulse and
    the write head start on the synth's first sample, so the window stays
    locked to the buffer for good: the note always begins at frame 0 and the
    silence always covers the same frames at the end.

    Everything that decides *what* is written is a control, because a looper
    is played by changing them while it runs. `loop` stays on, so the write
    head wraps at the end of the buffer and the recording never finishes.
    """
    bufnum = control("bufnum", 0.0)
    window = env_gen(Env([0.0, 1.0, 1.0, 0.0], [0.05, NOTE - 0.35, 0.3]),
                     gate=impulse(1.0 / LOOP))
    tone = sine(control("freq", 220.0)) * window * control("amp", 0.25)
    return SynthDef("looper-record", record_buf(
        bufnum, 0.0, tone,
        rec_level=control("rec", 1.0),
        pre_level=control("pre", 0.0),
        loop=1.0,
    ))


def player_def():
    """The looping reader. Its `gate` releases it, so the run ends with a
    fade rather than with a node cut off mid-sample.

    `start_pos` and `trigger` are left alone: a loop that never ends has
    nowhere to be cued to.
    """
    bufnum = control("bufnum", 0.0)
    fade = env_gen(Env([0.0, 1.0, 0.0], [0.05, 0.3], release_node=1),
                   gate=control("gate", 1.0),
                   done_action=DoneAction.FREE_SELF)
    sig = play_buf(bufnum, 0.0, rate=1.0, loop=1.0) * fade * control("amp", 0.8)
    return SynthDef("looper-play", out(0, sig), out(1, sig))


def oneshot_def():
    """A player that ends. Cued to `start_pos`, not looping, and freeing its
    own node when the pass reaches the end (`done_action` 2) — so a sample
    fired from a pattern leaves nothing behind.

    One trigger control drives both the player and the envelope: the rising
    edge re-cues the reader to `start_pos` and restarts the window over it,
    which is what makes the node re-usable rather than single-use. The grain
    is deliberately shorter than the gap between triggers, so a re-cue always
    lands in silence: both the reader's jump and the envelope's — which
    restarts from its initial level rather than gliding from wherever it was —
    are steps, and a step is inaudible only where there is nothing to step.
    The window is also shorter than what is left of the buffer, so the pass
    ends in silence and the node's own end is inaudible too.
    """
    bufnum = control("bufnum", 0.0)
    trig = control("trig", 1.0, "tr")
    shape = env_gen(Env([0.0, 1.0, 1.0, 0.0], [0.02, 0.15, 0.15]), gate=trig)
    sig = play_buf(
        bufnum, 0.0,
        rate=1.0,
        loop=0.0,
        trigger=trig,
        start_pos=control("start_pos", 0.0),
        done_action=DoneAction.FREE_SELF,
    ) * shape * control("amp", 0.8)
    return SynthDef("oneshot", out(0, sig), out(1, sig))


def echo_def():
    """A comb delay whose line lives **in a pool buffer**. The private
    `comb_c` allocates its own memory at build and nobody can look at it;
    this one's line is the buffer, so the delay's contents can be read,
    resampled or written to disk like any other buffer — which is what the
    last act does with it.

    `detect_silence` frees the voice once the tail has rung out, so nothing
    here has to guess how long a decay lasts.
    """
    bufnum = control("bufnum", 0.0)
    src = sine(control("freq", 660.0)) * env_gen(Env.perc(0.01, 0.3), gate=1.0) * 0.3
    sig = buf_comb_c(bufnum, 0.0, src, delaytime=0.18, decaytime=2.5)
    return SynthDef(
        "buffer-echo",
        out(0, sig), out(1, sig),
        detect_silence(sig, amp=0.0005, time=0.2,
                       done_action=DoneAction.FREE_SELF),
    )


def main():
    with Session.live() as session:
        server = session.server

        # Empty, mono, two seconds. `alloc` zeroes it; nothing declares it
        # writable, because every buffer is.
        loop_buf = Buffer.alloc(int(LOOP * SR), 1)
        echo_buf = Buffer.alloc(SR // 2, 1)
        for d in (recorder_def(), player_def(), oneshot_def(), echo_def()):
            d.send(server)

        stage = Group()

        print("1. recording into the buffer while a player loops it")
        rec = Synth("looper-record", {"bufnum": loop_buf.bufnum, "freq": 220.0},
                    target=stage)
        # The reader is started **after** the writer and runs at the same rate,
        # so it trails it by a fixed distance and never crosses the write head.
        # It is also added after it, which puts it later in the block: within
        # one block it reads frames the recorder has just written.
        player = Synth("looper-play", {"bufnum": loop_buf.bufnum}, target=stage)
        started = time.monotonic()

        def at_the_seam(pass_number):
            """Sleep until the loop's silence, where a change writes nothing.

            That is the whole trick behind a click-free looper: what is
            written changes only where the material is zero, so no edge is
            ever stored in the buffer.
            """
            target = started + pass_number * LOOP + NOTE + TAIL / 2
            time.sleep(max(0.0, target - time.monotonic()))

        at_the_seam(2)
        print("2. rec_level 0, pre_level 1: each frame written back unchanged")
        rec.set({"rec": 0.0, "pre": 1.0})

        at_the_seam(4)
        print("3. overdubbing a fifth on top (pre_level 1: new + old)")
        rec.set({"freq": 330.0, "rec": 1.0, "amp": 0.2})
        at_the_seam(5)
        rec.set({"rec": 0.0})

        at_the_seam(7)
        print("   pre_level 0.6: the older layers fade a little each pass")
        rec.set({"pre": 0.6})
        at_the_seam(10)
        rec.set({"pre": 1.0})
        rec.free()
        player.set({"gate": 0.0})       # releases, then frees itself
        time.sleep(0.6)

        print("4. a one-shot from the second half, re-triggered, then done")
        shot = Synth("oneshot", {"bufnum": loop_buf.bufnum,
                                 "start_pos": float(int(LOOP * SR) // 2)},
                     target=stage)
        time.sleep(0.4)
        print("   synths while it plays:", server.status()[2])
        shot.set({"trig": 1.0})         # re-cued mid-pass: it starts over
        time.sleep(0.4)
        shot.set({"trig": 1.0})
        # Now left alone: the pass runs out and the done action takes the node
        # with it. Nothing frees it from here.
        time.sleep(1.5)
        print("   synths once the pass ended:", server.status()[2],
              "(it freed itself)")

        print("5. a comb delay running its line inside a pool buffer")
        Synth("buffer-echo", {"bufnum": echo_buf.bufnum}, target=stage)
        time.sleep(3.5)

        # The delay line is a buffer, so it can simply be written out. That is
        # the capability the whole family exists for: shared, addressable
        # material rather than memory only one synth can see.
        path = os.path.join(tempfile.gettempdir(), "clausters-delay-line.wav")
        echo_buf.write(path, sample_format="float")
        print("   wrote the delay's own line to", path)

        stage.free()
        print("done")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
