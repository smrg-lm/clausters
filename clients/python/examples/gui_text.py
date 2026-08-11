#!/usr/bin/env python3
"""Text on the light widgets: ``text_size``, ``wrap`` and ``align``.

Every text-bearing light widget -- ``label``, ``button``, ``toggle``, ``text``,
``number``, ``menu`` and the control labels on ``slider``/``knob`` -- takes a
``text_size``: a glyph scale over the host's embedded bitmap font, whose default
2.0 is exactly the size everything drew at before the prop existed.

The face writes **both cases** and the **Latin-1** letters, so a label, a track
name or a file path in Spanish, French or German reads as written. Its cell is 5
columns by a 7-row body -- the height a line reserves -- and a diacritic draws
above that body while a descender draws below it, which is why an accented line
needs no more room than an unaccented one.

``label`` additionally takes:

- ``wrap=True`` -- word wrap on the font's fixed advance (a cheap width
  computation, no shaping); lines past the label's bottom edge are dropped;
- ``align`` -- ``"start"`` (the default left edge), ``"center"`` or ``"end"``,
  applied per line.

Single-line text that overflows its rect -- a long label on a narrow control, a
value read-out wider than a knob -- clips with an ellipsis instead of bleeding
into its neighbor.

The example opens one window showing all of it side by side, then exercises the
props live over ``set`` (a growing title, a re-aligned paragraph) -- addressed by
name, never by id. It **launches its own GUI host** (`GuiHost.boot`) and needs no
audio server: text is pure drawing.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python   # bundles the server + GUI binaries

then run it cell by cell (Shift+Enter) or as a plain script --
``python clients/python/examples/gui_text.py``. Needs a display and a GPU
adapter.
"""

# %%
import sys
import time

from clausters.gui import (GuiHost, button, knob, label, menu, panel, slider,
                           text, toggle, window)

LOREM = ("a wrapped label lays its words out on the font's fixed advance, "
         "drops the lines that overflow its rect, and aligns each line "
         "start, center or end")

# The face, spelled out: both cases, then the accented letters a Latin-1 label
# actually reaches for. Read the descenders (g j p q y) hanging under the body
# box and the marks sitting over it.
ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZ / abcdefghijklmnopqrstuvwxyz / 0123456789"
ACCENTED = "canción, año, ¿qué? ¡olé! Ñandú Ángel  ---  crème brûlée, Grüße, mañana"


# %% [markdown]
# ## The three panels
# All widgets are id-less: the host fills the ids in. Only the two the script
# later drives are *named* (the ``title`` label and the ``center`` paragraph).

# %%
def sizes() -> dict:
    """The same label at growing ``text_size`` -- 1.0 up to 4.0."""
    steps = [1.0, 1.5, 2.0, 3.0, 4.0]
    return panel(*[label(f"text size {s}", text_size=s) for s in steps],
                 layout="col")


def alignments() -> dict:
    """One wrapped paragraph per alignment, side by side."""
    return panel(label(LOREM, wrap=True, align="start"),
                 label(name="center", text=LOREM, wrap=True, align="center"),
                 label(LOREM, wrap=True, align="end"),
                 layout="row")


def alphabet() -> dict:
    """What the face carries, at the size the chrome is drawn at."""
    return panel(label(ALPHABET),
                 label(ACCENTED),
                 # The same string in a field you can type into: the caret and
                 # the selection measure by the cell, so an accent costs no
                 # width and a descender no height.
                 text(name="field", value="una canción, un año, ¡qué más!",
                      label="editable"),
                 layout="col", h=140.0)


def controls() -> dict:
    """The controls at two text sizes, with labels long enough to clip."""
    return panel(slider(label="a deliberately long slider label", value=0.4),
                 knob(label="cutoff", min=20.0, max=20000.0, value=800.0, text_size=3.0),
                 button(label="a very wordy button face"),
                 toggle(label="toggle at 3x", text_size=3.0),
                 menu(["sine", "sawtooth", "square"], label="wave", text_size=3.0),
                 layout="row")


# %% [markdown]
# ## Launch the host and open the window

# %%
gui = GuiHost().boot()
win = gui.open(window(
    label(name="title", text="title", text_size=3.0, align="center", h=40.0),
    sizes(), alphabet(), alignments(), controls(),
    title="Text", w=980, h=820, layout="col"))
print("one window: sizes, the face's own alphabet, wrapped alignments, "
      "and clipped controls")

# %% [markdown]
# ## The props are live
# Retitle bigger and re-align the centered paragraph -- the same keys the GuiDef
# carried, pushed by name through the widget handles.

# %%
win["title"].set(text="TEXT_SIZE IS LIVE", text_size=4.0)
win["center"].set(align="end")

# %% [markdown]
# ## Drive it
# Cell-run: set the named widgets from the cell above. Script-run: replay the
# timed changes once and then hold the window open until you close it.

# %%
_closed = False
win.on_closed(lambda: globals().__setitem__("_closed", True))

# (delay seconds, widget name, props)
CHANGES = [
    (3.0, "title", {"text": "TEXT_SIZE IS LIVE", "text_size": 4.0}),
    (6.0, "center", {"align": "start"}),
    (9.0, "center", {"align": "end"}),
    (12.0, "field", {"value": "el título cambió: ¿seguís ahí?"}),
]


def run(seconds: float | None = None) -> None:
    """Replay the timed changes, then keep pumping.

    ``seconds`` bounds the run from a cell; script-run it is ``None`` and the
    loop ends when **you** close the window -- the text is here to be read, and
    a window that times out is a manual test you cannot finish.
    """
    start = time.monotonic()
    pending = list(CHANGES)
    while not _closed and (seconds is None or time.monotonic() - start < seconds):
        gui.pump(timeout=0.1)
        while pending and time.monotonic() - start > pending[0][0]:
            _, name, props = pending.pop(0)
            win[name].set(**props)
            print(f"set {props} on {name}")


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        gui.close(win)
    sys.exit(0)
else:
    print("text up - run(10) to replay the live changes")
