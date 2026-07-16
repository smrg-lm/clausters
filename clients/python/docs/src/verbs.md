# The ambient verbs: play, plot, render

Three free-standing functions cover the interactive loop: **`play`** sounds a
thing now, **`plot`** shows a thing, **`render`** turns a thing into audio.
Each is one verb for many kinds — hand it whatever you have, and it resolves
the **ambient context** (the running session, else the default session) so a
quick take never spells out a server, a clock or a GUI host:

```python
from clausters import Server, play, plot, render
from clausters.defs import sin_osc

Server.boot()
node = play(sin_osc(440.0) * 0.2)       # a bare expression, sounding now
plot(sin_osc(440.0) * 0.2, dur=0.02)    # the same signal, on screen
samples, frames = render(sin_osc(440.0) * 0.2, dur=2.0, path="beep.wav")
```

The three verbs carry one semantic each, and the split is deliberate:

- **`play`** is for what already sounds directly — it starts something *now*
  (or on the clock's next beat) and returns a handle to it.
- **`render`** is the **change of state**: it evaluates a *generator* thing (a
  def, a pattern, an arrangement — an algorithm that describes sound) into a
  *generated* one (samples — random-access audio). Offline by default, into
  `(samples, frames)` or a WAV.
- **`plot`** is the visual sibling of `render`: the same materialization, drawn
  in its own window instead of returned. (Its live counterpart is
  [`scope`](api.md), which taps buses on a running server.)

Each playable kind also carries the same ambient `.play()` method; the free
function is the uniform entry that picks the right one.

## What each verb accepts

**Playables** — `play(x)`:

| You hand it | It does | Returns |
|---|---|---|
| an `Event`, or a plain **dict** of event keys | one note, now (timetagged at the logical beat inside a routine) | the synth node id |
| an event pattern (`Pbind`) | schedules it on a clock | the `EventStreamPlayer` |
| a `Routine` / `Stream`, or a bare **generator** | schedules it on a clock | the routine |
| a **bare expression** — a `Ugen` graph, a Faust `Signal` or `Box` | wraps it in an ephemeral def (adding the `out` if it lacks one), sends and instances it; it sounds until you free it | the `Synth` |
| a def — `SynthDef` / `FaustDef` / `GraphDef` | sends and instances it, with optional `controls` | the `Synth` (or instance `Group`) |
| a `Timeline` | drives it through a playhead on the ambient clock | the `Playhead` |
| a `Buffer` | sounds it through the stock playbuf instrument (`rate`/`amp` controls, freed when the take ends) | the `Synth` |
| an `Automation` | prepares it if needed and applies the curve to its target controls, now | the lane node id |
| anything with `play(destination)` (the timeline-item protocol: `OscEvent`, `MidiEvent`, …) | dispatches to it | whatever it returns |

**Plottables** — `plot(x)` (each call opens its own window; see the
[`plot` API](api.md) for the display options — `view="spectrum"`, rulers,
ranges):

| You hand it | What you see |
|---|---|
| a def, or a bare expression (`Ugen` / `Signal` / `Box`) | its output, rendered offline for `dur` seconds — one lane per channel |
| an `Env` | the curve as the engine's own `EnvGen` plays it (gate-released at its sustain point) |
| an `Automation` | its curve, the same engine-evaluated way, labelled with the control name |
| a `Buffer` (or buffer number) | its contents, fetched from the live server |
| any iterable of numbers — a list, a value pattern (`Pseq`, `Pwhite`, …), a stream | the sequence, index on the x axis (endless ones cap at `n`) |

**Renderables** — `render(x)` (offline paths return `(samples, frames)`,
interleaved float32; `path=` also writes a float32 WAV):

| You hand it | It does |
|---|---|
| a binary **score** (`bytes`) | the embedded offline renderer, unchanged (the historical `render`) |
| a def, or a bare expression | instances it offline for `dur` seconds — the audible sibling of `plot(def)` |
| an arrangement `Element` | with a `destination`, delegates to the arrangement's own render (RT or NRT by the destination); without one, **bounces** it in an ephemeral offline session |
| a `Timeline` | the same dual path |
| an event pattern, a `Routine`/`Stream`, a generator | offline bounce only — they are forward-only; sounding them live is `play`'s job |

## How it works

Each verb separates **coercion** from **dispatch**. `play` first coerces the
language-level things (a dict into an `Event`, a generator into a `Routine`, a
bare expression into an ephemeral def via `clausters.defs.as_def`), then
dispatches on the `.play()` the kinds already carry. `plot` and `render` share
the same expression coercion and the same def-to-samples change of state, so
`plot(x)` shows exactly what `render(x)` returns and `play(x)` sounds.

Ambient resolution is uniform: `server=None` takes the running session's
server, else the booted default (`Server.boot()`); `clock=None` takes the
running routine's clock or, outside one, the default session's, created and
started on first use. Passing `server=` / `clock=` explicitly overrides
per call — see [Sessions](sessions.md) for how sessions scope this.

## Rendered, not played: the arrangement

An arrangement `Element` is **not** playable, on purpose. The verbs follow the
generated/generator split the arrangement is built on: an element still needs
its change of state (the flatten/bounce), so its door is `render` —
`play(element)` raises a `TypeError` that points there. A flat `Timeline`,
being already generated, *is* playable. The full story is in
[Composition](composition.md); the design record lives in the server book.

## Caveats

- **An endless source needs `until`.** `render(Pbind(...))` with an infinite
  pattern would never drain; pass `until=beats` to bounce a fixed length.
- **`play(def)` blocks until the server confirms** (the `/done` of the def
  send). Fine interactively; inside a routine, send the def asynchronously
  first (`server.add_def(d, wait=False)`) and instance it with
  `server.synth` — a routine must never block the clock thread.
- **`play(buffer)` needs the buffer's length.** On a live server it queries an
  unfilled handle; offline (NRT) the handle must carry `frames` up front.
- **Beats read as seconds outside a clock.** An interactive `play(event)` or
  `play(automation)` (no routine in flight) times itself on wall time at
  tempo 1.0, exactly like a bare `Event().play()`.
