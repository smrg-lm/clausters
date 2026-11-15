# A DAW-style transport

A DAW has a *transport*: a shared timeline with a tempo and a bar grid. Everything you arm starts aligned to bars, and moving the tempo moves the whole arrangement with it. Clausters offers that same idea, but across **independent clients**: a server hosts one shared beat grid, several clients join it, and a routine on each starts on the same bar. This page is the practical guide to that workflow — being the conductor, joining as a follower, starting together on a bar, and following a tempo change live — and then to the transport that **governs** sound: freezing it, event lanes, and several transports on one server.

It builds on two other pages. [Timing models](timing-models.md) explains *why* the alignment is beat-accurate or sample-exact (the time reference a clock paces against); this page is the *how* of the transport itself. [Receiving OSC and MIDI](responders.md) is the input layer the live-change section uses.

The whole surface is on the `Server`, so it is carrier-agnostic: the same calls drive a native server<!-- web --> over its WebSocket and the in-page engine<!-- /web --><!-- python -->, local or remote, and an embedded one<!-- /python -->.

## The shared grid

The transport is deliberately small: an **origin sample** (the sample position of beat 0) and a **tempo** (beats per second). Together they are a grid — beat `b` is sample `origin + b·rate/tempo` — that the server stores under `/transport_set` and any client can read. That is the whole of it: the server *hosts* the grid but never plays from it. There is no server-side playhead rolling forward; each client's `TempoClock` is its own playhead, and the grid is the common ruler they all measure bars against.

One client is the **conductor**: it defines the grid once.

```python
from clausters.defs import Server

server = Server()                       # UDP to 127.0.0.1:57110
server.set_transport(origin_sample=0, tempo=2.0)   # beat 0 at sample 0, 2 beats/s
```

```ts
import { Server } from "clausters/defs";

const server = await new Server({ transport: "ws", url: "ws://127.0.0.1:57120" }).attach();
await server.setTransport(0, 2.0);      // beat 0 at sample 0, 2 beats/s
```

Every other client is a **follower**: it adopts that grid as its own tempo and origin.

```python
from clausters.base import TempoClock

clock = TempoClock()                    # its own tempo is about to be overwritten
clock.join_transport(server)            # adopt the shared tempo + origin
```

```ts
import { TempoClock } from "clausters/base";

const clock = new TempoClock();         // its own tempo is about to be overwritten
await clock.joinTransport(server);      // adopt the shared tempo + origin
```

With a `Session`, both sides are one call — {{alt `session.server.set_transport(...)` || `session.server.setTransport(...)`}} to conduct, `{{api session.join_transport}}()` to follow. `{{api clock.leave_transport}}()` (or never joining) returns a clock to its own private grid, and `clock.joined` says which of the two it is on.

Joining reads the grid once and keeps it — a tempo, an origin and which axis that origin lives on; afterwards the clock does not talk to the server, and nothing about a joined clock waits on one. From then on `{{api clock.grid_beat}}()` is where the shared grid is now — the clock's own `beats()` when it has not joined one. That is the number `quant` snaps against, and the one to read when computing where a bar falls. A server with no grid defined leaves the clock exactly as it was.

**The grid does not stop with the transport.** It is a metronome on the server's device clock, which never stops, and it is what independent clients phase-align on — including before anyone has pressed play, which is how `/transport_set` leaves it (stopped at position 0) and how {{alt `examples/transport/sync.py` || `examples/transport/sync.html`}} uses it. So a pause of a governed transport does not hold the grid: the music stops and the grid runs on, and after a three-second pause the music is three seconds behind the bar lines the grid draws. Where the music *is* is the transport's **position** ({{alt `transport_state()["position"]` || `(await server.transportState()).position`}}, in beats, or `{{api position_sample}}`), which holds through a pause and jumps with a locate; a client that wants the music's next bar reads that.

## Starting together on a bar

A DAW starts a clip on the next bar, not the instant you click. The client's equivalent is `quant`: the beat boundary a routine's start snaps to.

```python
clock.start()                                   # the playhead must be running first
clock.play(routine, quant=4)                    # start on the next 4-beat bar
session.play(pattern, quant=4)                  # the Session form
```

```ts
clock.start();                                  // the playhead must be running first
clock.play(routine, 4);                         // start on the next 4-beat bar
session.play(pattern, 4);                       // the Session form
```

