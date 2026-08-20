# Reading the server: buses and buffers

A view that shows what the server is doing has to read it. There are three
things worth reading, and each has its own path:

| What | Path | For |
|---|---|---|
| a **control bus** | `/bus_stream` → periodic snapshots | meters, read-outs, slow traces |
| an **audio bus** | `/bus_tapStream` → windows of samples | oscilloscopes, phasescopes, spectra |
| a **buffer** | `/buffer_getRange` in chunks, or `fetch` | waveforms, audio-editor views |

The GUI host already reads all three on its own — that is why a GuiDef naming a
bus or a URL draws with no script at all. This chapter describes the same
three paths opened to *your* script, for a view you draw yourself on your own
canvas.

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
buses.onSnapshot((values) => drawMeter(values[0]));    // ~30 times a second
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
across replies. So a page that draws a great deal reads it rather than assuming
it — `(await server.queryInfo()).maxStreamBuses` — and a request over the
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

A stream is a *latest value*, not a history. A rolling trace is the view's own
business:

```js
const history = new Float32Array(512);
buses.onSnapshot((values) => {
    history.copyWithin(0, 1);
    history[511] = values[0];
    drawTrace(history);
});
```

## Audio buses

A control bus carries one value per block; an oscilloscope needs the samples. A
control bus lives permanently in the server's shared segment, so it can always
be read; an audio bus does not, so the server **records** the ones it is asked
for. You never name the recording: you name the bus.

```js
const frames = data.scopeFrames(20.0, 48000);   // a 20 ms window, with slack
const taps = await data.TapStream.open(server, [bus], { frames, periodMs: 33 });

taps.onData((bus, window) => {
    const trace = data.scopeWindow(window.samples, { windowMs: 20.0 });
    drawScope(trace.samples, trace.locked);
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
drawing nothing.

Each window arrives with `endPosition` — the total samples ever recorded for
that bus at the window's end — so consecutive windows can be placed on the bus's
own timeline: they overlap or gap by exactly the position delta, never by a
guess about the period.

`scopeWindow` is what makes a trace stand still. Without it a periodic signal
crawls across the view; with it, the window starts at the latest rising crossing
of the trigger level, and `locked` says whether one was found (silence and DC
free-run on the newest samples instead of blanking). The alignment is the
server core's own — the same one the GUI host's `scope` widget draws with — so a
trace you draw and one the host draws from the same bus are the same trace.

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

A waveform is never drawn sample by sample. Reduce the samples once, then read
one column per pixel:

```js
const peaks = data.Peaks.build(samples, { channels: buffer.channels });
const { min, max } = data.joinColumns(peaks.columns(0, { width: canvas.width }));

for (let x = 0; x < min.length; x++) {
    ctx.moveTo(x, mid - max[x] * scale);
    ctx.lineTo(x, mid - min[x] * scale);
}
```

`columns` picks the resolution level the span and the width imply and reads the
whole row in one crossing into wasm — the work is proportional to the width of
your view, not to the length of the buffer. Zooming is the same call with a
narrower span:

```js
peaks.columns(0, { width: canvas.width, start: 44100, end: 88200 });
```

**`joinColumns` is what turns the measurement into the picture**, and a page
that strokes its own columns wants it. A column measures a *group* of samples,
and the groups partition the samples where the curve does not: between the last
sample of one column and the first of the next there is a segment nothing draws.
On ordinary audio it never shows, since consecutive columns already overlap. On
a one-sample jump — a square wave, a gate, an edge of any kind — it is the whole
of the feature, and the vertical stroke that *is* the edge comes and goes as you
zoom, because whether the jump lands inside a column or on its boundary is a
fact about the magnification rather than about the signal. `joinColumns`
extends each column to meet the one before it, which inks exactly the values the
curve takes while it crosses the boundary and leaves overlapping columns
untouched. It is the core's own rule, the one the GUI host's renderer draws
with, so a page and a `waveform` widget over one buffer draw the same picture.

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
    const row = stream.peaks(take).columns(0, { width: canvas.width });
    const { min, max } = data.joinColumns(row);
    draw(min, max, stream.written(take));   // draw only as far as it was written
});

new Synth("record_something", { buf: take.bufnum }, { server });
```

Each take's pyramid is allocated at the buffer's **full length** and empty, so
the axis does not move while it fills; `written` is how far the reports have
got, and past it the pyramid is the silence the buffer is — draw up to it and
the two read apart, which is what the GUI host's `fills` prop does for a
host-drawn view. `stop()` cancels the subscription and `free()` releases the
pyramids.

Two limits worth knowing, both of them the wire's rather than this class's. The
summary is the resolution: zoomed inside one bucket the picture is that bucket,
so a script drawing its own canvas reads the take back with `getSamples` for the
span it wants to show in detail. (A **host-drawn** view does that for itself —
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
data.spectrumDb(window, { fftSize: 1024 });     // one frame, dB per bin
```

`spectrumDb` returns `fftSize / 2` bins; bin `b` sits at
`b * sampleRate / fftSize` hertz, a full-scale sine reads about 0 dB at its own
bin, and silence sits at the -120 dB reference floor. It is one frame, with no
memory: the exponential averaging and the peak hold a spectrum display shows
across frames are smoothing — a look, not a measurement — and belong to
whatever draws.

## One implementation of every figure

Nothing here computes a number in TypeScript. Every figure — the trigger
alignment, the decibel curve, the peak columns, the correlation — comes out of
the same `clausters-core` function the GUI host calls. A canvas your script
feeds and a widget the host draws from the same bus or buffer therefore
show the same picture, which is why both may read these paths.
