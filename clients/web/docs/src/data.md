# Reading the server: buses and buffers

A view that shows what the server is doing has to read it. There are three
things worth reading, and each has its own path:

| What | Path | For |
|---|---|---|
| a **control bus** | `/bus_stream` → periodic snapshots | meters, read-outs, slow traces |
| an **audio bus** | `/bus_tapStream` → windows of samples | oscilloscopes, phasescopes, spectra |
| a **buffer** | `/buffer_getRange` in chunks, or `fetch` | waveforms, audio-editor views |

The GUI host already reads all three on its own — that is why a GuiDef naming a
bus or a URL draws with no script at all, and **the drawing is always its**: a
client names what to look at (`plot`, `scope`, a `meter`/`scope`/`spectrum`/
`waveform` widget) and the host paints it. This chapter is the same three paths
opened to *your* script, for everything else a program does with the data — a
read-out, a decision, a summary it stores or hands on, a test.

Everything here lives in the `data` namespace:

```js
import { Bus, data } from "clausters";
```

## Control buses

One subscription, one snapshot per period, decoded for you:

```js
const level = Bus.control(server);
// ... something writes it: a synth's `outCtl`, a knob bound to it, `bus.set`

const buses = await data.BusStream.open(server, [level], { periodMs: 33 });
buses.onSnapshot((values) => report(values[0]));       // ~30 times a second
```

`values` is a `Float32Array` in the order you asked for, always holding the
newest snapshot; `buses.value(level)` reads one by handle. When the view goes
away, `await buses.stop()` cancels the subscription — the buses themselves are
untouched, since a stream only ever reads.

Two limits come from the server and not from this class: a **ceiling on the
buses** one subscription may list, and **one subscription per client** —
opening a second `BusStream` on the same `Server` replaces the first. Watch
everything the page needs in one stream.

The ceiling is boot-time configuration on the server (`--max-stream-buses`, or
`maxStreamBuses` when the page boots its own engine) and its default is
generous. What matters is that the number is **per carrier**: the same server
answers a page over its shared-memory ring and a native client over TCP with
two different figures, because a snapshot is one message and is never split
across replies. So a page that watches a great deal reads it rather than
assuming it — `(await server.queryInfo()).maxStreamBuses` — and a request over the
ceiling is refused whole, leaving whatever subscription was already there.

> **In the page, the GUI host is that same client.** Over the in-page carrier
> the script and the host both reach the engine through one shared-memory ring,
> which the server sees as a single client — so a `meter` or a `scope` widget on
> a canvas and a `BusStream` in the script *take the subscription from each
> other*, and the host does not recover until one of its widgets changes. Until
> ring clients get their own identities (a gap recorded in the server's
> roadmap), pick one reader per page: either the host draws the live views, or
> the script does. The same holds for `TapStream` and `/bus_tapStream`. Over a
> WebSocket there is no such conflict — a native host and a script are
> different clients.

A stream is a *latest value*, not a history: a script that needs one keeps it.

```js
const history = new Float32Array(512);
buses.onSnapshot((values) => {
    history.copyWithin(0, 1);
    history[511] = values[0];
});
```

To *see* the history rather than hold it, name the view instead — a `scope`
widget at `rate: "control"` plots a control bus's recent past, and the host
keeps the window.

## Audio buses

A control bus carries one value per block; an oscilloscope needs the samples. A
control bus lives permanently in the server's shared segment, so it can always
be read; an audio bus does not, so the server **records** the ones it is asked
for. You never name the recording: you name the bus.

```js
const taps = await data.TapStream.open(server, [bus], { frames: 2048, periodMs: 33 });

taps.onData((bus, window) => {
    report(Math.max(...window.samples.map(Math.abs)));   // the newest peak
});
```

Opening the stream is what starts the recording, and stopping it is what ends
it:

```js
await taps.stop();
```

One subscription per client applies here too, with the same in-page caveat
above: a `TapStream` and a host oscilloscope on one page displace each other.
The server has a finite number of rings to record into (8 by default), shared
with whatever the GUI host is drawing — it counts watchers, so several views of
one bus cost one ring, and a stream that cannot get one fails loudly rather than
falling silent.

Each window arrives with `endPosition` — the total samples ever recorded for
that bus at the window's end — so consecutive windows can be placed on the bus's
own timeline: they overlap or gap by exactly the position delta, never by a
guess about the period.

**To *see* the bus, name it instead**: `await scope(bus)` opens an
oscilloscope window on it, and a `scope` widget in a GuiDef puts one inside a
window you compose. The framing and the trigger that make a periodic signal
stand still are the host's, over the same tap — there is nothing to compute
here, and nothing that would agree with the host by coincidence.

A stereo pair reads as one interleaved window, which is what a phasescope and a
correlation take:

```js
const pair = taps.interleaved(bus, 2);          // adjacent buses b and b+1
const [left, right] = data.deinterleave(pair, 2);
const points = data.lissajous(left, right);     // [x, y] per frame
const r = data.correlation(left, right);        // -1 … +1, or undefined
```

## Buffer samples

A buffer's samples come back in chunks sized to the carrier — over a WebSocket a
chunk is megabytes, so a buffer of any ordinary size is one or two round trips:

```js
const samples = await buffer.getSamples();            // the whole thing
const slice = await buffer.getSamples({ start: 0, count: 1024 });
```

The samples are interleaved (`frame * channels + channel`), so a stereo buffer
reads `L R L R …`; `data.deinterleave(samples, buffer.channels)` splits it.

