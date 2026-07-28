// The data paths: what the script reads off the server, and what it computes
// from it.
//
// The GUI host reads these paths itself — that is why a GuiDef naming a bus, a
// tap or a URL draws without a line of script. This module is the same three
// paths opened to the **script**, for a view the page draws on its own canvas:
//
// - `BusStream` — control buses, streamed (`/c_stream`): meters, read-outs,
//   control-rate traces.
// - `TapStream` (+ `scopeWindow`) — audio taps, streamed (`/tap_stream`):
//   oscilloscopes, phasescopes, spectra.
// - `Peaks`, `fetchAudio` and `Server.getSamples` — bulk buffer samples and
//   their reduction: waveforms, audio-editor views.
//
// Plus `analysis`, the measurements those views are drawn from. Every number
// in here comes out of `clausters-core`, so what the page draws and what the
// host draws from the same source are the same picture.

export { BusStream, STREAM_PERIOD_MS } from "./buses.ts";
export {
    TapStream,
    decodeSamples,
    scopeFrames,
    scopeWindow,
} from "./taps.ts";
export type { ScopeTrace, TapWindow } from "./taps.ts";
export { Peaks } from "./peaks.ts";
export type { Columns } from "./peaks.ts";
export { correlation, lissajous, spectrumDb } from "./analysis.ts";
export type { WindowShape } from "./analysis.ts";
export { deinterleave, fetchAudio, interleave } from "./samples.ts";
