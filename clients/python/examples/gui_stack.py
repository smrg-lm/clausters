#!/usr/bin/env python3
"""Tabs with no script in the loop: a ``stack`` and a control bound to its index.

Two things meet here. A `stack` shows **one child at a time** — the one its
``index`` names — and everything else it holds is neither laid out nor drawn
while it is away. And a widget can be **bound to another widget**
(`clausters.gui.host.GuiHost.bind_widget`), which applies its value to that
widget's property with no round-trip through this process. Put together, a menu
bound to a stack's ``index`` *is* a tab bar: the pages flip inside the host, and
nothing prints here while you click.

The two pages are the same take seen twice — its waveform and its spectrogram,
the pair the `stack` exists for. Both are **heavy** views, which is what makes
the switch worth watching: a hidden one keeps its GPU slot, so coming back to it
is instant rather than a re-upload. Flip back and forth as fast as you can and
see that neither ever rebuilds.

The bottom half is the same binding aimed at an ordinary prop: a slider bound to
``view_start`` scrolls the time window. It moves **both** pages, not the one
that happens to be showing, because the two name the same ``link`` — a
navigation group, where the window, the selection and the playhead are the
*axis'* and not any one view's. So the slider works on whatever page you are
looking at, and switching pages keeps the position.

That is also why the slider drives a horizontal prop and not a vertical one:
the y axis is deliberately **per view** (a waveform's is amplitude, a
spectrogram's is frequency), so no single number could shift "the center" of
both. What the views genuinely share is time.

A binding fires an **apply, never another binding**, so widgets wired to each
other settle instead of cascading.

No audio server is involved, so this boots only the GUI host.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the GUI binary

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/gui_stack.py``. It self-launches the windowed
host with `GuiHost().boot()`; by hand that is ``clausters-gui``. Needs a display
and a GPU adapter.
"""

# %%
import math
import sys
import time

from clausters.gui import GuiHost, label, menu, panel, slider, spectrogram, stack, view, waveform

SR = 48_000

# %% [markdown]
# ## The take both pages show
# One sweep, in memory: the pages are two views of the *same* samples, which is
# what makes the switch worth having.

# %%
def sweep(seconds: float = 2.0) -> list:
    """A log sine sweep, 40 Hz -> 8 kHz."""
    n = int(seconds * SR)
    phase = 0.0
    out = []
    for i in range(n):
        f = 40.0 * (8000.0 / 40.0) ** (i / n)
        phase += 2.0 * math.pi * f / SR
        out.append(math.sin(phase) * 0.8)
    return out


TAKE = sweep()

#: The window the two pages start zoomed to, and how far the slider can scroll
#: it: a quarter of the take on screen, the rest to the right of it.
WINDOW = len(TAKE) // 4
SCROLL = float(len(TAKE) - WINDOW)

# %% [markdown]
# ## Launch the GUI host

# %%
gui = GuiHost().boot()

# %% [markdown]
# ## The window: a picker over a stack, and a scroll slider under it
# The `menu`'s options are the page names and its index *is* the stack's index,
# which is exactly why the binding is one line and no event handler.
#
# Both views name `link=1`: one navigation group, so the time window belongs to
# the axis rather than to either page.
#
# The stack has no arrangement to make — a page fills it — so it takes only a
# `margin`. Its `weight` gives it the leftover, the way any work surface takes
# the room the chrome does not.

# %%
win = gui.open(view(
    panel(label("view:", w=48.0),
          menu(["waveform", "spectrogram"], name="picker", index=0),
          layout="row", h=32.0),
    stack(waveform(name="wave", data=TAKE, sample_rate=float(SR), ruler="time",
                   link=1),
          spectrogram(data=TAKE, sample_rate=float(SR), ruler="time", link=1),
          name="pages", index=0, weight=1.0),
    panel(label("scroll:", w=64.0),
          slider(name="scroll", min=0.0, max=SCROLL, value=0.0),
          layout="row", h=32.0),
    title="stack: tabs with no script in the loop", w=900, h=560, layout="col"))

# %% [markdown]
# ## The two bindings
# `bind_widget` names the target widget and the property its value lands on.
# From here on the picker flips the page and the slider scrolls the axis
# **inside the host** -- drive either and nothing prints in this process. The
# slider names the waveform, but what it moves is the *group*, so the
# spectrogram follows it whether or not it is the page on screen.

# %%
win["wave"].set(view_len=float(WINDOW))     # zoom the group in, so there is room to scroll
win["picker"].bind_widget(win["pages"], "index")
win["scroll"].bind_widget(win["wave"], "view_start")
print("bound: picker -> pages.index, scroll -> wave.view_start; "
      "flip the pages and drag the slider -- nothing prints here "
      "(no script round-trip)")

# %% [markdown]
# ## Drive it
# The script can still set the page itself -- a binding is another writer of the
# same prop, not an owner of it. `page(1)` from a REPL shows the spectrogram.
#
# Panning or zooming a page with the pointer prints, though: an unbound view
# reports its new window as an event, which is the path the slider replaced.

# %%
_closed = False
win.on_closed(lambda: globals().__setitem__("_closed", True))
win["wave"].on_event(
    lambda tag, *rest: print(f"the axis moved by hand: {tag} {rest}")
    if tag == "view" else None)


def page(index: int) -> None:
    """Show a page from the script (what the bound picker does on its own)."""
    win["pages"].set(index=index)


def run(seconds: float | None = None) -> None:
    """Pumps events for ``seconds`` (only a pointer gesture on a view arrives:
    the two bound widgets send none back).

    Script-run there is no bound and the window is what ends it; the
    ``seconds`` argument is for a cell run, where a notebook wants the loop to
    give the prompt back.
    """
    start = time.monotonic()
    while not _closed and (seconds is None or time.monotonic() - start < seconds):
        gui.pump(timeout=0.1)


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        gui.stop()
else:
    print("stack up - page(1) to switch from here, run(10) to pump, gui.stop() to end")
