#!/usr/bin/env python3
"""A piece whose tempo changes, drawn where it is actually heard.

A beat is not a unit of time. It is a **logical coordinate**, and what turns one
into a second is the tempo — which can change along the piece. So "when does beat
8 happen?" has no answer from the tempo in force *now*: it depends on the whole
tempo history before it. That function is the piece's **time map**
(`clausters.base.TempoMap`), and this example is about the one thing that goes
wrong when a view does not read it.

Set the tempo once, in the middle, and watch the line. With a tempo doubled at
beat 2 in a piece at 48 kHz, beat 8 falls at **5.0 s** of wall clock — but a
view that held the starting tempo as a number would draw it at 8.0 s, and the
sweeping line would cross the clip three seconds after you heard it. The clip
here is drawn from the map, so the line crosses it when it sounds.

The two tempo gestures, and they are different acts:

1. `clock.set_tempo(bps)` is a **step from now on** — a new tempo, pinned at the
   instant you call it, so nothing already scheduled jumps.
2. `clock.ramp_tempo(bps, over=beats)` is a **shape written over a stretch** — an
   accelerando or a ritardando. Its length in seconds is a logarithm of the tempo
   ratio, not the span divided by an average of the two tempos, and the printed
   comparison below is the difference the two answers make.

Both write on `clock.map`, and a change is **recorded** rather than replacing
what came before — which is why the beats *before* it still convert correctly
afterwards. A single anchor cannot do that: it would extrapolate its current
slope backwards and report that beat 1 happened at a second it did not.

**The map is also a question you can ask with nothing playing.** It is a pure
function of a beat, so the cells below use it as an analysis of the piece: when
bar 5 arrives, how long the accelerando lasts, how many beats fit in the first
thirty seconds. A piece can hold a map no clock ever read.

**What to look at.** Hand the editor the clock's map (``tempo_map=clock.map``)
and the drawing and the sound are one function. `Editor.render` adopts the
clock's map anyway and redraws if it moved — the two cannot silently disagree —
but passing it up front means the first draw is already right.

Needs an audio device, a display and a GPU adapter; the install bundles the GUI
binary (see ``views/editor.py`` for the setup notes). Run it as a script
(``python tempo_map.py``) or cell by cell (``# %%``): the window stays up between
cells.
"""

# %%
import math
import sys

from clausters import Session, TempoMap
from clausters.form import Aggregate, Clang, Sequence
from clausters.gui import Editor
from clausters.seq.event import Event as SeqEvent

TEMPO = 1.0          # beats per second, before any change
QUANT = 0.5          # the drag grid: half a beat


# %% [markdown]
# ## A map, before there is a clock
# `TempoMap` is a plain function: it answers about a piece nobody is playing.
# Build the tempo of a piece here and ask it questions — that is the whole of the
# "composition" half of this example, and none of it needs a server.

# %%
tempo = TempoMap(TEMPO)
tempo.push(2.0, 2.0)                    # doubled at beat 2
tempo.ramp(8.0, 16.0, 2.0, 4.0)         # then accelerating over bars 3-4

print("beat 2 falls at ", tempo.secs_at(2.0), "s")
print("beat 8 falls at ", tempo.secs_at(8.0), "s")
print("beat 16 falls at", tempo.secs_at(16.0), "s")

# %% [markdown]
# ## A length in beats is not a duration
# The same eight beats are eight seconds at the start of this piece and rather
# less after the tempo has doubled. So seconds come from **two positions**, never
# from a beat count and a tempo — which is what `span_secs` is, and why every
# conversion in the client takes a position.

# %%
print("beats 0-8 last  ", tempo.span_secs(0.0, 8.0), "s")
print("beats 8-16 last ", tempo.span_secs(8.0, 16.0), "s")
print("30 s from beat 0 reaches beat", tempo.span_beats(0.0, 30.0))

# %% [markdown]
# ## The accelerando is a logarithm
# Averaging the two tempos is the plausible wrong answer. Over beats 8 to 16, from
# 2 to 4 beats per second, the true length is `ln(T1/T0) / k` — and the average
# would be out by a tenth of a second, which is audible and, drawn, visible.

# %%
ramp = tempo.span_secs(8.0, 16.0)
average = 8.0 / 3.0                     # 8 beats at the mean of 2 and 4 bps
print("the ramp lasts   ", ramp, "s")
print("the average says ", average, "s   (wrong by", round(average - ramp, 4), "s)")
print("closed form      ", math.log(4.0 / 2.0) / ((4.0 - 2.0) / (16.0 - 8.0)))

# %% [markdown]
# ## The piece
# Four bars of one note a beat, so the accelerando is something you can hear
# rather than something the numbers assert. The lane is a `Sequence` of clangs —
# each one an event with its own length — placed on the shared beat axis.

# %%
def bar(pitch: float) -> Sequence:
    """Four notes at one beat each, at ``pitch``."""
    return Sequence([Clang(SeqEvent({"freq": pitch, "dur": 1.0, "sustain": 0.9}))
                     for _ in range(4)])


song = Aggregate(name="tempo")
song.add(Sequence([bar(220.0), bar(277.2), bar(330.0), bar(440.0)],
                  name="lead"), 0.0)

# %% [markdown]
# ## The piece's tempo, handed to the clock
# The map above **is** the piece's tempo, written before any clock existed. Give
# it to the clock and there is one function: the same one this file asked its
# questions of, the same one the editor is about to draw with.
#
# The clock also writes on it live, and those are the two gestures — a step and a
# shape — the same acts, spelled from the other side:
#
# ```python
# clock.set_tempo(4.0)                  # a step, pinned at the beat you call it
# clock.ramp_tempo(4.0, over=8.0)       # a shape, over the eight beats from here
# ```
#
# Do that while it runs and the piece accelerates under your hand; the map keeps
# both, and every beat already played stays convertible.

# %%
session = Session.live(tempo=TEMPO, latency=0.1)
clock = session.clock
clock.map = tempo                       # the piece's tempo is the clock's

print("the clock's map:", clock.map.segments())

# %% [markdown]
# ## Drawn from the map
# ``tempo_map=clock.map`` is the whole of it. The clip for beat 8 lands on the
# sample the clock plays beat 8 at, and the sweeping line — which the host moves
# by engine samples — crosses it exactly then.

# %%
editor = Editor(song, sample_rate=session.server.sample_rate,
                tempo_map=clock.map, quant=QUANT, follow=True,
                title="A tempo that changes")
editor.open()

print("beat 8 is drawn at sample", editor.beats_to_units(8.0),
      "=", editor.beats_to_units(8.0) / session.server.sample_rate, "s")
print("a frozen tempo would have said",
      8.0 / TEMPO * session.server.sample_rate)

# %% [markdown]
# ## Play it, and watch the line
# The line sweeps by the engine's own clock, so if the drawing and the sound
# disagreed you would see it: the line would reach a clip early or late by
# whatever the tempo change moved. Here it crosses each note as it sounds, and
# the notes visibly crowd together over the last two bars.

# %%
def run():
    """Play it and hold until the window is closed — a by-eye and by-ear test
    ends when the person looking at it says so, not on a timer."""
    editor.render(session.server, clock)
    while editor.window is not None:
        editor.transport.update()
        editor.poll(0.05)


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        session.close()
else:
    print("up - run() to play it and hold the window, session.close() to end")
