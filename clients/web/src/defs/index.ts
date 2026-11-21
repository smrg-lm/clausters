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

export { DefPatch, GraphPatch, synthdefPorts } from "./patch.ts";
export { Window } from "./window.ts";
export type {
    PatchBox,
    BoxKind,
    Compiled,
    CordRate,
    Cord,
    PatchWidget,
    Port,
    PortRate,
    PortSpec,
} from "./patch.ts";
export {
    formatLoad, formatMidiBinding, formatProfile, formatServerInfo, formatServerStatus, loadName,
    Server, Transport,
} from "./server/index.ts";
export type {
    Load,
    Profile, ProfileRow,
    MidiBinding, MidiBindOptions,
    MsgArg, ServerBootOptions, ServerInfo, ServerSizing, ServerStatus,
    TimedMessage,
    TransportGrid, TransportState,
} from "./server/index.ts";
export {
    formatBufferInfo,
    formatControlInfo,
    formatDefInfo,
    formatNodeInfo,
    formatNodeMap,
    formatUgenInfo,
    formatUgenInput,
    Tree,
} from "./info.ts";
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
export type { Anchor, ServerSampleClock } from "./clocksync.ts";

export { AddAction, Group, Node, NodeIdAllocator, ROOT_NODE_ID, Synth, Voices } from "./node.ts";
export type { Controls, NodeLike, Placement } from "./node.ts";

export { AudioBusAllocator, Bus, ControlBusAllocator, EventBusAllocator } from "./bus.ts";
export type { BusLike, BusRate } from "./bus.ts";

export { Buffer, BufferAllocator } from "./buffer.ts";
export type { BufferLike } from "./buffer.ts";

export { SynthDef } from "./synthdef.ts";
export type { ControlSpec, SpecInput, SynthDefSpec, UgenSpec } from "./synthdef.ts";

export { FaustDef } from "./faustdef.ts";
export type { FaustDefKind } from "./faustdef.ts";
export { Box } from "./boxes.ts";
export type { BoxInput, BoxNode, EvalArg, FaustOptions } from "./boxes.ts";

export { FaustExpr } from "./expr.ts";

// `Expr` is a union of the two families here and a base class in Python, so
// `isExpr` is what a TypeScript caller asks in place of `isinstance`.
export { asDef, exprChannels, isExpr } from "./asdef.ts";
export type { Expr } from "./asdef.ts";

export { GraphDef } from "./graphdef.ts";
export type { GraphBusRef, MemberRef, PortTarget } from "./graphdef.ts";
export type { GraphDefSpec, MemberControlValue, MemberKind, MemberSpec } from "./graphdef.ts";

// The UGens the Python client's `clausters.defs` names flat, by the same
// names; the whole catalogue is `ugens` (`clausters/defs/ugens`), as there.
export {
    Bpf, ChannelList, Control, DoneAction, Env, Ugen, bufAllpassC,
    bufAllpassL, bufAllpassN, bufChannels, bufCombC, bufCombL, bufCombN, bufDelayC,
    bufDelayL, bufDelayN, bufDur, bufRateScale, bufRd, bufSampleRate, bufWr, chans,
    clipCount, control, conv, dbrown, dbufrd, demand, dgeom, dibrown, diskIn,
    diskOut, diwhite, drand, dseq, dseries, dshuf, dstutter, dswitch1, dup, duty,
    dwhite, dxrand, envGen, envShape, fft, ifft, impulse, in_, inCtl, lag, lagUd, localIn,
    localOut, madd, meter, mix, osc, oscN, out, outCtl, partconvFrames, playBuf,
    poll, pvAdd, pvBinShift, pvBrickWall, pvCopyPhase, pvKernel, pvMagAbove,
    pvMagBelow, pvMagClip, pvMagFreeze, pvMagMul, pvMagShift, pvMagSmear, pvMax,
    pvMin, pvMul, rand, recordBuf, replaceOut, sampleRate, sendReply, sendTrig,
    shaper, sine, sum3, sum4, tduty, transportBufRd, transportDiskIn, transportFade,
    transportPos, truePeak, varLag, vosc, whiteNoise,
} from "./ugens/index.ts";
export type * from "./ugens/index.ts";
export * as ugens from "./ugens/index.ts";
export { SynthExpr } from "./ugens/graph.ts";
export * as signals from "./signals.ts";
export { Signal } from "./signals.ts";
export { IdSpaces } from "../base/core.ts";
export * as boxes from "./boxes.ts";
export * as pvExpr from "./pv_expr.ts";

export { Plugin, PluginCatalog, PluginDef } from "./plugindef.ts";
export type { PluginQuery, PluginReference, PluginPortReference, PluginRoute } from "./plugindef.ts";
