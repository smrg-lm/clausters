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
from clausters.defs import SynthDef, control, lag, out, sine
from clausters.defs import Synth

#: Seconds each visual step stays on screen.
PAUSE = 4.0

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
left = sine(freq)
right = left * (1.0 - spread) + sine(freq * 1.02) * spread
drone = SynthDef("scoping_drone",
                 out(0.0, left * amp),
                 out(1.0, right * amp))
drone.send(server)
server.sync()
node = Synth.new("scoping_drone", server=server)

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
node.set({"spread": 1.0})
time.sleep(PAUSE)
print("    spread -> 0 (round trip), and the window closes")
node.set({"spread": 0.0})
time.sleep(PAUSE)
win.close()

# %% 3/3 — the live spectrum (view="spectrum"): one curve per channel, and
# the four frequency scales in turn. The corner read-out names the FFT size
# and the active scale (e.g. "2048 LOG").
print("3/3 spectrum: outs 0/1, the 220 Hz peak — the corner reads '2048 LOG'")
node.set({"spread": 1.0})   # detune R so the two curves differ
win = scope(0, view="spectrum", channels=2)
time.sleep(PAUSE)
for scale in ("lin", "mel", "bark"):
    print(f"    freq_scale -> {scale} (watch the peak, the ruler and the tag)")
    win.set(freq_scale=scale)
    time.sleep(PAUSE)
print("    back to log with fft_size -> 4096 ('4096 LOG'), and it closes")
win.set(freq_scale="log", fft_size=4096)
time.sleep(PAUSE)
win.close()

# %% Teardown: fade to silence before freeing, so the cut is inaudible too.
node.set({"amp": 0.0})
time.sleep(0.3)
node.free()
server.close()
print("done — three scopes appeared, retuned live and closed; taps all freed")
