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
// The data paths (`data/`) are the reading direction of that same seam: the
// control buses, the audio taps and the buffer samples a view feeds on,
// streamed or fetched, plus the core's own measurements to draw them with.
//
// The sequencing layer sits above both: a `TempoClock` resumes generator
// routines on musical time, and `seq` holds the events, patterns and
// timelines that ride on it.
//
// A `Session` bundles one server, one clock and one GUI host into the handle
// a piece is written against, and the **default session** (`defaultSession`)
// is the ambient one everything falls back to — which is what lets `play(...)`
// and a bare `new Synth(...)` name no server at all.
//
// What this module exports flat is what you name while writing a piece: the
// hosts (`Server`, `GuiHost`), the server's resources, the three def formats,
// the timing types and the verbs. Everything enumerative — the UGen and signal
// callables, the value patterns, the GUI builders — is named through its
// namespace (`defs.sine`, `seq.Pbind`, `gui.knob`), the same criterion the
// Python client applies: there are too many of them for a flat namespace to
// stay readable.
//
// Importing this module registers the `<clausters-bundle>` and
// `<clausters-power>` custom elements as a side effect; the singletons stay
// lazy until first used.

export { server, engine, ANY_PEER, DEFAULT_PEER } from "./engine/server.ts";
export type { ClaustersServer, ReplyListener } from "./engine/server.ts";
export type { BootOptions, ClockAnchor } from "./engine/loader.ts";
export { GuiHost, guiHost, newGuiHost, pageGuiConnection } from "./gui/host.ts";
// Measuring an element against the display, which a page needs wherever it
// sizes a canvas itself -- a component, an embedder's own element.
export { canvasBox, onScaleChange } from "./gui/page.ts";
export type { CanvasBox } from "./gui/page.ts";
export type { ClaustersGui } from "./gui/host.ts";
export type { GuiHostOptions, GuiTransportName } from "./gui/host.ts";
export * as gui from "./gui/index.ts";
export { bootBundle, freeBundle, openBundle, startBundle } from "./bundle.ts";
export type { BundleManifest, MountOptions, Mounted, ParamSpec } from "./bundle.ts";
// Authoring one, the other direction: `write` is a node verb, `files` is the
// same bundle in memory for a page to mount. Its own subpath as well
// (`clausters/bundle-writer`), because node cannot import this module — the
// custom elements it registers need a document.
export { Bundle, DEFAULT_RUNTIME } from "./bundle-writer.ts";
export type { Hole, ParamOptions, ParamType, WritableDef, WriteOptions } from "./bundle-writer.ts";
export { ClaustersBundle, ClaustersPower, defineComponent, startPage } from "./elements.ts";
export { Session } from "./session.ts";
export type { SessionOptions } from "./session.ts";
export { defaultSession, main } from "./base/main.ts";
export type { Main, SessionLike } from "./base/main.ts";
export { Environment, RandomContext } from "./base/environment.ts";
// The operator vocabulary as methods, exported the way the Python client
// exports `AbstractObject` from `clausters.base`: what a subclass implements
// to make one written expression compose a graph, a per-bin program or a value.
export { AbstractObject } from "./base/absobject.ts";
export type { Composed, Fan } from "./base/absobject.ts";
export { play } from "./play.ts";
export type { Playable, PlayOptions } from "./play.ts";
export { plot, PatchWindow, PlotWindow } from "./plot.ts";
export type { PatchViewOptions } from "./plot.ts";
export type { Plottable, PlotOptions } from "./plot.ts";
export { scope, ScopeWindow } from "./scope.ts";
export type { ScopeOptions, ScopeView } from "./scope.ts";
export { bounceDef, channel, render, renderScore, wavBytes } from "./render.ts";
export type {
    RenderOptions,
    RenderStats,
    RenderVerbOptions,
    Renderable,
} from "./render.ts";
export { loadRenderer } from "./engine/render.ts";
export type { EngineModule } from "./engine/render.ts";
export { newPools, pagePools } from "./base/pool.ts";
export type { Pool, Pools } from "./base/pool.ts";
export {
    decodePacket,
    decodePacketTimed,
    encodeBundle,
    encodeImmediateBundle,
    encodeMessage,
    loadOsc,
} from "./base/osc.ts";
export type { BundleMessage, OscArg, OscMessage, TimedOscMessage } from "./base/osc.ts";
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
    SampleClockTimebase,
} from "./base/timebase.ts";
export type { Timebase } from "./base/timebase.ts";
export { FunctionStream, Routine, StopStream, Stream, YieldAndReset } from "./base/stream.ts";
export type { RoutineFunc, RoutineState } from "./base/stream.ts";
export { currentRoutine } from "./base/context.ts";
export { Moment } from "./base/moment.ts";
export { OscDestination } from "./base/destination.ts";
export type { Destination } from "./base/destination.ts";
export {
    MidiFunc,
    OscFunc,
    defaultMidiReceiver,
    defaultOscReceiver,
    midifunc,
    oscfunc,
    setDefaultMidiReceiver,
    setDefaultOscReceiver,
} from "./responders.ts";
export type {
    ArgMatcher,
    MidiCallback,
    MidiMatcher,
    OscCallback,
    OscValue,
    ResponderMessage,
} from "./responders.ts";
export { OscReceiver } from "./base/receiver.ts";
export type { OscHandler } from "./base/receiver.ts";
// MIDI: the ports, the score, the destination and the receiving door. The
// reference client keeps these in `clausters.base`; the flat facade is this
// client's spelling of the same names.
export {
    MidiNrtInterface,
    MidiReceiver,
    MidiRtInterface,
    MidiScore,
    MidiServer,
    parseMidi,
    requestMidiPorts,
} from "./base/midi.ts";
export type {
    MidiHandler,
    MidiInputPort,
    MidiInterface,
    MidiMessage,
    MidiOutputPort,
    MidiPortOptions,
    MidiPorts,
    MidiReceiverOptions,
    MidiServerOptions,
} from "./base/midi.ts";
export { Rng, choice, currentRng, seed, spawnRng, uniform } from "./base/rand.ts";
export * as builtins from "./base/builtins.ts";
export * as seq from "./seq/index.ts";
// The names a piece types without a namespace, as the Python facade exports
// them; the enumerative half (the value patterns) stays behind `seq`.
export { Event, rest } from "./seq/event.ts";
export { MidiEvent, Playhead, Timeline } from "./seq/timeline.ts";
export * as data from "./data/index.ts";
export { loadCore } from "./base/core.ts";
// The id share every constructor that allocates ids accepts, for the same
// reason the options bags below are exported: a public signature names it.
export type { IdShare } from "./base/ids.ts";
export { WHOLE_SHARE, shareOf } from "./base/ids.ts";
export { Score, ScoreConnection, WsConnection, pageConnection } from "./base/connection.ts";
export type { Connection, SampleClock } from "./base/connection.ts";
export * as defs from "./defs/index.ts";
export { Server } from "./defs/server/index.ts";
// The records the transport surface reports, for the same reason: a public
// signature that names a type the reference cannot reach is a broken page.
export type { TransportGrid, TransportState } from "./defs/server/index.ts";
export type { ServerOptions, ServerTransportName } from "./defs/server/index.ts";
export { AddAction, Group, Node, Synth } from "./defs/node.ts";
// The options bags the node constructors take. Exported as types so the API
// reference documents what those parameters accept: TypeDoc reports a public
// signature that names a type it cannot reach, and an intersection like
// GroupOptions is one it cannot inline away.
export type { Controls, GroupOptions, NodeLike, Placement } from "./defs/node.ts";
export { Bus } from "./defs/bus.ts";
export { Buffer } from "./defs/buffer.ts";
export type { BufferOptions } from "./defs/buffer.ts";
export { SynthDef } from "./defs/synthdef.ts";
export { FaustDef } from "./defs/faustdef.ts";
export { GraphDef } from "./defs/graphdef.ts";
export * as errors from "./errors.ts";
export { ClaustersError } from "./errors.ts";

/**
 * The **arrangement**: a recursive algebra of elements placed in time, grouped
 * and rendered — what a multitrack editor edits. See `./form/index.ts`.
 */
export * as form from "./form/index.ts";

/**
 * The document: the composition's authoritative model, applied by the shared
 * crate rather than by any client. See `./document.ts`.
 */
export * as document from "./document.ts";
export { Log, applyIntent, resolveSelection } from "./document.ts";
export type {
    Against,
    Applied,
    ClaustersDocument,
    Intent,
    NodeId,
    Outcome,
    Redone,
    Resolved,
    Selection,
    Step,
    Undone,
} from "./document.ts";

/**
 * The page's own filesystem (OPFS) — where a soundfile a tab reads actually
 * lives.
 *
 * Not a client verb and not part of the surface the two clients share: it is
 * the platform's answer to "where is the file", the way a disk is a native
 * server's. `/buffer_allocRead "take.wav"` names the server's filesystem in a
 * window and this one in a tab, and the call is the same call either way.
 */
export * as opfs from "./engine/opfs.ts";
