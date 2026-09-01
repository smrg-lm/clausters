#!/usr/bin/env python3
"""Ten clocks accelerating to ten different tempos, and arriving together.

A **tempo canon**: one line, played by ten voices at once, each one speeding up
towards a different tempo. They begin in unison, fan apart over half a minute
into something that sounds like one instrument coming unglued, and every voice
reaches its final tempo at the *same instant* — which is the whole point, and
the thing that is awkward to arrange any other way.

**Why they arrive together.** Each voice is a `TempoClock` of its own, and the
gesture is one call::

    clock.set_tempo(destination, over=30.0, unit="seconds")

``unit="seconds"`` is what makes the ten ramps the same length: a stretch of
**wall clock**, not of beats. The same request in beats would not do it. A beat
is a logical coordinate, so thirty beats of a voice rushing to 10 is a far
shorter *time* than thirty beats of one rising to 2 — the ten would finish
accelerating at ten different moments, and the piece would dissolve instead of
landing. With ten different starting tempos as well, the beat each voice reaches
is nowhere near the others; the **seconds** are the only thing they share, and
that is exactly what the call asks for. The first cell prints both columns side by side, so the difference
is a number rather than a claim. Nothing about it is approximate either: the
width in beats a request in seconds implies is solved in closed form
(`clausters.base.time` carries the formulas), so a ramp ends on the second it
was asked for.

**Ten clocks, one server.** A `Session` owns a server and as many clocks as the
piece has tempos, so this file makes ten. Each is `lock_to` the server, which
puts its scheduling on the server's own sample counter — ten clocks drifting
apart on ten OS timers would be a different and much less interesting piece.

Each also **adopts the session**, because it is built while the session is
ambient: they are in ``session.clocks``, ``session.start()`` starts them
together, and ``session.close()`` closes them. That adoption is not bookkeeping
— it is what an ambient `play` follows. ``Session.activate`` makes a session
ambient *on the calling thread*, and a routine does not run on that thread; it
runs on its own clock's. So inside a routine the only thing left to follow is
the clock's own `session`.

Needs an audio device. Run it as a script (``python tempo_canon.py``, optionally
with the number of seconds to play) or step through it cell by cell (``# %%``):
the clocks stay up between cells, so you can retune the fan and start again.
"""

# %%
import sys
import time
from functools import partial

from clausters import Session
from clausters.base import Routine, TempoClock, TempoMap, uniform
from clausters.seq.event import Event

#: How long the fan takes to open, in seconds of wall clock. Every voice
#: accelerates over exactly this, whatever tempo it is heading for.
SPREAD = 30.0

#: Where each voice starts: a slow draw, a beat every one to four seconds.
STARTS = [uniform(0.25, 2.0) for _ in range(10)]

#: Where each voice ends up: a fast draw, between 2 and 10 beats a second.
#: Both are drawn afresh every run, so the fan is a different shape each time.
#: `uniform` is the client's own generator rather than Python's, so
#: ``clausters.base.main.seed`` replays a canon you liked.
TARGETS = [uniform(8.0, 10.0) for _ in range(10)]

#: One partial each, so the fan is heard as one instrument coming apart rather
#: than as ten unrelated players.
PITCHES = [110.0 * (i + 2) for i in range(10)]

#: How long the whole run lasts: the fan, and a while to sit at the far end.
SECONDS_TO_PLAY = float(sys.argv[1]) if len(sys.argv) > 1 else SPREAD + 8.0


# %% [markdown]
# ## The two units, before anything sounds
# A `TempoMap` is a pure function of a beat: it answers about a piece nobody is
# playing. So the choice of unit can be *read* rather than argued about. Asked
# for over 30 seconds, every voice takes 30 seconds and arrives at a different
# beat; asked for over 30 beats, every voice takes a different time — which is
# ten voices landing at ten different moments.

# %%
print(f"{'from':>6} {'to (bps)':>9}  {'over 30 s':>21}  {'over 30 beats':>21}")
for start, target in zip(STARTS, TARGETS):
    by_secs = TempoMap(start)
    by_secs.env(0.0, [start, target], [SPREAD], unit="seconds")
    end_beat = by_secs.segment(1)[0]

    by_beats = TempoMap(start)
    by_beats.ramp(0.0, 30.0, start, target)

    print(f"{start:6.2f} {target:9.2f}  "
          f"{by_secs.secs_at(end_beat):8.3f} s at beat {end_beat:6.2f}  "
          f"{by_beats.secs_at(30.0):8.3f} s at beat {30.0:6.2f}")


# %% [markdown]
# ## The session
# `Session.live` boots a server if none answers and stops the one it started.
# Activating it makes it ambient, so a note played from inside a routine finds
# this server without being handed it — and so every clock built below adopts
# this session.

# %%
session = Session.live(tempo=1.0).activate()
server = session.server


# %% [markdown]
# ## One voice's line
# A note a beat, and nothing about the tempo. Each `yield 1.0` is one beat,
# always; what changes is how long that beat lasts, and this generator never
# finds out. It is the only function here — a routine *is* one — and everything
# that acts is written straight into the cells.

# %%
def line(pitch: float):
    """One note a beat at ``pitch``, for longer than any run needs."""
    for _ in range(400):
        # No explicit `sustain`: it would pin the sounding length, and the notes
        # would stop shortening as the tempo rises -- which is the thing this
        # example exists to let you hear. Left alone a note sounds
        # ``dur * legato`` beats and follows the tempo.
        Event(freq=pitch, legato=0.1, amp=0.06).play()
        yield 1.0


# %% [markdown]
# ## The ten clocks
# Built here, in the open: a clock, its ramp, its routine. Each one adopts the
# ambient session as it is constructed, so by the end of this cell
# `session.clocks` holds all eleven — the session's own default clock, which
# this piece does not use, and these ten.

# %%
clocks = []
for start, target, pitch in zip(STARTS, TARGETS, PITCHES):
    clock = TempoClock(tempo=start)
    clock.lock_to(server)                  # schedule on the server's samples
    clock.set_tempo(target, over=SPREAD, unit="seconds")
    Routine(partial(line, pitch)).play(clock)   # a hand-made clock is named
    clocks.append(clock)

print("clocks in the session:", len(session.clocks))


# %% [markdown]
# ## Press play
# Ten slow pulses, none of them agreeing with another. From there each pulls
# away at its own rate, and at `SPREAD` seconds they all stop
# accelerating at once and hold what they reached. `session.start` starts them
# together — a Python loop over ten `start` calls staggers them by whatever the
# loop costs.

# %%
session.start()

# %% [markdown]
# ## Watch the fan open
# The main thread waits here while the ten clocks play on their own threads.
# Sleeping is right *outside* a routine and a defect inside one, where it would
# freeze that voice's timeline.

# %%
started = time.monotonic()
while (elapsed := time.monotonic() - started) < SECONDS_TO_PLAY:
    time.sleep(min(5.0, SECONDS_TO_PLAY - elapsed))
    beats = "  ".join(f"{clock.beats():6.1f}" for clock in clocks)
    print(f"{time.monotonic() - started:5.1f} s  beats {beats}")

print("tempos reached:", "  ".join(f"{clock.tempo:.2f}" for clock in clocks))

# %% [markdown]
# ## Stop
# `stop` is a transport: the beats are held and a later `session.start()` picks
# the canon up where it was. `close` ends it — every clock the session owns, the
# server, and the process `live` launched.

# %%
session.stop()

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    session.close()
else:
    print("up - session.start() to run it again, session.close() to end")
