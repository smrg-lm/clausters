"""Several servers on one machine, and the two ways to get a handle on one.

A `Server` is a handle that keeps the address it was built with, and booting
tells the process to bind exactly that address -- so a machine runs as many
servers as you give ports. This steps through the pair of verbs that reach one:

- `boot`, for a server that is **not there yet**. It starts the process, owns
  it, and `close` stops it. Booting where something already answers raises
  rather than adopting it.
- `attach`, for a server **already running** -- started from a terminal, owned
  by another process, or left behind by a client that crashed while it was
  sounding. It verifies that someone is there, re-reads the server's real
  capacities into this handle's allocators, and does *not* take ownership:
  `close` lets go and leaves the server standing, `quit` is what stops it, and
  `free_all` cuts the sound while keeping the server (defs, buffers, bindings).

The same three verbs are on the console script, which is what you want when the
client is gone and the only alternative is `kill`:

    clausters status --port 57130
    clausters panic  --port 57130
    clausters stop   --port 57130

Needs an audio backend that mixes several streams (PipeWire and CoreAudio do; an
exclusive ALSA device does not, and the second boot then fails saying so). You
will **hear** two independent servers, one per ear's worth of pitch.

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/servers.py``.
"""

# %%
import sys
import time

from clausters import Server, play
from clausters.defs import sine

PAUSE = 2.0

# %% [markdown]
# ## Two servers, side by side
# Each handle is built at its own port and boots a process there. The segments
# are picked apart automatically, so the two never share shared memory; the def
# store *is* shared, on purpose -- a def sent to one is on disk for the other.

# %%
a = Server().boot()                    # 57110, the default
b = Server(port=57130).boot()          # a second, beside it
print(f"a -> {a.target.host}:{a.target.port}, segment {a.shm}")
print(f"b -> {b.target.host}:{b.target.port}, segment {b.shm}")

# %% [markdown]
# ## Both sounding at once
# Every verb takes the server it acts on, so the two are told apart by handle,
# not by any ambient state. Two pitches, two processes, one audio device.

# %%
low = play(sine(220.0) * 0.12, server=a)
high = play(sine(330.0) * 0.12, server=b)
print("both servers sounding -- a fifth, one note from each")
time.sleep(PAUSE)

# %% [markdown]
# ## Booting where one already runs
# `boot` starts a process, so it refuses to adopt one it did not start: a handle
# that never launched anything must not end up owning it.

# %%
try:
    Server(port=57130).boot()
except Exception as e:
    print(f"boot on a busy port -> {type(e).__name__}: {e}")

# %% [markdown]
# ## Attaching to the one already running
# This is the handle a *second* script would build -- or the one you open after
# the first client crashed and left its server playing. `attach` raises if
# nobody answers, so a wrong address is an error here rather than silence later.

# %%
other = Server(port=57130).attach()
info = other.query_info()
print(f"attached to b: {info.actual_sample_rate:.0f} Hz, {info.channels} ch, "
      f"{info.audio_buses} audio buses")
print(f"allocators reconciled to the real server: max_nodes={other.options.max_nodes}")

# %% [markdown]
# ## What each verb ends
# `free_all` cuts the sound and keeps the server. `close` lets go of an attached
# server without stopping it -- ownership is what separates the two handles.

# %%
other.free_all()
print("b freed every node: the fifth drops to a single note")
time.sleep(PAUSE)

other.close()
print(f"the attached handle closed, and b is still up: {b.query_info() is not None}")


# %%
def run():
    """Tear the two servers down: the low note first, then both processes."""
    low.free()
    time.sleep(0.5)
    a.close()          # this handle booted it, so closing stops the process
    b.close()
    print("both servers stopped")


if __name__ == "__main__" and not hasattr(sys, "ps1"):
    run()
else:
    print("up - run() to stop both, or try `clausters status --port 57130` "
          "from a terminal first")
