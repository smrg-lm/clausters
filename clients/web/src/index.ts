// Clausters in the browser — the package surface.
//
// The audio server compiled to wasm inside an AudioWorklet, the GUI host on a
// canvas, and web components that boot native-format standalone bundles — no
// server process anywhere. On top of that runtime sits the TypeScript client
// (clients/web/PLAN.md): the `Connection` seam (`base/connection.ts`) carries
// OSC over either carrier — the in-page engine or a `--ws` server over
// WebSocket — and `defs/` builds and drives what runs on it (`Server`, the
// two def families, nodes, buses and buffers), naming no transport.
//
// The GUI host is driven the same way: `GuiHost` (the `gui` namespace holds
// the GuiDef builders — `gui.window`, `gui.knob`, …) sits on that same
// connection seam, over the in-page host or a `--ws` one.
//
// The sequencing layer sits above both: a `TempoClock` resumes generator
// routines on musical time, and `seq` holds the events, patterns and
// timelines that ride on it.
//
// Importing this module registers the `<clausters-bundle>` and
// `<clausters-power>` custom elements as a side effect; the singletons stay
// lazy until first used.

export { server } from "./engine/server.ts";
export type { ClaustersServer, ReplyListener } from "./engine/server.ts";
export type { BootOptions, ClockAnchor } from "./engine/loader.ts";
export { GuiHost, guiHost, pageGuiConnection } from "./gui/host.ts";
export type { ClaustersGui } from "./gui/host.ts";
export * as gui from "./gui/index.ts";
export { bootBundle, openBundle, startBundle } from "./bundle.ts";
export type { BundleManifest, MountOptions, Mounted, ParamSpec } from "./bundle.ts";
export { ClaustersBundle, ClaustersPower, defineComponent, startPage } from "./elements.ts";
export { pagePools } from "./base/pool.ts";
export type { Pool, Pools } from "./base/pool.ts";
export {
    decodePacket,
    encodeBundle,
    encodeImmediateBundle,
    encodeMessage,
    loadOsc,
} from "./base/osc.ts";
export type { BundleMessage, OscArg, OscMessage } from "./base/osc.ts";
export {
    TempoClock,
    defaultTicker,
    manualTicker,
    timerTicker,
    workerTicker,
} from "./base/clock.ts";
export type { ManualTicker, Schedulable, TempoClockOptions, Ticker } from "./base/clock.ts";
export {
    ManualTimebase,
    MonotonicTimebase,
    SampleTimebase,
} from "./base/timebase.ts";
export type { Timebase } from "./base/timebase.ts";
export { FunctionStream, Routine, StopStream, Stream } from "./base/stream.ts";
export type { RoutineFunc, RoutineState } from "./base/stream.ts";
export { currentRoutine } from "./base/context.ts";
export { Rng, choice, currentRng, seed, spawnRng, uniform } from "./base/rand.ts";
export * as builtins from "./base/builtins.ts";
export * as seq from "./seq/index.ts";
export { loadCore } from "./base/core.ts";
export { WsConnection, pageConnection } from "./base/connection.ts";
export type { Connection, SampleClock } from "./base/connection.ts";
export * from "./defs/index.ts";
export {
    AllocationError,
    ClaustersError,
    CommandError,
    ReplyTimeout,
} from "./errors.ts";
