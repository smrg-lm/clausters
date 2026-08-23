#!/usr/bin/env python3
"""Two handles on one GUI host: `boot` owns the process, `attach` does not.

The GUI parallel of ``servers.py``. A `GuiHost` is a handle on a host, and there
are two verbs that reach one:

- `boot`, for a host that is **not there yet**. It starts the ``clausters-gui``
  process, owns it, and `stop` ends it.
- `attach`, for a host **already running** -- one left behind by a script that
  ended, one launched from a terminal, one another process owns. It verifies
  that someone answers there, and takes no ownership: `stop` closes the
  connection and leaves the host standing, windows and all.

This script plays both parts at once, so you can watch the difference in one
run: it boots a host, attaches a *second* handle to it, and each handle opens a
window of its own on the same host. Closing the attached handle leaves both
windows on screen; only the owner's `stop` takes the host down.

Two handles naming widgets on one host want a **share** of the id space
(`IdShare`, the same arithmetic as two clients on one audio server), or both
start numbering at 1000 and the second one's widgets collide with the first's.

No audio server is involved, so this boots only the GUI host.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the GUI binary

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/gui_attach.py``. By hand the host is
``clausters-gui``, and then *both* handles here would be `attach` calls. Needs a
display and a GPU adapter.
"""

# %%
import sys
import time

from clausters.base import IdShare
from clausters.gui import GuiHost, label, view
from clausters.launch import gui_is_up

# %% [markdown]
# ## The handle that starts the host
# `boot` launches the process and owns it. It also registers as the *ambient*
# host (first-wins), which is why a bare `open()` would land here.

# %%
owner = GuiHost(share=IdShare(0, 2)).boot()
print(f"booted a host at {owner.target[0]}:{owner.target[1]}")

# %% [markdown]
# ## The handle that only connects
# `attach` is what a second script would call. It verifies -- point it at a port
# nobody answers and it raises here, instead of dropping every later `/gui_def`
# into a void that reports nothing back. It adopts no process, so `_process` is
# `None` and `stop` will leave the host running.

# %%
guest = GuiHost(port=owner.target[1], share=IdShare(1, 2)).attach()
print(f"attached a second handle; it owns no process: {guest._process is None}")

# %% [markdown]
# ## One window from each
# The same host draws both. The ids come from disjoint shares, so the two
# handles never name the same widget.

# %%
first = view(
    label(name="caption", text="opened by the handle that booted the host"),
    title="owner", w=420, h=120).open(host=owner)

second = view(
    label(name="caption", text="opened by the attached handle"),
    title="guest", w=420, h=120).open(host=guest)

print(f"window ids: owner {int(first)}, guest {int(second)} -- "
      "two windows, one host")

# %% [markdown]
# ## Letting go is not stopping
# The attached handle closes its connection. Both windows stay on screen and the
# host still answers: nothing it did not start is torn down.

# %%
time.sleep(2.0)
guest.stop()
print(f"the guest let go, and the host still answers: {gui_is_up(port=owner.target[1])}")

# %% [markdown]
# ## Keep it open
# Close either window to end the run; the owner's `stop` is what ends the host.

# %%
_closed = False
first.on_closed(lambda: globals().__setitem__("_closed", True))


def run(seconds: float | None = None) -> None:
    """Pumps events for ``seconds``.

    Script-run there is no bound and the window is what ends it; the
    ``seconds`` argument is for a cell run, where a notebook wants the loop to
    give the prompt back.
    """
    start = time.monotonic()
    while not _closed and (seconds is None or time.monotonic() - start < seconds):
        owner.pump(timeout=0.1)


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        owner.stop()          # this handle booted the process, so this ends it
else:
    print("two windows up - run(10) to keep them open, owner.stop() to end")
