// The def model and the server client (mirrors `clausters/defs/__init__.py`).
//
// Two def families, peers: UGen-graph `SynthDef`s built with the lowercase
// callables in `./ugens.ts`, and `FaustDef`s built with `./signals.ts` (or
// from Faust source, or from a box tree). A `GraphDef` wires several of
// either into one named, instantiable configuration. `Server` is the only
// object that knows a connection.

export { Server } from "./server.ts";
export type {
    BufferInfo,
    ControlInfo,
    DefInfo,
    MsgArg,
    NodeInfo,
    PortTargetInfo,
    ServerInfo,
    ServerSizing,
    TimedMessage,
    TreeNode,
    UgenInfo,
    UgenInput,
} from "./server.ts";
export {
    DEFAULT_AUDIO_BUSES,
    DEFAULT_CONTROL_BUSES,
    DEFAULT_MAX_BUFFERS,
    DEFAULT_MAX_NODES,
    DEFAULT_SAMPLE_RATE,
} from "./server.ts";

export { AddAction, Group, Node, NodeIdAllocator, ROOT_NODE_ID, Synth } from "./node.ts";
export type { Controls, NodeLike, Placement } from "./node.ts";

export { AudioBusAllocator, Bus, ControlBusAllocator } from "./bus.ts";
export type { BusLike, BusRate } from "./bus.ts";

export { Buffer, BufferAllocator, NUM_BUFFERS } from "./buffer.ts";
export type { BufferLike } from "./buffer.ts";

export { DEFAULT_TAPS } from "./tap.ts";

export { SynthDef } from "./synthdef.ts";
export type { ControlSpec, SpecInput, SynthDefSpec, UgenSpec } from "./synthdef.ts";

export { FaustDef } from "./faustdef.ts";
export type { FaustDefKind } from "./faustdef.ts";

export { GraphBusRef, GraphDef, MemberRef, PortTarget } from "./graphdef.ts";
export type { GraphDefSpec, MemberControlValue, MemberSpec } from "./graphdef.ts";

export * from "./ugens.ts";
export * as signals from "./signals.ts";
