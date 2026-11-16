# Sessions

A `Session` is the client's ergonomic entry point: one object that owns a `Server` and a `TempoClock` together and drives them as a unit. It exists so that the everyday case — "set up a place to make sound, play a pattern, hear it or render it" — is a couple of lines, without giving up the design that makes this client flexible.

## Why it exists

SuperCollider's sclang is convenient largely because of globals: a default `Server`, a default clock, an implicit "current environment". You type `Synth("x")` and it just goes somewhere. That convenience has a cost — there is only ever *one* of each, so you cannot, say, run a live take and an offline render in the same {{alt script || page}} without them fighting over the same global state.

This client keeps the convenience but contains it. There is one ambient environment — the **default session** — and everything else is an explicit `Session` you name. The rule is one line: *what does not run in an explicit session runs in the default session*. So the one-liner works (a booted server, a bare {{alt `Event().play()` || `new Event().play()`}}), and yet you can spin up several isolated sessions — a live take next to an offline render — that never touch the default one or each other.

`Session` is that explicit, isolated environment: an object that holds a `Server` and one or more `TempoClock`s and offers `play` / `render` / `run`, plus factories that pick sensible defaults. Because it is a plain object and not a forced global, you can have as many as you like, each with its own server, clocks and random context.

The default session and a named `Session` are literally *the same kind of thing* — both extend one `Environment` base (a server plus a random context), so {{alt `clausters.default_session` || `defaultSession`}} **is** a session, the one used when you name none. Everything below (`play`, seeding, isolation) reads the same whether it runs in the default session or an explicit one.

## The default session

{{alt `clausters.default_session` || `defaultSession`}} (the `main` singleton) is the environment used whenever you did not name a session. It holds the ambient defaults: the default **server**, an opt-in default **clock** (created and started on first use), and the random context (`main.seed`). Booting a server free-standing adopts it there, first-wins:

```python
from clausters import Server, Event, play
from clausters.seq import pbind, pseq

Server().boot()               # -> clausters.default_session.server (first-wins)
play(Event(degree=0))         # one note now, no clock — resolves the default server
play(pbind(degree=pseq([0, 2, 4]), dur=0.5))   # on the default session's clock
```

```ts
import { Event, Server, play } from "clausters";
import { pbind, pseq } from "clausters/seq";

await new Server().boot();    // -> defaultSession.server (first-wins)
play(new Event({ degree: 0 }));   // one note now, no clock — resolves the default server
play(pbind({ degree: pseq([0, 2, 4]), dur: 0.5 }));   // on the default session's clock
```

