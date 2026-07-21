#!/usr/bin/env python3
"""An editable ``text`` field types an OSC message straight to the server.

The editable ``text`` field: you click into it, type, move the caret, select and
delete, and cut/copy/paste (Ctrl+C/X/V) — the ordinary editing of a one-line
entry. What it emits is a **string**, delivered exactly the way a slider delivers
its float: as a ``/gui_event`` on **every** edit, never gated on Enter. This
script consumes that stream, parses each line into an OSC address plus typed
arguments, and sends it to the audio server.

Because the send is ungated, editing the message is *live*: this example seeds a
running synth's ``/n_set <id> freq 220`` and, as you edit the number, the pitch
follows what you type digit by digit — the string field behaving just like a
numeric control. A half-typed or unknown message is simply ignored by the server
(this is a demo of the entry widget, not a validating console), so type freely.

A second, ``multiline`` field is a plain scratch pad — Enter inserts a newline
and the arrow keys move the caret across lines — to show the multi-line mode; its
contents are only printed here, not sent.

The parsing is deliberately minimal (`parse_osc` below): whitespace-split, the
first token is the address, and each remaining token becomes an ``int`` if it
looks like one, else a ``float``, else a string — the `clausters` OSC encoder
then tags each argument by its Python type. A real application would layer
quoting or an explicit send action on top; the widget's contract stops at
"here is the string, live."

Run it with the client installed (from the repo root)::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python   # bundles the server + GUI host

then::

    .venv/bin/python clients/python/examples/gui_oscsend.py

`Session.gui` boots the GUI host wired to this session's audio server, so a
parsed message reaches the engine with no extra setup. Needs a display and a
Vulkan/Metal/DX12/GL adapter (the host opens a window).
"""

import sys
import time

from clausters import Session
from clausters.defs import SynthDef, control, out, sine
from clausters.gui import label, text, window

# Widget ids.
OSC_FIELD = 10
SCRATCH = 20


def parse_osc(line: str):
    """Parse ``"/addr a b c"`` into ``(address, [args])`` with each argument
    coerced to ``int`` / ``float`` / ``str`` by how it reads, or ``None`` when
    the line has no ``/``-address yet. Minimal on purpose (no quoting)."""
    tokens = line.split()
    if not tokens or not tokens[0].startswith("/"):
        return None
    args = []
    for tok in tokens[1:]:
        try:
            args.append(int(tok))
        except ValueError:
            try:
                args.append(float(tok))
            except ValueError:
                args.append(tok)
    return tokens[0], args


def beep() -> SynthDef:
    """A quiet stereo sine on the ``freq`` control (default 220 Hz) — the target
    the typed ``/n_set <id> freq <value>`` drives."""
    sig = sine(freq=control("freq", 220.0)) * 0.2
    return SynthDef("gui_oscsend_beep", out(0.0, sig), out(1.0, sig))


def scene(node_id: int) -> dict:
    """A window: an OSC-message field seeded to set the synth's freq, and a
    multiline scratch pad."""
    return window(
        label(1, "type an OSC message; it is sent to the server as you type",
              h=28.0),
        text(OSC_FIELD, value=f"/n_set {node_id} freq 220", h=40.0),
        label(2, "a multiline scratch pad (Enter = newline); not sent", h=28.0),
        text(SCRATCH, multiline=True, value="line one\nline two"),
        title="OSC message field", w=560, h=360, layout="col",
    )


def main():
    with Session.live() as session:
        server = session.server
        server.add_synthdef(beep())          # blocks until /done
        synth = server.synth("gui_oscsend_beep", {"freq": 220.0})

        gui = session.gui()                  # host wired to this server
        gui.define(1, scene(synth.id))
        print(f"synth {synth.id} playing at 220 Hz; edit the freq in the field "
              "and the pitch follows as you type (close the window to end)")

        deadline = time.monotonic() + 120.0
        last_sent = None
        while time.monotonic() < deadline:
            msg = gui.poll(timeout=0.1)
            if msg is None:
                continue
            addr, args = msg
            if addr == "/gui_closed":
                print("window closed")
                break
            if addr != "/gui_event" or len(args) < 2:
                continue
            wid, value = args[0], args[1]
            if wid == SCRATCH:
                print(f"scratch: {value!r}")
                continue
            if wid == OSC_FIELD and isinstance(value, str):
                parsed = parse_osc(value)
                if parsed is None:
                    continue
                out_addr, out_args = parsed
                if (out_addr, tuple(out_args)) == last_sent:
                    continue
                last_sent = (out_addr, tuple(out_args))
                try:
                    server.send_msg(out_addr, *out_args)
                    print(f"sent {out_addr} {out_args}")
                except (OSError, ValueError) as e:
                    print(f"(not sent: {e})")

        server.free(synth)


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
