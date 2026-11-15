# Receiving OSC and MIDI (responders)

Everything so far has the client *sending* — building OSC or MIDI and pushing it to the server. Responders add the other direction: the client **receives** OSC and MIDI from any application, matches each message, and dispatches it to a callback. That callback can do anything, including emit OSC or MIDI onward — so the client becomes a hub: a MIDI keyboard or a controller app on one side, the Clausters server (or another program) on the other.

Sending *to* another application is the counterpart, and it is not a responder: it is a destination, covered in [Routines and clocks](routines-and-clocks.md#sending-to-another-application).

The two responder classes mirror SuperCollider's `OSCFunc` and `MIDIFunc`:

- **{{alt `OscFunc(func, path)` || `new OscFunc(func, path)`}}** fires `func` on incoming OSC messages whose address matches `path`.
- **{{alt `MidiFunc(func, midi_msg)` || `new MidiFunc(func, midiMsg)`}}** fires `func` on incoming MIDI of a given type (`"note_on"`, `["note_on", "note_off"]`, …).

Both are enabled the moment you create them, and you call `free()` (or `disable()`, which `enable()` undoes) when done.

## Receivers: the transport under a responder

A responder registers with a **receiver** — the object that decodes incoming messages and hands them to the responders registered there:

<!-- python -->
- **`OscReceiver`** binds a UDP socket and decodes each datagram (bundles unwrapped) through the same single decode door the server uses, on a background thread.
- **`MidiReceiver`** opens a virtual MIDI input port (through the `clausters-midi` crate's `live` feature) that other apps and devices route into, and parses each message into a dict.

You can pass a receiver explicitly, or let the responder use a lazily-created module default. The default OSC receiver binds an ephemeral port; the default MIDI receiver opens a virtual input port the first time a `MidiFunc` needs it. They are the one bit of process-wide state in this layer, in the spirit of `main.default_clock` — explicit receivers are always available when you want full control (a fixed port, a shared clock).

```python
from clausters.base import OscReceiver

# A receiver on a fixed port external apps can target.
recv = OscReceiver(port=57121).start()
```
<!-- /python -->
<!-- web -->
- **`OscReceiver`** wraps a connection — to the in-page engine, or to a server over its WebSocket — and decodes what arrives on it through the same decode door the server uses. A page can bind no port and be targeted by nobody, so the connection it already opened is what it receives on.
- **`MidiReceiver`** opens one of the MIDI inputs the browser grants the page (below), and parses each message into an object.

Two consequences follow, both visible in the API:

- **`src` names a carrier**, not a `(host, port)` pair: a socket's URL, or `"page"` for the in-page engine. Narrowing a responder with `{ src }` still answers the question it answers in a script — *who sent this?* — with what a browser actually knows.
- **The default receiver is the ambient session's server.** A responder that names no `recv` resolves it the way [the ambient verbs](verbs.md) resolve a server, and fails the same way when there is none. Each server has one receiver (`server.receiver`), so a page holding two sessions gets each one's messages from each one's responders; `setDefaultOscReceiver` pins one anyway.

Every server reaches its own responders through `server.receiver`, which is also the door the client's own reply handling goes through — the node ids that recycle as `/node_end` arrives are an `OscFunc` like any other. Under it, `server.onReply` is the raw seam: it sees every decoded message in arrival order with no matching of its own, which is what a stream decoder wants and what a responder is built on.

```ts
const recv = server.receiver;   // what this server's messages arrive on
```
<!-- /web -->

## OSC: matching and the callback

An `OscFunc` callback is called `func(msg, time, src)`:

- `msg` is the message as a list — `[addr, arg1, arg2, …]`.
- `time` is the bundle's Unix time, or {{alt `None` || `null`}} for an immediate or bare message.
- `src` is {{alt the `(host, port)` of the sender || the carrier it arrived on}}.

You can narrow what matches with `src` (respond only to one sender) and `{{api arg_template}}` (match arguments by position — a literal compares equal, a {{alt callable || function}} is a predicate, {{alt `None` || `null`}} matches anything):

```python
from clausters.base import OscReceiver
from clausters.defs import Synth
from clausters.responders import OscFunc, oscfunc

recv = OscReceiver(port=57121).start()

# Relay an incoming /note <midinote> <dur> to the server as a synth.
def play(msg, time, src):
    freq = 440.0 * 2 ** ((msg[1] - 69) / 12)
    Synth("default", {"freq": freq}, server=server)

OscFunc(play, "/note", recv=recv)

# The decorator form reads the same and returns the OscFunc.
@oscfunc("/ctl", arg_template=[7, None], recv=recv)
def on_cc(msg, time, src):
    print("controller 7 ->", msg[2])
```

```ts
import { OscFunc, Synth, oscfunc } from "clausters";

const recv = server.receiver;

// Relay an incoming /note <midinote> <dur> to the server as a synth.
function play(msg: unknown[], time: number | null, src: string) {
    const freq = 440.0 * 2 ** (((msg[1] as number) - 69) / 12);
    new Synth("default", { freq }, { server });
}

new OscFunc(play, "/note", { recv });

// The builder form reads the same and returns the OscFunc.
const onCc = oscfunc("/ctl", { argTemplate: [7, null], recv })((msg, time, src) => {
    console.log("controller 7 ->", msg[2]);
});
```

{{alt `OscFunc(...).one_shot()` || `new OscFunc(...).oneShot()`}} frees the responder after its first match, for a one-time wait.

## MIDI: message {{alt dicts || objects}}

A `MidiReceiver` decodes raw channel-voice bytes into {{alt a dict || an object}} with a `type` and the type's fields — {{alt `{"type": "note_on", "channel": 0, "note": 60, "velocity": 100}` || `{ type: "note_on", channel: 0, note: 60, velocity: 100 }`}}, {{alt `{"type": "control_change", "channel": 0, "control": 7, "value": 127}` || `{ type: "control_change", channel: 0, control: 7, value: 127 }`}}, and so on (`pitchwheel` carries a single 14-bit `pitch`). A `MidiFunc` callback is called `func(message, src)` with that {{alt dict || object}} and the port name, and you match on the type, an optional `chan`, and an `{{api arg_template}}` over the fields:

```python
from clausters.base import MidiReceiver
from clausters.defs import Synth
from clausters.responders import MidiFunc

recv = MidiReceiver(port="clausters-in").start()
voices = {}

def note_on(m, src):
    if m["velocity"] == 0:
        return note_off(m, src)
    freq = 440.0 * 2 ** ((m["note"] - 69) / 12)
    amp = m["velocity"] / 127 * 0.3
    voices[m["note"]] = Synth("default", {"freq": freq, "amp": amp}, server=server)

def note_off(m, src):
    synth = voices.pop(m["note"], None)
    if synth is not None:
        synth.free()

MidiFunc(note_on, "note_on", recv=recv)
MidiFunc(note_off, "note_off", recv=recv)
```

```ts
import { MidiFunc, MidiReceiver, Synth, requestMidiPorts } from "clausters";

const recv = await new MidiReceiver({ port: "Keystation", access: await requestMidiPorts() }).start();
const voices = new Map<number, Synth>();

function noteOn(m: any, src: string) {
    if (m.velocity === 0) {
        return noteOff(m, src);
    }
    const freq = 440.0 * 2 ** ((m.note - 69) / 12);
    const amp = m.velocity / 127 * 0.3;
    voices.set(m.note, new Synth("default", { freq, amp }, { server }));
}

function noteOff(m: any, src: string) {
    voices.get(m.note)?.free();
    voices.delete(m.note);
}

new MidiFunc(noteOn, "note_on", { recv });
new MidiFunc(noteOff, "note_off", { recv });
```

<!-- python -->
That virtual input port is a loose cable until you wire a MIDI source into it (`pw-link`, qpwgraph, or `aconnect`).
<!-- /python -->
<!-- web -->
**A page picks a port; it does not make one.** That is Web MIDI's shape rather than a choice: a script's `MidiReceiver` opens a virtual port that other applications are then wired into, and a page can create nothing. `requestMidiPorts()` asks the user once and hands back the ports that already exist, so `port` here **selects** one — by a case-insensitive fragment of its name, by its exact id, or omitted for the first — and passing the grant as `access` to several receivers and outputs reuses one permission prompt. Since the grant is asynchronous and the user's, `start()` is awaited, and there is no lazily-created default: `defaultMidiReceiver()` returns what `setDefaultMidiReceiver(...)` pinned and otherwise throws saying so, because opening a port behind the page's back is exactly what the browser forbids. Pin one after starting it and `new MidiFunc(fn, "note_on")` needs no other arguments.
<!-- /web -->

This is the client-side mirror of the server's own direct MIDI input: a Clausters server can be played by MIDI it receives itself, *or* by a client that listens to MIDI and forwards `/synth_new`. Both coexist.

## Playing *to* MIDI

The other direction is a destination, not a responder: a `MidiServer` is something an event pattern plays to, so the same `pbind` sounds through the audio engine or out a MIDI port depending only on what it is handed.

```python
from clausters.base import MidiRtInterface, MidiServer
from clausters.seq import pbind, pseq

midi = MidiServer(interface=MidiRtInterface(port="clausters"), channel=0)
pbind(midinote=pseq([60, 64, 67]), dur=0.5).play(midi)
```

```ts
import { MidiRtInterface, MidiServer, requestMidiPorts } from "clausters";
import { pbind, pseq } from "clausters/seq";

const midi = new MidiServer({
    interface: await MidiRtInterface.open({ access: await requestMidiPorts() }),
    channel: 0,
});
pbind({ midinote: pseq([60, 64, 67]), dur: 0.5 }).play(midi);
```

Each `Event` becomes a note on/off pair — the note from `midinote()`, the velocity from `velocity()`, the release at `sustain()` — and `MidiItem` makes an event of raw bytes for a `Timeline` the way `OscItem` makes one of a raw message. **Timing is best-effort by design** (see [Timing models](timing-models.md#midi-always-rides-osc-time))<!-- web -->, though the browser gives some of it back: `MIDIOutput.send` takes a `performance.now()` deadline, so a note-off two beats out is handed its deadline rather than slept to<!-- /web -->.

Constructed with no interface, a `MidiServer` accumulates a **score** instead: {{alt `midi.score.to_smf(480)` and `.to_clip(480)` || `midi.score.toSmf(480)` and `.toClip(480)`}} return the bytes of a Standard MIDI File or a MIDI 2.0 clip, written by the shared `clausters-midi` crate, so the file is the same from either client. {{alt `midi.write("take.mid")` || `await midi.write("take.mid")`}} puts that file at a path<!-- web --> (the page's own storage in a tab, the disk under node)<!-- /web -->, and {{alt `fmt="clip"` || `{ fmt: "clip" }`}} writes the clip instead. A MIDI destination can also be an MPE zone — see [Timing models](timing-models.md#an-mpe-zone-as-the-destination).

## A panic frees them, unless they are permanent

A panic (`panic()`, `session.panic()`, `Ctrl+.` in clausters-editor) frees every responder, OSC and MIDI, as sclang's `CmdPeriod` does — whichever session panics, since responders belong to the program and not to a session. One that must survive it — a controller that keeps a fader mapped while everything else is stopped — is made `permanent`:

```python
OscFunc(on_fader, "/fader", recv=recv, permanent=True)
```

```ts
new OscFunc(onFader, "/fader", { recv, permanent: true });
```

`permanent` is an attribute too, so a responder can be pinned or released later.

## Threading: the golden rule

A responder's callback runs {{alt on the receiver's background thread || on the page's one thread as the packet arrives}} (or, if the receiver was given a `clock`, through the clock, with its logical time). The [golden rule](routines-and-clocks.md#the-one-rule) holds: **never block {{alt that thread || the page}}.** Keep callbacks quick. To *sequence* in response to an incoming message — play a phrase, not a single note — schedule a routine on a clock, which returns immediately:

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

```ts
import { Routine } from "clausters";

function startPhrase(msg: unknown[], time: number | null, src: string) {
    function* phrase() {
        for (const degree of [0, 2, 4]) {
            new Event({ instrument: "default", degree }).play(server);
            yield 0.25;
        }
    }
    clock.play(new Routine(phrase));   // returns at once; the clock runs it
}

new OscFunc(startPhrase, "/go", { recv });
```

## Reacting to the shared transport

Setting the server's shared transport (`/transport_set`, the beat grid several clients phase-align on — see [Timing models](timing-models.md)) **pushes** the new grid to every client registered with `/server_notify`. A responder on `/transport_query.reply` re-aligns this client the moment a conductor changes the tempo or origin, with no polling.<!-- python --> Register `/server_notify` from the receiver's own socket so the push lands where the responder listens:<!-- /python --><!-- web --> The page's connection to the server is already registered, so the responder only has to be on its receiver:<!-- /web -->

```python
recv = OscReceiver(port=57121).start()
recv.send(server.target.addr(), "/server_notify", 1)   # subscribe on this socket

def on_transport(msg, time, src):
    origin, tempo, defined = msg[1], msg[2], msg[3]
    if defined:
        clock.join_transport(server)             # adopt the new grid

OscFunc(on_transport, "/transport_query.reply", recv=recv)
```

```ts
function onTransport(msg: unknown[], time: number | null, src: string) {
    const [, origin, tempo, defined] = msg;
    if (defined) {
        void clock.joinTransport(server);        // adopt the new grid
    }
}

new OscFunc(onTransport, "/transport_query.reply", { recv: server.receiver });
```

## Examples

<!-- python -->
- **`osc_responder.py`** — the client as an OSC hub: relay incoming `/note` to the server, and re-align on a `/transport_query.reply` push. Runs against a live server, self-feeding a few messages to demonstrate.
- **`midi_responder.py`** — a `MidiFunc` turning a MIDI keyboard into synths on the server.
<!-- /python -->
<!-- web -->
- **`io/responders.html`** — a def reports its own onsets with `SendReply`, a responder answers each one with a synth, two more keep the count of what is alive, and one re-aligns on a `/transport_query.reply` push.
- **`io/midi-responder.html`** — a `MidiFunc` turning a MIDI keyboard into synths on the in-page engine.
<!-- /web -->

See [Examples](examples.md).
