#!/usr/bin/env python3
"""Watching a live bus with the free-standing ``scope`` — the three views.

`clausters.scope` is the real-time sibling of `clausters.plot`: where `plot`
draws a finished render, `scope` opens a window that follows a **live audio
bus**, frame by frame. One call resolves the ambient server and GUI host,
takes a free audio tap from the server's registry (``server.taps``), routes
the bus into it (``/tap``) and opens the window; closing the handle stops the
tap and returns it. The host reads the tap rings straight out of the server's
shared-memory segment — zero messages per frame.

This is a **sequential visual tour**: each window appears alone, announces
itself on the console, makes one live change *and comes back* (the trace
window, the stereo width, the frequency scale — all through ``win.set`` or
``/n_set``), and closes before the next one opens.

Run it as a script (``python scoping.py``) or cell by cell (``# %%``). Needs a
display, a GPU adapter and an audio device; the install bundles both binaries.
It plays a quiet drone throughout.
"""

# %% Setup: a server, and a quiet stereo drone to watch.
import time

from clausters import Server, scope
from clausters.defs import SynthDef, control, out, sin_osc

#: Seconds each visual step stays on screen.
PAUSE = 3.0

server = Server.boot()

# Left is a plain sine; right crossfades (with `spread`) from a copy of the
# left (mono — a phasescope draws a vertical line) to a detuned sine
# (decorrelated — the trace opens into the lozenge). Quiet on purpose.
freq = control("freq", 220.0)
spread = control("spread", 0.0)
left = sin_osc(freq)
right = left * (1.0 - spread) + sin_osc(freq * 1.02) * spread
drone = SynthDef("scoping_drone",
                 out(0.0, left * 0.1),
                 out(1.0, right * 0.1))
server.add_synthdef(drone)
server.sync()
node = server.synth("scoping_drone")

# %% 1/3 — the oscilloscope (view="signal"): a stable, triggered trace.
print("1/3 signal: an oscilloscope on bus 0, 20 ms window, trigger at 0")
win = scope(0)
time.sleep(PAUSE)
print("    window_ms -> 5 (a few cycles fill the field)")
win.set(window_ms=5.0)
time.sleep(PAUSE)
print("    window_ms -> 20 (round trip), and the window closes")
win.set(window_ms=20.0)
time.sleep(PAUSE)
win.close()

# %% 2/3 — the phasescope (view="phase"): the stereo field of buses 0/1.
print("2/3 phase: mono reads as a vertical line (correlation ~ +1)")
win = scope(0, view="phase")
time.sleep(PAUSE)
print("    spread -> 1 (/n_set): the field opens, the correlation drops")
server.set(node, {"spread": 1.0})
time.sleep(PAUSE)
print("    spread -> 0 (round trip), and the window closes")
server.set(node, {"spread": 0.0})
time.sleep(PAUSE)
win.close()

# %% 3/3 — the live spectrum (view="spectrum"): the partials, per frame.
print("3/3 spectrum: the 220 Hz peak on a log axis")
win = scope(0, view="spectrum")
time.sleep(PAUSE)
print("    freq_scale -> mel (watch the peak move), fft_size -> 4096")
win.set(freq_scale="mel", fft_size=4096)
time.sleep(PAUSE)
print("    freq_scale -> log (round trip), and the window closes")
win.set(freq_scale="log", fft_size=2048)
time.sleep(PAUSE)
win.close()

# %% Teardown.
server.free(node)
server.close()
print("done — three scopes appeared, retuned live and closed; taps all freed")
