# Sessions

A `Session` is the client's ergonomic entry point: one object that owns a `Server` and a `TempoClock` together and drives them as a unit. It exists so that the everyday case — "set up a place to make sound, play a pattern, hear it or render it" — is a couple of lines, without giving up the design that makes this client flexible.

## Why it exists

SuperCollider's sclang is convenient largely because of globals: a default `Server`, a default clock, an implicit "current environment". You type `Synth("x")` and it just goes somewhere. That convenience has a cost — there is only ever *one* of each, so you cannot, say, run a live take and an offline render in the same script without them fighting over the same global state.

This client keeps the convenience but contains it. There is one ambient environment — the **default session** — and everything else is an explicit `Session` you name. The rule is one line: *what does not run in an explicit session runs in the default session*. So the one-liner works (a booted server, a bare `Event().play()`), and yet you can spin up several isolated sessions — a live take next to an offline render — that never touch the default one or each other.

`Session` is that explicit, isolated environment: an object that holds a `Server` and one or more `TempoClock`s and offers `play` / `render` / `run`, plus factories that pick sensible defaults. Because it is a plain object and not a forced global, you can have as many as you like, each with its own server, clocks and random context.

The default session and a named `Session` are literally *the same kind of thing* — both extend one `Environment` base (a server plus a random context), so `clausters.default_session` **is** a session, the one used when you name none. Everything below (`play`, seeding, isolation) reads the same whether it runs in the default session or an explicit one.

## The default session

`clausters.default_session` (the `main` singleton) is the environment used whenever you did not name a session. It holds the ambient defaults: the default **server**, an opt-in default **clock** (created and started on first use), and the random context (`main.seed`). Booting a server free-standing adopts it there, first-wins:

```python
from clausters import Server, Event, play
from clausters.seq import Pbind, Pseq

Server.boot()                 # -> clausters.default_session.server (first-wins)
play(Event(degree=0))         # one note now, no clock — resolves the default server
play(Pbind(degree=Pseq([0, 2, 4]), dur=0.5))   # on the default session's clock
```

The free-standing `play` plays anything against this ambient context — an `Event` (immediate outside a clock, timetagged inside one), an event `Pbind`, or a `Routine` — resolving the server and clock for you. Each playable also has the same ambient `.play()`. A note or pattern played from *inside* a running session (its routine's clock) resolves *that* session instead, so isolation holds even for the ambient verb. An explicit `Session` never adopts the default: `Session.live()` and friends keep their server to themselves.

## Kinds of session

You almost always build a session with one of the factories rather than the constructor. They differ only in *where the bytes go* — offline into a score, over the network to a separate server, or by function call to a server running inside this process — and otherwise behave identically.

`Session.nrt()` is an **offline** (non-real-time) session. Its server accumulates a timetagged *score* instead of sending anything, and `render()` turns that score into samples through the renderer bundled with the package. No server process and no audio device are involved.

```python
from clausters import Session
from clausters.seq import Pbind, Pseq, Pwhite

session = Session.nrt(tempo=2.0)
session.seed(1)   # this session's root seed reproduces its every random draw
session.play(Pbind(
    degree=Pseq([0, 2, 4, 7, 4, 2], repeats=2),
    dur=0.25,
    amp=Pwhite(0.1, 0.2),
))
samples, frames = session.render(sample_rate=48000.0, channels=2)
print(f"{frames} frames; peak {max(abs(s) for s in samples):.3f}")
```

Randomness (`Pwhite`, `Prand`, `clausters.uniform`/`choice`/…) always draws
from a **single seedable context** — the running routine's generator, derived
from the context that created it — never from per-pattern seeds. Each session
is its **own** such context: `session.seed(n)` reproduces *that* session's
material end to end, and two sessions (even both offline) reproduce
independently — seeding one never perturbs another. Outside any session,
`main.seed(n)` governs the **default session** (`clausters.uniform`/… and
anything played free-standing). See [Routines and clocks](routines-and-clocks.md).

`Session.live()` is a **real-time** session that sounds on a device over the network. By default it **ensures a server** the way `nrt()` ensures a renderer: if one already answers it attaches to it, and if none does it **launches a separate `clausters` process** — choosing a shared-memory segment for you — and connects to that. So the everyday live case is one line, whether or not a server is already up; a server the session started is stopped when the session is closed or the interpreter exits, and one it merely attached to is left alone.

The two protocols have two roles: **UDP finds the server, TCP talks to it**. The boot-or-attach probe rides UDP (discovery stays zero-config, and any scsynth-style tool can do the same), and the session's command interface then connects over **TCP by default** — reliable, ordered, and not bounded by the ~64 KB UDP datagram, so a large def, a whole GuiDef tree or a megabyte buffer read travels as one frame (the ceiling is the server's `--max-frame`, advertised in `/server_info`). Pass `transport="udp"` (or set `[client].transport` in the config) for a datagram-only setup — e.g. a server started with `--no-tcp` — or `transport="ws"` for a `--ws` server. Timing is unaffected either way: it rides on bundle timetags and `/sched`, never on arrival time.

