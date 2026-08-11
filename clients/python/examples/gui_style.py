#!/usr/bin/env python3
"""Style: theme groups, per-widget accents, and the two paint props.

The host draws every chrome color from one **theme** -- a table of named roles
(``background``, ``panel``, ``text``, ``accent``, ...) -- and the customization
is the same partial table at every level, each overlaying the previous:

1. **The host style file** (``--theme file.toml``, or ``[gui.theme]`` in the
   shared config): one look for the whole host. This example writes a small
   file that warms the accent and boots the host with it.
2. **A theme group** (the ``theme`` prop on a container -- ``window``, ``panel``,
   ``scroll``, ``track``): a partial ``{"role": "#rrggbb[aa]"}`` table scoped
   to that subtree, recursive by construction -- a nested group overlays the
   *inherited* table, not the default.
3. **The `color` prop** (any widget): the single-color shorthand -- it re-seeds
   just the roles that carry the widget's function (a slider's handle and
   fill, a meter's bar, a trace), leaving the rest of its theme alone.

Overlays resolve when a def arrives or a ``set`` changes them -- never per
frame -- so a styled window costs exactly what a plain one costs.

**Two more props shape the drawing rather than its color**, on any widget and
live the same way:

- ``opacity`` (0..1) fades a widget. Like a theme group it is a *group's*
  property -- it multiplies down the whole subtree, so a control at 0.5 inside
  a panel at 0.5 draws at 0.25 -- and a negative number clears it. It fades the
  flat drawing (chrome, controls, text); a heavy view's own picture keeps its
  opacity, and two overlapping shapes inside a faded widget show through each
  other, because the fade is per-shape rather than a layer.
- ``radius`` rounds the corners of the boxes a widget draws, in logical pixels.
  It applies to that widget alone -- a rounded panel says nothing about the
  controls in it -- and each box clamps it to half its shorter side, so a
  widget's own frame and focus ring round while the hairlines inside it (a
  divider, a tick, a track edge) keep their shape.

**Antialiasing is the host's, not a prop**: the window's whole render pass is
multisampled, so it is one launch flag (``--msaa 4`` below, or ``msaa = 4``
under ``[gui]``) costing one attachment per window and nothing per widget. The
rounded corners are what shows it -- drop the flag from the boot line below and
they come back stepped.

The window shows five rows: the host look (from the file theme), a cool theme
group, a nested darker group inside it, a row of per-widget accents, and a row
of rounded, faded panels. After a few seconds the script restyles the group and
one accent live via ``set``, addressed by name (the ``cool`` group, the
``accent_a`` slider), and fades a panel.

The example **launches its own GUI host** and needs no audio server.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python   # bundles the server + GUI binaries

then run it cell by cell (Shift+Enter) or as a plain script --
``python clients/python/examples/gui_style.py``. Needs a display and a GPU
adapter.
"""

# %%
import sys
import tempfile
import time
from pathlib import Path

from clausters.gui import GuiHost, knob, label, panel, slider, toggle, window

# Level 1: the host style file -- the whole host warms up.
HOST_THEME = """\
accent = "#e08840"
accent_dim = "#8a5428"
hilite = "#f0a060"
"""

# Level 2: a theme group -- a cool pane inside the warm host.
COOL = {"accent": "#4090e0", "accent_dim": "#285a8a", "hilite": "#60b0f0",
        "panel": "#10141cbf"}
# Level 3: nested -- the inherited cool table, darkened further.
NESTED = {"panel": "#0a0d12", "text": "#8098b0"}


# %% [markdown]
# ## The window
# The widgets are id-less (the host fills the ids in); only the two the script
# later restyles are *named* -- the ``cool`` theme group and the ``accent_a``
# slider.

# %%
def controls(tag: str) -> dict:
    """One row of ordinary controls; the theme in force colors all of them."""
    return panel(label(tag, w=220.0),
                 slider(label="amp", value=0.6),
                 knob(label="freq", min=20.0, max=2000.0, value=440.0),
                 toggle(label="on", value=True),
                 layout="row")


# %% [markdown]
# ## Launch the host with the file theme and open the window

# %%
theme_file = Path(tempfile.mkdtemp(prefix="clausters-style-")) / "warm.toml"
theme_file.write_text(HOST_THEME)
# `--msaa 4` antialiases the whole pass: the rounded corners below are its
# by-eye test. Drop it and the same window draws its arcs stepped.
gui = GuiHost().boot(extra_args=("--theme", str(theme_file), "--msaa", "4"))

win = gui.open(window(
    controls("host theme (file)"),
    panel(controls("theme group (cool)"),
          panel(controls("nested group (darker)"), layout="col", theme=NESTED),
          name="cool", layout="col", theme=COOL),
    panel(label("per-widget accents", w=220.0),
          slider(name="accent_a", label="a", value=0.3, color="#e04060"),
          slider(label="b", value=0.5, color="#40c080"),
          slider(label="c", value=0.7, color="#c0b040"),
          layout="row"),
    # The paint props: a rounded panel whose controls stay square (the radius
    # is the widget's own), and one that starts half faded -- the fade reaches
    # every control in it, since opacity is a group's property.
    panel(panel(label("rounded", w=110.0), toggle(label="on", value=True),
                layout="row", radius=10.0),
          panel(label("faded", w=110.0), slider(label="amp", value=0.4),
                name="ghost", layout="row", radius=10.0, opacity=0.35),
          layout="row"),
    title="Style", w=980, h=620, layout="col"))
print("host warm (file theme); one cool theme group, nested darker; three "
      "per-widget accents; a rounded panel and a faded one. "
      "Live restyles follow.")

# %% [markdown]
# ## Live restyles, by name
# Turn the cool group violet, then one accent cyan, then clear the group back to
# the host theme -- each a ``set`` on a named widget.

# %%
win["cool"].set(theme='{"accent": "#b060e0", "accent_dim": "#6a3a8a", '
                      '"hilite": "#d090f0", "panel": "#161020bf"}')
win["accent_a"].set(color="#40d0d0")

# %% [markdown]
# ## Drive it
# Cell-run: restyle from the cell above. Script-run: replay the timed restyles
# once, pumping for the window-close in between.

# %%
_closed = False
win.on_closed(lambda: globals().__setitem__("_closed", True))

# (delay seconds, widget name, props, message)
CHANGES = [
    (4.0, "cool", {"theme": '{"accent": "#b060e0", "accent_dim": "#6a3a8a", '
                            '"hilite": "#d090f0", "panel": "#161020bf"}'},
     "the cool group turns violet (a set of its theme prop)"),
    (8.0, "accent_a", {"color": "#40d0d0"}, "accent 'a' turns cyan (a set of color)"),
    (12.0, "cool", {"theme": ""}, "the group clears: back to the host theme"),
    (16.0, "ghost", {"opacity": 1.0}, "the faded panel comes back to full opacity"),
    (20.0, "ghost", {"radius": -1.0}, "and its corners square off (a negative clears)"),
]


def run(seconds: float | None = None) -> None:
    start = time.monotonic()
    pending = list(CHANGES)
    while not _closed and (seconds is None or time.monotonic() - start < seconds):
        gui.pump(timeout=0.1)
        while pending and time.monotonic() - start > pending[0][0]:
            _, name, props, what = pending.pop(0)
            win[name].set(**props)
            print(what)


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        gui.close(win)
    sys.exit(0)
else:
    print("style up - run(15) to replay the restyles")
