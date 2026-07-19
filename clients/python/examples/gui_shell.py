#!/usr/bin/env python3
"""The application shell: menu bar, working area, status bar — layout props.

Every widget takes the **generic place props** and every container the flow
props, so a GuiDef composes a real application face from the same light
elements a control panel uses:

- ``w``/``h`` fix a child's main-axis size in a ``row``/``col`` — the menu bar
  and the status bar here are ``h``-fixed rows;
- ``weight`` shares the leftover among the flexible children (default 1) —
  the working area takes everything between the two bars, and inside it the
  sidebar is ``w``-fixed while the scope stretches;
- ``margin``/``gap`` tune a container's inset and spacing (the shell sets both
  to 0 so the bars run edge to edge, and reintroduces them inside);
- in a ``free`` container, ``x``/``y`` (+ ``w``/``h``) place a child
  absolutely — not used here, same props, different layout.

Everything is live: the sidebar's controls retune a quiet server voice, the
oscilloscope draws the server's **actual stereo output** (two audio taps on
buses 0/1, read by the host from shared memory — zero per-frame messages),
and the status bar is a plain ``label`` the script rewrites via ``set`` on
every event — the whole "application" is one GuiDef plus ordinary client
code.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention).
Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the server + GUI binaries

Run it cell by cell (Shift+Enter), or as a plain script —
``python clients/python/examples/gui_shell.py`` — which stays open for a
while, then tears everything down. Needs a display, a GPU adapter and an
audio device.
"""

# %%
import sys
import time

from clausters import Session
from clausters.defs import DoneAction, Env, SynthDef, control, env_gen, out, sine
from clausters.gui import button, knob, label, menu, panel, scope, slider, window

# %% [markdown]
# ## Launch the server and the GUI
# `Session.live()` boots the server with a shared-memory segment (`shm="auto"`),
# and `session.gui()` maps the same segment — the oscilloscope reads the audio
# taps straight from it.

# %%
session = Session.live()
server = session.server
gui = session.gui()

# %% [markdown]
# ## A quiet voice to drive from the shell
# A gated sine (amp well below the default — this is a layout demo, not a
# listening test) with the conventional `freq`/`amp`/`gate` surface.

# %%
def voice(name: str = "gui_shell_voice") -> SynthDef:
    freq = control("freq", 220.0)
    amp = control("amp", 0.08)
    gate = control("gate", 1.0)
    env = env_gen(Env.asr(attack=0.05, sustain=1.0, release=0.3), gate=gate,
                  done_action=DoneAction.FREE_SELF)
    sig = sine(freq) * env * amp
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


server.add_synthdef(voice())

# Two adjacent audio taps on the output buses: the oscilloscope's source.
tap0 = server.taps.alloc(2)
for k in range(2):
    server.tap(tap0 + k, k)

# %% [markdown]
# ## The shell
# A `col` window with `margin=0, gap=0`: an `h`-fixed menu bar, a `weight`ed
# working row (a `w`-fixed sidebar + a stretching scope), an `h`-fixed status
# bar. Only the containers reintroduce margins for their own contents.

# %%
wave_menu = menu(None, ["sine"], w=120)
play_btn = button(label="play", w=80)
stop_btn = button(label="stop", w=80)
menu_bar = panel(None, wave_menu, play_btn, stop_btn,
                 label(None, "gui_shell - the application shell", weight=1.0),
                 layout="row", h=40, gap=4)

freq_knob = knob(label="freq", min=55.0, max=880.0, value=220.0)
amp_slider = slider(label="amp", min=0.0, max=0.15, value=0.08)
sidebar = panel(None, freq_knob, amp_slider, layout="col", w=190)

out_scope = scope(tap=tap0, channels=2, window_ms=25.0, label="output")
work_area = panel(None, sidebar, out_scope, layout="row", weight=1.0, gap=4)

status = label(None, "ready", h=24)

win = gui.open(window(menu_bar, work_area, status,
                      title="gui_shell", w=760, h=420, layout="col",
                      margin=0, gap=0))
print(f"opened window {win}")

# %% [markdown]
# ## Drive it
# The script is the application logic: button presses start/stop the voice,
# the knob and slider retune it, and every action rewrites the status label —
# `set(status, text=...)` is the whole status-bar API. The oscilloscope needs
# nothing from this loop — the host reads the taps from the segment on its own.

# %%
_voice = None
_closed = False


def set_status(text: str) -> None:
    gui.set(status["id"], text=text)


def on_event(widget_id: int, value) -> None:
    global _voice
    if widget_id == play_btn["id"] and _voice is None:
        _voice = server.synth("gui_shell_voice", {
            "freq": float(freq_knob["value"]), "amp": float(amp_slider["value"]),
        })
        set_status("playing")
    elif widget_id == stop_btn["id"] and _voice is not None:
        server.set(_voice, {"gate": 0.0})
        _voice = None
        set_status("stopped")
    elif widget_id == freq_knob["id"]:
        freq_knob["value"] = value
        if _voice is not None:
            server.set(_voice, {"freq": float(value)})
        set_status(f"freq {value:.1f} Hz")
    elif widget_id == amp_slider["id"]:
        amp_slider["value"] = value
        if _voice is not None:
            server.set(_voice, {"amp": float(value)})
        set_status(f"amp {value:.3f}")


def run(seconds: float) -> None:
    """Dispatches shell events for ``seconds``."""
    global _closed
    start = time.monotonic()
    while time.monotonic() - start < seconds and not _closed:
        msg = gui.poll(timeout=0.03)
        if msg is None:
            continue
        addr, args = msg
        if addr == "/gui_closed":
            _closed = True
        elif addr == "/gui_event" and len(args) >= 2:
            on_event(int(args[0]), args[1])


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run(30.0)
    finally:
        if _voice is not None:
            server.set(_voice, {"gate": 0.0})
        session.close()
else:
    print("shell up - run(10) to dispatch events, session.close() to end")