The free-standing `play` plays anything against this ambient context — an `Event` (immediate outside a clock, timetagged inside one), an event `pbind`, or a `Routine` — resolving the server and clock for you. Each playable also has the same ambient `.play()`. A note or pattern played from *inside* a running session (its routine's clock) resolves *that* session instead — unless a {{alt `with` || `use`}} block has put another session in force — so isolation holds even for the ambient verb. An explicit `Session` never adopts the default: `Session.live()` and friends keep their server to themselves.

Resolution is one ladder, and it is worth knowing because it is what makes several sessions safe {{alt in one script || on one page}}:

- **the server**: the one you passed, else the session of the routine running right now (a clock names its session), else the session in force ({{alt a `with` block's, or the activated one || a `use()` block's, or the activated one}}), else the default session's;
- **the clock**: the one you passed, else the clock driving the running routine, else the session in force's, else the default session's — created at tempo 1.0 and started on first use, never at import.

## Kinds of session

You almost always build a session with one of the factories rather than the constructor. They differ only in *where the bytes go* — offline into a score, over the network to a separate server, or by function call to a server running inside this {{alt process || tab}} — and otherwise behave identically.

`Session.nrt()` is an **offline** (non-real-time) session. Its server accumulates a timetagged *score* instead of sending anything, and `render()` turns that score into samples through the renderer bundled with the package. No server process and no audio device are involved.

```python
from clausters import Session
from clausters.seq import pbind, pseq, pwhite

session = Session.nrt()
session.clock.set_tempo(2.0)
session.seed(1)   # this session's root seed reproduces its every random draw
session.play(pbind(
    degree=pseq([0, 2, 4, 7, 4, 2], repeats=2),
    dur=0.25,
    amp=pwhite(0.1, 0.2),
))
take = session.render(sample_rate=48000.0, channels=2)
print(f"{take.frames} frames; peak {max(take.peak):.3f}; seed {take.seed}")
```

```ts
import { Session } from "clausters";
import { pbind, pseq, pwhite } from "clausters/seq";

const session = await Session.nrt();
session.clock.setTempo(2.0);
session.seed(1);  // this session's root seed reproduces its every random draw
session.play(pbind({
    degree: pseq([0, 2, 4, 7, 4, 2], 2),
    dur: 0.25,
    amp: pwhite(0.1, 0.2),
}));
const take = await session.render({ sampleRate: 48000.0, channels: 2 });
console.log(`${take.frames} frames; peak ${Math.max(...take.peak).toFixed(3)}; seed ${take.seed}`);
```

Randomness (`pwhite`, `prand`, {{alt `clausters.rrand`/`choose`/… || `rrand`/`choose`/…}}) always draws
from a **single seedable context** — the running routine's generator, derived
from the context that created it — never from per-pattern seeds. Each session
is its **own** such context: `session.seed(n)` reproduces *that* session's
samples end to end, and two sessions (even both offline) reproduce
independently — seeding one never perturbs another. Outside any session,
{{alt `clausters.seed(n)` (equivalently `main.seed(n)`) || `seed(n)` (equivalently `main.seed(n)`)}} governs the **default session** (the free-standing draws and
anything played free-standing). See [Routines and clocks](routines-and-clocks.md).

<!-- python -->
`Session.live()` is a **real-time** session that sounds on a device over the network. By default it **ensures a server** the way `nrt()` ensures a renderer: if one already answers it attaches to it, and if none does it **launches a separate `clausters` process** — choosing a shared-memory segment for you — and connects to that. So the everyday live case is one line, whether or not a server is already up; a server the session started is stopped when the session is closed or the interpreter exits, and one it merely attached to is left alone.

The two protocols have two roles: **UDP finds the server, TCP talks to it**. The boot-or-attach probe rides UDP (discovery stays zero-config, and any scsynth-style tool can do the same), and the session's command interface then connects over **TCP by default** — reliable, ordered, and not bounded by the ~64 KB UDP datagram, so a large def, a whole GuiDef tree or a megabyte buffer read travels as one frame (the ceiling is the server's `--max-frame`, advertised in `/server_query`). Pass `transport="udp"` (or set `[client].transport` in the config) for a datagram-only setup — e.g. a server started with `--no-tcp` — or `transport="ws"` for a `--ws` server. Timing is unaffected either way: it rides on bundle timetags and `/sched_at`, never on arrival time.
<!-- /python -->
<!-- web -->
`Session.live()` is a **real-time** session on a server that is **already running**: a native `clausters --ws` server reached over its WebSocket (`Session.live(url)`, `ws://127.0.0.1:57120` when none is given), or, in a worker, the engine of the page that started it ([below](#a-session-in-a-worker)). It attaches, and attaching verifies: a WebSocket that connects proves a listener, not a server, so a carrier with nothing behind it throws a `ServerError` naming the address rather than every later command leaving without a trace. Reconciling comes with it: the handle's allocators match the server that is actually running rather than the page's defaults. A page starts no process, so `live` takes an address and not a launch.
<!-- /web -->

```python
from clausters import Session
from clausters.seq import pbind, pseq, pwhite

with Session.live(latency=0.1) as session:   # attaches, or boots one
    session.clock.set_tempo(2.0)
    session.play(pbind(
        instrument="default",
        degree=pseq([0, 2, 4, 7, 4, 2], repeats=2),
        dur=0.25,
        amp=pwhite(0.1, 0.2),
    ))
    session.run(3.5)   # advance the clock in real time, then stop
```

```ts
import { Session } from "clausters";
import { pbind, pseq, pwhite } from "clausters/seq";

const session = await Session.live(undefined, { latency: 0.1 });   // attaches
session.clock.setTempo(2.0);
session.play(pbind({
    instrument: "default",
    degree: pseq([0, 2, 4, 7, 4, 2], 2),
    dur: 0.25,
    amp: pwhite(0.1, 0.2),
}));
await session.run(3.5);    // advance the clock in real time, then stop
session.close();
```

Arguments worth knowing on `live()`:

<!-- python -->
- `boot` — whether to start a server when none is up (default `True`). Pass `boot=False` for plain attach-only behavior: connect to a server you launched yourself (possibly remote), never starting a process. When booting, `options` (a `ServerOptions` that sizes the launched server *and* this client's allocators), `shm` (`"auto"`, a path, or `None`), and `verbose`/`data_dir`/`server_args` shape the launched process.
<!-- /python -->
- `latency` — seconds added to each event's timetag so it arrives a touch ahead of its play time and the server sounds it *on* time rather than late. `0.0` means "as soon as possible"; a small value such as `0.1` is typical for a live take. Left unset<!-- python --> (and with no `[client].latency` in the config)<!-- /python -->, it defaults to `0.1` for a real-time carrier — {{alt UDP, TCP, WS **and** the embedded server || the WebSocket, a worker's link **and** the page's engine}}, which is wall-clock timetagged just the same — and to `0.0` for an offline NRT session. So `Session.live()` and `Session.embed()` land on time out of the box, without an explicit `latency`.
- `clock` — the session's clock. Left unset, the session makes one at tempo 1.0 on its timebase; the tempo is the clock's, so it is set there (`{{api session.clock.set_tempo}}(2.0)`). A clock that belongs to another session, or is on another timebase, is refused.
- `timebase` — the time every clock of the session is made on, fixed for the session's life. Left unset, it is the server's own sample clock<!-- python --> (config `[client].clock`, default `"sample"`)<!-- /python -->, for drift-free, sample-accurate scheduling, and a server that does not answer it **{{alt raises || throws}}**: a clock's timebase is fixed when it is made, so there is nothing to fall back to afterwards. Pass {{alt `MonotonicTimebase()` (or set `[client].clock = "monotonic"`) || `new MonotonicTimebase()`}} for wall-clock timetags. See [Timing models](timing-models.md) for the distinction.
<!-- web -->
- `share` — the slice of the server's id spaces this session allocates from, when another client works on the same server (see [Several clients on one server](#several-clients-on-one-server)).
<!-- /web -->

<!-- python -->
`Session.embed()` is a **real-time** session whose server runs *inside this process*. It opens the whole engine — audio device and all — through the native library bundled with the package, and OSC is delivered by function call rather than over a socket. There is no separate process to start and no port to connect to, yet it is real-time: it sounds on a device just like `live()`.
<!-- /python -->
<!-- web -->
`Session.embed()` is a **real-time** session whose server runs *inside this tab*: the engine compiled to wasm in an AudioWorklet, OSC delivered through the page's memory rather than over a socket. It **boots** its own engine — its own `AudioContext` and its own node, bus and buffer space — so the session's nodes share nothing with the rest of the document. The `AudioContext` needs a user gesture to start, so it is called from a click rather than at load.
<!-- /web -->

```python
from clausters import Session
from clausters.seq import pbind, pseq, pwhite

with Session.embed(latency=0.1) as session:
    session.clock.set_tempo(2.0)
    session.play(pbind(
        instrument="default",
        degree=pseq([0, 2, 4, 7, 4, 2], repeats=2),
        dur=0.25,
        amp=pwhite(0.1, 0.2),
    ))
    session.run(3.5)
```

```ts
import { Session } from "clausters";
import { pbind, pseq, pwhite } from "clausters/seq";

const session = await Session.embed({ latency: 0.1 });
session.clock.setTempo(2.0);
session.play(pbind({
    instrument: "default",
    degree: pseq([0, 2, 4, 7, 4, 2], 2),
    dur: 0.25,
    amp: pwhite(0.1, 0.2),
}));
await session.run(3.5);
session.close();
```

<!-- python -->
It takes the same `clock`, `latency` and `timebase` as `live()`, plus `workers` (engine threads for parallel node processing) and an optional `server=` to reuse an existing `clausters.ipc.Clausters` handle instead of opening a fresh one. You do not normally build that handle: `embedded()` opens and owns it, and hands it back as `session.server.interface.server`. Because the server lives in this process, that property reads its sample clock (`.clock`, `.sample_rate`) and control buses (`.ctl_get` / `.ctl_set`) directly, with no OSC round trip.

There is deliberately **no separate "spawn" factory**: launching a server is not a different kind of session, just `live()`'s default behavior, so the option lives on `live` rather than multiplying constructors. See [Launching the server and the GUI](#launching-the-server-and-the-gui) below for the details and the object-level `Server.boot` / `GuiHost.boot`.
<!-- /python -->
<!-- web -->
It takes the same `clock`, `latency`, `timebase` and `share` as `live()`, plus `channels` (the engine's output count: an `AudioContext`'s output width is fixed when it is created, so this is the only place to say it) and an optional `engine` to drive one already open instead of booting — the Python client's `server=`, and the same rule: an engine handed in is not this session's to close.
<!-- /web -->

Which factory to reach for:

<!-- python -->
| Factory | Server | Use it when |
| --- | --- | --- |
| `Session.nrt()` | none (a score + renderer) | rendering offline — a plot, an analysis, a `.wav`; no device. |
| `Session.embed()` | in-process (bundled library) | making sound from one script, no setup — but the engine shares this process. |
| `Session.live()` | a separate process (booted if needed) | the everyday real-time / live-coding case — a real, separate server a GUI or another client can also talk to. Boots one if none is up; `boot=False` attaches only. |
<!-- /python -->
<!-- web -->
| Factory | Server | Use it when |
| --- | --- | --- |
| `Session.nrt()` | none (a score + renderer) | rendering offline — a plot, an analysis, a `.wav`; no device, no gesture. |
| `Session.embed()` | this tab's engine (booted, from a gesture) | making sound from one page, no setup. |
| `Session.live(url)` | a native server over its WebSocket, or the page's engine from a worker | a real, separate server a native GUI or another client can also talk to. Attaches only. |
<!-- /web -->

## Driving a session

Once you have a session, a small set of methods drives it. Some are offline-only, some live-only — the table makes the split explicit.

| Call | Kind | What it does |
| --- | --- | --- |
| {{alt `play(pattern, quant=None)` || `play(pattern, quant?)`}} | all | Plays an event pattern (e.g. a `pbind`) on this session's clock and server. Returns the `EventStreamPlayer`. `quant` is the beat grid to start on; none starts immediately. |
| {{alt `render(sample_rate, channels, path=None, seed=None)` || `render({ sampleRate, channels, path, seed })`}} | offline | Drains the clock logically (no waiting), then renders the score. Returns a [`RenderStats`](verbs.md#what-a-render-gives-back): frame/channel/event counts, per-channel `peak` and `rms`, the `seed` it used, and `samples` ({{alt interleaved float32 in a stdlib `array('f')` || an interleaved `Float32Array`}}) unless `path` sent the audio to a file. |
| `run(seconds)` | real-time | Starts the clock, advances it in real time for `seconds`, then stops. {{alt Returns `self` || Resolves with the session}}. (`live` and `embed`.) |
| `start()` / `stop()` | real-time | Start or stop the real-time clock yourself when `run` (which does both) is not enough. Both return the session. |
| `panic()` | all | Stops everything the session plays and leaves it ready to play again: see [Stopping everything](#stopping-everything-panic). Returns the session. |
| `close()` | all | Closes the underlying `Server` and its {{alt interface (for `embed`, shuts the in-process server down) || connection (for `embed`, the engine it booted), its GUI host and its clocks}}. |

`play` is the one call shared by both kinds, and that is the whole point — see the next section.

<!-- python -->
Because `close()` releases the server, the idiomatic shape for a live session is a context manager, which closes it for you even if the block raises:

```python
with Session.live() as session:
    session.clock.set_tempo(2.0)
    session.play(my_pattern)
    session.run(4.0)
# server closed here
```

An offline session holds no socket and renders synchronously, so the context manager is optional there — though harmless, and tidy if you mix both.

One rule carries over from the rest of the client: a routine must never block the clock thread. `render()` and `run()` are *driver* calls you make from your own (main) thread, not from inside a routine — they advance the clock, so calling them from a routine the clock is running would deadlock it.
<!-- /python -->
<!-- web -->
`close()` releases what the session owns — an engine it booted for itself among them; the page's shared engine, or one handed in, is not a session's to stop. `render()` and `run()` are *driver* calls, awaited from the page's own code and never from inside a routine, which must not await.
<!-- /web -->

## Stopping everything: `panic`

`panic()` is the stop-everything key — sclang's `CmdPeriod`. On a session it stops whatever that session plays, in this order:

1. **Its clocks are emptied**, so no routine, pattern or curve wakes again. The clocks keep their beat and go on running.
2. **Its timelines are halted**, those on one of its clocks and those on a transport of its server.
3. **Its server is silenced** (`Server.panic`): every transport stopped, every timed bundle still queued dropped, every node freed. Freeing the nodes alone (`{{api free_all}}`) is not silence: a bundle already sent — the server `latency` ahead of every played event, a transport's plan — makes a node a moment later.
4. **Every OSC and MIDI responder not marked `permanent` is freed.**

Nothing is closed, so a `play` on the next line sounds. Two of those reach beyond the session, because they are not its own: the server is silenced whole, another client's sound included, and the responders belong to the program, not to a session. A responder that must outlive every panic says so — {{alt `OscFunc(func, "/fader", permanent=True)` || `new OscFunc(func, "/fader", { permanent: true })`}} — as one that keeps a controller mapped would. The `AppClock` is left alone: what runs on it draws, and is not played.

The free-standing `panic()` stops the **session in force**, the one an ambient `play` would use: {{alt a `with session:` block's || a `use` block's}}, the running routine's, the activated one, else the default session — whose clocks are its default clock and every clock made with no session ambient.

```python
from clausters import Session, panic, play
from clausters.defs import sine

session = Session.live()
with session:
    play(sine(880) * 0.1)
    panic()                 # the session in force: this one
session.panic()             # the same, naming it
```

```ts
import { Session, panic, play } from "clausters";
import { sine } from "clausters/defs";

const session = await Session.live();
session.use(() => {
    void play(sine(880).mul(0.1));
    panic();                // the session in force: this one
});
session.panic();            // the same, naming it
```

In clausters-editor, `Ctrl+.` interrupts the code that is running and then panics every open session and the default one, so the key that stops the code stops the sound too.

## The same code, live or offline

The reason a session draws no line between "play" and "render" is the client's central design property, the *seam*: a `Server` holds one {{alt communication interface || connection}}, and which one it holds — not your pattern, not your clock — decides where the bytes go. A live session's server holds a network {{alt interface (TCP by default) || connection (a WebSocket, or the page's engine)}}; an offline session's server holds a score-accumulating one. Everything above the server is identical.

So the only thing that changes between a live take and an offline render is **which factory you called**. You can write the pattern once and run it both ways:

```python
from clausters import Session
from clausters.seq import pbind, pseq, pwhite

def phrase():
    return pbind(
        instrument="default",
        degree=pseq([0, 2, 4, 7, 4, 2], repeats=2),
        dur=0.25,
        amp=pwhite(0.1, 0.2),
    )

# Offline: capture it to samples.
offline = Session.nrt()
offline.clock.set_tempo(2.0)
offline.play(phrase())
take = offline.render()

# Live: hear the very same phrase.
with Session.live(latency=0.1) as live:
    live.clock.set_tempo(2.0)
    live.play(phrase())
    live.run(3.5)
```

```ts
import { Session } from "clausters";
import { pbind, pseq, pwhite } from "clausters/seq";

function phrase() {
    return pbind({
        instrument: "default",
        degree: pseq([0, 2, 4, 7, 4, 2], 2),
        dur: 0.25,
        amp: pwhite(0.1, 0.2),
    });
}

// Offline: capture it to samples.
const offline = await Session.nrt();
offline.clock.setTempo(2.0);
offline.play(phrase());
const take = await offline.render();

// Live: hear the very same phrase.
const live = await Session.live(undefined, { latency: 0.1 });
live.clock.setTempo(2.0);
live.play(phrase());
await live.run(3.5);
live.close();
```

<!-- python -->This is exactly what the two shipped examples do — `offline_render.py` and `live_udp.py` share their pattern and differ only in the session factory. See [Examples](examples.md).<!-- /python -->

## Several sessions at once

Because a session is an ordinary object rather than a global, more than one can be live at the same time. The common case is rendering a score offline (for a plot, an analysis or a `.wav`) right next to a live session you are listening to, in a single {{alt script || page}}:

```python
live = Session.live(latency=0.1)
live.clock.set_tempo(2.0)
plot = Session.nrt()
plot.clock.set_tempo(2.0)

live.play(phrase())
plot.play(phrase())

live.run(2.0)                       # heard in real time
take = plot.render()                # captured offline, no audio device
live.close()
```

```ts
const live = await Session.live(undefined, { latency: 0.1 });
live.clock.setTempo(2.0);
const plot = await Session.nrt();
plot.clock.setTempo(2.0);

live.play(phrase());
plot.play(phrase());

await live.run(2.0);                // heard in real time
const take = await plot.render();   // captured offline, no audio device
live.close();
```

The two never interfere: each has its own server, its own clock and its own {{alt interface || connection}}. With globals this is impossible; here it is the default.

### Ambient for good: `activate`

{{alt `with session:` || `session.use(() => ...)`}} makes a session ambient for a block, which is the right shape when the session's life *is* the block's. {{alt A REPL || A page}} has no block to be inside of — {{alt each statement runs on its own || a block cannot span an event handler, a timer or an `await`}} — so `session.activate()` makes it ambient and leaves it there, and `session.deactivate()` gives the slot up (closing the session does too). After it, everything that names no session (`play(...)`, a bare {{alt `Synth(...)` || `new Synth(...)`}}) resolves to this one's server, clock and random root. A `play()` that resolves the clock of an activated session nobody started starts it, so it sounds; one stopped on purpose (`stop()`) stays stopped, and the play says once that it is queued until the session starts again.

<!-- python -->
Both are thread-local, like the ambient session itself: another thread is unaffected, and a `with` block nests inside an activated session rather than replacing it — it restores what was in force when it ends.
<!-- /python -->
<!-- web -->
A `use` block nests inside an activated session rather than replacing it — it restores what was in force when it ends — and it is synchronous by design: an `await` inside it would let another task run while this session is ambient, and the page's one thread has no way to scope that. Await outside, create inside:

```ts
b.use(() => new Synth("beep"));   // reaches b, whatever the page's default is
```
<!-- /web -->

### The session is the context clocks are made in

A session is global **per context**: whichever one is in force is where a clock is made. A `TempoClock` made while a session is active is made on **the session's timebase** — the server's sample clock for a live or embedded session, a `LogicalTimebase` for an offline one — and kept in the session, which starts, stops and closes it with the rest. With no session active, a clock belongs to the default session and paces on wall-clock time, unless it is given a timebase of its own.

The {{alt `with` || `use`}} block is how a session is switched, {{alt on the same thread and even || even}} inside a running routine, so an offline render in the middle of a live one needs nothing but the block:

```python
live = Session.live().activate()

with Session.nrt() as offline:          # in force for the block, closed after it
    offline.play(pbind(degree=pseq([0, 2, 4]), dur=0.25))
    take = offline.render()             # every clock made here is on logical time
```

```ts
const live = (await Session.live()).activate();

const offline = await Session.nrt();
offline.use(() => {                     // in force for the block
    offline.play(pbind({ degree: pseq([0, 2, 4]), dur: 0.25 }));
});
const take = await offline.render();    // every clock made in it is on logical time
offline.close();
```

An offline session has only logical time: a clock or a `timebase` of another kind given to one {{alt raises || throws}}, and so does rendering a clock that is not on it. A timeline's hidden clock follows the same rule — it belongs to the session the timeline sounds in (see [Timelines](timelines.md)).

## Launching the server and the GUI

<!-- python -->
Live coding wants the whole system reachable from one interpreter: a separate audio server (so it survives a client restart, is shared, and keeps the audio thread out of Python) and, often, the visual server beside it — without opening three terminals or spelling out a shared-memory path. `Session.live` and `Session.gui` do exactly that, and everything they start is torn down when the session is closed **or the interpreter exits** — a normal exit, an unhandled exception, or an abandoned handle garbage-collected. Nothing is left running.

`session.gui()` launches the `clausters-gui` visual server and returns a [`GuiHost`](api/clausters.gui.host.md#clausters.gui.host.GuiHost) connected to it — the GUI parallel of `live()` booting a server. You never spell out an address or a segment: the host is started with its client leg pointed at this session's server and mapping the same shared-memory segment the server was booted with, so meters, scopes and playheads read the engine with no per-frame messages. The host is owned by the session and stopped on `close`.
<!-- /python -->
<!-- web -->
A page wants the same as a script: a server to play on and the visual host beside it, wired to each other, in a couple of lines. `session.gui()` opens the session's GUI host once — the host compiled to wasm in this tab, drawing on canvases of the page — and wires it to **this session's** engine, so a bound widget reaches this server and not the page's, and meters, scopes and playheads read the engine with no per-frame messages. The host is owned by the session and closed with it.
<!-- /web -->

```python
from clausters import Session
from clausters.gui import view, label

session = Session.live()       # attaches, or boots a server (segment auto-chosen)
gui = session.gui()            # clausters-gui, wired to that server + same segment

win = view(label("hello", name="greeting"), title="Panel", w=320, h=120).open()
win["greeting"].set(text="edited live")  # edit the widget by name
win.close()                              # close the window

session.close()                          # stops whatever the session started; leaving the interpreter would too
```

```ts
import { Session } from "clausters";
import { label, view } from "clausters/gui";

const session = await Session.embed();   // this tab's engine, from a gesture
const host = await session.gui();        // the GUI host, wired to that engine

const win = await view({ title: "Panel", w: 320, h: 120 },
                           label("hello", { name: "greeting" })).open();
win.widget("greeting").set({ text: "edited live" });   // edit the widget by name
win.close();                                           // close the window

session.close();                         // closes what the session opened
```

A view opens itself on the session's host: `session.gui()` registers it as the **ambient** host (first-wins, as the default session adopts the first free-standing {{alt `Server.boot()` || `Server` booted}}), so `open()` needs no {{alt argument || host}}, and {{alt `host=` || `host`}} says which when there is more than one. `View.open` sends a `window`-rooted GuiDef and returns a **window handle** (it *is* the window id and also resolves the tree's named widgets), `set` edits a live widget, and `close` closes it (`{{api close_all}}` closes every window still open). You need not pick ids at all: name a widget and address it through the handle ({{alt `win["greeting"]` || `win.widget("greeting")`}}). The ids ride only in the document that goes out — the tree you wrote is left as you wrote it, which is what lets one view open as many times as you like, each window with ids of its own. Repeated `session.gui()` calls return the same host.<!-- web --> `session.guiHost` reads back what the session has without opening anything.<!-- /web -->

<!-- python -->
The visual server binary ships **bundled in the same package** as the audio server (built from the `crates/clausters-gui` crate, stripped), so there is nothing extra to install — the launcher finds it out of the box. In a source checkout a binary built under `target` is used, and `CLAUSTERS_GUI_BIN` overrides the lookup. See [Getting started](getting-started.md#the-visual-server-gui) for building a lighter, server-only install.
<!-- /python -->
<!-- web -->
The host ships in the same package as the engine, as wasm. A session drives a native `clausters-gui --ws` host instead by being **given** one — `new Session(server, undefined, await new GuiHost({ transport: "ws", url }).attach())` — the way it is given a `Server` it did not open (below).
<!-- /web -->

### Without a Session: `boot` and `attach`

If you are not using a `Session`, the server and the GUI host each carry their own launch and teardown, so you don't juggle a separate {{alt process || engine}} object. {{alt `Server().boot()` starts a server process and returns a connected `Server` that owns it (its `close()` stops the process); `GuiHost().boot()` does the same for the visual server, returning a started `GuiHost` (its `stop()` stops the process). Both also die with the interpreter. || `await new Server().boot()` brings up an engine of the page's and returns a connected `Server` that owns it (its `close()` stops the engine); `await new GuiHost().boot()` does the same for the visual host (its `stop()` closes it).}}

```python
from clausters.defs import Server
from clausters.gui import GuiHost, view, label

server = Server().boot()                                  # a server process starts
gui = GuiHost().boot(server=f"{server.target.host}:{server.target.port}", shm=server.shm)

view(label("hi", name="greeting"), title="Panel", w=320, h=120).open()
# ...
gui.stop()        # stops the clausters-gui process
server.close()    # stops the server process
```

```ts
import { Server } from "clausters/defs";
import { GuiHost, label, view } from "clausters/gui";

const server = await new Server().boot();                 // an engine starts, from a gesture
const host = await new GuiHost({ engine: server.engine }).boot();

await view({ title: "Panel", w: 320, h: 120 }, label("hi", { name: "greeting" })).open();
// ...
host.stop();      // closes the GUI host
server.close();   // stops the engine
```

This is exactly what {{alt `Session.live`/`gui` || `Session.embed`/`gui`}} use internally — the session just bundles them with a clock.

### Several servers, and the one you did not start

<!-- python -->
A handle keeps the address it was built with, and the process it boots is told to bind it, so **one machine runs as many servers as you give ports**:

```python
a = Server().boot()              # 57110, the default
b = Server(port=57130).boot()    # a second, side by side
```

Each gets its own shared-memory segment, and they may share one def store — the store is written concurrently on purpose, so a def sent to one is on disk for the next. Only the audio device has to cooperate: PipeWire and CoreAudio mix several streams, an exclusive ALSA device does not, and there the second boot fails saying so.
<!-- /python -->
<!-- web -->
Each `boot()` brings up an engine the handle owns — its own `AudioContext`, its own node, bus and buffer space — so **one page runs as many servers as it boots**:

```ts
const a = await new Server().boot();     // its own engine
const b = await new Server().boot();     // a second, sharing nothing
```

An engine is an `AudioContext` and browsers cap those (Chrome at six), so a page holds fewer servers than a machine does, and the seventh boot fails saying so. A second handle on an engine one of them booted is pointed at it by `engine`, which is a server's address in a page the way a port is on a machine: `await new Server({ engine: a.engine }).attach()`.

The GUI host has the same pair: `newGuiHost()` boots an instance — its own engine unless you hand it one — where `guiHost()` returns the page's, and `newPools()` gives the independent client an id space of its own beside it. Instances share the tab and nothing else, so two of them may use the very same window, widget and node ids without colliding; `gui.close()` releases one. `examples/panels/two-hosts.html` shows the whole arrangement.
<!-- /web -->

`boot()` is for a server that is **not there yet**.<!-- python --> If something already answers on the port it raises instead of adopting it, because a handle that did not start a process must not stop one either.<!-- /python --><!-- web --> In a worker whose page left it its links, the engine is the page's and already running, so `boot()` refuses and names `attach()`.<!-- /web --> The other half is **`attach()`**, for a server already running — {{alt one launched from a terminal, one belonging to another process, or one left behind by a client that crashed while it was sounding || the page's own engine, one at the far end of a socket, or one a script left sounding on a native server}}:

```python
server = Server(port=57130).attach()   # raises if nobody answers there
server.panic()                         # cut the sound, keep the server
server.quit()                          # or stop it altogether
```

```ts
const server = await new Server({ transport: "ws", url: "ws://127.0.0.1:57130" }).attach();   // throws if nobody answers there
server.panic();                        // cut the sound, keep the server
await server.quit();                   // or stop it altogether
```

Ownership is what separates them, all the way down. An attached handle did not start the server, so `close()` releases the connection and leaves the server standing; stopping it is `quit()`, which the server obeys over the wire. Unlike a bare {{alt `Server(...)` || `new Server(...)`}}, `attach()` verifies: a handle pointing where nobody answers {{alt raises || throws}} there and then, rather than dropping every later message into a void that reports nothing back. It also **reconciles** — a server this client did not launch may have been booted with other flags, so the handle re-reads the real capacities ({{alt `query_info` || `queryInfo`}}) and resizes its allocators to them instead of trusting its own {{alt `options` || defaults}}.

The GUI host has the same pair, and ownership means the same thing there. **`GuiHost.attach()`** connects to a host already running — {{alt one left behind by a script that ended, one launched from a terminal, one another process owns || the page's own, or a native `clausters-gui --ws`}} — and `stop()` then closes the connection and leaves the host standing, windows and all:

```python
from clausters.gui import GuiHost, view, label

gui = GuiHost().attach()               # raises if no host answers there
view(label("from a second script"), title="Panel").open(host=gui)
gui.stop()                             # the host keeps running
```

```ts
import { GuiHost, label, view } from "clausters/gui";

const host = await new GuiHost({ transport: "ws", url: "ws://127.0.0.1:57140" }).attach();   // throws if no host answers there
await view({ title: "Panel" }, label("from a second page")).open(undefined, { host });
host.stop();                           // the host keeps running
```

<!-- python -->
It verifies like the server's, over UDP — the one carrier a host always listens on, so the probe is valid whichever one you then talk over. There is no reconcile step: a host publishes no capacities to resize a handle from. What it does share is the widget-id namespace, so **two handles on one host want `share=`** (the same arithmetic as two clients on one server), or both start naming widgets at 1000.
<!-- /python -->

A session takes a host it did not start the same way it takes a server: through the constructor, not a verb of its own.

```python
session = Session(Server(port=57130).attach(), gui=GuiHost().attach())
session.gui()                          # that host; nothing is launched
```

```ts
const session = new Session(server, undefined, host);
await session.gui();                   // that host; nothing is opened
```

<!-- python -->
From a terminal, the same three verbs need no script (see [the command line](cli.md)):

```sh
clausters status --port 57130   # is anyone there, and how is it configured
clausters panic --port 57130    # silence it; the server stays up
clausters stop  --port 57130    # /server_quit
```

That is the answer to the stray server: a crashed client leaves a process holding the audio device, quite possibly still playing, and `kill` should not be the only way to end it.

### The raw processes

One level lower, `clausters.launch` exposes the processes themselves — `ServerProcess` and `GuiProcess` — for when you want to own them directly (e.g. a custom `Server`/`GuiHost` wiring). They are named through their module, not re-exported at the top level: the ordinary way to reach one is to let `live()` or `Server.boot` own it and read its choices back off the `Server` (`server.shm` is the segment it picked). Both are context managers, both register the same exit hooks, and `default_shm_path()` is what `shm="auto"` resolves to (`None` on platforms where shared memory does not apply); `server_is_up()` is the probe `live` uses to decide boot-or-attach.

```python
from clausters.launch import ServerProcess, GuiProcess
from clausters.gui import GuiHost

with ServerProcess() as server_proc:            # clausters --shm <auto>
    with GuiProcess(server=f"{server_proc.host}:{server_proc.port}",
                    shm=server_proc.shm) as gui_proc:
        host = GuiHost(port=gui_proc.port).start()
        ...   # both processes stop when the `with` blocks exit
```
<!-- /python -->

### Several clients on one server

Two clients on the **same** server — two scripts, a script and a GUI host, a page and the worker it started — allocate ids independently, so both allocators would start at the same base. They split the space instead, each taking one slice of it: {{alt `Server(share=IdShare(index=1, of=2))`, and the same `share=` on the `GuiHost` || `Session.embed({ share: { index: 1, of: 2 } })`}}, with the other client given index 0. The slices are equal and in a fixed order, so they are disjoint by arithmetic and the two clients need no channel to agree.<!-- web --> A session's share governs both of its legs, the server's node, bus and buffer ids and its host's widget ids alike.<!-- /web -->

<!-- web -->
## A session in a worker

A page can run the code that plays in a **worker** and keep the engine itself. The page boots the engine, as always from a gesture, and makes a **link** to it for the worker: a client tag of its own and a port that goes to the engine directly. In the worker, that port is the engine's address:

```js
// the page
import { server } from "clausters/page";

const engine = await server();
await engine.resume();
const worker = new Worker("./player.js", { type: "module" });
const link = engine.link();
worker.postMessage({ port: link.port }, [link.port]);
```

```js
// player.js, the worker
import { Session, play } from "clausters";
import { pbind, pseq } from "clausters/seq";
self.onmessage = async ({ data }) => {
    const s = (await Session.live(data.port)).activate();
    s.clock.start();
    play(pbind({ degree: pseq([0, 2, 4], Infinity), dur: 0.25 }));
};
```

The worker imports from `clausters` as the page does; the package's elements are registered only where there is a document, and a worker has none.

The page's **GUI host** is reached the same way, with a link of its own, and `session.gui({ port })` attaches to it:

```js
// the page
import { guiHost } from "clausters/page";

const host = await guiHost();
const guiLink = host.link({ place: (window) => document.getElementById("panel") });
worker.postMessage({ engine: link.port, gui: guiLink.port }, [link.port, guiLink.port]);
```

```js
// the worker
import { Session, gui } from "clausters";

const s = await Session.live(data.engine);
await s.gui({ port: data.gui });
await gui.view({ title: "level" }, gui.slider({ name: "level" })).open();
```

The page keeps the drawing. A window the worker opens gets a canvas in the element `place` answers for its id, or, when it answers nothing, a canvas of its own appended to the document, as a view opened with no element gets. The worker's widgets report back to the worker, a widget it binds drives the engine the host is wired to, and `plot` and `scope` from the worker open their windows on that host. `guiLink.close()` takes the worker's windows off the page.

A page that also plays or names widgets of its own shares the id spaces with the worker as two clients of one server do: each takes a slice (`Session.live(port, { share: { index: 1, of: 2 } })` in the worker, the other slice on the page).

What this gives a page:

- **Code that can be stopped.** `worker.terminate()` ends the worker whatever it is doing, an infinite loop included, and the engine and the page go on. What the worker left sounding stops from the page, with a handle of its own on the engine (`new Server({ engine }).attach()`, then `panic()`). `link.close()` lets the worker's client go; a new worker gets a new link onto the same engine.
- **A clock that does not compete with drawing.** The worker's routines and patterns run off the page's thread, and what they send reaches the engine without passing through it, so a heavy layout or the GUI host's frames do not delay them.

A program that starts a worker for code of someone else's — clausters-editor's page does — can leave the links where the client looks for a worker's default address, `globalThis[Symbol.for("clausters.links")] = { engine, gui }`, before that code runs. Then the code names no port, as a script names no server: `Session.live()`, `session.gui()` and `await new Server().attach()` reach the page's engine and GUI host, and `new Server().boot()` refuses, since that engine is the page's and already running. Each connection opened over a link is a client of its own.

The session attaches, as `live` does over a socket: the engine is the page's, so the worker cannot `boot` it, and `/quit` from the worker stops it for everyone, as it stops any server. It shares no clock with the engine, so its timebase is anchored over `/clock_query`, as over a socket. When the engine closes, the worker's carrier reports it `gone`.
<!-- /web -->

## Plotting and scoping

`plot` and `scope` are the visual siblings of the free-standing `play`, and they resolve the session's GUI host as everything else resolves its server: {{alt the session's host if one is up, else a host it boots lazily (with no client leg: plot data reaches the host as a mapped file, so no audio server is involved unless the object itself needs one) || the session's host if one is up, else the page's}}. [The ambient verbs](verbs.md) has what each one accepts and the options of each view; two things are the session's:

- **`plot` needs no server** unless the object itself does: a def or an expression is rendered by an ephemeral offline session, an `Env` through the engine's own `EnvGen`, a sequence read as it is. Only a `Buffer` is fetched from the ambient live server.
- **`scope` needs a live server whose buses the host can read**<!-- python --> — one with a shared-memory segment, which `Server.boot` and `Session.live` create by default. To scope a server you merely *attached* to, pass `host=` pointed at a `GuiHost` booted with that server's segment path<!-- /python --><!-- web --> — the page's engine, or the one a host was given with `engine`<!-- /web -->. The server has a finite number of sample rings to record into (`--taps`, 8 by default): a stereo scope holds two while open, so close scopes you are done with. Two scopes on one bus share a ring, and one that cannot get a ring fails loudly instead of drawing nothing.

## When you don't need a Session

A `Session` is sugar over two objects you can always build yourself. When you want more control — several servers behind one clock, a clock shared across subsystems, or a custom {{alt interface || connection}} — skip the factory and wire them directly:

```python
from clausters.base import TempoClock
from clausters.defs import Server

server = Server("127.0.0.1", 57110, latency=0.1)
clock = TempoClock(tempo=2.0)

phrase().play(clock, server)        # what Session.play does for you
clock.run(3.5)
server.close()
```

```ts
import { TempoClock } from "clausters/base";
import { Server } from "clausters/defs";

const server = await new Server({ transport: "ws", url: "ws://127.0.0.1:57120", latency: 0.1 }).attach();
const clock = new TempoClock(2.0);

phrase().play(clock, server);   // what Session.play does for you
await clock.run(3.5);
server.close();
```

`Session` adds no behaviour of its own — it only bundles these two and forwards to them — so reaching for the longer form costs nothing and loses nothing.

## See also

- {{alt [API reference](api/clausters.session.md#clausters.session.Session) || [API reference](api/index.md)}} — the generated reference for `Session` and every method.
- [The client, layer by layer](guide.md) — where the `Server`, the clock and the seam fit in the whole client.
- [Routines and clocks](routines-and-clocks.md) — the level below a session: driving a `Routine`, a `TempoClock` and a `Server` yourself.
- [Timing models](timing-models.md) — the ways a clock keeps time (wall-clock, sample-locked, shared transport) and how to observe each.
- [Examples](examples.md) — the session in runnable form.
