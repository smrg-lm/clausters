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
    ddup, dconst, dreset, dbufwr, demandEnvGen, dser, dwrand, dswitch, dpoll, checkBadValues,
    sanitize,
    grainSin, grainFm, grainBuf, grainIn, tGrains, warp1, grainTap,
    freeVerb, freeVerb2, gverb, spring, ball, tball, pan4, panB, panB2, bipanB2, decodeB2,
    pluck, pitchShift, delTapWr, delTapRd, scopeOut, localBuf, maxLocalBufs, setBuf, clearBuf, vdiskIn,
    Bpf, ChannelList, Control, DoneAction, Env, Ugen, bufAllpassC,
    bufAllpassL, bufAllpassN, bufChannels, bufCombC, bufCombL, bufCombN, bufDelayC,
    bufDelayL, bufDelayN, bufDur, bufRateScale, bufRd, bufSampleRate, bufWr, chans,
    clipCount, control, conv, dbrown, dbufrd, demand, dgeom, dibrown, diskIn,
    diskOut, diwhite, drand, dseq, dseries, dshuf, dstutter, dswitch1, dup, duty,
    dwhite, dxrand, envGen, envShape, fft, ifft, impulse, in_, inCtl, lag, lagUd, delay1, delay2, lpz1, hpz1, lpz2, hpz2, bpz2, brz2, slope, flip, blockSize, controlRate, controlDur, sampleDur, radiansPerSample, subsampleOffset, numAudioBuses, numControlBuses, numInputBuses, numOutputBuses, numBuffers, nodeId, numRunningSynths, bufSamples, inFeedback, inTrig, lagIn, sharedIn, sharedOut, offsetOut, xOut, free, pause, peak, runningMax, runningMin, peakFollower, runningSum, lastValue, mostChange, leastChange, zeroCrossing, pauseSelfWhenDone, sendPeakRms, median, moogFf, amplitude, compander, normalizer, limiter, hilbert, freqShift, cuspN, cuspL, gbmanN, gbmanL, henonN, henonL, henonC, latoocarfianN, latoocarfianL, latoocarfianC, linCongN, linCongL, linCongC, lorenzL, quadN, quadL, quadC, standardN, standardL, fbSineN, fbSineL, fbSineC, gendy1, gendy2, gendy3, lfdNoise0, lfdNoise1, lfdNoise3, lfdClipNoise, logistic, hasher, mantissaMask, index, indexL, wrapIndex, foldIndex, indexInBetween, detectIndex, degreeToKey, tWindex, fsinOsc, sinOscFb, psinGrain, blip, formant, klang, klank, vosc3, cosc, lfCub, lfPar, syncSaw, vibrato, linExp, inRange, inRect, modDif, unwrap, ampComp, ampCompA, k2a, a2k, t2a, t2k, dc, ienvGen, linen, lfGauss, twoZero, fos, sos, ramp, slew, twoPole, apf, ringz, formlet, midEq, bLowPass, bHiPass, bAllPass, bBandPass, bBandStop, bPeakEq, bLowShelf, bHiShelf, lag2, lag3, lag2Ud, lag3Ud, localIn,
    localOut, madd, meter, mix, osc, oscN, out, outCtl, partconvFrames, playBuf,
    fftTrigger, packFft, partConv, stereoConvolution2L,
    mfcc, beatTrack, beatTrack2, pitch, specCentroid, specFlatness, specPcile, loudness, keyTrack, onsets, pvHainsworthFoote, pvJensenAndersen, pvMagSquared, pvMagNoise, pvConj, pvPhaseShift90, pvPhaseShift270, pvPhaseShift, pvDiv, pvMagDiv, pvBinWipe, pvRectComb, pvRectComb2, pvRandComb, pvRandWipe, pvBinScramble, pvDiffuser, pvLocalMax, pvConformalMap, pvCopy, unpack1Fft, convolution, convolution2, convolution2L, convolution3,
    poll, pvAdd, pvBinShift, pvBrickWall, pvCopyPhase, pvKernel, pvMagAbove,
    pvMagBelow, pvMagClip, pvMagFreeze, pvMagMul, pvMagShift, pvMagSmear, pvMax,
    pvMin, pvMul, rand, irand, expRand, linRand, nRand, tRand, tiRand, tExpRand, coinGate, randId, randSeed, recordBuf, replaceOut, sampleRate, sendReply, sendTrig,
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
