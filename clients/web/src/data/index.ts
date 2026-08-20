// The data paths: what the script reads off the server, and what it measures
// from it.
//
// The GUI host reads these paths itself — that is why a GuiDef naming a bus, a
// tap or a URL draws without a line of script. This module is the same paths
// opened to the **script**, for what a program does with the data besides look
// at it: a read-out, a decision, a summary it hands on, a test:
//
// - `BusStream` — control buses, streamed (`/bus_stream`): the newest value of
//   each bus, as often as asked for.
// - `TapStream` — audio taps, streamed (`/bus_tapStream`): the newest window of
//   an audio bus's samples, on that bus's own axis.
// - `Peaks`, `fetchAudio` and `Server.getSamples` — bulk buffer samples and
//   their reduction to the cache format every client reads.
// - `RecordingStream` — takes as they record (`/buffer_stream`): the overview
//   of what was written, since a page can map no memory to watch it fill.
//
// Plus `analysis`, the measurements *of the signal*. Every number in here comes
// out of `clausters-core`, so a figure a script reports and a figure the host
// draws from the same source are one number.
//
// **Nothing here draws, and nothing here computes a drawing.** A row of pixel
// columns, an oscilloscope's display window and trigger, a decibel curve: those
// belong to whoever draws, and what draws is the GUI host. A page that wants to
// see any of this names a view — `plot`, `scope`, or a `waveform`/`scope`/
// `spectrum` widget in a GuiDef — and the host reads the very same paths.

export { BusStream, STREAM_PERIOD_MS } from "./buses.ts";
export { TapStream, decodeSamples } from "./taps.ts";
export type { TapWindow } from "./taps.ts";
export { Peaks } from "./peaks.ts";
export { RECORDING_PERIOD_MS, RecordingStream } from "./recording.ts";
export type { TakeLike, TakeShape } from "./recording.ts";
export { correlation, lissajous } from "./analysis.ts";
export { deinterleave, fetchAudio, interleave } from "./samples.ts";
