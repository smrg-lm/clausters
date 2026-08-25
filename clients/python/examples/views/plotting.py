#!/usr/bin/env python3
"""Quick looks with the free-standing ``plot`` — defs, envelopes, sequences.

`clausters.plot` is the visual sibling of `clausters.play`: one verb that opens
**its own window** for whatever you hand it, booting the GUI host lazily on
first use (no audio server involved — a def is rendered *offline* by the
bundled NRT renderer and reaches the host as a mapped file). The returned
handle retunes the display live and closes the window.

This is a **sequential visual tour**: each window appears alone, announces
itself on the console, makes one live change *and comes back* (view, pinned
axis side, frequency scale — all through ``win.set``, no re-render), and
closes before the next one opens. While a window is up, hover the trace: a
hairline names the exact sample under the cursor (index or clock time, and the
value; on a spectrum view, the bin's frequency in the chosen scale and its
level in dB).

Run it as a script (``python plotting.py``) or cell by cell (``# %%``). Needs
a display and a GPU adapter; the install bundles the GUI binary.
"""

# %% Setup.
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
    sine,
)
from clausters.seq import Pseq, Pwhite

#: Seconds each visual step stays on screen.
PAUSE = 3.0


def ping(name: str = "ping") -> SynthDef:
    """A sine ping shaped by a percussive envelope, self-freeing."""
    freq = control("freq", 660.0)
    env = env_gen(Env.perc(attack=0.01, release=0.4),
                  done_action=DoneAction.FREE_SELF)
    sig = sine(freq) * env * 0.4
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


# %% 1/5 — a def's rendered output: eyeball what a SynthDef actually produces.
print("1/5 ping (SynthDef, rendered offline): two channel lanes, time ruler")
win_def = plot(ping(), dur=0.6, channels=2)
time.sleep(PAUSE)
print("    view -> spectrum (the same render, analyzed)")
win_def.set(view="spectrum")
time.sleep(PAUSE)
print("    view -> signal (round trip), and the window closes")
win_def.set(view="signal")
time.sleep(PAUSE)
win_def.close()

# %% 2/5 — an envelope, rendered through the engine's own EnvGen (the gate
# closes at the sustain point, so the release segments show too).
print("2/5 env (adsr through the engine's EnvGen): the whole curve")
win_env = plot(Env.adsr(attack=0.05, decay=0.2, sustain=0.6, release=0.4))
time.sleep(PAUSE)
print("    min -> -1.0 (pin the floor: the curve squashes into the top half)")
win_env.set(min=-1.0)
time.sleep(PAUSE)
print("    min -> auto (the fit gets the side back), and the window closes")
win_env.set(min="auto")
time.sleep(PAUSE)
win_env.close()

# %% 3/5 — a finite pattern: any iterable of numbers plots as a sequence,
# index counts on the x ruler.
print("3/5 sequence (Pseq): stepped values over an index axis")
win_seq = plot(Pseq([0, 2, 4, 7, 4, 2], repeats=4))
time.sleep(PAUSE)
win_seq.close()

# %% 4/5 — an arbitrary-range sequence: the value axis auto-fits, whatever
# the range; a pinned side is released live with "auto".
print("4/5 sequence (Pwhite 40..4700): the value axis auto-fits the range")
win_rand = plot(Pwhite(40.0, 4700.0), n=200)
time.sleep(PAUSE)
print("    min -> -4700 (pin far below the data: traces squash up)")
win_rand.set(min=-4700.0)
time.sleep(PAUSE)
print("    min -> auto (round trip), and the window closes")
win_rand.set(min="auto")
time.sleep(PAUSE)
win_rand.close()

# %% 5/5 — the spectrum view of a GraphDef's output (member defs ride along
# via `defs`), retuning the frequency scale live.
print("5/5 ping_chain (GraphDef spectrum): the 880 Hz peak on a log axis")
g = GraphDef("ping_chain")
g.add("ping", {"freq": 880.0})
win_spec = plot(g, defs=[ping()], dur=0.6, channels=2,
                view="spectrum", freq_scale="log")
time.sleep(PAUSE)
print("    freq_scale -> mel (watch the peak and the ruler move)")
win_spec.set(freq_scale="mel")
time.sleep(PAUSE)
print("    freq_scale -> log (round trip), and the window closes")
win_spec.set(freq_scale="log")
time.sleep(PAUSE)
win_spec.close()

print("done — five windows appeared, retuned live and closed")
