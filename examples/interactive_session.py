#!/usr/bin/env python3
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
import json
import os
import subprocess
import sys
import time

# Make the in-repo client importable when running from the examples folder.
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)),
                                "..", "clients", "python"))

from clausters.defs import Server, ServerOptions, SynthDef, control, out, sin_osc
from clausters.defs.node import AddAction

REPO = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
BIN = os.environ.get("CLAUSTERS_BIN", os.path.join(REPO, "target", "release", "clausters"))
print("server binary:", BIN, "(ok)" if os.path.exists(BIN) else "(MISSING: cargo build --release)")

# %% [markdown]
# ## 2. Configure and boot the server
#
# `ServerOptions` is the single source of truth: it both **launches** a matching
# server (`options.args()` -> the CLI flags) and **sizes the client's
# allocators**. We boot it as a subprocess and wait until it answers
# `/server_info`. Re-running this cell restarts the server cleanly.

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
# `server.query_tree()` returns the tree as a nested dict; this prints it
# indented. We call `show_tree()` after each change to watch the tree grow.

# %%
def show_tree(label=""):
    def walk(node, depth):
        pad = "  " * depth
        if "def" in node:
            ctl = ", ".join(f"{k}={v:g}" for k, v in node.get("controls", {}).items())
            print(f"{pad}- {node['id']} synth {node['def']}  [{ctl}]")
        else:
            print(f"{pad}o {node['id']} group")
            for child in node["children"]:
                walk(child, depth + 1)
    if label:
        print(f"--- {label} ---")
    walk(server.query_tree(), 0)


show_tree("empty tree (just the root group 0)")

# %% [markdown]
# ## 4. Load a SynthDef
#
# A one-oscillator `beep`. `add_synthdef` blocks until the server replies
# `/done` (it is an async command).

# %%
beep = SynthDef("beep", out(0.0, sin_osc(control("freq", 440.0)) * control("amp", 0.2)))
server.add_synthdef(beep)
print("loaded def 'beep'; status:", server.status())   # [..., num_defs, ...]

# %% [markdown]
# ## 5. Create groups
#
# Two groups give a defined execution order: everything in `sources` runs before
# `output`. They are added at the tail of the root in creation order.

# %%
sources = server.group()
output = server.group()
print(f"sources = group {sources.id}, output = group {output.id}")
show_tree("two empty groups under the root")

# %% [markdown]
# ## 6. Assign nodes to the groups
#
# Spawn two `beep` synths at the tail of `sources`. They appear nested under it.

# %%
a = server.synth("beep", {"freq": 220.0}, target=sources.id, action=AddAction.TAIL)
b = server.synth("beep", {"freq": 330.0}, target=sources.id, action=AddAction.TAIL)
print(f"spawned synths {a.id} and {b.id} in group {sources.id}")
show_tree("two synths under the sources group")

# %% [markdown]
# ## 7. Assign buses
#
# Allocate a **control bus** and map synth `b`'s `freq` to it: now one `/c_set`
# retunes it with no per-node command. `node_query` shows the map and the
# inferred read/write buses per node.

# %%
freq_bus = server.control_bus()
server.set_bus(freq_bus, 440.0)
server.map(b, "freq", freq_bus)
time.sleep(0.1)   # let the commands apply before querying

print(f"mapped synth {b.id}.freq -> control bus {freq_bus.index}")
print(json.dumps(server.node_query(b), indent=2))

# %% [markdown]
# ## 8. Change parameters live, and read them back
#
# Set `a`'s `amp` with `/n_set`, and retune `b` by writing its control bus. The
# tree reflects both.

# %%
server.set(a, {"amp": 0.05})
server.set_bus(freq_bus, 550.0)
time.sleep(0.1)
show_tree("after /n_set amp and /c_set on the mapped bus")
print("\ninferred bus graph of the sources group:")
print(server.dump_graph(sources.id), end="")

# %% [markdown]
# ## 9. Steer the server's logs from the client
#
# Logs are a separate channel (the server's **stderr**). The client can retune
# the level live with `/verbosity`, and toggle the OSC-traffic dump with
# `/dumpOSC`. Output lands wherever the server process writes.

# %%
print("verbosity ->", server.request("/verbosity", 2, timeout=2.0, expect=("/done", "/fail")))
print("dumpOSC   ->", server.request("/dumpOSC", 1, timeout=2.0, expect=("/done", "/fail")))
server.set(a, {"freq": 200.0})   # now traced on the server's stderr
print("sent /n_set; check the server's stderr for the trace line")

# %% [markdown]
# ## 10. Tear down
#
# Free the groups (which frees their synths) and stop the server. Re-running the
# boot cell (section 2) starts a fresh one.

# %%
server.free(sources, output)
show_tree("after freeing the groups")
server.quit()
server.close()
proc.wait(timeout=5)
print("server stopped")
