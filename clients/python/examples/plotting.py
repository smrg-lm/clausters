#!/usr/bin/env python3
"""Quick looks with the free-standing ``plot`` — defs, envelopes, sequences.

`clausters.plot` is the visual sibling of `clausters.play`: one verb that opens
**its own window** for whatever you hand it, booting the GUI host lazily on
first use (no audio server involved — a def is rendered *offline* by the
bundled NRT renderer and reaches the host as a mapped file). Each call is one
window; the returned handle retunes the display live and closes it.

What to look at in each window: the x/y rulers fit the signal (a sequence of
arbitrary range auto-fits its value axis), every channel draws in its own lane,
and hovering the trace shows a hairline with the exact sample under the cursor
— index or clock time, and the sample's value; on a spectrum view, the bin's
frequency (in the chosen scale) and its level in dB.

Run it as a script (``python plotting.py``; windows stay open until you close
them) or cell by cell (``# %%``). Needs a display and a GPU adapter; the
install bundles the GUI binary.
"""

# %% A def's rendered output — eyeball what a SynthDef actually produces.
import time
from clausters import plot
from clausters.defs import (
    DoneAction,
    Env,
    GraphDef,
    SynthDef,
    control,
    env_gen,
    out,
    sin_osc,
)
from clausters.seq import Pseq, Pwhite


# %%
def ping(name: str = "ping") -> SynthDef:
    """A sine ping shaped by a percussive envelope, self-freeing."""
    freq = control("freq", 660.0)
    env = env_gen(Env.perc(attack=0.01, release=0.4),
                  done_action=DoneAction.FREE_SELF)
    sig = sin_osc(freq) * env * 0.4
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


win_def = plot(ping(), dur=0.6, channels=2)   # two lanes, time ruler, hover it

time.sleep(3)

# %% An envelope, rendered through the engine's own EnvGen — what you plot is
# what an EnvGen plays (the gate closes at the sustain point, so the release
# segments show too).
win_env = plot(Env.adsr(attack=0.05, decay=0.2, sustain=0.6, release=0.4))

time.sleep(3)

# %% Sequences — any iterable of numbers, whatever its range. A pattern is
# materialized (endless ones capped at n); the value axis auto-fits and the
# x ruler reads index counts.
win_seq = plot(Pseq([0, 2, 4, 7, 4, 2], repeats=4))          # a finite pattern
win_rand = plot(Pwhite(40.0, 4700.0), n=200)                 # arbitrary range

time.sleep(3)

# %% The spectrum view — the averaged magnitude spectrum of a short signal,
# here a GraphDef's output (member defs ride along via `defs`).
g = GraphDef("ping_chain")
g.add("ping", {"freq": 880.0})
win_spec = plot(g, defs=[ping()], dur=0.6, channels=2,
                view="spectrum", freq_scale="log")

time.sleep(3)

# %% The display is live: retune a window without re-rendering. The spectrum
# window swaps its frequency axis from log to mel (watch the 880 Hz peak and
# the ruler move)...
win_spec.set(freq_scale="mel")

time.sleep(3)

# %% ...and a *view* can change too: the ping window turns from its two
# waveform lanes into the averaged spectrum.
win_def.set(view="spectrum", freq_scale="mel")

time.sleep(3)

# %% Pin one side of the random sequence's value axis. Far below the data, so
# the pin is unmistakable: the traces squash into the top half.
win_rand.set(min=-4700.0)

time.sleep(3)

# %% ...and "auto" gives the pinned side back to the data fit.
win_rand.set(min="auto")

time.sleep(3)

# %% Close them (or just close the windows / exit the interpreter).
# for w in (win_def, win_env, win_seq, win_rand, win_spec):
#     w.close()

if __name__ == "__main__":
    print("five plot windows are up; close them or Ctrl+C to end")
    try:
        time.sleep(60.0)
    except KeyboardInterrupt:
        pass
