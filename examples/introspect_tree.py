#!/usr/bin/env python3
"""Reading the server's node tree as structured data (and steering its logs).

The node tree — groups, synths, ids, defs, controls, `/n_map` bindings, the
inferred bus usage — is available to a client as **structured replies**, never
by scraping the server's console. This builds a small tree and reads it back
three ways:

  * ``server.query_tree()``  -> the whole tree as a nested dict (``/g_queryTree``);
  * ``server.node_query(n)`` -> one node in full detail (``/n_query`` ->
    ``/n_info``): parent, siblings, def, controls, maps, reads/writes buses;
  * ``server.dump_graph(g)`` -> the inferred bus graph as readable text
    (``/g_dumpGraph``), a debugging aid.

The server's own logs are a separate channel (its stderr), which the client can
retune live with ``/verbosity`` (level) and ``/dumpOSC`` (OSC-traffic target) —
shown at the end. No Faust needed; a plain server build works.

    cargo build --release
    python3 examples/introspect_tree.py

Point it at a prebuilt binary with ``CLAUSTERS_BIN=/path/to/clausters``.
"""

import json
import os
import subprocess
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "clients", "python"))

from clausters.defs import Server, ServerOptions, SynthDef, control, out, sin_osc
from clausters.defs.node import AddAction

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
    bus, so it shows up as a `/n_map` in the node detail."""
    server.add_synthdef(SynthDef("beep", out(0.0, sin_osc(control("freq", 440.0))
                                              * control("amp", 0.2))))
    group = server.group()
    a = server.synth("beep", {"freq": 220.0}, target=group.id, action=AddAction.TAIL)
    b = server.synth("beep", {"freq": 330.0}, target=group.id, action=AddAction.TAIL)
    freq_bus = server.control_bus()
    server.set_bus(freq_bus, 440.0)
    server.map(b, "freq", freq_bus)     # b.freq now follows the control bus
    time.sleep(0.2)                     # let the commands apply before querying
    return group, a, b


def main():
    proc = launch()
    server = connect()
    try:
        group, _a, b = build_tree(server)

        print("query_tree() — the whole tree as structured data:")
        print(json.dumps(server.query_tree(), indent=2))

        print(f"\nnode_query({b.id}) — the mapped synth in full detail:")
        print(json.dumps(server.node_query(b), indent=2))

        print(f"\ndump_graph({group.id}) — inferred bus graph (debug text):")
        print(server.dump_graph(group.id), end="")

        # The logs are a separate channel: the client can retune the server's
        # verbosity live (output lands on the server's stderr, not here).
        server.request("/verbosity", 1, timeout=2.0, expect=("/done", "/fail"))
        print("\nset server log level to info via /verbosity (see the server's stderr)")

        server.free(group)
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
