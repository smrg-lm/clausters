#!/usr/bin/env python3
"""Play a server instrument from the ``piano`` virtual keyboard.

The ``piano`` widget draws a playable keyboard with **real piano proportions**
(equal white keys; narrower, shorter black keys distributed as on the physical
instrument), so it resizes freely with the window. The strip above the keys is
its "ruler": a miniature of the full 0-127 MIDI range with the visible window
marked — drag it to pan, wheel over it to zoom (wheel over the keys pans by
white keys); ``pan=False`` freezes the range. Keys outside
``active_min``/``active_max`` draw grayed and are inert, showing the range the
instrument actually answers to.

Playing emits **MIDI-shaped** events — ``/gui_event <id> "note" <pitch>
<velocity> <state> <channel>`` (ints; state 1 on press, 0 on release), ready to
be translated 1:1 to MIDI note-on/note-off by a later consumer. Dragging across
keys glissandos; the press height sets the velocity (nearer the front edge =
louder) unless a fixed ``velocity`` prop overrides it.

The mapping to server instruments is **programmable, like every GuiDef**, and
this example shows both paths:

- **Script voices (the event path, shown live)** — the widget stays unbound;
  this script turns each ``"note"`` event into a server voice: ``state 1``
  spawns the gated SynthDef below with ``freq``/``amp`` from pitch/velocity,
  ``state 0`` closes its gate (the envelope releases and the node frees
  itself). Swap the instrument, layer several, or route by ``channel`` — it is
  ordinary client code.
- **Host voices (the standalone path, one line)** — pass
  ``voice="gui_piano_voice"`` to the builder and the *host* manages the same
  ``/s_new`` / ``gate 0`` pair per held key with no script in the loop; a saved
  GuiDef bundle then plays with no language client at all (that is what the
  web example ``clients/web/examples/piano`` does).

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention).
Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the server + GUI binaries

Run it cell by cell (Shift+Enter) and keep playing from the live handles, or as
a plain script — ``python clients/python/examples/gui_piano.py`` — which stays
open for a while, printing the notes as you play, then tears everything down.
Needs a display and a GPU adapter, plus an audio device.
"""

# %%
import sys
import time

from clausters import Session
from clausters.defs import DoneAction, Env, SynthDef, control, env_gen, out, sine
from clausters.gui import label, piano, window

# %% [markdown]
# ## Launch the server and the GUI

# %%
session = Session.live()
server = session.server
gui = session.gui()

# %% [markdown]
# ## The voice a key plays
# A gated ADSR sine: the note-on opens the gate, the note-off closes it and the
# release tail frees the synth (`FREE_SELF`) — the conventional
# ``freq``/``amp``/``gate`` control surface both mapping paths drive.


# %%
def voice(name: str = "gui_piano_voice") -> SynthDef:
    freq = control("freq", 440.0)
    amp = control("amp", 0.2)
    gate = control("gate", 1.0)
    env = env_gen(
        Env.adsr(attack=0.005, decay=0.1, sustain=0.7, release=0.4),
        gate=gate,
        done_action=DoneAction.FREE_SELF,
    )
    sig = sine(freq) * env * amp
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


server.add_synthdef(voice())

# %% [markdown]
# ## Open the keyboard
# Five octaves visible, panning enabled (the overview strip above the keys is
# the navigation surface), and a grayed region outside the 88-key piano range —
# the active range is settable live (``gui.set(10, active_min=..., ...)``).
#
# For the **host-voice** path instead, build it as
# ``piano(10, ..., voice="gui_piano_voice")`` and skip the event loop below.


# %%
def scene() -> dict:
    return window(
        label(1, "click/drag plays; drag the strip to pan, wheel to zoom"),
        piano(10, min=48, max=84, active_min=21, active_max=108, label="keys"),
        title="Piano -> server voices", w=900, h=260, layout="col",
    )


win = gui.open(scene())
print(f"opened window {win} -- play the keys")

# %% [markdown]
# ## Map the note events to server voices
# One held synth per sounding pitch: ``state 1`` spawns it, ``state 0`` gates it
# off. The dict is the whole voice allocator — this is the "programmable like a
# GuiDef" path, plain client code between the event and the server.

# %%
_voices = {}  # sounding pitch -> Synth
_closed = False


def midi_to_hz(note: float) -> float:
    return 440.0 * 2.0 ** ((note - 69.0) / 12.0)


def note_event(pitch: int, velocity: int, state: int, channel: int) -> None:
    if state:
        # A re-press replaces the old voice (its gate closes first).
        note_event(pitch, 0, 0, channel)
        _voices[pitch] = server.synth(
            "gui_piano_voice",
            {"freq": midi_to_hz(pitch), "amp": velocity / 127.0 * 0.3},
        )
        print(f"  note on  {pitch} vel {velocity} ch {channel}")
    else:
        synth = _voices.pop(pitch, None)
        if synth is not None:
            server.set(synth, {"gate": 0.0})
            print(f"  note off {pitch}")


def drain_events() -> None:
    global _closed
    while (msg := gui.poll(0.0)) is not None:
        addr, args = msg
        if addr == "/gui_closed":
            _closed = True
        elif addr == "/gui_event" and len(args) >= 6 and args[1] == "note":
            note_event(int(args[2]), int(args[3]), int(args[4]), int(args[5]))
        elif addr == "/gui_event" and len(args) >= 4 and args[1] == "range":
            print(f"  visible range {args[2]}..{args[3]}")


# %% [markdown]
# ## Plain-script run
# Cell-run: call `drain_events()` between cells while you play. Script-run:
# the loop maps notes for a while, then everything is torn down (any voice
# still sounding is gated off).

# %%
if __name__ == "__main__":
    try:
        deadline = time.monotonic() + 45.0
        while time.monotonic() < deadline and not _closed:
            drain_events()
            time.sleep(0.02)
        for pitch in list(_voices):
            note_event(pitch, 0, 0, 0)
        gui.close(win)
        session.close()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
