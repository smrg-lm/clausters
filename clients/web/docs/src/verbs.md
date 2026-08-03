# The ambient verbs: play, plot, render

Three free-standing functions cover the interactive loop: **`play`** sounds a thing now, **`plot`** shows a thing, **`render`** turns a thing into audio. Each is one verb for many kinds — hand it whatever you have, and it resolves the **ambient context** (the running session, else the default one) so a quick take never spells out a server, a clock or a GUI host:

```js
import { Session, play, plot, render } from "clausters";
import { sine } from "clausters/defs";

const session = (await Session.page()).adoptDefault();
const node = play(sine(440.0).mul(0.2));   // a bare expression, sounding now
node.free();                                // ...and gone
await plot(sine(440.0).mul(0.2), { dur: 0.02 });   // the same signal, on screen
const stats = await render(sine(440.0).mul(0.2), { dur: 2.0 });
```

The three carry one semantic each, and the split is deliberate:

- **`play`** is for what already sounds directly — it starts something *now* (or on the clock's next beat) and returns a handle to it.
- **`render`** is the **change of state**: it evaluates a *generator* thing (a def, a pattern — an algorithm that describes sound) into a *generated* one (samples — random-access audio). Always offline, and it reports what it did.
- **`plot`** is the visual sibling of `render`: the same materialization, drawn in its own window instead of returned.

`play` is synchronous; `plot` and `render` resolve with a promise, because opening a host and running a render both wait and a page may not block. That is this client's one standing difference from the [Python client](https://clausters-python.readthedocs.io/), not a difference in the verbs.

## What each verb accepts

**Playables** — `play(x)`:

| You hand it | It does | Returns |
|---|---|---|
| an `Event`, or a plain object of event keys | sounds one note | the completed event |
| an event `Pattern` (a `Pbind`) | schedules it on a clock | the `EventStreamPlayer` |
| a `Routine`/`Stream`, or a bare generator | schedules it on a clock | the routine |
| a def, or a bare expression (`Ugen` / `ChannelList` / `Signal`) | sends it and instances it | the node handle |
| a `Timeline` | a `Playhead` over the ambient clock and server | the playhead |
| an `Automation` | triggers its lane and maps its targets | the automation |
| a `Buffer` | sounds it through the stock playbuf instrument | the synth |

**Plottables** — `await plot(x)` (each call opens its own window):

| You hand it | It shows |
|---|---|
| a def, or a bare expression | its output, rendered offline for `dur` seconds — one lane per channel |
| an `Env` or an `Automation` | the curve, rendered through the engine's own `envGen` |
| a `Buffer` or a buffer number | its contents, fetched from the ambient live server |
| any iterable of numbers (a `Pattern`, an array, a `Float32Array`) | the sequence, index on the x axis and the value range fitted |

**Renderables** — `await render(x)`:

| You hand it | It renders |
|---|---|
| a binary **score** (`Uint8Array`) | the score, as is |
| a def, or a bare expression | instances it offline for `dur` seconds — the audible sibling of `plot(def)` |
| a `Timeline`, an event `Pattern`, a `Routine` or a generator | **bounces** it in an ephemeral offline session |

`plot` and `render` share their materialization: `plot(x)` shows exactly what `render(x)` returns and `play(x)` sounds.

They part company on one word. `play` and `plot` are **conveniences** — free to infer what you meant, so `plot` sizes its render from the expression and a stereo pair shows two lanes without being told. `render` is part of the **offline interface**: its `channels` is the render's *output* count, a fact about the server being configured and not about the graph, so it derives nothing. An expression laid past those outputs is refused rather than half-rendered.

## The offline session

Under `render` sits a third `Server` carrier, beside the in-page engine and the WebSocket: one that **writes time instead of waiting for it**.

```js
const session = await Session.nrt({ tempo: 2.0 });
await def.send(session.server);
session.play(new Pbind({ degree: new Pseq([0, 2, 4]), dur: 0.5 }));
const stats = await session.render({ channels: 2 });
```

`Session.nrt()` gives a `Server` whose connection accumulates every command as a timestamped **score** instead of sending it; `session.render()` drains the clock — logically, with no sleeping — and hands that score to the engine's own renderer, the same wasm that makes this page's sound, running as fast as the machine manages. No `AudioContext`, no gesture, no socket and no server process.

Nothing above the carrier changes: the same patterns, defs and routines play into it, because only the connection under the `Server` is different. That is what makes a piece written for a live take renderable without editing a line of it — and the score it writes is **byte-identical** to the one the Python client writes for the same piece, which the package asserts against committed vectors.

Schedule a closing event — freeing the root group, or whatever ends the piece — so the render has a defined length: it stops when the score does, and commands do not sound. `until` bounds the drain in beats, which an endless source needs (an infinite pattern never drains on its own).

## What a render gives back

```js
const stats = await render(myPattern, { defs: [myInstrument], channels: 2 });
// { frames, channels, sampleRate, duration, peak, rms, seed, samples }
```

`peak` and `rms` are **per channel**, measured by the shared core, so they are the same numbers the server and the Python client report for the same audio. `samples` is interleaved `Float32Array`.

`seed` is the one this take's stochastic UGens started from. Unless you asked for a seed you got a fresh one, so **this is how you get a take back**: pass it as `seed` and the render repeats sample for sample. (The engine's own entropy source does not exist on wasm, so the client draws the word from the platform's `crypto` and forwards it — without that, every take of a noisy piece in a browser would be the same take.) A pattern's own jitter — a `Pwhite` — is a different randomness: it is the *session's* seeded stream, reproduced with `session.seed(n)`.

Every offline path starts from an **empty** ephemeral session, so whatever the material names has to ride along in `defs`.

## Where the audio goes

The Python client's `render` takes a `path` and the **server** writes the file, streaming straight to disk. A page has no such process and no filesystem, so there is no `path` here: the render produces a `Float32Array` in this tab, and what to do with it is the page's.

```js
const wav = wavBytes(stats);                       // a float32 WAV, in memory
const url = URL.createObjectURL(new Blob([wav], { type: "audio/wav" }));
// ...or feed it straight back into the engine:
const buffer = await Buffer.fromSamples(stats.samples, stats.channels, stats.sampleRate);
play(buffer);
```

`Buffer.fromSamples` is the browser's render-then-load with the file taken out of the middle — the samples go into a buffer on the page's engine directly, since the carrier shares memory with it. The render is *material* now: random-access audio, sliceable and playable like any other.

`examples/offline.html` runs the whole loop — render, look, download, play back.

## See also

- [Routines and clocks](routines-and-clocks.md) — the clock a bounce drains, and the logical time it keeps.
- [Reading the server: buses and buffers](data.md) — the live data paths `plot`'s buffer leg reads through.
- The **[Python client's book](https://clausters-python.readthedocs.io/)** — the same three verbs from the reference client, with the file-writing half a page has no use for.
