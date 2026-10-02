# The ambient verbs: play, plot, scope, render

Four free-standing functions cover the interactive loop: **`play`** sounds a thing now, **`plot`** shows a thing, **`scope`** watches what is sounding, **`render`** turns a thing into audio. Each is one verb for many kinds — hand it whatever you have, and it resolves the **ambient context** (the running session, else the default one) so a quick take never spells out a server, a clock or a GUI host:

```js
import { Session, play, plot, render } from "clausters";
import { sine } from "clausters/defs";

const session = (await Session.embed()).activate();
const node = play(sine(440.0).mul(0.2));   // a bare expression, sounding now
node.free();                                // ...and gone
await plot(sine(440.0).mul(0.2), { dur: 0.02 });   // the same signal, on screen
const stats = await render(sine(440.0).mul(0.2), { dur: 2.0 });
```

The three carry one semantic each, and the split is deliberate:

- **`play`** is for what already sounds directly — it starts something *now* (or on the clock's next beat) and returns a handle to it.
- **`render`** is the **change of state**: it evaluates a *generator* thing (a def, a pattern — an algorithm that describes sound) into a *generated* one (samples — random-access audio). Always offline, and it reports what it did.
- **`plot`** is the visual sibling of `render`: the same render, drawn in its own window instead of returned.
- **`scope`** is the *live* one: where `plot` draws a thing that was rendered once, `scope` opens a window that follows a running server's audio buses frame by frame, with no per-frame messages from the script.

`play` is synchronous; `plot`, `scope` and `render` resolve with a promise, because opening a host and running a render both wait and a page may not block. That is this client's one standing difference from the [Python client](https://clausters-python.readthedocs.io/), not a difference in the verbs.

## What each verb accepts

**Playables** — `play(x)`:

| You hand it | It does | Returns |
|---|---|---|
| an `Event`, or a plain object of event keys | sounds one note | the completed event |
| an event `Pattern` (a `Pbind`) | schedules it on a clock | the `EventStreamPlayer` |
| a `Routine`/`Stream`, or a bare generator | schedules it on a clock | the routine |
| a def, or a bare expression (`Ugen` / `ChannelList` / `Signal`) | sends it and instances it | the node handle |
| a `Timeline` | plays it on its own clock, on the ambient server | the timeline |
| an `EventSequence` | loads it as an event lane on the server's notes transport, its pass ending where its contents do; boots the page's engine for the default session when there is none | a promise of the `Transport` it plays on — `stop()`, `wait()` (and `pause`/`locate`/`loop`), in the sequence's beats |
| a `Buffer` | sounds it through the stock playbuf instrument | the synth |

**Plottables** — `await plot(x)` (each call opens its own window):

| You hand it | It shows |
|---|---|
| a def, or a bare expression | its output, rendered offline for `dur` seconds — one lane per channel |
| an `Env` or a `Bpf` | the curve, rendered through the engine's own `envGen` |
| a `Buffer` or a buffer number | its contents, fetched from the ambient live server |
| any iterable of numbers (a `Pattern`, an array, a `Float32Array`) | the sequence, index on the x axis and the value range fitted |

**Scope views** — `await scope(bus, { view })`, one window each:

| `view` | It shows |
|---|---|
| `"signal"` (default) | a triggered **oscilloscope** over `channels` adjacent buses — every frame aligned on a rising crossing of `trigger` in the first channel, so a periodic signal stands still and the channels keep their true relative phase |
| `"phase"` | a **goniometer** of the pair `bus` / `bus + 1`: mono draws a vertical line, anti-phase horizontal, with the correlation under the field |
| `"spectrum"` | a live **FFT** curve per channel, hertz on `freqScale` against dB over `[dbFloor, dbCeil]` |

`win.set({...})` retunes any prop of the open view while it runs, and `win.close()` closes the window — after which the host stops recording whatever no open view is drawing any more. Naming a bus is all a script does: the host asks the server to record it.

**Renderables** — `await render(x)`:

| You hand it | It renders |
|---|---|
| a binary **score** (`Uint8Array`) | the score, as is |
| a def, or a bare expression | instances it offline for `dur` seconds — the audible sibling of `plot(def)` |
| a `Timeline`, an `EventPattern`, a `Routine` or a generator | **bounces** it in an offline session |
| a value pattern (`Pseq`, `Pwhite`, …) | the values it generates, as an array (an endless one needs `count`) |

**The tempo is the clock's.** Neither verb takes one: `play` and `render` both take an optional `clock`, and use a default one when none is given. A render bounces in an **offline session** — the one its `clock` belongs to, which has to be a clock of `Session.nrt()`, or, with no `clock`, a session of the render's own at tempo 1.0. A value pattern is not a playable at all: `play` refuses it by name, and `render` generates its values.

`plot` and `render` do the same render: `plot(x)` shows exactly what `render(x)` returns and `play(x)` sounds.

They part company on one word. `play` and `plot` are **conveniences** — free to infer what you meant, so `plot` sizes its render from the expression and a stereo pair shows two lanes without being told. `render` is part of the **offline interface**: its `channels` is the render's *output* count, a fact about the server being configured and not about the graph, so it derives nothing. An expression laid past those outputs is refused rather than half-rendered.

## The offline session

Under `render` sits a third `Server` carrier, beside the in-page engine and the WebSocket: one that **writes time instead of waiting for it**.

```js
const session = await Session.nrt();
session.clock.setTempo(2.0);
await def.send(session.server);
session.play(new Pbind({ degree: new Pseq([0, 2, 4]), dur: 0.5 }));
const stats = await session.render({ channels: 2 });
```

`Session.nrt()` gives a `Server` whose connection accumulates every command as a timestamped **score** instead of sending it; `session.render()` drains the clock — logically, with no sleeping — and hands that score to the engine's own renderer, the same wasm that makes this page's sound, running as fast as the machine manages. No `AudioContext`, no gesture, no socket and no server process.

Nothing above the carrier changes: the same patterns, defs and routines play into it, because only the connection under the `Server` is different. That is what makes a session written for a live take renderable without editing a line of it — and the score it writes is **byte-identical** to the one the Python client writes for the same session, which the package asserts against committed vectors.

Schedule a closing event — freeing the root group, or whatever ends the take — so the render has a defined length: it stops when the score does, and commands do not sound. `until` bounds the drain in beats, which an endless source needs (an infinite pattern never drains on its own); with none, `render` refuses an event pattern after a million events rather than rendering it forever.

## What a render gives back

```js
const stats = await render(myPattern, { defs: [myInstrument], channels: 2 });
// { frames, channels, sampleRate, events, duration, peak, rms, seed, path, samples }
```

`peak` and `rms` are **per channel**, measured by the shared core, so they are the same numbers the server and the Python client report for the same audio. `events` is how many score events the render ran. `samples` is interleaved `Float32Array` — or `null`, when a `path` sent the audio to a file.

`seed` is the one this take's stochastic UGens started from. Unless you asked for a seed you got a fresh one, so **this is how you get a take back**: pass it as `seed` and the render repeats sample for sample. (The engine's own entropy source does not exist on wasm, so the client draws the word from the platform's `crypto` and forwards it — without that, every take of a noisy score in a browser would be the same take.) A pattern's own jitter — a `Pwhite` — is a different randomness: it is the *session's* seeded stream, reproduced with `session.seed(n)`.

Every offline path with no `clock` starts from an **empty** session of its own, so whatever the samples names has to ride along in `defs`.

## Where the audio goes

Without a `path` the samples come back in `stats.samples`. With one the take is written to that file and `stats.samples` is `null` — the path chooses where the output goes, not whether there is one, as in the Python client:

```js
const stats = await session.render({ sampleRate: 48_000, channels: 2, path: "out/take.wav" });
stats.samples;         // null - the audio is in out/take.wav
stats.path;            // "out/take.wav"
```

The file is where every path this client takes is: the page's own storage — the origin private file system, `opfs` — in a tab, and the disk under node. It is also what `Buffer.read` and the engine's `/buffer_allocRead` read in a tab, so a take written by a render loads straight into a buffer. `sampleFormat` is `"float"` (the default), `"int24"` or `"int16"`, and the framing and the conversion are the server crate's, so the file is the one a native render writes. What a page cannot do is stream while rendering: its renderer is the wasm engine in the tab, so the samples exist in full before the file is written.

### Reading a file back

`readSoundfile` decodes through **the server's decoder** — WAV, FLAC, OGG/Vorbis, MP3, MP4/AAC, ALAC, AIFF — the one `/buffer_allocRead` uses, so the client and the engine never disagree about a file:

```js
const audio = await readSoundfile("out/take.wav");
audio.frames; audio.channels; audio.sampleRate;
const [left, right] = channels(audio.samples, audio.channels);
```

Integer files are scaled to `[-1, 1]`, nothing resamples, and `start`/`frames` read a span. `channels` deinterleaves and `interleave` weaves back.

`examples/buffers/render-then-load.html` runs the whole loop — render to a file, read it back, load it into a buffer and play it.

## See also

- [Routines and clocks](routines-and-clocks.md) — the clock a bounce drains, and the logical time it keeps.
- [Reading the server: buses and buffers](data.md) — the live data paths `plot`'s buffer leg reads through.
- The **[Python client's book](https://clausters-python.readthedocs.io/)** — the same three verbs from the reference client, with the file-writing half a page has no use for.
