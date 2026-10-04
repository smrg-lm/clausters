#!/usr/bin/env python3
"""An application's face: every element of chrome, in one window.

A work surface is the middle of a window and the rest of it is **chrome**: the
menus that name what can be done, the tools that do it, the tabs and dividers
that say how the room is shared, the dialog that asks before something is lost.
This window holds one of each, laid out the way an application lays them out:

- a **menu bar** along the top -- the window's own ``menu``, drawn by the host
  as a band like the status bar at the bottom. Its entries are actions, a
  check, a group where one of several is on, a submenu, and a disabled one;
- a **toolbar** of flat buttons whose faces are glyphs (``ICON``), split into
  groups by separators, with a spring pushing the last group to the far edge
  and a button that opens a menu of its own;
- a **split** row: drag the divider between the sidebar and the pages;
- a sidebar of **sections** that fold on their title strips;
- **tabs** over five pages: one choice in each of its six views, the controls
  with their other looks, a plane with **scroll bars**, a **pager**, and rows
  of data -- a **table** whose order is the script's and a **tree**;
- a **context menu** on the pages (the secondary button), and a **tip** on
  every tool (rest the pointer on one);
- a **dialog**, opened by ``About`` and closed by freeing it, and a **file
  chooser** in another, opened by ``File > Open``: the host lists the
  directory itself -- the disk here, the page's own storage in the twin page.

It also shows what they share. Every control takes the focus: Tab walks them,
Space or Enter presses, the arrows step. A press on a title of the bar opens
its list and the arrows walk it; Escape closes it. And nothing here keeps state
for the script: a folded section, a moved divider and a checked entry are props
the host reports when a hand changes them -- the lines this prints.

No audio server is involved, so this boots only the GUI host.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the GUI binary

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/panels/chrome.py``. It self-launches the
windowed host with `GuiHost().boot()`; by hand that is ``clausters-gui``. Needs
a display and a GPU adapter.
"""

# %%
import sys

from clausters.gui import (ICON, GuiHost, button, choice, dialog, entry, file_dialog, knob,
                           label, layout, menu, number, pager, panel, progress, row, scroll,
                           separator, slider, table, tabs, text, toggle, toolbar, view)

#: The options every view of the one choice is drawn over.
WAVES = ["sine", "saw", "square", "noise"]

#: The rows of the data page's table: a take's name, its length, its rate.
TAKES = [["kick", "0.4 s", "48 kHz"], ["snare", "0.3 s", "48 kHz"],
         ["pad", "8.0 s", "44.1 kHz"], ["voice", "2.1 s", "48 kHz"]]

# %% [markdown]
# ## Launch the GUI host
# `GuiHost().boot()` starts a windowed `clausters-gui` process and returns a host
# connected to it (stopped by `stop`, or on interpreter exit).

# %%
gui = GuiHost().boot()

# %% [markdown]
# ## The menus
# A menu is a **tree of entries** and the value of a prop, not a widget: it
# opens over the window, so nothing lays it out. `entry(label, verb)` is an
# action; `checked=` makes it a check, `group=` one of several, `menu=` a
# submenu, and `"-"` is a separator. What a pick reports is the **verb**.

# %%
bar = menu(
    entry("File", menu=menu(
        entry("New", "new"),
        entry("Open", "open"),
        entry("Recent", menu=menu("take 1.wav", "take 2.wav", "take 3.wav")),
        "-",
        entry("Export", "export", enabled=False),
        "-",
        entry("Quit", "quit"))),
    entry("Edit", menu=menu(
        entry("Undo", "undo", icon=ICON.left),
        entry("Redo", "redo", icon=ICON.right, enabled=False),
        "-",
        entry("Loop", "loop", checked=False))),
    entry("View", menu=menu(
        entry("Waveform", "view wave", group="view", checked=True),
        entry("Spectrogram", "view spectrum", group="view"),
        "-",
        entry("Rulers", "rulers", checked=True))),
    entry("About", "about"))

#: What the secondary button opens over the pages.
on_pages = menu(entry("Copy", "copy"), entry("Paste", "paste"), "-",
                entry("Reset the page", "reset"))

# %% [markdown]
# ## The toolbar
# A row as tall as its tools. The buttons are `flat` -- no box until the pointer
# is over one -- and their faces are **glyphs**: there are no icons in the host
# and there are fonts, so `icon=` is a character, and `ICON` names the ones the
# host's own face draws. A `separator` splits the groups, and one with a
# `weight` is the spring that sends what follows to the far edge.