A `quant` of 4 snaps the start to the next beat that is a multiple of 4 — a bar in 4/4. 1 is the next beat; {{alt `None` || `null`}} or `0` starts immediately. Because every follower's `quant` snaps to the **same shared grid**, they all land on the same bar, so independent clients begin in phase. Start the clock *before* playing the quantized routine, so `quant` measures against the running grid rather than a stopped one.

`quant` works without a transport too — then it snaps to the clock's own elapsed beats, which is the clean way for a single client to drop a new voice in on the next bar. Joining a transport is what makes that bar the *same* bar across clients.

## Beat-accurate or sample-exact

How tightly the clients align depends on the time reference each clock paces against — the subject of [Timing models](timing-models.md), in one paragraph here:

- **Plain (wall-clock) followers** align to the **beat**, drift-bounded: the grid's sample origin is mapped to OSC time through the server's `/clock_query` anchor, so everyone agrees on the bar to within the wall-vs-audio drift.
- **Followers whose clock is on the server's sample clock** align to the **sample**: the grid lives on the master's sample axis, so the shared bar is one exact sample for all of them. A live session's clocks are, by default<!-- web -->, and so are an embedded one's<!-- /web -->.

```python
clock = TempoClock(timebase=server.sample_timebase())  # sample-exact, drift-free
clock.join_transport(server) # ...then phase-align on the shared bar
```

```ts
const clock = new TempoClock(1, { timebase: await server.sampleTimebase() });  // sample-exact, drift-free
await clock.joinTransport(server);  // ...then phase-align on the shared bar
```

A clock's timebase is fixed when it is made, so the reference is chosen first, and the clock aligns on it afterwards.

## Following a tempo change live

This is where the transport behaves most like a DAW's: when the conductor changes the tempo (or origin), every follower should move with it. Setting `/transport_set` again **pushes** the new grid to every client registered for notifications ({{alt `server.notify(True)` || `server.notify(true)`, which `boot` and `attach` do by default}}), so followers do not have to poll. A follower reacts with an [OSC responder](responders.md) on `/transport_query.reply`:

```python
from clausters.base import OscReceiver
from clausters.responders import OscFunc

recv = OscReceiver().start()
recv.send(server.target.addr(), "/server_notify", 1)   # subscribe on this socket

def follow_transport(msg, time, src):
    # msg == ["/transport_query.reply", origin_sample, tempo, defined]
    if msg[3]:                                   # defined
        clock.join_transport(server)             # re-adopt the new grid

OscFunc(follow_transport, "/transport_query.reply", recv=recv)
```

```ts
import { OscReceiver } from "clausters/base";
import { OscFunc } from "clausters/responders";

// The page's connection to the server is already its receiver, and already
// registered for notifications.
function followTransport(msg: unknown[], time: number | null, src: string) {
    // msg == ["/transport_query.reply", originSample, tempo, defined]
    if (msg[3]) {                                // defined
        void clock.joinTransport(server);        // re-adopt the new grid
    }
}

new OscFunc(followTransport, "/transport_query.reply", { recv: server.receiver });
```

