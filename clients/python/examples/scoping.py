#!/usr/bin/env python3
"""Watching live buses with the free-standing ``scope`` — the three views.

`clausters.scope` is the real-time sibling of `clausters.plot`: where `plot`
draws a finished render, `scope` opens a window that follows **live audio
buses**, frame by frame. One rule covers every view: the verb monitors
``channels`` consecutive buses from ``bus`` (each on its own audio tap, taken
from the server's registry and released on ``win.close()``), and each view
presents them its way — oscilloscope lanes, the stereo phase field, one
spectrum curve per channel. The host reads the tap rings straight out of the
server's shared-memory segment — zero messages per frame.

This is a **sequential visual tour**: each window appears alone, announces
itself on the console, makes one live change *and comes back* (the trace
window, the overlay, the stereo width, the frequency scale — all through
``win.set`` or ``/n_set``), and closes before the next one opens. Watch the
oscilloscope's corner read-out: ``lock`` means the trigger found a rising
crossing and the trace stands still (the faint line marks the level);
``free`` means it is free-running (silence or DC).

Run it as a script (``python scoping.py``) or cell by cell (``# %%``). Needs a
display, a GPU adapter and an audio device; the install bundles both binaries.
It plays a quiet drone throughout.
"""

# %% Setup: a server, and a quiet stereo drone to watch.
import time

from clausters import Server, scope
from clausters.defs import SynthDef, control, lag, out, sin_osc

#: Seconds each visual step stays on screen.
PAUSE = 3.0

server = Server.boot()

# Left is a plain sine; right crossfades (with `spread`) from a copy of the
# left (mono — a phasescope draws a vertical line) to a detuned sine
# (decorrelated — the trace opens into the lozenge). Quiet on purpose.
# The control is lagged: an /n_set lands as a step, and stepping a crossfade
# clicks — smoothing it is the def's job (the scopes themselves are passive:
# a tap only copies a bus into shared memory and can never alter the sound).
freq = control("freq", 220.0)
spread = lag(control("spread", 0.0), 0.1)
amp = lag(control("amp", 0.1), 0.1)
left = sin_osc(freq)
right = left * (1.0 - spread) + sin_osc(freq * 1.02) * spread
drone = SynthDef("scoping_drone",
                 out(0.0, left * amp),
                 out(1.0, right * amp))
server.add_synthdef(drone)
server.sync()
node = server.synth("scoping_drone")

# %% 1/3 — the oscilloscope (view="signal"): both channels, phase-locked.
print("1/3 signal: outs 0/1 as two lanes; 'lock' + the trigger line at 0")
win = scope(0, channels=2)
time.sleep(PAUSE)
print("    window_ms -> 5 (the ms ruler follows; a few cycles fill it)")
win.set(window_ms=5.0)
time.sleep(PAUSE)
print("    overlay -> on (both channels as colored traces in one field)")
win.set(overlay=1, window_ms=20.0)
time.sleep(PAUSE)
print("    overlay -> off (round trip), and the window closes")
win.set(overlay=0)
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

# %% 3/3 — the live spectrum (view="spectrum"): one curve per channel.
print("3/3 spectrum: outs 0/1, the 220 Hz peak on a log axis with Hz/dB rulers")
server.set(node, {"spread": 1.0})   # detune R so the two curves differ
win = scope(0, view="spectrum", channels=2)
time.sleep(PAUSE)
print("    freq_scale -> mel (watch the peak and ruler move), fft_size -> 4096")
win.set(freq_scale="mel", fft_size=4096)
time.sleep(PAUSE)
print("    freq_scale -> log (round trip), and the window closes")
win.set(freq_scale="log", fft_size=2048)
time.sleep(PAUSE)
win.close()

# %% Teardown: fade to silence before freeing, so the cut is inaudible too.
server.set(node, {"amp": 0.0})
time.sleep(0.3)
server.free(node)
server.close()
print("done — three scopes appeared, retuned live and closed; taps all freed")