# %%
tools = toolbar(
    button(name="play", icon=ICON.play, flat=True, tip="Play from the cursor"),
    button(name="stop", icon=ICON.stop, flat=True, tip="Stop"),
    button(name="record", icon=ICON.record, flat=True, tip="Record a take"),
    separator(),
    toggle(name="loop", label="loop", icon=ICON.loop, view="button",
           tip="Loop the selection"),
    separator(weight=1.0, line=False),
    button(name="more", label="tools", icon=ICON.menu, flat=True,
           menu=menu("Split", "Join", "-", entry("Quantize", "quantize")),
           tip="A button that opens a menu"),
    margin=2, gap=2)

# %% [markdown]
# ## The sidebar: sections that fold
# A `panel` with a `title` is a group, and one that also says `collapsed` is a
# **section**: a press on its title strip folds it to the strip and unfolds it.
# The second one starts folded.

# %%
sidebar = panel(
    panel(knob(name="cutoff", label="cutoff", min=20.0, max=20000.0, value=800.0,
               curve=4.0),
          slider(name="res", label="res", min=0.0, max=1.0, value=0.3),
          title="Filter", collapsed=False, frame=True, hug=True, name="filter"),
    panel(number(name="gain", label="gain", min=-24.0, max=24.0, value=0.0, step=1.0,
                 stepper=True),
          toggle(name="mute", label="mute", view="switch"),
          title="Output", collapsed=True, frame=True, hug=True, name="output"),
    label("drag the divider ->", weight=1.0),
    flow="col", w=220.0, name="sidebar")

# %% [markdown]
# ## One choice, six views
# A combobox, a radio group, a segmented control, tabs, a pager and a list hold
# the same thing -- some options and which one is current -- so they are **one
# element** and `view=` is the picture. The value is the index in all of them.

# %%
choices = panel(
    panel(choice(WAVES, name="combo", label="combo"),
          choice(WAVES, name="segmented", label="segmented", view="segmented"),
          choice(WAVES, name="strip", label="tabs", view="tabs"),
          choice(WAVES, name="steps", label="pager", view="pager"),
          flow="col"),
    choice(WAVES, name="radio", label="radio", view="radio"),
    choice(WAVES, name="rows", label="list", view="list"),
    flow="row")

# %% [markdown]
# ## The controls, in their other looks
# A toggle is a box, a switch or a button that stays pressed. A number grows a
# **stepper**. A `progress` bar shows a fraction -- here the slider's, **bound**
# to it by naming the bar itself, with no id and no round trip through this
# script -- or, given no value, says "working" without claiming how far. And
# `enabled=False` takes a widget, or a whole container, out of the hand's reach.

# %%
done = progress(0.4, name="done", label="progress (bound to the slider)")
controls = panel(
    panel(toggle(name="check", label="check", value=True),
          toggle(name="switch", label="switch", view="switch"),
          toggle(name="held", label="button", view="button"),
          flow="row", hug=True),
    number(name="steps_n", label="stepper", min=0.0, max=16.0, value=4.0, step=1.0,
           stepper=True),
    slider(name="amount", label="amount", min=0.0, max=1.0, value=0.4,
           bind=["widget", done, "value"]),
    done,
    progress(name="working", label="progress (indeterminate)"),
    panel(button(label="disabled"), knob(label="too"), toggle(label="and this"),
          flow="row", enabled=False, title="A disabled group", frame=True, hug=True),
    text(name="note", label="a field", value="Tab walks every control"),
    flow="col")

# %% [markdown]
# ## A plane with scroll bars, and a pager
# `bars=True` on a `scroll` draws a bar along each axis whose content is larger
# than the view. `pager` is `tabs` with the other strip: the same composition, a
# choice bound to a stack.

# %%
long_list = scroll(
    *(label(f"row {n + 1} of 40", x=8.0, y=8.0 + 28.0 * n, w=240.0, h=24.0)
      for n in range(40)),
    axis="y", zoom=False, bars=True, name="rows_plane")

book = pager(label("the first page", align="center"),
             label("the second page", align="center"),
             label("the third page", align="center"))

# %% [markdown]
# ## Rows of data
# A `table` is also a list and a tree: rows of cells under named columns, and a
# row with a `depth` sits under the less deep one above it -- a branch, with
# `open`, folds what is under it. Only the rows on screen are drawn. Ordering the
# rows is the script's: a press on a header moves the mark and asks, and the
# script sends the rows back in that order (below).