Now the conductor doing {{alt `server.set_transport(0, 3.0)` || `server.setTransport(0, 3.0)`}} later in the session re-tempos every follower at once — the bar grid they quantize against moves together.<!-- python --> (Register `/server_notify` from the *receiver's* socket, as above, so the push lands where the responder is listening.)<!-- /python --> The shipped {{alt `osc_responder.py` || `io/responders.html`}} example wires exactly this reaction; see [Examples](examples.md).

## Rolling the transport: a conductor with play / stop / locate

The transport also carries a DAW-style **rolling state** — whether it is playing, and the position — that a conductor drives and every timeline **on that transport** obeys. The server holds the state and owns the time; it plays no notes of its own.

A conductor (any client) drives it through the `Server`:

```python
server.transport_group(group)          # the transport owns the nodes it plays
server.transport_play()                # roll -- every timeline on it rolls
server.transport_locate_sample(sample) # seek the position
server.transport_stop()                # freeze it, and the plans with it
```

```ts
await server.transportGroup(group);          // the transport owns the nodes it plays
await server.transportPlay();                // roll -- every timeline on it rolls
await server.transportLocateSample(sample);  // seek the position
await server.transportStop();                // freeze it, and the plans with it
```

`{{api transport_play}}` takes the song-position beat to roll from, and `{{api transport_locate}}(beat)` seeks by beat as `{{api transport_locate_sample}}` does by sample. Reading the whole state at once:

```python
state = server.transport_state()
# {origin_sample, tempo, playing, position, group, transport_sample, ...}
```

```ts
const state = await server.transportState();
// { originSample, tempo, playing, position, group, transportSample, ... }
```

`group` is the governed group or {{alt `None` || `null`}}, and `{{api transport_sample}}` is the transport clock — samples elapsed *under the transport*, held while it is stopped. It is the time of the governed, as against the device clock (the taps, the bus streams, `/clock_query`), which never stops. The two are one physical clock and cannot drift apart; they differ only while a governed transport is stopped.

A follower puts its timeline on that transport, and that is the whole of following:

```python
timeline.transport = server     # needs a governed group bound
```

```ts
timeline.transport = server;    // needs a governed group bound
```

From then on `timeline.play`, `pause`, `stop` and `locate` are the transport's own commands, and the timeline **plans** its items onto the transport's clock (`/sched_atTransport`) from the position it is at — so a freeze holds them with the governed and a locate clears the transport queue ({{alt `server.sched_clear("transport")` || `server.schedClear("transport")`}}) and re-cues from the new position, `latency` ahead. A locate or a play somebody *else* sent arrives as a broadcast (`/transport_query.reply`, the [responder layer](responders.md)), and the plan is written again from where it says. Every follower reads the **one** position the engine holds, so they are in lockstep by construction rather than by each computing its own. {{alt `conductor.py` || `transport/conductor.html`}} ([Examples](examples.md)) shows two followers on one transport; {{alt `timeline.transport = None` || `timeline.transport = null`}} gives it back its own clock.

**What is sounding keeps its release** through any re-cue — a locate, a conductor's locate — since the timeline keeps what it queued and sends back the releases of the notes already started, on their own samples. An edit to the timeline is heard from the next play or locate.<!-- web -->

**The verbs are queued rather than awaited**, where the Python client's block: the transport's commands are requests, so each of a timeline's verbs goes onto one chain, and `await timeline.refresh()` settles with it and reads the position back.<!-- /web -->

What the mode refuses, and why: a `quant` (the start is the transport's), a `loop` (the wrap is the engine's, and a timeline's events would have to be re-cued on every one of them), and a **forward-only item** — a routine or a pattern cannot be planned from a position. Those are the client-clock mode's.

## A worked example: two clients, one bar

{{alt `sync.py` || `sync.html`}} runs two completely independent client pairs — each its own `Server` and `TempoClock`, the state two separate programs would hold — and lands a note from each on the same bar. The check uses public state only, so any client on the same transport computes the same number:

```python
import math

def next_bar_sample(server, clock, quant=4):
    origin, tempo = server.transport()
    rate = clock.timebase.sample_rate
    target = math.ceil(clock.grid_beat() / quant) * quant
    return round(origin + target * rate / tempo)
```

```ts
async function nextBarSample(server: Server, clock: TempoClock, quant = 4): Promise<number> {
    const { originSample, tempo } = (await server.transport())!;
    const rate = (clock.timebase as SampleClockTimebase).sampleRate;
    const target = Math.ceil(clock.gridBeat() / quant) * quant;
    return Math.round(originSample + target * rate / tempo);
}
```

Sampled back to back, the two clients return the same next-bar sample — that equality *is* the phase alignment. Each then plays a note with a `quant` of 4, and the two sound together. The example is in [Examples](examples.md).

## What it is, and what it is not

The analogy to a DAW transport is the **bar grid and tempo plus a play/stop/position state** — enough to lock clients to the same bars, tempo, and rolling playhead. The rest of a DAW's transport is intentionally not here:

- **The server owns the time and broadcasts transport *control*.** It holds the grid, the rolling state and the position, and pushes changes. What it plays by itself is **concrete data**: a take through a reader, and notes and messages through an **event lane** (`{{api server.lane_new}}`, `{{api lane_set}}`, `{{api lane_free}}`), which the transport plays by its position as a reader plays a take — the notes editor sounds that way. What is generated as it plays — a pattern, a routine — is each client's own plan, stamped on the transport's clock (`/sched_atTransport`) or on the device's (`/sched_at`) — see [Timelines](timelines.md).
- **One grid per transport, last-writer-wins.** Whoever calls `{{api set_transport}}` most recently defines it, and defining it resets the rolling state to stopped at position 0. Several conductors are a coordination choice you make, not something the server arbitrates.
- **Tempo and origin only — no meter object.** A "bar" is whatever beat multiple you pass as `quant`; there is no separate time-signature the server stores. Pick a `quant` that matches your meter (4 for 4/4, 3 for 3/4).
- **No server-side recording or sequencing.** The timeline lives in the client; the server holds only the position, the rolling state and the grid, not the notes.

These are the honest edges of a small, composable feature: shared bars, a shared tempo, and a shared play/stop/position that several clients phase-align on, with each client owning its own playhead and arrangement.

## Cheat-sheet

<!-- python -->
| You want to… | Do this |
| --- | --- |
| Define the shared grid (conductor) | `server.set_transport(origin_sample, tempo)` |
| Read the current grid | `server.transport()` → `(origin_sample, tempo)` or `None` |
| Join the grid (follower) | `clock.join_transport(server)` / `Session.join_transport()` |
| Leave it | `clock.leave_transport()` |
| Start on the next bar | `clock.play(routine, quant=4)` / `session.play(pattern, quant=4)` |
| Align to the sample, not just the beat | a clock on `server.sample_timebase()` — a live session's default (see [Timing models](timing-models.md)) |
| Follow live tempo changes | an `OscFunc("/transport_query.reply", …)` that re-`join_transport`s (see [Receiving OSC and MIDI](responders.md)) |
| Roll a playhead from a conductor | `server.transport_play()` / `transport_stop()` / `transport_locate(beat)`; followers put a timeline on it, `timeline.transport = server` |
| Read the rolling state | `server.transport_state()` → `{tempo, playing, position, …}` |
<!-- /python -->
<!-- web -->
| You want to… | Do this |
| --- | --- |
| Define the shared grid (conductor) | `await server.setTransport(originSample, tempo)` |
| Read the current grid | `await server.transport()` → `{ originSample, tempo }` or `null` |
| Join the grid (follower) | `await clock.joinTransport(server)` / `await session.joinTransport()` |
| Leave it | `clock.leaveTransport()` |
| Start on the next bar | `clock.play(routine, 4)` / `session.play(pattern, 4)` |
| Align to the sample, not just the beat | a clock on `await server.sampleTimebase()` — a session's default (see [Timing models](timing-models.md)) |
| Follow live tempo changes | a `new OscFunc(…, "/transport_query.reply")` that re-`joinTransport`s (see [Receiving OSC and MIDI](responders.md)) |
| Roll a playhead from a conductor | `server.transportPlay()` / `transportStop()` / `transportLocate(beat)`; followers put a timeline on it, `timeline.transport = server` |
| Read the rolling state | `await server.transportState()` → `{ tempo, playing, position, … }` |
<!-- /web -->

## Freezing the sound: when the transport governs it

Everything above is a transport clients obey **by choice** — a shared grid and a
rolling state the server broadcasts, which each client reads to start on the
same bar. `{{api server.transport_group}}(group)` changes that. With a group bound, the
transport stops being an advisory and the **engine enforces it**:
`{{api transport_stop}}()` freezes that group and everything under it at the exact
sample it lands on, and `{{api transport_play}}()` thaws it.

A frozen node stays in the tree with its internal state untouched — filters keep
their memory, phasors their phase, envelopes their position. So a resume
**continues** the sound rather than starting it again:

```python
governed = Group(server=server)
server.set_transport(0, 2.0)
server.transport_group(governed)
server.transport_play()
...
server.transport_stop()    # the subtree freezes, mid-gesture
...
server.transport_play()    # and carries on from exactly there
```

```ts
const governed = new Group({ server });
await server.setTransport(0, 2.0);
await server.transportGroup(governed);
await server.transportPlay();
// ...
await server.transportStop();    // the subtree freezes, mid-gesture
// ...
await server.transportPlay();    // and carries on from exactly there
```

This matters most for samples the server *generates*. A def running a
stochastic process or a demand-rate sequence has nothing to read and no
messages arriving, so there is no position to seek to — its position **is** its
internal state. Continuing is the only thing a pause can honestly mean for it,
and it is what no DAW transport offers, because a DAW's audio exists before
you press play.

That asymmetry runs through the whole feature:

- **Pause and resume work on anything.** They are a freeze, so they do not care
  what produced the sound.
- **Locate does not.** `{{api transport_locate}}(beat)` moves the position and never a
  node's state, and locating over a multitrack holding a resident generator is
  refused instead of moving a cursor the sound will not follow.
  Render the element first and it becomes samples like any other — the change
  of state the [document](document.md) chapter is built around.

Anything **scheduled** against a governed node waits out the pause with it and
fires on resume in its right relative place, so a look-ahead already in flight
is not lost. A bundle is atomic, so a bundle holding one governed message and
one live one waits entirely — the one way a message aimed at a live node ends up
waiting. Unbinding thaws whatever the transport governed, and so does freeing
the group — no frozen subtree is ever left with nobody to resume it.

The clock follows too. A clock's beats come from a timebase that only decides how long to sleep before the next wake, so a client whose server froze would keep advancing beats and scheduling samples ahead — running away from a governed that is not moving. `TempoClock.freeze()` holds the beat where it is and
`thaw()` picks it up there, shifting the pacing origin by the time spent frozen. The reaction does not have to be precise: between the server's stop and the call, a little look-ahead has already gone out, and it lands in the server's frozen queue to fire on the resume in its exact relative place.<!-- python --> `clausters.gui.PlayheadSync` does this for you, and its `resume()` is
deliberately **not** `play()` — play re-renders from a position, resume
continues the frozen sound.<!-- /python -->

<!-- python -->
| You want to… | Do this |
| --- | --- |
| Let the engine enforce the transport | `server.transport_group(group)` |
| Give it back | `server.transport_group(None)` (thaws what it governed) |
| Freeze / carry on | `server.transport_stop()` / `server.transport_play()` |
| Read the transport's own clock | `server.transport_state()["transport_sample"]` |
| Continue rather than restart, client-side | `transport.resume()` (not `play()`) |
| Ask whether a seek means anything | `editor.locatable` |
<!-- /python -->
<!-- web -->
| You want to… | Do this |
| --- | --- |
| Let the engine enforce the transport | `await server.transportGroup(group)` |
| Give it back | `await server.transportGroup(null)` (thaws what it governed) |
| Freeze / carry on | `await server.transportStop()` / `await server.transportPlay()` |
| Read the transport's own clock | `(await server.transportState()).transportSample` |
<!-- /web -->

{{alt `freeze.py` || `freeze.html`}} in the examples freezes a generative texture and resumes
it, which is the way to hear the difference between continuing and restarting.

## Scheduling on the transport axis

Every sample count on the wire belongs to one of the two clocks, and which one a scheduled bundle rides is decided by where its messages point: a bundle targeting a node at or under the governed group waits out a pause, and everything else fires on the device clock as always. Classification happens once, when the bundle is queued.

A client naming an absolute sample of the *transport* declares that axis:

```python
server.sched_at_transport(sample, ("/synth_new", "grain", node, 0, governed.id))
```

```ts
await server.schedAtTransport(sample, [
    ["/synth_new", "grain", ["i", node], ["i", 0], ["i", governed.id]],
]);
```

The declaration is not there to disambiguate — the classification is deterministic, and a client that bound the group knows which of its nodes are governed. It is there to be **verified**: the server compares it against its own classification and fails when they disagree, rather than playing the bundle in the wrong place. It needs a group bound.

## Event lanes: notes the transport plays by its position

A scheduled bundle rides the transport's **clock**, which never jumps, so a plan stamped on it has to be written again after every locate. Notes that are **data** — a roll's, a notes region's — have a better home: an **event lane**, which holds them at samples of the transport's **position** and is played by the server the way a reader plays a take. A locate moves them, a loop plays them again on every pass, a stop releases them — as a DAW's stop sends its note-offs, so their releases ring out — and nothing is sent per pass.

```python
server.lane_new(7, voices)         # lane 7 on this transport, notes made in `voices`,
                                   # a group the transport does not govern
server.lane_set(7, {"notes": [[0, 24_000, "default", {"freq": 440.0}, "gate"]],
                    "messages": [[12_000, "/bus_set", 100, 1.0]]})
server.transport_play()
server.lane_set(7, new_data)       # an edit: heard from where the position is
server.lane_free(7)
```

```ts
await server.laneNew(7, voices);   // lane 7 on this transport, notes made in `voices`,
                                   // a group the transport does not govern
await server.laneSet(7, { notes: [[0, 24_000, "default", { freq: 440.0 }, "gate"]],
                          messages: [[12_000, "/bus_set", 100, 1.0]] });
await server.transportPlay();
await server.laneSet(7, newData);  // an edit: heard from where the position is
await server.laneFree(7);
```

Every position is a sample of the transport; a note is a synth of the def it names, or — written {{alt `{"graph": id, "slot": name}` || `{ graph: id, slot: name }`}} — one more of a slot of a running graph instance, which is how the notes editor plays a note that curves shape, or — written {{alt `{"node": id}` || `{ node: id }`}} — a note of a `Voices` or plugin node. A synth is released at its end by `gate 0` or by a free, a node's note by its note-off, and a note that names an earlier one as the one it `joins` goes on from it as one held voice, and a message is a command the server takes in a timed bundle, run as written. A `midi` list, `[[position, byte, byte, ...], ...]`, plays as though the messages had reached the server's MIDI input there: the channel's binding routes it to an event bus, a note-on and its note-off are one note, and a controller reaches the lane's own notes on that channel. A binding is the server's and is made once, wherever the notes come from — {{alt `server.midi_bind(0, "default")` || `server.midiBind(0, "default")`}} routes channel 0 to a `Voices` node of the default instrument for this lane and for the live MIDI input alike, {{alt `server.midi_bind(0, bus)` || `server.midiBind(0, bus)`}} to an event bus a node already reads, `{{api midi_bind_zone}}` an MPE zone, `{{api midi_map}}` routes a message to a control, and `{{api midi_query}}` reads back what a server already holds. A `ump` list, `[[position, word, word, ...], ...]`, is MIDI 2.0: one packet's words each, read among the MIDI bytes — its notes and controllers at their own resolution, and a per-note bend or controller reaching the note on its channel and key. A new `{{api lane_set}}` keeps the release of whatever is sounding; a locate or a stop releases it, as a DAW does. The notes editor plays this way — its sequence is its lane's data — and so do a multitrack's notes regions.

## Several transports

A server has several transports — 16 unless it was booted with another count
({{alt `ServerOptions(transports=...)` || `new Server({ sizing: { transports } })`, or `--transports` for a native one}}, read back as {{alt `server.query_info().transports` || `(await server.queryInfo()).transports`}})
— and each is independent: its own grid, rolling state, position, loop, end
mark, governed group and clock. Two applications on one server, a multitrack
and an audio editor say, each play, pause and locate their own nodes without
moving the other's.

Every transport method on a `Server` addresses **transport 0**, the one there
always was. `{{api server.transport_at}}(n)` is transport `n` as an object of its own,
a `Transport` — the same one every time it is asked for, and the way to
address one already known, such as an editor's `transport.id` — played the way a
routine or a timeline is: `play`, `pause`, `stop`, `locate`, `loop` and
`unloop`, {{alt `end` || `end()` and `setEnd()`}} (where a pass stops), {{alt `playing` || `playing()`}}, and a `wait()` a {{alt script calls || page awaits}}
or not. Its other commands
are there by their own names — `group`, `follow`, `fade`, `{{api locate_sample}}`,
`{{api lane_new}}`, `state` — and it goes wherever a transport is taken.

```python
left = server                          # transport 0, through the server's own methods
right = server.transport_new()         # a free one, taken for this script
right.group(other_group)               # a group has one transport
right.play()                           # rolls it alone
timeline.transport = right             # a timeline on it
right.free()                           # and it goes back to the server's
```

```ts
const left = server;                   // transport 0, through the server's own methods
const right = server.transportNew();   // a free one, taken for this script
await right.group(otherGroup);         // a group has one transport
await right.play();                    // rolls it alone
timeline.transport = right;            // a timeline on it
await right.free();                    // and it goes back to the server's
```

Its positions are those of what is loaded on it: with nothing loaded, the
transport's own seconds; once `play(sequence)` has put a sequence's lane there,
that sequence's beats — `play` answers the `Transport` it loaded, so
`transport.locate(4)` is beat 4 of the sequence and `transport.wait()` returns
when its last note ends.

**The loop is the roll's.** A transport keeps a time range, its `span`, and a
loop switch, and both are state rather than commands: kept while it is
stopped, read by the next `play`. With a sequence loaded they are the ones its
rolls show — the band an Alt+drag sweeps, and `L` — so a span set here is drawn
there and a range swept there reads here, and `play` plays the span from its
start to its end, as the space bar does.

```python
transport.loop(0, 2)          # span (0, 2) and the loop on: Alt+drag and L at once
transport.unloop()            # the loop off; the span stays
transport.span = None         # no span: play plays from the cursor
```

```ts
await transport.loop(0, 2);       // span [0, 2] and the loop on: Alt+drag and L at once
await transport.unloop();         // the loop off; the span stays
await transport.setSpan(null);    // no span: play plays from the cursor
```

The applications' transports are the same object. A multitrack editor's
`editor.transport` is the one it plays on, in the multitrack's seconds: its
`span` is the band an Alt+drag sweeps over the tracks, its loop switch is that
window's `L`, and `play`, `pause`, `stop` and `locate` are the transport row's.
An audio editor's is the take's, in its seconds, its `span` the range a drag
marks. A notes editor's is its sequence's — the same object `play(sequence)`
answers for that sequence, in its beats — and asking a roll for it plays
nothing.

A node reads the transport that governs it — the nearest governed group above
it — so a reader following `{{api transport_pos}}` needs no id of its own: it follows
whichever transport its group is bound to, and transport 0 when none is.
`{{api transport_state}}()` says which transport it read in its `transport` entry.

What must go on running while a transport is stopped and still has to know it
— an output with its meter and its declick, beside the governed group or
around it — **follows** the transport instead of being governed by it:
`{{api transport_follow}}(group)` has its nodes read the transport and nothing freeze
them.

**A stop can fade.** A declick cannot run after the freeze — on that sample the
readers stop producing anything — so `{{api transport_fade}}(samples)` gives a
transport a ramp: a stop then rolls on while the ramp falls to zero and
freezes when it gets there, so the position rests where the readers stopped
reading, and a play rises from zero. What reads the ramp is `{{api transport_fade}}()`,
the UGen, in the following group: an output multiplies what the readers wrote
by it, and neither edge clicks. With no ramp set — the default — a stop freezes
on its own sample.

**A transport above 0 is taken, not named.** A multitrack plays on transport
0. Everything else that plays on its own takes one of the rest from the
server's: each sequence (`play(sequence)`, a roll), the audio editors of a
server, and the GUI host's monitor, for a window that draws samples with no
application behind it. So two sequences are two transports and sound together,
and a roll's play cursor is its own sequence's. `{{api server.transport_new}}()` takes
one for a script — a timeline of its own, a group to govern apart — and
`transport.free()` gives any of them back: for a sequence it also stops it and
frees its lane. A roll gives its sequence's back when the last roll over it
closes, unless a script asked for the transport, whose it then is to free.

A server has a fixed number of them, and a GUI host sharing the server takes
half of what is above 0: with the default 16, a script alone has fifteen and
a script with a host seven. With none left, playing fails and says so; boot
the server with more ({{alt `ServerOptions(transports=32)` || `{ sizing: { transports: 32 } }`}}, at most 64).

## See also

- [Timing models](timing-models.md) — the time reference behind beat-accurate vs sample-exact alignment.
- [Routines and clocks](routines-and-clocks.md) — the playhead (`TempoClock`) and the routines you start on the bar.
- [Receiving OSC and MIDI](responders.md) — the responder layer the live-change reaction uses.
- [Sessions](sessions.md) — the handle that bundles a clock and a server, with `{{api join_transport}}`.
- [Examples](examples.md) — {{alt `sync.py` (two clients on one bar) and `osc_responder.py` (the live transport reaction) || `transport/sync.html` (two clients on one bar) and `io/responders.html` (the live transport reaction)}}.
- The **[Clausters server book](https://clausters.readthedocs.io/)** — `/transport_set`, `/transport_group`, `/sched_atTransport` and `/clock_query` on the wire.
