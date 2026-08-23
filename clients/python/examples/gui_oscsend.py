#!/usr/bin/env python3
"""An editable ``text`` field types an OSC message straight to the server.

The editable ``text`` field: you click into it, type, move the caret (word-wise
with Ctrl), select, delete -- a character, or a whole word with
**Ctrl+Backspace** / **Ctrl+Delete**, one run per press (in ``a, b`` the first
removes ``b`` and the second the ``", "`` before it) -- and cut/copy/paste
(Ctrl+C/X/V): the ordinary editing of a one-line entry. What it emits is a **string**, delivered exactly the way a slider delivers
its float: as a ``/gui_event`` on **every** edit, never gated on Enter. This
script consumes that stream, parses each line into an OSC address plus typed
arguments, and sends it to the audio server.

Because the send is ungated, editing the message is *live*: this example seeds a
running synth's ``/node_set <id> freq 220`` and, as you edit the number, the pitch
follows what you type digit by digit -- the string field behaving just like a
numeric control. A half-typed or unknown message is simply ignored by the server
(this is a demo of the entry widget, not a validating console), so type freely.

A second, ``multiline`` field is a plain scratch pad -- Enter inserts a newline
and the arrow keys move the caret across lines -- to show the multi-line mode; its
contents are only printed here, not sent.

**The keyboard, and where it points.** Only one widget at a time reads the
keyboard, and it wears a ring saying so. Press **Tab** to walk between the two
fields (**Shift+Tab** backwards), and press it once more from the scratch pad:
the ring disappears, because Tab past the last field *hands the keyboard back* --
on a desktop there is nothing outside the window to take it, in a web page the
document's own tab order carries on. This script also points the keyboard itself
with ``focus()``, so the message field is ready to type into the moment the
window opens, and prints every move the *user* then makes: a widget hears when it
gains the focus and when it loses it, as a ``("focus", 1)`` / ``("focus", 0)``
payload beside its value. (The ``focus()`` call itself is not echoed back --
no ``set`` is -- which is why the first line the console prints is the field
*losing* the keyboard to the first Tab.) That is why the handlers below take ``*payload`` rather than one string --
a widget's event stream carries tagged payloads next to plain values.

The two fields are *named*, so the script wires a per-field ``on_event`` and
never matches a widget id -- the `field` handler parses and sends, the `scratch`
handler just prints.

The parsing is deliberately minimal (`parse_osc` below): whitespace-split, the
first token is the address, and each remaining token becomes an ``int`` if it
looks like one, else a ``float``, else a string -- the `clausters` OSC encoder
then tags each argument by its Python type. A real application would layer
quoting or an explicit send action on top; the widget's contract stops at
"here is the string, live."

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python   # bundles the server + GUI host

then run it cell by cell (Shift+Enter) or as a plain script --
``python clients/python/examples/gui_oscsend.py``. `Session.gui` boots the GUI
host wired to this session's audio server, so a parsed message reaches the engine
with no extra setup. Needs a display and a GPU adapter.
"""

# %%
import sys
import time

from clausters import Session
from clausters.defs import SynthDef, control, out, sine
from clausters.gui import label, text, view
from clausters.defs import Synth

# %% [markdown]
# ## Launch the server and the GUI
# `Session.live` connects to a running audio server or starts one; `session.gui()`
# starts ``clausters-gui`` wired to it.

# %%
session = Session.live()
server = session.server
gui = session.gui()


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


# %% [markdown]
# ## The synth the field drives
# A quiet stereo sine on the ``freq`` control -- the target the typed
# ``/node_set <id> freq <value>`` retunes as you type.

# %%
def beep() -> SynthDef:
    sig = sine(freq=control("freq", 220.0)) * 0.2
    return SynthDef("gui_oscsend_beep", out(0.0, sig), out(1.0, sig))


beep().send(server)          # blocks until /done
synth = Synth("gui_oscsend_beep", {"freq": 220.0}, server=server)

# %% [markdown]
# ## Open the window
# An OSC-message field seeded to set the synth's freq, and a multiline scratch
# pad. Both are *named*, so the script listens to each by name.

# %%
win = gui.open(view(
    label("type an OSC message; it is sent to the server as you type", h=28.0),
    text(name="field", value=f"/node_set {synth.id} freq 220", h=40.0),
    label("a multiline scratch pad (Enter = newline); not sent", h=28.0),
    text(name="scratch", multiline=True, value="line one\nline two"),
    title="OSC message field", w=560, h=360, layout="col"))
print(f"synth {synth.id} playing at 220 Hz -- edit the freq in the field and the "
      "pitch follows as you type")

# %% [markdown]
# ## Wire the fields by name
# The `field` handler parses each edit and forwards it to the server (skipping a
# repeat of the last line sent); the `scratch` handler just prints. Both take
# ``*payload``, because a field reports its focus moving as well as its value.

# %%
_last_sent = None
_closed = False


def report_focus(who: str, payload) -> bool:
    """Print and swallow a ``("focus", 1|0)`` payload, so the handlers below see
    only the field's own value. Returns whether this was one."""
    if len(payload) == 2 and payload[0] == "focus":
        print(f"{who} {'has' if payload[1] else 'lost'} the keyboard")
        return True
    return False


def on_field(*payload):
    global _last_sent
    if report_focus("field", payload):
        return
    (value,) = payload
    parsed = parse_osc(value)
    if parsed is None:
        return
    out_addr, out_args = parsed
    if (out_addr, tuple(out_args)) == _last_sent:
        return
    _last_sent = (out_addr, tuple(out_args))
    try:
        server.send_msg(out_addr, *out_args)
        print(f"sent {out_addr} {out_args}")
    except (OSError, ValueError) as e:
        print(f"(not sent: {e})")


def on_scratch(*payload):
    if not report_focus("scratch", payload):
        print(f"scratch: {payload[0]!r}")


win["field"].on_event(on_field)
win["scratch"].on_event(on_scratch)
win.on_closed(lambda: globals().__setitem__("_closed", True))

# The message field starts focused, so the window opens ready to type into --
# no click needed. Tab moves the keyboard to the scratch pad and out again.
win["field"].focus()

# %% [markdown]
# ## Drive it
# Cell-run: type in the field and watch the console. Script-run: pump events for
# a while, then tear everything down.

# %%
def run(seconds: float | None = None) -> None:
    """Dispatches field events for ``seconds``."""
    start = time.monotonic()
    while not _closed and (seconds is None or time.monotonic() - start < seconds):
        gui.pump(timeout=0.1)


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        synth.free()
        session.close()
    sys.exit(0)
else:
    print("oscsend up - run(10) to dispatch events, session.close() to end")