# %%
data = panel(
    table(rows=TAKES, sort=[0, "up"], multiple=True, name="takes",
          columns=["take", {"title": "length", "w": 100.0}, {"title": "rate", "w": 120.0}]),
    table(rows=[row("drums", depth=0, open=True), row("kick", depth=1),
                row("snare", depth=1), row("pads", depth=0, open=False),
                row("warm", depth=1), row("voice", depth=0)], name="tree"),
    flow="row", split=True)

pages = tabs(choices, controls, long_list, book, data,
             titles=["choices", "controls", "scroll bars", "pager", "data"],
             weight=1.0, context=on_pages, name="pages")

# %% [markdown]
# ## The window
# The bar is the window's `menu`. Under it, a `col`: the toolbar, then a `row`
# whose gap is a divider (`split=True`), then a holder of no height where a
# dialog is defined when it is wanted. The host adds its own status bar below.

# %%
scene = view(
    tools,
    panel(sidebar, pages, flow="row", split=True, weight=1.0, name="work"),
    layout(h=0.0, name="dialogs"),
    title="Chrome: every element, one window", w=900, h=620, flow="col",
    margin=0, gap=0, menu=bar)

win = scene.open()

# %% [markdown]
# ## What the window reports
# A menu's pick comes from the widget that carries the menu: the window for its
# bar, the tabs for their context menu, the tools button for its own. A folded
# section says `collapsed`, a divider let go of says `split` with the size it
# left each child.

# %%
def say(who):
    """A printer for whatever `who` reports."""
    return lambda *payload: print(f"{who}: {payload}")


for name in ("pages", "more", "filter", "output", "work", "loop", "combo", "radio",
             "segmented", "strip", "steps", "rows", "switch", "held", "steps_n", "tree"):
    win[name].on_event(say(name))


def on_takes(tag, *payload):
    """The table asks to be ordered by a column: order the rows and send them."""
    print(f"takes: {(tag, *payload)}")
    if tag == "sort":
        column, direction = payload
        ordered = sorted(TAKES, key=lambda r: r[column], reverse=direction == "down")
        win["takes"].set(rows=ordered)


win["takes"].on_event(on_takes)
for name in ("play", "stop", "record"):
    win[name].on_click(lambda name=name: print(f"{name}: clicked"))

# %% [markdown]
# ## The dialogs
# A dialog is a `layout` that stands over the window: it is **defined** to open
# and **freed** to close, and while it exists nothing behind it can be reached
# -- not by the pointer and not by Tab. Here the bar's `About` defines one into
# the holder, and its button frees it -- as do Escape and the close mark at the
# end of its title strip, which ask the dialog to go as `("cancel",)`. `File > Open` defines a `file_dialog`
# there instead: a chooser the host lists, a field that follows its selection,
# and the two buttons; a double click on a file picks it.


# %%
def about():
    """Define the dialog into its holder; its button frees it."""
    box = gui.define(win["dialogs"].id, layout(
        dialog(label("Every element of chrome, one window.", align="center"),
               label("Nothing behind this can be reached.", align="center"),
               panel(separator(weight=1.0, line=False),
                     button(name="ok", label="OK", w=80.0),
                     flow="row", hug=True),
               title="About", flow="col", w=360.0, name="box"),
        h=0.0))
    box["ok"].on_click(lambda: box["box"].free())
    box["box"].on_event(lambda tag, *payload: box["box"].free() if tag == "cancel" else None)


def open_file():
    """Define the file dialog into the holder; a pick, or OK, closes it."""
    box = gui.define(win["dialogs"].id,
                     layout(file_dialog(".", title="Open a file", name="chooser"), h=0.0))
    chosen = {"path": None}

    def done(path):
        print(f"open: {path}")
        box["chooser"].free()

    def on_files(tag, *payload):
        if not payload:
            chosen["path"] = tag            # the value: the file selected
        elif tag == "pick":
            done(payload[0])

    box["files"].on_event(on_files)
    box["file"].on_event(lambda value: chosen.update(path=value))
    box["ok"].on_click(lambda: done(chosen["path"]))
    box["cancel"].on_click(lambda: box["chooser"].free())
    box["chooser"].on_event(
        lambda tag, *payload: box["chooser"].free() if tag == "cancel" else None)


def on_bar(tag, *payload):
    """The window's own events: a pick in the menu bar is one of them."""
    print(f"window: {(tag, *payload)}")
    if tag == "menu" and payload[0] == "about":
        about()
    elif tag == "menu" and payload[0] == "open":
        open_file()
    elif tag == "menu" and payload[0] == "quit":
        win.close()


win.handle().on_event(on_bar)

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        win.wait()
    finally:
        gui.stop()
else:
    print("window up - win.wait() to hold it, gui.stop() to end")
