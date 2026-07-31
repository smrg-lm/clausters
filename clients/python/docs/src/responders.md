# Receiving OSC and MIDI (responders)

Everything so far has the client *sending* — building OSC or MIDI and pushing it to the server. Responders add the other direction: the client **receives** OSC and MIDI from any application, matches each message, and dispatches it to a callback. That callback can do anything, including emit OSC or MIDI onward — so the client becomes a hub: a MIDI keyboard or a controller app on one side, the Clausters server (or another program) on the other.

Sending *to* another application is the counterpart, and it is not a responder: it is a destination, covered in [Routines and clocks](routines-and-clocks.md#sending-to-another-application).

The two responder classes mirror SuperCollider's `OSCFunc` and `MIDIFunc`:

- **`OscFunc(func, path)`** fires `func` on incoming OSC messages whose address matches `path`.
- **`MidiFunc(func, midi_msg)`** fires `func` on incoming MIDI of a given type (`"note_on"`, `["note_on", "note_off"]`, …).

Both are enabled the moment you create them, and you call `free()` (or `disable()`) when done.

## Receivers: the transport under a responder

A responder registers with a **receiver** — the object that owns the listening socket (or MIDI port) and a background thread that decodes and demuxes incoming messages:

- **`OscReceiver`** binds a UDP socket and decodes each datagram (bundles unwrapped) through the same single decode door the server uses.
- **`MidiReceiver`** opens a virtual MIDI input port (through the `clausters-midi` crate's `live` feature) that other apps and devices route into, and parses each message into a dict.

You can pass a receiver explicitly, or let the responder use a lazily-created module default. The default OSC receiver binds an ephemeral port; the default MIDI receiver opens a virtual input port the first time a `MidiFunc` needs it. They are the one bit of process-wide state in this layer, in the spirit of `main.default_clock` — explicit receivers are always available when you want full control (a fixed port, a shared clock).

```python
from clausters.base import OscReceiver
from clausters.responders import OscFunc

# A receiver on a fixed port external apps can target.
recv = OscReceiver(port=57121).start()

@OscFunc  # not how you'd normally call it -- see the decorator form below
def _(): ...
```

## OSC: matching and the callback

An `OscFunc` callback is called `func(msg, time, src)`:

- `msg` is the message as a list — `[addr, arg1, arg2, …]`.
- `time` is the bundle's Unix time, or `None` for an immediate or bare message.
- `src` is the `(host, port)` of the sender.

You can narrow what matches with `src=(host, port)` (respond only to one sender) and `arg_template` (match arguments by position — a literal compares equal, a callable is a predicate, `None` matches anything):

```python
from clausters.base import OscReceiver
from clausters.responders import OscFunc, oscfunc

recv = OscReceiver(port=57121).start()

# Relay an incoming /note <midinote> <dur> to the server as a synth.
def play(msg, time, src):
    freq = 440.0 * 2 ** ((msg[1] - 69) / 12)
    server.synth("default", {"freq": freq})

OscFunc(play, "/note", recv=recv)

# The decorator form reads the same and returns the OscFunc.
@oscfunc("/ctl", arg_template=[7, None], recv=recv)
def on_cc(msg, time, src):
    print("controller 7 ->", msg[2])
```

`OscFunc(...).one_shot()` frees the responder after its first match, for a one-time wait.

## MIDI: message dicts

A `MidiReceiver` decodes raw channel-voice bytes into a dict with a `type` and the type's fields — `{"type": "note_on", "channel": 0, "note": 60, "velocity": 100}`, `{"type": "control_change", "channel": 0, "control": 7, "value": 127}`, and so on (`pitchwheel` carries a single 14-bit `pitch`). A `MidiFunc` callback is called `func(message, src)` with that dict and the port name, and you match on the type, an optional `chan`, and an `arg_template` over the fields:

```python
from clausters.base import MidiReceiver
from clausters.responders import MidiFunc

recv = MidiReceiver(port="clausters-in").start()
voices = {}

def note_on(m, src):
    if m["velocity"] == 0:
        return note_off(m, src)
    freq = 440.0 * 2 ** ((m["note"] - 69) / 12)
    voices[m["note"]] = server.synth("default", {"freq": freq, "amp": m["velocity"] / 127 * 0.3})

def note_off(m, src):
    synth = voices.pop(m["note"], None)
    if synth is not None:
        server.free(synth)

MidiFunc(note_on, "note_on", recv=recv)
MidiFunc(note_off, "note_off", recv=recv)
```

That virtual input port is a loose cable until you wire a MIDI source into it (`pw-link`, qpwgraph, or `aconnect`). This is the client-side mirror of the server's own direct MIDI input: a Clausters server can be played by MIDI it receives itself, *or* by a client that listens to MIDI and forwards `/s_new`. Both coexist.

## Threading: the golden rule

A responder's callback runs on the receiver's background thread (or, if the receiver was given a `clock`, on the clock thread). The [golden rule](routines-and-clocks.md) holds: **never block that thread.** Keep callbacks quick. To *sequence* in response to an incoming message — play a phrase, not a single note — schedule a routine on a clock, which returns immediately:

```python
from clausters.base import Routine

def start_phrase(msg, time, src):
    def phrase():
        for degree in (0, 2, 4):
            Event(instrument="default", degree=degree).play(server)
            yield 0.25
    clock.play(Routine(phrase))   # non-blocking; the clock thread runs it

OscFunc(start_phrase, "/go", recv=recv)
```

## Reacting to the shared transport

Setting the server's shared transport (`/transport`, the beat grid several clients phase-align on — see [Timing models](timing-models.md)) **pushes** the new grid to every client registered with `/notify`. A responder on `/transport.reply` re-aligns this client the moment a conductor changes the tempo or origin, with no polling. Register `/notify` from the receiver's own socket so the push lands where the responder listens:

```python
recv = OscReceiver(port=57121).start()
recv.send(server.target.addr(), "/notify", 1)   # subscribe on this socket

def on_transport(msg, time, src):
    origin, tempo, defined = msg[1], msg[2], msg[3]
    if defined:
        clock.join_transport(server)             # adopt the new grid

OscFunc(on_transport, "/transport.reply", recv=recv)
```

## Examples

- **`osc_responder.py`** — the client as an OSC hub: relay incoming `/note` to the server, and re-align on a `/transport.reply` push. Runs against a live server, self-feeding a few messages to demonstrate.
- **`midi_responder.py`** — a `MidiFunc` turning a MIDI keyboard into synths on the server.

See [Examples](examples.md).