**Writing is the other direction**, and it closes the cycle an editor view
makes: read a buffer, edit the samples client-side, write them back.

```js
const samples = await buffer.getSamples();
await buffer.setSamples(samples.map((v) => v * 0.5));   // half as loud
await buffer.setSample(0, 0);                           // one sample
```

`setSamples` chunks against the same carrier bound `getSamples` reads with, and
lays its samples down from a flat index, so the two are mirror images. One rule
differs: writing past the end **rejects** rather than being clamped the way a
read is — a short read hands back less than was asked for and says so, a short
write would lose samples you believe you stored.

Samples also get into a buffer without being sent at all: `buffer.gen` has the
server compute them, `Buffer.read` has a native server read a file, and — in the
page, where the carrier shares memory with the engine — `Buffer.load(server,
url)` fetches and decodes with the browser's own decoder:

```js
const buffer = await Buffer.load(server, "./kick.wav");   // in-page carrier only
```

That last one stays in-page by cost rather than by impossibility: over a socket
the same file would travel sample by sample through `setSamples`, where a native
server reads it in one command.

### The peak pyramid

A waveform is never drawn sample by sample. The samples are reduced once into a
min/max pyramid — level 0 summarizes every `baseBucket` samples, each level
above halves the resolution — and a picture reads the level its magnification
calls for, so the drawing costs the width of the view rather than the length of
the buffer:

```js
const peaks = data.Peaks.build(samples, { channels: buffer.channels });
```

**The picture over it is the host's.** A `waveform` widget takes a `buffer`
number, a `cache` file or a `path` and walks this same path itself, choosing the
level, laying a column across each pixel and joining the columns so an edge is
inked rather than falling between two of them. None of that is in this client,
and a page that wants a waveform names one:

```js
gui.waveform({ buffer: buffer.bufnum, sampleRate, ruler: "time", rulerY: "db" })
```

What a script asks a pyramid is **what it is**, never what it says — a cell's
min/max, the level a zoom reads at and the row across a width are the drawing's
questions, and the drawing is not here:

```js
peaks.frames;        // samples per channel
peaks.channels;
peaks.baseBucket;    // what to subscribe with, so reports fold in
peaks.numLevels;
peaks.toBytes();     // the cache, to hand on or to store
```

The reduction is the shared core's, and so is its cache format: `peaks.toBytes()`
writes the same bytes the GUI host maps and the Python client produces, and
`Peaks.fromBytes` reads one back. When a page is done with a pyramid,
`peaks.free()` returns its wasm memory.

### Watching a take record

A recording is the one samples a page cannot fetch as it grows: the samples are
being written into the server's own memory, and a page maps nothing. So the
server sends the **overview** instead — what the writer measured over the frames
that appeared, at about 2 kB/s a channel where the audio would be 190.
`RecordingStream` is the receiving end: one pyramid per take, growing as the
reports land.

```js
const take = await Buffer.alloc(10 * 48000, 1, { server });
const stream = await data.RecordingStream.open(server, [take]);

stream.onReport(() => {
    report(stream.written(take));           // how far the take has got
});

new Synth("record_something", { buf: take.bufnum }, { server });
```

Each take's pyramid is allocated at the buffer's **full length** and empty, so
the axis does not move while it fills; `written` is how far the reports have
got, and past it the pyramid is the silence the buffer is — read up to it and
the two stay apart, which is what the GUI host's `fills` prop does for the
picture. `stop()` cancels the subscription and `free()` releases the pyramids.

**To watch a take fill, name it**: a `waveform` widget with `fills` over the
take's buffer follows the same reports from the host's side, drawn to the write
frontier and no further. This class is for a script that wants the summary
itself — and only one of the two may have it, which is the next paragraph.

Two limits worth knowing, both of them the wire's rather than this class's. The
summary is the resolution: inside one bucket there is one figure, the bucket's,
so a script that needs the detail reads the take back with `getSamples` for the
span it cares about. (A **host-drawn** view does that for itself —
it asks the server for the span it is zoomed into, which is how a page resolves
to the sample where a desktop host reads the samples it mapped.) And the server
keeps **one buffer subscription per client**, replacing it on every call: on a
page where the GUI host is already following a recording of its own (a
`waveform` with `fills`), opening this beside it cancels the host's. Following a
take is either the host's job or the script's.

## The measurements

Beside the paths sit the figures the views are made of, each the core's own
function — the one the host draws with:

```js
data.correlation(left, right);                  // Pearson's r, or undefined
data.lissajous(left, right);                    // interleaved [x, y] points
```

Both are measurements **of the signal**, which is why they are here: a number a
script can report, compare or act on. What is *not* here is anything of the
screen — a decibel curve, an oscilloscope's display window and trigger, a row of
pixel columns. Those are drawing, the host draws, and it reaches the same
`clausters-core` for them: `scope(bus, { view: "spectrum" })` gives you the
spectrum a page would otherwise be computing, over the very same tap.

## One implementation of every figure

Nothing here computes a number in TypeScript. Every figure — the correlation,
the stereo projection, the peak summary — comes out of the same
`clausters-core` function the GUI host calls, so a figure your script reports
and the picture the host draws from the same bus or buffer are the same
measurement rather than two that agree today.

That is also the reason the drawing itself is not here. A second implementation
of a picture is exactly what would drift, and it would drift where no compiler
looks: **everything drawn is drawn by the host**. A script names what to look at
and gets the host's own rendering of it; anyone who would rather stroke their
own canvas is welcome to, and that is their program, not a surface this package
provides, documents or keeps in step with the Python client.
