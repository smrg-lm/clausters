#!/usr/bin/env python3
"""The ambient verbs, end to end: one ``play`` and one ``render`` for
everything.

``play`` sounds whatever you hand it against the ambient context — an event or
a plain dict, a generator, a bare signal expression (a UGen graph or a Faust
box), a named def, a timeline, a buffer, an automation — and ``render``
performs the change of state offline: an expression or a pattern in, samples
(and here a WAV) out. This tour visits every playable kind audibly, and closes
the circle by rendering a phrase to a file, loading it back as a buffer and
playing the take.

The visual sibling has its own tour (``views/plotting.py``); the arrangement, being
*rendered* rather than played, has its walkthrough in the composing chapters
(see the book's "The ambient verbs" for why the split).

Run it as a script (``python verbs.py``) or cell by cell (``# %%``). Needs an
audio device; the install bundles the server.
"""

# %% Setup: boot once — the booted server becomes the default session, and
# every verb below finds it with no wiring.
import tempfile
import time

from clausters import Event, play, render
from clausters.defs import SynthDef, boxes as box, control, out, sine
from clausters.defs.ugens import Env
from clausters.seq import Pbind, Pseq
from clausters.seq.automation import Automation
from clausters.seq.timeline import Timeline

from clausters import Server
from clausters.defs import Buffer

server = Server().boot()

#: Seconds between audible steps.
PAUSE = 1.2


# %% An Event — and a plain dict, which coerces to one.
print("an Event, then the same note as a bare dict")
play(Event(degree=0, dur=0.5))
time.sleep(PAUSE)
play({"degree": 4, "dur": 0.5, "amp": 0.15})
time.sleep(PAUSE)

# %% A generator — coerced to a Routine on the default clock. Each yield is
# the gap in beats to the next wake.
print("a generator, three notes up the scale")


def arpeggio():
    for degree in (0, 2, 4):
        play(Event(degree=degree, dur=0.3))
        yield 0.35


play(arpeggio)
time.sleep(PAUSE + 1.0)

# %% A bare expression: the verb wraps it in an ephemeral def (adding the
# `out`), sends it and instances it. Everything play returns knows how to end
# what it started — a Synth handle frees itself.
print("a bare UGen expression, sounding until freed")
node = play(sine(330.0) * 0.15)
time.sleep(PAUSE)
node.free()

# %% The Faust family is a peer: a box expression takes the same door.
print("a Faust box expression (os.osc from the Faust library)")
node = play(box.faust("os.osc")(box.hslider("freq", 440.0, 20.0, 2000.0, 0.01)) * 0.15)
time.sleep(PAUSE)
node.free()

# %% A named def, instanced with controls. A played event is also actionable
# after the fact: play returns it completed (node/server/derived keys filled),
# so a note with an extreme sustain can be cut (.free()) or ended musically
# (.release()) before its time.
print("a named def with controls; a long note, released early")
beep = SynthDef("verbs_beep", out(0.0, sine(control("freq", 440.0)) * 0.15))
node = play(beep, controls={"freq": 660.0})
time.sleep(PAUSE)
node.free()
long_note = play(Event(degree=0, dur=30.0))   # would sustain ~24 s...
time.sleep(PAUSE)
long_note.free()                              # ...cut now

# %% An automation coupled to a sounding node: the curve is written to a
# control bus and /node_map'd onto the control — the node follows it, then keeps
# the last value. Outside a clock the curve's beats read as seconds; the
# returned automation stops the sweep early (the control holds where it was).
print("an automation sweeping a sounding node's freq, interrupted mid-sweep")
node = play(sine(control("freq", 440.0)) * 0.15)
sweep = play(Automation(Env([440.0, 1760.0, 440.0], [1.5, 1.5]),
                        target=(node, "freq")))
time.sleep(1.5)
sweep.stop()                # interrupted at the top: the freq holds there
time.sleep(1.0)
node.free()

# %% A timeline: already-generated placement, driven by a playhead on the
# ambient clock.
print("a timeline, two placed notes")
tl = Timeline()
tl.add(0.0, Event(degree=0, dur=0.5))
tl.add(0.5, Event(degree=7, dur=0.5))
play(tl)
time.sleep(PAUSE + 1.0)

# %% render: the change of state. A pattern bounces offline to samples — and
# with path=, to a WAV — with no server involved (an ephemeral one renders).
print("rendering a phrase to a WAV (offline, no audio device)")
wav = tempfile.NamedTemporaryFile(suffix=".wav", delete=False).name
render(Pbind(instrument="default", degree=Pseq([0, 4, 7, 12]), dur=0.25),
       path=wav)

# %% ...and the circle closes: the rendered file, loaded as a buffer and
# played through the stock playbuf instrument (freed when the take ends).
print("playing the rendered take back as a buffer")
take = Buffer.read(wav, server=server)
play(take, controls={"amp": 0.8})
time.sleep(2.0)

server.close()
print("done")
