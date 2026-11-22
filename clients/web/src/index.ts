// Clausters in the browser -- the package surface.
//
// The audio server compiled to wasm inside an AudioWorklet, the GUI host on a
// canvas, and web components that boot native-format standalone bundles -- no
// server process anywhere. On top of that runtime sits the TypeScript client
// (clients/web/PLAN.md): the `Connection` seam (`base/connection.ts`) carries
// OSC over either carrier -- the in-page engine or a `--ws` server over
// WebSocket -- and `defs/` builds and drives what runs on it (`Server`, the
// two def families, nodes, buses and buffers), naming no transport.
//
// The GUI host is driven the same way: `GuiHost` (the `gui` namespace holds
// the GuiDef builders -- `gui.window`, `gui.knob`, ...) sits on that same
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
// a session is written against, and the **default session** (`defaultSession`)
// is the ambient one everything falls back to -- which is what lets `play(...)`
// and a bare `new Synth(...)` name no server at all.
//
// What this module exports is the Python client's `clausters`, name for name
// (`tests/names.test.ts` holds the two together): flat, what you name while
// playing -- the hosts (`Server`, `GuiHost`), the server's resources, the
// three def formats, the timing types and the verbs; through a namespace,
// everything enumerative -- the UGen and signal callables, the value
// patterns, the GUI builders (`defs.sine`, `seq.pbind`, `gui.knob`) -- and the
// layers beneath (`base`). What only a page has is `clausters/page`, as what
// only a native process has is the Python client's `ipc` and `launch`.
//
// Importing this module in a page registers the `<clausters-bundle>` and
// `<clausters-power>` custom elements as a side effect; where there is no page
// (a worker, node) it registers nothing and imports all the same. The
// singletons stay lazy until first used.

import "./elements.ts";

// the layers, for the names too many to spell out flat
export * as base from "./base/index.ts";
export * as builtins from "./base/builtins.ts";
export * as data from "./data/index.ts";
export * as defs from "./defs/index.ts";
export * as document from "./document.ts";
export * as errors from "./errors.ts";
export * as form from "./form/index.ts";
export * as gui from "./gui/index.ts";
export * as log from "./log.ts";
export * as notation from "./notation/index.ts";
export * as page from "./page.ts";
export * as segments from "./segments.ts";
export * as seq from "./seq/index.ts";
// the hosts
export { Session } from "./session.ts";
export type { SessionOptions } from "./session.ts";
export { Server } from "./defs/server/index.ts";
export type { ServerOptions, ServerTransportName } from "./defs/server/index.ts";
export { GuiHost } from "./gui/host.ts";
export type { GuiHostOptions, GuiTransportName } from "./gui/host.ts";
export { defaultSession, main } from "./base/main.ts";
export type { Main, SessionLike } from "./base/main.ts";
// the server's resources: nodes, definitions, buses, buffers
export { AddAction, Group, Synth, Voices } from "./defs/node.ts";
export type { Controls, GroupOptions, NodeLike, Placement } from "./defs/node.ts";
export { SynthDef } from "./defs/synthdef.ts";
export { FaustDef } from "./defs/faustdef.ts";
export { Plugin, PluginCatalog, PluginDef } from "./defs/plugindef.ts";
export type { PluginQuery, PluginReference, PluginPortReference, PluginRoute } from "./defs/plugindef.ts";
export { GraphDef } from "./defs/graphdef.ts";
export { Bus } from "./defs/bus.ts";
export { Buffer } from "./defs/buffer.ts";
export type { BufferOptions } from "./defs/buffer.ts";
// time
export { TempoClock } from "./base/clock.ts";
export type { Schedulable, SetTempoOptions, TempoClockOptions } from "./base/clock.ts";
// A generated wasm class grows no method, so the map's `env` and `segments`
// are functions taking it (`docs/bindings.md`, "The client surface").
export {
    TempoMap,
    bar,
    beatInBar,
    quantDelay,
    samplesToSecs,
    secsToSamples,
    tempoEnv,
    tempoSegments } from "./base/time.ts";
export type { CurveSpec, TempoChange, TempoEnvelope, TimeUnit } from "./base/time.ts";
export { Routine } from "./base/stream.ts";
export type { RoutineFunc, RoutineState } from "./base/stream.ts";
export { Event, rest } from "./seq/event.ts";
export { Timeline } from "./seq/timeline.ts";
export { EventSequence } from "./seq/sequence.ts";
export type { Writing } from "./seq/sequence.ts";
// the free verbs
export { play } from "./play.ts";
export type { Playable, PlayOptions } from "./play.ts";
export { panic } from "./panic.ts";
export { render } from "./render.ts";
export type {
    RenderOptions,
    RenderStats,
    RenderVerbOptions,
    Renderable } from "./render.ts";
export { plot } from "./plot.ts";
// The windows the verbs answer, named for their signatures: the classes are
// `clausters/plot`'s and `clausters/scope`'s, as in the Python client.
export type { PatchViewOptions, PatchWindow, PlotWindow, Plottable, PlotOptions } from "./plot.ts";
export { scope } from "./scope.ts";
export type { ScopeOptions, ScopeView, ScopeWindow } from "./scope.ts";
// the editing verbs: a structure's view in an editor of its kind, and an
// application with its chrome -- `clausters/gui`'s, as in the Python client
export { edit, openApp } from "./gui/editing/edit.ts";
// audio files on the client side
export { SoundFile } from "./soundfile.ts";
export type { SoundFileFormat } from "./soundfile.ts";
// incoming OSC and MIDI
export { MidiFunc, OscFunc, midifunc, oscfunc } from "./responders.ts";
export type {
    ArgMatcher,
    MidiCallback,
    MidiMatcher,
    OscCallback,
    OscValue,
    ResponderMessage } from "./responders.ts";
// the random context (one seedable source: `seed(n)` on the default session,
// or `session.seed(n)` on a named one, and these draws from whichever stream
// the context says, named as sclang names them)
export {
    Rng, betarand, bilinrand, cauchy, choose, coin, currentRng, exprand, gauss, hprand, irand,
    irand2, irrand, linrand, lprand, meanrand, poisson, rand, rand2, rrand, scramble, seed,
    spawnRng, sputter, sum3rand, tablerand, wchoose,
} from "./base/rand.ts";
// the root of every error this package raises; the leaves are in `errors`
export { ClaustersError } from "./errors.ts";
