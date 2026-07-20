#!/usr/bin/env python3
"""Asking a running server what it actually holds: defs, buffers, UGens.

`introspect_tree.py` reads the *node tree* — what is playing right now. This
reads the other half: the **catalog**, the material a client can build with.
Three queries, all of them retrieval only (nothing here changes the server):

  * ``server.query_defs()``    -> the loaded defs and their control surface
    (``/d_query`` -> ``/d_info``), across all three families;
  * ``server.query_buffers()`` -> every allocated buffer and its shape (an
    argument-less ``/b_query``);
  * ``server.query_ugens()``   -> the UGen catalog this server was built with
    (``/u_query`` -> ``/u_info``): named inputs, defaults, rate rules.

Why ask instead of assume? Because the answers are genuinely not knowable from
the client's own state. The **def store persists across restarts**, so a server
may hold defs that no client in this process ever sent — and the buffer pool
outlives any one client too. The UGen catalog depends on how the server was
built. This is what a patcher's palette is fed from.

    cargo build --release
    python3 examples/introspect_server.py

Point it at a prebuilt binary with ``CLAUSTERS_BIN=/path/to/clausters``.
"""

import os
import subprocess
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "clients", "python"))

from clausters.defs import Server, ServerOptions, SynthDef, control, out, sine

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


def main():
    proc = launch()
    server = connect()
    try:
        # Put something in the store and the buffer pool, so the queries have
        # more than the built-ins to report.
        server.add_synthdef(SynthDef(
            "beep",
            out(0.0, sine(control("freq", 440.0)) * control("amp", 0.2)),
        ))
        buf = server.alloc_buffer(1024, channels=1)

        print("query_defs() — what the server holds, with each control surface:")
        for d in server.query_defs():
            surface = ", ".join(f"{c.name}={c.default:g} ({c.rate})"
                                for c in d.controls) or "no controls"
            print(f"  [{d.family:>5}] {d.name}: {surface}")

        # A name the server does not have is reported, not raised — one bad
        # name never fails the batch.
        missing = server.query_defs("beep", "never_sent")[1]
        print(f"\nquery_defs('never_sent') -> exists={missing.exists!r}")

        print("\nquery_buffers() — the allocated pool:")
        for b in server.query_buffers():
            print(f"  buffer {b.bufnum}: {b.frames} frames x {b.channels} ch "
                  f"@ {b.sample_rate:g} Hz")

        catalog = server.query_ugens()
        print(f"\nquery_ugens() — {len(catalog)} kinds in this build. A few "
              "signatures:")
        for u in catalog:
            if u.name not in ("Sine", "PlayBuf", "EnvGen", "Out", "FFT"):
                continue
            args = ", ".join(f"{i.name}={i.default:g}" for i in u.inputs)
            arity = "variadic" if u.variadic else f"{u.arity} inputs"
            extra = f", {u.bus} bus" if u.bus else ""
            print(f"  {u.name}({args})  [{arity}, rates {'/'.join(u.rates)}"
                  f", default {u.default_rate}{extra}]")
        print("\n  (a variadic kind names only its fixed head — EnvGen's five "
              "come before the envelope array)")

        server.free_buffer(buf)
    finally:
        server.close()
        proc.terminate()
        proc.wait(timeout=5)


if __name__ == "__main__":
    main()
