# Receiving: responders

Everything else here has the client *sending* — building OSC and pushing it to a server. A responder is the other direction: a callback that fires when a matching message **arrives**, and which may send onward, so the client becomes a hub rather than only a mouth.

The object is `OscFunc`, the same one the [Python client](https://clausters-python.readthedocs.io/) documents and the same one SuperCollider calls `OSCFunc`. Its model is shared and is not restated here; what follows is what the browser changes.

```js
import { OscFunc, Synth } from "clausters";

// Every node the server starts, as it starts.
const starts = new OscFunc((msg) => console.log("node", msg[1]), "/node_start");

// A def's SendReply, narrowed to one reply id.
new OscFunc(([, , , weight]) => new Synth("ping", { amp: weight }), "/onset", {
    argTemplate: [null, 1],
});

starts.free();
```

A responder is enabled the moment it is created. `disable()` suspends it and `enable()` puts it back; `free()` is permanent; `oneShot()` frees it after its first match. The callback is `func(msg, time, src)` — `msg` the message as `[addr, ...args]`, `time` the containing bundle's Unix seconds (`null` for an immediate or bare message), `src` the sender. `argTemplate` matches the arguments by position, an entry being a literal, a predicate or `null` for a hole; `oscfunc(path, options)(callback)` is the builder form.

## The receiver is the carrier, because a page has no port

A responder registers with a **receiver**, the thing that decodes packets and hands them around. In the reference client that is a UDP socket it binds — a port any application on the machine can target. A page can bind nothing and be targeted by nobody: what it has is the connection it already opened, to the in-page engine or to a `--ws` server. So `OscReceiver` wraps a `Connection`, and everything that arrives on it reaches the responders registered there.

Two consequences follow, both visible in the API:

- **`src` names a carrier**, not a `(host, port)` pair: a socket's URL, or `"page"` for the in-page engine. Narrowing a responder with `{ src }` still answers the question it answers there — *who sent this?* — with what a browser actually knows.
- **The default receiver is the ambient session's server.** A responder that names no `recv` resolves it the way [the ambient verbs](verbs.md) resolve a server, and fails the same way when there is none. Each server has one receiver (`server.receiver`), so a page holding two sessions gets each one's messages from each one's responders; `setDefaultOscReceiver` pins one anyway.

Every server reaches its own responders through `server.receiver`, which is also the door the client's own reply handling goes through — the node ids that recycle as `/node_end` arrives are an `OscFunc` like any other. Under it, `server.onReply` is still there as the raw seam: it sees every decoded message in arrival order with no matching of its own, which is what a stream decoder wants and what a responder is built on.

## Keep a callback quick

A callback runs on the page's one thread as the packet arrives, so a slow one delays everything else — the same golden rule the reference client states for its receiver thread, with the thread taken out. To *sequence* in response to a message, schedule a routine on a clock rather than looping inside the callback.

A receiver can also be given a clock (`new OscReceiver(connection, { clock })`), which dispatches its handlers through it: they then run as clock items, with the clock's logical time, instead of at the instant the bytes landed.

## MIDI: message objects

A `MidiReceiver` decodes raw channel-voice bytes into an object with a `type` and the type's fields — `{type: "note_on", channel: 0, note: 60, velocity: 100}`, `{type: "control_change", channel: 0, control: 7, value: 127}`, and so on (`pitchwheel` carries a single 14-bit `pitch`). A `MidiFunc` callback is called `func(message, src)` with that object and the port's name, and you match on the type, an optional `chan`, and an `argTemplate` over the fields:

```js
import { MidiFunc, MidiReceiver, Synth, requestMidiPorts } from "clausters";

const ports = await requestMidiPorts();
const recv = await new MidiReceiver({ port: "Keystation", access: ports }).start();
const voices = new Map();

function noteOn(m, src) {
    if (m.velocity === 0) return noteOff(m, src);
    const freq = 440.0 * 2 ** ((m.note - 69) / 12);
    const amp = m.velocity / 127 * 0.3;
    voices.set(m.note, new Synth("default", { freq, amp }, { server }));
}

function noteOff(m) {
    voices.get(m.note)?.free();
    voices.delete(m.note);
}

new MidiFunc(noteOn, "note_on", { recv });
new MidiFunc(noteOff, "note_off", { recv });
```

This is the client-side mirror of the server's own direct MIDI input: a Clausters server can be played by MIDI it receives itself, *or* by a client that listens to MIDI and forwards `/synth_new`. Both coexist.

### The page picks a port; it does not make one

The one place this differs from the reference client, and it is Web MIDI's shape rather than a choice. There, `MidiReceiver(port="clausters-in")` **opens a virtual port** other applications are then wired into. A page can create nothing: `navigator.requestMIDIAccess()` asks the user once and hands back the ports that already exist, so `port` here **selects** one — by a case-insensitive fragment of its name, by its exact id, or omitted for the first. `requestMidiPorts()` is that grant, and passing its result as `access` to several receivers and outputs reuses one permission prompt.

Two things follow from the grant being asynchronous and user-facing:

- **`start()` is awaited**, where the reference client's returns immediately.
- **There is no lazily-created module default.** `defaultMidiReceiver()` returns what `setDefaultMidiReceiver(...)` pinned and otherwise throws saying so, because opening a port behind the page's back is exactly what the browser forbids. Pin one after starting it and `new MidiFunc(fn, "note_on")` needs no arguments, as there.

## Playing *to* MIDI

The other direction is the same object the reference client has: a `MidiServer` is a destination an event pattern plays to, so the same `Pbind` sounds through the audio engine or out a MIDI port depending only on what it is handed.

```js
import { MidiRtInterface, MidiServer, requestMidiPorts, seq } from "clausters";

const midi = new MidiServer({
    interface: await MidiRtInterface.open({ access: await requestMidiPorts() }),
    channel: 0,
});
new seq.Pbind({ midinote: new seq.Pseq([60, 64, 67]), dur: 0.5 }).play(midi);
```

Each `Event` becomes a note on/off pair — the note from `midinote()`, the velocity from `amp`, the release at `sustain()` — and `MidiItem` puts raw bytes on a `Timeline` the way `OscItem` puts a raw message. **Timing is best-effort by design**, as it is there, but the browser gives some of it back: `MIDIOutput.send` takes a `performance.now()` deadline, so a note-off two beats out is handed its deadline rather than slept to.

Constructed with no interface, a `MidiServer` accumulates a **score** instead: `server.score.toSmf(480)` and `.toClip(480)` return the bytes of a Standard MIDI File or a MIDI 2.0 clip, written by the same `clausters-midi` the reference client writes with — so a `.mid` saved from a tab is the file a script would have saved. A page has no filesystem, so the bytes come back and the page decides (a download, a `fetch`, a buffer), exactly as `wavBytes` does for a take.

## Examples

The repository's `examples/io/responders.html` is the OSC worked example: a def reports its own onsets with `SendReply`, a responder answers each one with a synth, and two more keep the count of what is alive. `examples/io/midi-responder.html` is the MIDI one — a keyboard playing the in-page engine — and `examples/editors/pianoroll-midi.html` paints what you play into a piano roll.