```python
from clausters import Session
from clausters.seq import Pbind, Pseq, Pwhite

with Session.live(tempo=2.0, latency=0.1) as session:   # attaches, or boots one
    session.play(Pbind(
        instrument="default",
        degree=Pseq([0, 2, 4, 7, 4, 2], repeats=2),
        dur=0.25,
        amp=Pwhite(0.1, 0.2),
    ))
    session.run(3.5)   # advance the clock in real time, then stop
```

Arguments worth knowing on `live()`:

- `boot` — whether to start a server when none is up (default `True`). Pass `boot=False` for plain attach-only behavior: connect to a server you launched yourself (possibly remote), never starting a process. When booting, `options` (a `ServerOptions` that sizes the launched server *and* this client's allocators), `shm` (`"auto"`, a path, or `None`), and `verbose`/`data_dir`/`server_args` shape the launched process.
- `latency` — seconds added to each event's timetag so it arrives a touch ahead of its play time and the server sounds it *on* time rather than late. `0.0` means "as soon as possible"; a small value such as `0.1` is typical for a live take. Left unset (and with no `[client].latency` in the config), it defaults to `0.1` for a real-time transport — UDP, TCP, WS **and** the embedded server, which is wall-clock timetagged just the same — and to `0.0` for an offline NRT session. So `Session.live()` and `Session.embed()` land on time out of the box, without an explicit `latency=`.
- `timebase` — the clock's pacing source. Left unset, the session anchors to the server's own sample clock by default (config `[client].clock`, default `"sample"`), for drift-free, sample-accurate scheduling, falling back to wall-clock time if no server answers; pass `MonotonicTimebase()` (or set `[client].clock = "monotonic"`) to keep wall-clock timetags. See [Timing models](timing-models.md) for the distinction.

`Session.embed()` is a **real-time** session whose server runs *inside this process*. It opens the whole engine — audio device and all — through the native library bundled with the package, and OSC is delivered by function call rather than over a socket. There is no separate process to start and no port to connect to, yet it is real-time: it sounds on a device just like `live()`.

```python
from clausters import Session
from clausters.seq import Pbind, Pseq, Pwhite

with Session.embed(tempo=2.0, latency=0.1) as session:
    session.play(Pbind(
        instrument="default",
        degree=Pseq([0, 2, 4, 7, 4, 2], repeats=2),
        dur=0.25,
        amp=Pwhite(0.1, 0.2),
    ))
    session.run(3.5)
```

It takes the same `latency` and `timebase` as `live()`, plus `workers` (engine threads for parallel node processing) and an optional `server=` to reuse an existing `clausters.Clausters` handle instead of opening a fresh one. Because the server lives in this process, you can read its sample clock and control buses directly through `session.server.interface.server` (the `Clausters` handle), with no OSC round trip.

There is deliberately **no separate "spawn" factory**: launching a server is not a different kind of session, just `live()`'s default behavior, so the option lives on `live` rather than multiplying constructors. See [Launching the server and the GUI](#launching-the-server-and-the-gui) below for the details and the object-level `Server.boot` / `GuiHost.boot`.

Which factory to reach for:

| Factory | Server | Use it when |
| --- | --- | --- |
| `Session.nrt()` | none (a score + renderer) | rendering offline — a plot, an analysis, a `.wav`; no device. |
| `Session.embed()` | in-process (bundled library) | making sound from one script, no setup — but the engine shares this process. |
| `Session.live()` | a separate process (booted if needed) | the everyday real-time / live-coding case — a real, separate server a GUI or another client can also talk to. Boots one if none is up; `boot=False` attaches only. |

## Driving a session

Once you have a session, a small set of methods drives it. Some are offline-only, some live-only — the table makes the split explicit.

| Call | Kind | What it does |
| --- | --- | --- |
| `play(pattern, quant=None)` | all | Plays an event pattern (e.g. a `Pbind`) on this session's clock and server. Returns the `EventStreamPlayer`. `quant` is the beat grid to start on; `None` starts immediately. |
| `render(sample_rate, channels)` | offline | Drains the clock logically (no waiting), then renders the score. Returns `(samples, frames)` — interleaved float32 in a stdlib `array('f')`, and the frame count. |
| `run(seconds)` | real-time | Starts the clock, advances it in real time for `seconds`, then stops. Returns `self`. (`live` and `embed`.) |
| `start()` / `stop()` | real-time | Start or stop the real-time clock yourself when `run` (which does both) is not enough. Both return `self`. |
| `close()` | all | Closes the underlying `Server` and its interface (for `embed`, shuts the in-process server down). |

`play` is the one call shared by both kinds, and that is the whole point — see the next section.

Because `close()` releases the server, the idiomatic shape for a live session is a context manager, which closes it for you even if the block raises:

```python
with Session.live(tempo=2.0) as session:
    session.play(my_pattern)
    session.run(4.0)
# server closed here
```

An offline session holds no socket and renders synchronously, so the context manager is optional there — though harmless, and tidy if you mix both.

One rule carries over from the rest of the client: a routine must never block the clock thread. `render()` and `run()` are *driver* calls you make from your own (main) thread, not from inside a routine — they advance the clock, so calling them from a routine the clock is running would deadlock it.

## The same code, live or offline

The reason a session draws no line between "play" and "render" is the client's central design property, the *seam*: a `Server` holds one communication interface, and which interface it holds — not your pattern, not your clock — decides where the bytes go. A live session's server holds a network interface (TCP by default); an offline session's server holds a score-accumulating one. Everything above the server is identical.

So the only thing that changes between a live take and an offline render is **which factory you called**. You can write the pattern once and run it both ways:

```python
from clausters import Session
from clausters.seq import Pbind, Pseq, Pwhite

def phrase():
    return Pbind(
        instrument="default",
        degree=Pseq([0, 2, 4, 7, 4, 2], repeats=2),
        dur=0.25,
        amp=Pwhite(0.1, 0.2),
    )

# Offline: capture it to samples.
offline = Session.nrt(tempo=2.0)
offline.play(phrase())
samples, frames = offline.render()

# Live: hear the very same phrase.
with Session.live(tempo=2.0, latency=0.1) as live:
    live.play(phrase())
    live.run(3.5)
```

This is exactly what the two shipped examples do — `offline_render.py` and `live_udp.py` share their pattern and differ only in the session factory. See [Examples](examples.md).

## Several sessions at once

Because a session is an ordinary object rather than a global, more than one can be live at the same time. The common case is rendering a score offline (for a plot, an analysis or a `.wav`) right next to a live session you are listening to, in a single script:

```python
live = Session.live(tempo=2.0, latency=0.1)
plot = Session.nrt(tempo=2.0)

live.play(phrase())
plot.play(phrase())

live.run(2.0)                       # heard in real time
samples, frames = plot.render()     # captured offline, no audio device
live.close()
```

The two never interfere: each has its own server, its own clock and its own interface. With globals this is impossible; here it is the default.

## Launching the server and the GUI

Live coding wants the whole system reachable from one interpreter: a separate audio server (so it survives a client restart, is shared, and keeps the audio thread out of Python) and, often, the visual server beside it — without opening three terminals or spelling out a shared-memory path. `Session.live` and `Session.gui` do exactly that, and everything they start is torn down when the session is closed **or the interpreter exits** — a normal exit, an unhandled exception, or an abandoned handle garbage-collected. Nothing is left running.

`session.gui()` launches the `clausters-gui` visual server and returns a [`GuiHost`](api.md) connected to it — the GUI parallel of `live()` booting a server. You never spell out an address or a segment: the host is started with its client leg pointed at this session's server and mapping the same shared-memory segment the server was booted with, so meters, scopes and playheads read the engine with no per-frame messages. The host is owned by the session and stopped on `close`.

```python
from clausters import Session
from clausters.gui import window, label

session = Session.live()       # attaches, or boots a server (segment auto-chosen)
gui = session.gui()            # clausters-gui, wired to that server + same segment

win = gui.open(window(label(1, text="hello"), title="Panel", w=320, h=120))
gui.set(1, text="edited live")  # edit the open window
gui.close(win)                  # close it

session.close()                 # stops whatever the session started; leaving the interpreter would too
```

`GuiHost` opens, edits and closes windows: `open` sends a `window`-rooted GuiDef and returns its id, `set` edits a live widget, and `close` closes it (`close_all` closes every window still open). Repeated `session.gui()` calls return the same host.

The visual server binary ships **bundled in the same package** as the audio server (built from the independent `clients/gui` workspace, stripped), so there is nothing extra to install — the launcher finds it out of the box. In a source checkout a binary built under `clients/gui/target` is used, and `CLAUSTERS_GUI_BIN` overrides the lookup. See [Getting started](getting-started.md#the-visual-server-gui) for building a lighter, server-only install.

### Without a Session: `Server.boot` and `GuiHost.boot`

If you are not using a `Session`, the server and the GUI host each carry their own launch and teardown, so you don't juggle a separate process object. `Server.boot()` starts a server process and returns a connected `Server` that owns it (its `close()` stops the process); `GuiHost.boot()` does the same for the visual server, returning a started `GuiHost` (its `stop()` stops the process). Both also die with the interpreter.

```python
from clausters.defs import Server
from clausters.gui import GuiHost, window, label

server = Server.boot()                                    # a server process starts
gui = GuiHost.boot(server=f"{server.target.host}:{server.target.port}", shm=server.shm)

gui.open(window(label(1, text="hi"), title="Panel", w=320, h=120))
# ...
gui.stop()        # stops the clausters-gui process
server.close()    # stops the server process
```

This is exactly what `Session.live`/`gui` use internally — the session just bundles them with a clock.

### The raw processes

One level lower, `clausters.launch` exposes the processes themselves — `ServerProcess` and `GuiProcess` — for when you want to own them directly (e.g. a custom `Server`/`GuiHost` wiring). Both are context managers, both register the same exit hooks, and `default_shm_path()` picks a segment (`None` on platforms where shared memory does not apply); `server_is_up()` is the probe `live` uses to decide boot-or-attach.

```python
from clausters import ServerProcess, GuiProcess
from clausters.gui import GuiHost

with ServerProcess() as server_proc:            # clausters --shm <auto>
    with GuiProcess(server=f"{server_proc.host}:{server_proc.port}",
                    shm=server_proc.shm) as gui_proc:
        host = GuiHost(port=gui_proc.port).start()
        ...   # both processes stop when the `with` blocks exit
```

## Plotting a signal: the free-standing `plot`

`plot` is the visual sibling of the free-standing `play`: one verb that opens
**its own window** for whatever signal-like thing you hand it, resolving the
ambient context — the session's GUI host if one is up, else a host it boots
lazily (with no client leg: plot data reaches the host as a mapped file, so no
audio server is involved unless the object itself needs one).

```python
from clausters import plot
from clausters.defs import Env
from clausters.seq import Pwhite

plot(my_synthdef, dur=2.0)            # a def's output, rendered offline
plot(my_graphdef, defs=[member_def])  # a GraphDef (members ride along)
plot(Env.adsr())                      # an envelope, played by the engine's EnvGen
plot(some_buffer)                     # a buffer's contents, fetched live
plot(Pwhite(40.0, 4700.0), n=200)     # a sequence: the value axis auto-fits
```

Dispatch is by kind. A **def** (`SynthDef`, `FaustDef`, `GraphDef`) is rendered
by an ephemeral **offline (NRT) session** — sent, instanced with `controls`,
freed at `dur` — and every output channel is drawn in its own lane: the way to
eyeball what a def actually produces, with no server and no audio device. An
**`Env`** is rendered through the server's own `EnvGen` (gate-released at its
sustain point when it has one), so the curve you see is what the engine plays.
A **`Buffer`** is fetched from the ambient live server, with its channel count
and sample rate. Anything else **iterable of numbers** — a list, a `Pseq`, a
`Pwhite`, any stream — is materialized (up to `n` items for the endless ones)
and plotted as a sequence, index counts on the x axis and the value axis
fitted to the data, whatever its range.

The window is static (no zoom, pan or editing) but **measured**: x/y rulers fit
the signal, the whole sequence is always drawn (a min/max envelope per pixel
column when it outnumbers the pixels, so nothing aliases visually), and
hovering shows a hairline plus the exact sample under the cursor — index or
clock time, and the sample's value. `view="spectrum"` plots the **averaged
magnitude spectrum** instead (dB over a log/linear/mel/bark frequency axis,
analyzed with the same shared-core FFT the spectrogram uses; hovering reads
the bin's frequency and level). The returned `PlotWindow` keeps the display
live:

```python
win = plot(seq)
win.set(view="spectrum", freq_scale="mel")   # /gui_set, no re-render
win.set(min="auto")                          # give a pinned side back to the fit
win.close()
```

## Scoping a live signal: the free-standing `scope`

`scope` is the real-time sibling of `plot`: where `plot` draws a *finished* signal, `scope` opens a window that follows a **live audio bus** of the ambient server, frame by frame. One call does the whole wiring — it resolves the ambient live server and GUI host (here booted as a *client of the server*, mapping its shared-memory segment), takes a free **audio tap** from the server's tap registry (`server.taps`, a finite boot-time resource like buses — `--taps` rings, 8 by default), routes the bus into it with `/tap`, and opens the window. The host then reads the ring straight out of shared memory: zero messages per frame.

```python
from clausters import Server, scope

server = Server.boot()
# ... play something ...
win = scope()                       # oscilloscope on hardware out 0
scope(0, view="phase")              # the stereo field of outs 0/1
scope(0, view="spectrum", freq_scale="mel")
```

`view` picks the instrument. **`"signal"`** (default) is a triggered oscilloscope: a `window_ms` display window re-read every frame and aligned on a rising crossing of `trigger` (with hysteresis; free-running when the signal never crosses), so a periodic signal draws a stable trace. **`"phase"`** is the phasescope (goniometer): the stereo pair `bus`/`bus + 1` drawn as the 45°-rotated Lissajous figure with a correlation read-out — mono reads vertical, anti-phase horizontal, a wide field fills the lozenge; it takes **two adjacent taps**. **`"spectrum"`** is the live spectroscope: one FFT per frame of the newest `fft_size` window, in dB over a `freq_scale` frequency axis (log/linear/mel/bark, the same scales as the spectrogram), exponentially smoothed (`averaging`) with an optional decaying `peak_hold` trace.

The returned `ScopeWindow` retunes the display live and — unlike a plot — owns server resources, so prefer its `close` to the window's close button:

```python
win = scope(0, view="spectrum")
win.set(freq_scale="mel", fft_size=4096)   # /gui_set, live
win.close()                                # stops the tap and frees it
```

Two scopes never fight over one ring: the tap indices come from the client-side registry (sized from `ServerOptions.taps`, like the bus allocators), and `close` both stops the tap on the server (`/tap … -1`) and returns the index. Because the native host reads taps from the shared segment, the resolved server must carry one (`Server.boot` and `Session.live` create it by default); to scope a server you merely *attached* to, pass `host=` pointed at a `GuiHost` booted with that server's segment path.

## When you don't need a Session

A `Session` is sugar over two objects you can always build yourself. When you want more control — several servers behind one clock, a clock shared across subsystems, or a custom interface — skip the factory and wire them directly:

```python
from clausters.base import TempoClock
from clausters.defs import Server

server = Server("127.0.0.1", 57110, latency=0.1)
clock = TempoClock(tempo=2.0)

phrase().play(clock, server)        # what Session.play does for you
clock.run(3.5)
server.close()
```

`Session` adds no behaviour of its own — it only bundles these two and forwards to them — so reaching for the longer form costs nothing and loses nothing.

## See also

- [API reference](api.md) — the generated reference for `Session` and every method.
- [The client, layer by layer](guide.md) — where the `Server`, the clock and the seam fit in the whole client.
- [Routines and clocks](routines-and-clocks.md) — the level below a session: driving a `Routine`, a `TempoClock` and a `Server` yourself.
- [Timing models](timing-models.md) — the ways a clock keeps time (wall-clock, sample-locked, shared transport) and how to observe each.
- [Examples](examples.md) — `offline_render.py` and `live_udp.py`, the session in runnable form.
