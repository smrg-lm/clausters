#!/usr/bin/env python3
"""A post window: the lines a session writes, appended as it writes them.

A ``post`` element holds lines of four kinds and draws each in a role of the
theme: **output**, an **error**, a **result** (written ``-> value`` on a line
of its own) and a **notice** (always on a line of its own). A set of a kind
writes its text -- ``set(out=...)``, ``err``, ``result``, ``info`` -- and
output and errors continue the line left open, so two writes with no line's
end between them share a line, as two ``print(..., end="")`` do.

It follows the newest line while you are at the end of it: scroll up with the
wheel (or drag the bar on its right) and it stays where you read while lines
keep arriving; scroll back down to the end and it follows again. Drag over the
text to select it (two clicks a word, three a line, Shift extends), copy it
with Ctrl+C, and open its menu with the secondary button to copy, select all,
clear or switch the autoscroll off.

Only the rows in view are measured and drawn, so the ten thousand lines the
last cell writes at once cost the write -- try scrolling through them.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the GUI binary

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/panels/post.py``. No audio server is
involved, so this boots only the GUI host. Needs a display and a GPU adapter.
"""

# %%
import sys

from clausters.gui import GuiHost, post, view

# %% [markdown]
# ## A post window
# One element, filling its window.

# %%
host = GuiHost().boot()
window = view(post(name="post"), title="post", w=640, h=360).open(host=host)
out = window.widget("post")

# %% [markdown]
# ## The four kinds
# A notice, output over two writes that share a line, a result, an error.

# %%
out.set(info="session started\n")
out.set(out="counting: ")
out.set(out="1 2 3\n")
out.set(result="42")
out.set(err="Traceback (most recent call last):\n  ValueError: not a number\n")

# %% [markdown]
# ## A long line wraps at the window's width
# Make the window narrower and it wraps again.

# %%
out.set(out=" ".join(f"word{i}" for i in range(60)) + "\n")

# %% [markdown]
# ## Ten thousand lines at once
# One write; scroll up through them, and back down to follow again.

# %%
out.set(out="".join(f"line {i}\n" for i in range(10_000)))
out.set(info="done\n")

# %% [markdown]
# ## Keep it open
# Close the window to end the run.

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        window.wait()
    finally:
        host.stop()
else:
    print("the post window is up - window.wait(10) to hold it, host.stop() to end")
