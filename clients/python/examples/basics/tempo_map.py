#!/usr/bin/env python3
"""The tempo map: a piece's whole tempo history, as a function you can ask.

A beat is not a unit of time. It is a **logical coordinate**, and what turns one
into a second is the tempo -- which changes along the piece. So "when does beat 8
happen?" has no answer from the tempo in force *now*: it depends on every tempo
change before it. That function is the piece's `clausters.TempoMap`, and this
example is about using it as a **thing you interrogate** rather than as a number
a clock happens to hold.

Its sibling `tempo_canon.py` is about one gesture -- ten ramps in wall clock,
landing together. This one is about the map underneath that gesture: how it is
built without a clock, what it answers, and why the plausible arithmetic is
wrong.

**Three things it shows, and the third is the one that catches people.**

- A tempo change is **recorded**, never a replacement. `push` writes a step at a
  beat and `ramp` a stretch between two, and the beats *before* one still convert
  correctly afterwards. A single anchor could not do that: it would extrapolate
  its current slope backwards and report that beat 1 happened at a second it did
  not.
- **A length in beats is not a duration.** The same eight beats last eight
  seconds early in this piece and rather less once the tempo has doubled, so
  seconds always come from **two positions** (`span_secs`), never from a beat
  count times a tempo. Every conversion in the client takes a position for this
  reason.
- **An accelerando is a logarithm.** Averaging the two tempos is the plausible
  wrong answer, and over four bars it is out by a tenth of a second -- audible,
  and visible if it were drawn. The cell prints both.

Then the map is handed to a clock (`clock.map = tempo`) and the piece is played,
so the acceleration is something you **hear** rather than something the numbers
assert. The same map that answered the questions above is the one the clock runs
on: there is one function, not a description and a performance.

The *drawn* side of this -- a tempo curve you drag, and the beat ruler re-ruling
under your hand -- is `views/tempo_ruler.py`.

Needs an audio device. Run it as a script (``python tempo_map.py``) or step
through it cell by cell (``# %%``): the session stays up between cells.
"""

# %%
import math
import sys
import time

from clausters import Session, TempoMap
from clausters.seq import Playhead, Timeline
from clausters.seq.event import Event

#: Beats per second before any change: one beat a second, so the first bars are
#: easy to count against a watch.
TEMPO = 1.0

#: How long to let the piece run, in seconds.
SECONDS_TO_PLAY = float(sys.argv[1]) if len(sys.argv) > 1 else 20.0

# %% [markdown]
# ## A map, before there is a clock
# `TempoMap` is a plain function of a beat. It answers about a piece nobody is
# playing, so the whole first half of this file needs no server at all: build the
# tempo of a piece, then ask it questions.
#
# `push` is a **step** at a beat; `ramp` is a stretch between two. Both are
# recorded on top of what came before.

# %%
tempo = TempoMap(TEMPO)
tempo.push(2.0, 2.0)                    # doubled from beat 2 on
tempo.ramp(8.0, 16.0, 2.0, 4.0)         # then accelerating over bars 3-4

print("beat 2 falls at ", tempo.secs_at(2.0), "s")
print("beat 8 falls at ", tempo.secs_at(8.0), "s")
print("beat 16 falls at", tempo.secs_at(16.0), "s")

# %% [markdown]
# ## And the question runs both ways
# `beats_at` is `secs_at` inverted: given a second, which beat is the piece on?
# A transport that has to draw a playhead asks this one on every frame.

# %%
print("at 5 s the piece is on beat", tempo.beats_at(5.0))
print("the recorded history:", tempo.segments())

# %% [markdown]
# ## A length in beats is not a duration
# The same eight beats are eight seconds at the start of this piece and rather
# less after the tempo has doubled. So seconds come from **two positions**, never
# from a beat count and a tempo -- which is what `span_secs` is, and why every
# conversion in the client takes a position.

# %%
print("beats 0-8 last  ", tempo.span_secs(0.0, 8.0), "s")
print("beats 8-16 last ", tempo.span_secs(8.0, 16.0), "s")
print("30 s from beat 0 reaches beat", tempo.span_beats(0.0, 30.0))

# %% [markdown]
# ## The accelerando is a logarithm
# Averaging the two tempos is the plausible wrong answer. Over beats 8 to 16,
# from 2 to 4 beats per second, the true length is `ln(T1/T0) / k` -- and the
# average is out by a tenth of a second.

# %%
ramp = tempo.span_secs(8.0, 16.0)
average = 8.0 / 3.0                     # 8 beats at the mean of 2 and 4 bps
print("the ramp lasts   ", ramp, "s")
print("the average says ", average, "s   (wrong by", round(average - ramp, 4), "s)")
print("closed form      ", math.log(4.0 / 2.0) / ((4.0 - 2.0) / (16.0 - 8.0)))

# %% [markdown]
# ## The piece
# Sixteen beats of one note each, on a `Timeline` -- the static, random-access
# sequencing structure, so the notes are placed at beats and the map decides when
# each of those beats arrives. Four bars, one pitch per bar, so the accelerando
# is something to hear rather than something the numbers assert.

# %%
PITCHES = [220.0, 277.2, 330.0, 440.0]

line = Timeline()
for beat in range(16):
    line.add(float(beat),
             Event({"freq": PITCHES[beat // 4], "dur": 1.0, "sustain": 0.9}))

print(f"{len(line)} notes over {line.duration()} beats")

# %% [markdown]
# ## The piece's tempo, handed to the clock
# The map above **is** the piece's tempo, written before any clock existed.
# Assigning it is the whole of the handover, and from here there is one function:
# the same one this file asked its questions of, and the one the notes are played
# by.

# %%
session = Session.live(tempo=TEMPO, latency=0.1).activate()
session.start()
clock = session.clock
clock.map = tempo                       # the piece's tempo is the clock's

print("the clock's map:", clock.map.segments())
print("the clock is at beat", round(clock.beats(), 3))

# %% [markdown]
# ## The other spelling: writing on the map from the clock
# `set_tempo` is the same act from the other side, and it writes on the same map.
# Four forms, and each is one call:
#
# ```python
# clock.set_tempo(4.0)                            # a step, pinned where you call it
# clock.set_tempo(4.0, over=8.0)                  # a shape, over eight beats
# clock.set_tempo(4.0, over=3.0, unit="seconds")  # the same, over three seconds
# clock.set_tempo(4.0, over=8.0, curve="exp")     # equal ratios, not equal steps
# ```
#
# Do any of them while the piece runs and it accelerates under your hand; the map
# keeps both histories, and every beat already played stays convertible. Which is
# also why `tempo_canon.py` can ask ten clocks to land together: `unit="seconds"`
# is a stretch of wall clock, and the map solves the beats it implies in closed
# form.

# %% [markdown]
# ## Play it
# The notes crowd together over the last two bars, which is what the map said
# they would do: beat 16 at the second `secs_at` printed above, not at sixteen.

# %%
head = Playhead(line, clock, session.server)
head.play(at=0.0)

started = time.monotonic()
while (elapsed := time.monotonic() - started) < SECONDS_TO_PLAY:
    time.sleep(min(2.0, SECONDS_TO_PLAY - elapsed))
    print(f"{time.monotonic() - started:5.1f} s   beat {clock.beats():6.2f}"
          f"   tempo {clock.tempo:.2f}")

# %% [markdown]
# ## Stop
# `stop` holds the position; `close` ends the session and the server it launched.

# %%
head.stop()
session.stop()

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    session.close()
else:
    print("up - head.play(at=0) to hear it again, session.close() to end")
