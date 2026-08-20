# The ambient verbs: play, plot, render

Three free-standing functions cover the interactive loop: **`play`** sounds a
thing now, **`plot`** shows a thing, **`render`** turns a thing into audio.
Each is one verb for many kinds — hand it whatever you have, and it resolves
the **ambient context** (the running session, else the default session) so a
quick take never spells out a server, a clock or a GUI host:

```python
from clausters import Server, play, plot, render
from clausters.defs import sine

Server().boot()
node = play(sine(440.0) * 0.2)       # a bare expression, sounding now
node.free()                             # ...and gone
plot(sine(440.0) * 0.2, dur=0.02)    # the same signal, on screen
stats = render(sine(440.0) * 0.2, dur=2.0, path="beep.wav")
```

The three verbs carry one semantic each, and the split is deliberate:

- **`play`** is for what already sounds directly — it starts something *now*
  (or on the clock's next beat) and returns a handle to it.
- **`render`** is the **change of state**: it evaluates a *generator* thing (a
  def, a pattern, an arrangement — an algorithm that describes sound) into a
  *generated* one (samples — random-access audio). Offline by default, into
  memory or a file; either way it reports what it did.
- **`plot`** is the visual sibling of `render`: the same render, drawn
  in its own window instead of returned. (Its live counterpart is
  [`scope`](api.md), which taps buses on a running server.)

Each playable kind also carries the same ambient `.play()` method; the free
function is the uniform entry that picks the right one.

## What each verb accepts

**Playables** — `play(x)`:

| You hand it | It does | Returns |
|---|---|---|
| an `Event`, or a plain **dict** of event keys | one note, now (timetagged at the logical beat inside a routine) | the **completed event** — the derived keys (`freq`, `sustain`, …) plus `node`/`server` written in; `.free()` cuts it, `.release()` ends it musically (gate 0 when it releases by gate) |
| an event pattern (`Pbind`) | schedules it on a clock | the `EventStreamPlayer` — `.stop()` |
| a `Routine` / `Stream`, or a bare **generator** | schedules it on a clock | the routine |
| a **bare expression** — a `Ugen` graph, a `ChannelList` of them, a Faust `Signal` or `Box` | wraps it in an ephemeral def (adding the `out` if it lacks one; a channel list lands on buses 0, 1, …), sends and instances it; it sounds until you free it | the `Synth` — `.free()` |
| a def — `SynthDef` / `FaustDef` / `GraphDef` | sends and instances it, with optional `controls` | the `Synth` (or instance `Group`) — `.free()` |
| a `Timeline` | drives it through a playhead on the ambient clock | the `Playhead` — `.stop()` (and `locate`/`loop`) |
| a `Buffer` | sounds it through the stock playbuf instrument (`rate`/`amp` controls, freed when the take ends) | the `Synth` — `.free()` cuts the take early |
| an `Automation` | prepares it if needed and applies the curve to its target controls, now | the automation itself — `.stop()` interrupts the sweep (the controls hold their last value) |
| anything with `play(destination)` (the timeline-item protocol: `OscEvent`, `MidiEvent`, …) | dispatches to it | whatever it returns |

Everything `play` returns knows how to **end what it started** — even the
self-terminating kinds, whose duration can be extreme: a note frees itself
after its sustain, but the completed event's `.free()` cuts it *now*; a take
and a sweep end on their own, but the handle's `.free()` / the automation's
`.stop()` interrupt them. (The release already scheduled at play time still
arrives; it lands on a node that is gone and is harmless.)

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

**Renderables** — every offline path returns a `RenderStats` (see
[What a render gives back](#what-a-render-gives-back)):

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

They part company on one word. `play` and `plot` are **conveniences** — free to
infer what you meant, so `plot` sizes its render from the expression and a
stereo pair shows two lanes without being told. `render` is part of the **NRT
interface**: its `channels` is how many outputs the offline server *has*, a
fact about the render and not about the graph, so it derives nothing. An
expression wider than that would write its surplus onto internal buses that
reach no file, so it raises and names the fix (`render(dup(sig, 4),
channels=4)`) instead of quietly returning half the take.

Ambient resolution is uniform: `server=None` takes the running session's
server, else the booted default (`Server().boot()`); `clock=None` takes the
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
  first (`d.send(server, wait=False)`) and instance it with
  `Synth` — a routine must never block the clock thread.
- **`play(buffer)` needs the buffer's length.** On a live server it queries an
  unfilled handle; offline (NRT) the handle must carry `frames` up front.
- **Beats read as seconds outside a clock.** An interactive `play(event)` or
  `play(automation)` (no routine in flight) times itself on wall time at
  tempo 1.0, exactly like a bare `Event().play()`.

## What a render gives back

Every render — the free-standing `render`, `Session.render`, `Server.render` —
returns one `RenderStats`. There is a single return type whatever you ask for,
because **`path` chooses where the audio goes, not whether there is a result**:

| field | what it is |
|---|---|
| `frames` | length in frames (one frame = one sample per channel) |
| `channels` | how many channels were rendered |
| `sample_rate` | the rate it was rendered at, in Hz |
| `duration` | `frames / sample_rate`, in seconds |
| `events` | how many score events the render executed |
| `peak` | peak magnitude **per channel**, in channel order |
| `rms` | RMS **per channel**, over the whole render |
| `seed` | the seed its stochastic UGens started from — pass it back to replay this take |
| `path` | the file it was written to, or `None` |
| `samples` | the audio, interleaved `float32` — or `None` when it went to a file |

`peak` and `rms` are measured by the renderer *as it streams*, not by a pass
over the result afterwards, so they cost nothing extra and exist even when the
samples never came back:

```python
stats = session.render(sample_rate=48_000.0, channels=2)
print(f"{stats.frames} frames ({stats.duration:.2f} s), peak {max(stats.peak):.3f}")
if max(stats.peak) > 1.0:
    print("it clips")
```

### Where the file is written

`path` does not hand the samples to Python to be written out. The score goes to
the **server's own offline renderer** (`clausters --nrt`), which streams
straight to disk:

```python
stats = session.render(sample_rate=48_000.0, channels=2, path="take.wav")
stats.samples          # None - the audio is in take.wav
stats.path             # 'take.wav'
```

Two things follow. First, a long bounce never lands in the client: a
sixty-second stereo render is 5.7 million floats, and not carrying them across
is most of the reason the file path is several times faster than the in-memory
one. Second, the file's format is the server's to choose — `sample_format` is
`"float"` (the default), `"int24"` or `"int16"`. **Float32 is the default
everywhere**, because f32 is what the engine computes in and what buffers hold,
so writing and reading one loses nothing.

Because the renderer is the `clausters` binary, `path=` needs that binary
findable — the same lookup `clausters.launch` uses (the wheel bundles it).

This holds for **every** kind of render, a bare expression included:

```python
render(sine(440.0) * 0.2, dur=2.0, path="beep.wav").samples   # None, like the rest
```

There is no second writer in the client for the short bounces — one writer
means one answer about the format, and reading a file back never depends on
which verb wrote it.

### Reading a file back

The counterpart is `read_soundfile`, which decodes through **the server's
decoder**, not a Python one:

```python
from clausters.render import read_soundfile

audio = read_soundfile("take.wav")
audio.frames, audio.channels, audio.sample_rate
audio.samples          # interleaved float32
```

WAV goes through hound and FLAC, OGG/Vorbis, MP3, MP4/AAC, ALAC and AIFF
through symphonia — the same path `/buffer_allocRead` takes, so client and server
never disagree about a file. Integer files are scaled to `[-1, 1]` on the way
out: whatever the file holds, what you get is `float32`. Nothing resamples;
`sample_rate` is the file's own.

This is not a convenience wrapper over the standard library. Python's `wave`
module cannot read a float32 WAV at all — the format `render` writes — so
borrowing the server's reader is what keeps the client free of an audio
dependency.

### Interleaved, and how to split it

Samples are **interleaved** everywhere: `L R L R …`, `frame * channels +
channel`. That is not an arbitrary choice — it is the server's own buffer
layout, the order `/buffer_getRange` indexes and `/buffer_export` writes — so audio *going
to* the server needs no conversion at all.

For analysis on the client side, split it:

```python
from clausters.render import channels, interleave

left, right = channels(stats.samples, stats.channels)
stats.channel(0)                     # the same thing, one channel
back = interleave(left, right)       # the inverse
```

Both directions are C-level strided copies on an `array` — deinterleaving a
sixty-second stereo render takes about 13 ms, a few percent of the render
itself — so there is no reason to keep per-channel copies around. Reach for
`channels` when you are measuring or plotting one channel, and leave the
interleaved buffer alone the rest of the time.

### Every render is a new take, and you can ask for an old one

A score with a random process in it — `white_noise`, `dust`, an `lf_noise`
sweeping a filter — renders **differently every time**:

```python
a = session.render(channels=2)
b = session.render(channels=2)
a.samples == b.samples      # False: two performances of the same piece
```

That is the point of writing a random process into a piece. Playing it again is
another performance, and a renderer that quietly gave you the same one would be
hiding the thing you asked for.

Reproducibility is the *request*, and `stats.seed` is what makes it possible to
make. Every render reports the seed it started from, whether you chose it or it
drew one:

```python
take = session.render(channels=2)
print(take.seed)                            # 12157665459056928801

again = session.render(channels=2, seed=take.seed)
again.samples == take.samples               # True, sample for sample
```

So the working habit is: render until you like what you hear, write down
`stats.seed`, and from then on that take is a value you can pass around — into a
golden file, into a bounce, into a piece that has to sound the same tomorrow.
The seed sequence belongs to the render, so a fixed seed reproduces in any
process, at any worker count, in memory or through a file.

This is the same rule the client's own random context has always followed: a
`Pwhite` draws unpredictably until `main.seed(n)` or `session.seed(n)` pins it
(see [Routines and clocks](routines-and-clocks.md)). The two seeds are
independent — one is the server's noise UGens, the other is the client's
patterns — but they answer to the same idea, so a piece that has to repeat
exactly pins both.
