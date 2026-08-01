#!/usr/bin/env python3
"""Reading the server's node tree as structured data (and steering its logs).

The node tree — groups, synths, ids, defs, controls, `/node_map` bindings, the
inferred bus usage — is available to a client as **structured replies**, never
by scraping the server's console. This builds a small tree and reads it back
three ways:

  * ``server.query_tree()`` -> the whole tree (``/group_queryTree``), asked of the
    server because it is about *every* node it holds. Every entry is the same
    record one node reports about itself, so nothing needs a second query;
  * ``node.info()`` -> that record for one node (``/node_query`` -> ``/node_query.reply``),
    asked of the node because it is about *itself*: parent, siblings, def,
    controls, maps, reads/writes buses. A node that is gone answers
    ``exists = False`` rather than raising;
  * ``server.dump_graph(g)`` -> the inferred bus graph as readable text
    (``/group_dumpGraph``), a debugging aid.

The server's own logs are a separate channel (its stderr), which the client can
retune live with ``/server_verbosity`` (level) and ``/server_dumpOsc`` (OSC-traffic target) —
shown at the end. No Faust needed; a plain server build works.

    cargo build --release
    python3 examples/introspect_tree.py

Point it at a prebuilt binary with ``CLAUSTERS_BIN=/path/to/clausters``.
"""

import os
import subprocess
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "clients", "python"))

from clausters.defs import Bus, Server, ServerOptions, SynthDef, control, out, sine
from clausters.defs.node import AddAction
from clausters.defs import Group, Synth

REPO = os.path.join(os.path.dirname(__file__), "..")
BIN = os.environ.get("CLAUSTERS_BIN", os.path.join(REPO, "target", "release", "clausters"))


def launch() -> subprocess.Popen:
    if not os.path.exists(BIN):
        sys.exit(f"server binary not found at {BIN}\n"
                 "build it with: cargo build --release  (or set CLAUSTERS_BIN)")
    return subprocess.Popen([BIN, "--no-persist"])


def connect(timeout: float = 8.0) -> Server:
    server = Server(options=ServerOptions())
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            server.query_info(timeout=0.3)
            return server
        except Exception:
            time.sleep(0.2)
    server.close()
    raise RuntimeError("server did not come up in time")


def build_tree(server: Server):
    """A group holding two synths; the second's freq is mapped to a control
    bus, so it shows up as a `/node_map` in the node detail."""
    SynthDef("beep", out(0.0, sine(control("freq", 440.0))
                                              * control("amp", 0.2))).send(server)
    group = Group.new(server=server)
    a = Synth.new("beep", {"freq": 220.0}, target=group.id,
                  action=AddAction.TAIL, server=server)
    b = Synth.new("beep", {"freq": 330.0}, target=group.id,
                  action=AddAction.TAIL, server=server)
    freq_bus = Bus.control(server=server)
    freq_bus.set(440.0)
    b.map("freq", freq_bus)     # b.freq now follows the control bus
    time.sleep(0.2)                     # let the commands apply before querying
    return group, a, b


def main():
    proc = launch()
    server = connect()
    try:
        group, _a, b = build_tree(server)

        tree = server.query_tree()
        print("query_tree() — printing a tree draws it:")
        print(tree)

        print("\n...and it is data, not text: every entry is a NodeInfo, so a")
        print("walk finds the mapped synth without asking the server again:")
        for info in tree.walk():
            if info.maps:
                print(f"  {info.id} {info.defname} maps {info.maps} "
                      f"(reads {info.reads}, writes {info.writes})")

        print(f"\nb.info() — the same record, asked of the node itself:")
        print(f"  {b.info()}")

        gone = Synth(4242, "beep", server=server)
        print(f"  a node that was never there: exists={gone.info().exists}")

        print(f"\ndump_graph({group.id}) — inferred bus graph (debug text):")
        print(server.dump_graph(group.id), end="")

        # The logs are a separate channel: the client can retune the server's
        # verbosity live (output lands on the server's stderr, not here).
        server.request("/server_verbosity", 1, timeout=2.0, expect=("/done", "/fail"))
        print("\nset server log level to info via /server_verbosity (see the server's stderr)")

        group.free()
    finally:
        try:
            server.quit()
        except Exception:
            pass
        server.close()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.terminate()


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
