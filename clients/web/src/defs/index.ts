// The def model and the server client (mirrors `clausters/defs/__init__.py`).
//
// Two def families, peers: UGen-graph `SynthDef`s built with the lowercase
// callables in `./ugens/`, and `FaustDef`s built with `./signals.ts` (or
// from Faust source, or from a box tree). A `GraphDef` wires several of
// either into one named, instantiable configuration. `Server` is the only
// object that knows a connection.
//
// The two catalogues are packages rather than files, one module per family,
// the way the Python client splits them: `./ugens/` by UGen family and
// `./server/` by what a call does (the handle itself, the configuration, the
// queries, the subscriptions).

export { GraphPatch, patchToWidget, synthdefPorts } from "./patch.ts";
export type { Box, Compiled, Cord, PatchWidget, Port, PortRate, PortSpec } from "./patch.ts";
export { Server } from "./server/index.ts";
export type {
    MsgArg, ServerInfo, ServerSizing, TimedMessage, TransportGrid, TransportState,
} from "./server/index.ts";
export { Tree } from "./info.ts";
export type {
    BufferInfo,
    ControlInfo,
    DefInfo,
    NodeInfo,
    NodeMap,
    PortTargetInfo,
    UgenInfo,
    UgenInput,
} from "./info.ts";
export {
    DEFAULT_AUDIO_BUSES,
    DEFAULT_CONTROL_BUSES,
    DEFAULT_MAX_BUFFERS,
    DEFAULT_MAX_NODES,
    DEFAULT_SAMPLE_RATE,
    DEFAULT_TAPS,
} from "./server/index.ts";

export { EmbedSampleClock, WsSampleClock, sampleClockFor } from "./clocksync.ts";
export type { Anchor, ServerSampleClock } from "./clocksync.ts";

export { AddAction, Group, Node, NodeIdAllocator, ROOT_NODE_ID, Synth } from "./node.ts";
export type { Controls, NodeLike, Placement } from "./node.ts";

export { AudioBusAllocator, Bus, ControlBusAllocator } from "./bus.ts";
export type { BusLike, BusRate } from "./bus.ts";

export { Buffer, BufferAllocator, NUM_BUFFERS } from "./buffer.ts";
export type { BufferLike } from "./buffer.ts";

export { SynthDef } from "./synthdef.ts";
export type { ControlSpec, SpecInput, SynthDefSpec, UgenSpec } from "./synthdef.ts";

export { FaustDef } from "./faustdef.ts";
export type { FaustDefKind } from "./faustdef.ts";

export { asDef, exprChannels, isExpr } from "./asdef.ts";
export type { Expr } from "./asdef.ts";

export { GraphBusRef, GraphDef, MemberRef, PortTarget } from "./graphdef.ts";
export type { GraphDefSpec, MemberControlValue, MemberSpec } from "./graphdef.ts";

export * from "./ugens/index.ts";
export * as signals from "./signals.ts";
export * as pvExpr from "./pv_expr.ts";
