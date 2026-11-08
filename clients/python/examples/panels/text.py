#!/usr/bin/env python3
"""Text on the light widgets: ``text_size``, ``wrap`` and ``align``.

Every text-bearing light widget -- ``label``, ``button``, ``toggle``, ``text``,
``number``, ``choice`` and the control labels on ``slider``/``knob`` -- takes a
``text_size``: a glyph scale over the host's font, whose default 1.5 writes
capitals three quarters of the line they stand in.

**Text is shaped before it is drawn.** The host writes Latin, Greek and Cyrillic,
Arabic and Hebrew with the face it ships, and Devanagari with a second face it
falls back to: Arabic letters take the forms that join them, a combining mark
stands over its base, a Devanagari vowel sign is drawn where it is read, and a
right-to-left word reads from the right, inside a left-to-right line too. A line
reserves the same height whatever it holds: a diacritic draws above it and a
descender below, so an accented line needs no more room than an unaccented one.

``label`` additionally takes:

- ``wrap=True`` -- word wrap on the width of the words as they are drawn; lines
  past the label's bottom edge are dropped;
- ``align`` -- ``"start"`` (the default left edge), ``"center"`` or ``"end"``,
  applied per line.

Single-line text that overflows its rect -- a long label on a narrow control, a
value read-out wider than a knob -- clips with an ellipsis instead of bleeding
into its neighbor.

**The same window in the bitmap face.** A GUI host built without its default
``font-atlas`` feature draws a 5x7 bitmap face instead of the faces it ships:
Latin-1 and no shaping, and ``text_size`` on half-steps -- nothing else changes,
because the sizing table never followed the typeface. To see this window that
way, build the host without the feature and point the launcher at it::

    cd clients/gui && cargo build --release --no-default-features --bin clausters-gui
    CLAUSTERS_GUI_BIN=$PWD/target/release/clausters-gui \
        python clients/python/examples/panels/text.py

The example opens one window showing all of it side by side, then exercises the
props live over ``set`` (a growing title, a re-aligned paragraph) -- addressed by
name, never by id. It **launches its own GUI host** (`GuiHost.boot`) and needs no
audio server: text is pure drawing.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python   # bundles the server + GUI binaries

then run it cell by cell (Shift+Enter) or as a plain script --
``python clients/python/examples/panels/text.py``. Needs a display and a GPU
adapter.
"""

# %%
import sys

from clausters.gui import (GuiHost, button, choice, knob, label, panel, slider, text, toggle, view)

LOREM = ("a wrapped label lays its words out on their own measured width, "
         "drops the lines that overflow its rect, and aligns each line "
         "start, center or end")

# The face, spelled out: both cases, then the accented letters a Latin-1 label
# actually reaches for. Read the descenders (g j p q y) hanging under the body
# box and the marks sitting over it.
ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZ / abcdefghijklmnopqrstuvwxyz / 0123456789"
ACCENTED = "canción, año, ¿qué? ¡olé! Ñandú Ángel  ---  crème brûlée, Grüße, mañana"

# Text that is not a row of its characters. Arabic joins its letters; Hebrew
# and Arabic read from the right; Devanagari writes the vowel sign i after its
# consonant and draws it before; and a mark with no precomposed letter stands
# over its base. The last line is one left-to-right line with a Hebrew word in
# it, which still reads from the right.
ARABIC = "مرحبا بالعالم"
HEBREW = "שלום עולם"
DEVANAGARI = "नमस्ते दुनिया, किताब"
MARKS = "q̃ x̄ ŋ̊ ǫ́ ẏ̆"
MIXED = "play שלום twice, then stop"


# %% [markdown]
# ## The panels
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
                 # The same string in a field you can type into: the caret
                 # steps over a letter and its accent at once, and an accent
                 # costs no width and a descender no height.
                 text(name="field", value="una canción, un año, ¡qué más!",
                      label="editable"),
                 layout="col", h=140.0)


def scripts() -> dict:
    """The writing systems the host shapes, and a line that mixes two."""
    return panel(label(ARABIC), label(HEBREW), label(DEVANAGARI), label(MARKS),
                 # The mixed line in a field: the arrows walk it in the order it
                 # was typed, so inside the Hebrew word the caret moves left,
                 # and a selection across the word paints both runs.
                 text(name="mixed", value=MIXED, label="editable, mixed"),
                 layout="col", h=170.0)


def controls() -> dict:
    """The controls at two text sizes, with labels long enough to clip."""
    return panel(slider(label="a deliberately long slider label", value=0.4),
                 knob(label="cutoff", min=20.0, max=20000.0, value=800.0, text_size=3.0),
                 button(label="a very wordy button face"),
                 toggle(label="toggle at 3x", text_size=3.0),
                 choice(["sine", "sawtooth", "square"], label="wave", text_size=3.0),
                 layout="row")


# %% [markdown]
# ## Launch the host and open the window

# %%
gui = GuiHost().boot()
win = view(
    label(name="title", text="title", text_size=3.0, align="center", h=40.0),
    sizes(), alphabet(), scripts(), alignments(), controls(),
    title="Text", w=980, h=980, layout="col").open()
print("one window: sizes, the face's own alphabet, the scripts it shapes, "
      "wrapped alignments, and clipped controls")

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

# (delay seconds, widget name, props)
CHANGES = [
    (3.0, "title", {"text": "TEXT_SIZE IS LIVE", "text_size": 4.0}),
    (6.0, "center", {"align": "start"}),
    (9.0, "center", {"align": "end"}),
    (12.0, "field", {"value": "el título cambió: ¿seguís ahí?"}),
]


def change(name: str, props: dict) -> None:
    win[name].set(**props)
    print(f"set {props} on {name}")


def run(seconds: float | None = None) -> None:
    """Put the timed changes on the clock, then hold the window open.

    The delays are `clausters.base.appclock.AppClock.sched`, so each change runs
    on the host's own event-loop thread when its moment comes and nothing here
    keeps time. ``seconds`` bounds the wait from a cell; script-run it is
    ``None`` and it ends when **you** close the window -- the text is here to be
    read, and a window that times out is a manual test you cannot finish.
    """
    for delay, name, props in CHANGES:
        gui.clock.sched(delay, lambda n=name, p=props: change(n, p))
    win.wait(seconds)


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        gui.close(win)
    sys.exit(0)
else:
    print("text up - run(10) to replay the live changes")
