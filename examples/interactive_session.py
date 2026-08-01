"""Clausters as an interactive session, in `# %%` cells.

A guided, step-by-step tour of driving a live Clausters server from the Python
client. The file is split into `# %%` **cells**: run them one at a time in an
interactive kernel (VS Code, Spyder, `jupytext`, or paste each block into
`ipython`) and the session state — the server subprocess and the `Server`
object — persists between cells, just like a notebook. It also runs top to
bottom as a plain script.

Each step logs the server's state — the node tree, a node's detail, the
inferred bus graph — read back as **structured data** (never scraped from
logs). Needs a built server binary; no Faust required:

    cargo build --release
    python3 examples/interactive_session.py      # whole script
    # or open it in VS Code / ipython and run the cells one by one

Point at a prebuilt binary with CLAUSTERS_BIN=/path/to/clausters. Real audio
hardware is assumed (a live RT server).
"""

# %% [markdown]
# ## 1. Import the library

# %%
import os
import subprocess
import sys
import time

# Locate the in-repo client. As a script we start from this file; in an
# interactive window (VS Code/Jupyter) `__file__` is undefined, so we start from
# the working directory. Either way, walk up until we find clients/python.
def _repo_root():
    start = os.path.dirname(os.path.abspath(__file__)) if "__file__" in globals() else os.getcwd()
    here = start
    for _ in range(6):
        if os.path.isdir(os.path.join(here, "clients", "python")):
            return here
        here = os.path.dirname(here)
    raise RuntimeError("run this from inside the clausters repo (clients/python not found)")


REPO = _repo_root()
sys.path.insert(0, os.path.join(REPO, "clients", "python"))

from clausters.defs import Bus, Server, ServerOptions, SynthDef, control, out, sine
from clausters.defs.node import AddAction
from clausters.defs import Group, Synth

BIN = os.environ.get("CLAUSTERS_BIN", os.path.join(REPO, "target", "release", "clausters"))
print("server binary:", BIN, "(ok)" if os.path.exists(BIN) else "(MISSING: cargo build --release)")

# %% [markdown]
# ## 2. Configure and boot the server
#
# `ServerOptions` is the single source of truth: it both **launches** a matching
# server (`options.args()` -> the CLI flags) and **sizes the client's
# allocators**. We boot it as a subprocess and wait until it answers
# `/server_query`. Re-running this cell restarts the server cleanly.

# %%
# Tear down a previous run if this cell is re-executed in the same session.
try:
    server.quit()       # noqa: F821 (defined below on first run)
    server.close()
    proc.wait(timeout=5)  # noqa: F821
except Exception:
    pass

options = ServerOptions(audio_buses=64, control_buses=512, sample_rate=48000)
proc = subprocess.Popen([BIN, *options.args(), "--no-persist"])
server = Server(options=options)

deadline = time.monotonic() + 8.0
while time.monotonic() < deadline:
    try:
        info = server.query_info(timeout=0.3)
        break
    except Exception:
        time.sleep(0.2)
else:
    raise RuntimeError("server did not come up")

print(f"booted: {info.audio_buses} audio / {info.control_buses} control buses "
      f"@ {info.actual_sample_rate:.0f} Hz, {info.channels} channels, block {info.block_size}")

# %% [markdown]
# ## 3. A helper to log the server state
#
# `server.query_tree()` returns the tree as data — a `NodeInfo` per entry —
# and printing it draws it indented. We call `show_tree()` after each change
# to watch the tree grow.

# %%
def show_tree(label=""):
    if label:
        print(f"--- {label} ---")
    print(server.query_tree())


show_tree("empty tree (just the root group 0)")

# %% [markdown]
# ## 4. Load a SynthDef
#
# A one-oscillator `beep`. `add_synthdef` blocks until the server replies
# `/done` (it is an async command).

# %%
beep = SynthDef("beep", out(0.0, sine(control("freq", 440.0)) * control("amp", 0.2)))
beep.send(server)
print("loaded def 'beep'; status:", server.status())   # [..., num_defs, ...]

# %% [markdown]
# ## 5. Create groups
#
# Two groups give a defined execution order: everything in `sources` runs before
# `output`. They are added at the tail of the root in creation order.

# %%
sources = Group.new(server=server)
output = Group.new(server=server)
print(f"sources = group {sources.id}, output = group {output.id}")
show_tree("two empty groups under the root")

# %% [markdown]
# ## 6. Assign nodes to the groups
#
# Spawn two `beep` synths at the tail of `sources`. They appear nested under it.

# %%
a = Synth.new("beep", {"freq": 220.0}, target=sources.id,
              action=AddAction.TAIL, server=server)
b = Synth.new("beep", {"freq": 330.0}, target=sources.id,
              action=AddAction.TAIL, server=server)
print(f"spawned synths {a.id} and {b.id} in group {sources.id}")
show_tree("two synths under the sources group")

# %% [markdown]
# ## 7. Assign buses
#
# Allocate a **control bus** and map synth `b`'s `freq` to it: now one `/bus_set`
# retunes it with no per-node command. `b.info()` shows the map and the
# inferred read/write buses of that node.

# %%
freq_bus = Bus.control(server=server)
freq_bus.set(440.0)
b.map("freq", freq_bus)
time.sleep(0.1)   # let the commands apply before querying

print(f"mapped synth {b.id}.freq -> control bus {freq_bus.index}")
print(b.info())

# %% [markdown]
# ## 8. Change parameters live, and read them back
#
# Set `a`'s `amp` with `/node_set`, and retune `b` by writing its control bus. The
# tree reflects both.

# %%
a.set({"amp": 0.05})
freq_bus.set(550.0)
time.sleep(0.1)
show_tree("after /node_set amp and /bus_set on the mapped bus")
print("\ninferred bus graph of the sources group:")
print(server.dump_graph(sources.id), end="")

# %% [markdown]
# ## 9. Steer the server's logs from the client
#
# Logs are a separate channel (the server's **stderr**). The client can retune
# the level live with `/server_verbosity`, and toggle the OSC-traffic dump with
# `/server_dumpOsc`. Output lands wherever the server process writes.

# %%
print("verbosity ->", server.request("/server_verbosity", 2, timeout=2.0, expect=("/done", "/fail")))
print("dumpOSC   ->", server.request("/server_dumpOsc", 1, timeout=2.0, expect=("/done", "/fail")))
a.set({"freq": 200.0})   # now traced on the server's stderr
print("sent /node_set; check the server's stderr for the trace line")

# %% [markdown]
# ## 10. Tear down
#
# Free the groups (which frees their synths) and stop the server. Re-running the
# boot cell (section 2) starts a fresh one.

# %%
sources.free()
output.free()
show_tree("after freeing the groups")
server.quit()
server.close()
proc.wait(timeout=5)
print("server stopped")
