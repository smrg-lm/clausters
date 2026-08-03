// UGen graph as composable, lowercase callables (mirrors
// `clausters/defs/ugens/`, adapted to TypeScript).
//
// Each function here is a small **lowercase** callable returning a `Ugen` node
// (one output); composing nodes with these functions and the nodes' math
// methods builds the graph a `SynthDef` serializes into the JSON
// `SynthDefSpec` the server's `/def_send synth` consumes — the same JSON the Python
// builders emit, which the parity vectors in `tests/` hold.
//
// **Composition is by method, not by operator.** TypeScript has no operator
// overloading, so where the Python client writes `sine(freq) * amp` this one
// writes `sine(freq).mul(amp)`, and every other operator or math method
// (`mod`, `min`/`max`, comparisons, `.sin()`, `.midicps()`, `.distort()` …)
// is a method carrying the same operator **name** the wire uses — so the two
// clients emit identical specs. The free `add`/`sub`/`mul`/`div` functions
// take the number-on-the-left case (`sub(1, sig)`), which a method cannot.
//
// **Instance-based, no global build context**: the graph *is* the tree of
// composed objects, so several defs build concurrently.
//
// **Multichannel is an explicit container**, not implicit expansion: `dup`
// fans a signal out into a `ChannelList`, the math methods broadcast/zip over
// it (wrapping the shorter side modulo), `out(bus, chans)` lays the channels
// on consecutive buses, and `mix` folds a list back to one channel through
// the fused sums. Per-argument expansion (`sine(chans(440, 443))`) is
// deliberately **not** implemented — a channel list reaching a single-channel
// input is a type error, and a `TypeError` at serialization.
//
// Reserved controls `in` and `out` (the input/output buses, set with
// `/synth_new … "in" b "out" b`) are added by the server, not declared here.
//
// **Where things live.** The callables are grouped by family, one module each
// — the same split the Python package makes: `graph` (the node, control and
// channel-list types, plus the fused arithmetic), `osc`, `filter` (filters,
// delays, smoothers), `pan`, `io` (buses, replies, disk, feedback), `buf`,
// `spectral`, `trig`, `demand` and `env`. Every name is re-exported here, so
// importing from `defs/ugens` — and the `defs` package's own re-export — is
// what it always was: the split is navigational.
//
// The per-bin expressions `pvKernel` takes are a module of their own,
// `defs/pv_expr`, exactly where the Python client keeps them.

export {
    ChannelList,
    Control,
    SynthExpr,
    SynthLeaf,
    Ugen,
    add,
    chans,
    control,
    div,
    dup,
    madd,
    mix,
    mul,
    sub,
    sum3,
    sum4,
} from "./graph.ts";
export type { Channel, ControlRate, OpOperand, OpResult, UgenRate } from "./graph.ts";

export {
    brownNoise,
    clipNoise,
    crackle,
    dust,
    dust2,
    grayNoise,
    impulse,
    lfClipNoise,
    lfNoise0,
    lfNoise1,
    lfNoise2,
    lfPulse,
    lfSaw,
    lfTri,
    phasor,
    pinkNoise,
    pulse,
    saw,
    sine,
    varSaw,
    whiteNoise,
} from "./osc.ts";

export {
    allpassC,
    allpassL,
    allpassN,
    bpf,
    brf,
    combC,
    combL,
    combN,
    delayC,
    delayL,
    delayN,
    hpf,
    integrator,
    lag,
    leakDc,
    lpf,
    onePole,
    oneZero,
    resonz,
    rhpf,
    rlpf,
    svf,
    svfMorph,
    varLag,
} from "./filter.ts";
export type { Resonance } from "./filter.ts";

export {
    balance2,
    linPan2,
    linXfade2,
    midSide,
    pan2,
    panAz,
    rotate2,
    select,
    selectX,
    splay,
    stereoWidth,
    xfade2,
} from "./pan.ts";

export {
    diskIn,
    diskOut,
    inCtl,
    in_,
    localIn,
    localOut,
    out,
    outCtl,
    poll,
    replaceOut,
    sendReply,
    sendTrig,
} from "./io.ts";

export {
    bufChannels,
    bufDur,
    bufFrames,
    bufRateScale,
    bufRd,
    bufSampleRate,
    osc,
    oscN,
    playBuf,
    rand,
    sampleRate,
    shaper,
    vosc,
} from "./buf.ts";

export {
    changed,
    decay,
    decay2,
    gate,
    latch,
    pulseCount,
    pulseDivider,
    schmidt,
    setResetFf,
    stepper,
    sweep,
    tDelay,
    timer,
    toggleFf,
    trig,
    trig1,
} from "./trig.ts";

export {
    dbrown,
    dbufrd,
    demand,
    dgeom,
    dibrown,
    diwhite,
    drand,
    dseq,
    dseries,
    dshuf,
    dstutter,
    dswitch1,
    duty,
    dwhite,
    dxrand,
    tduty,
} from "./demand.ts";

export {
    conv,
    fft,
    ifft,
    partconvFrames,
    pvAdd,
    pvBinShift,
    pvBrickWall,
    pvCopyPhase,
    pvKernel,
    pvMagAbove,
    pvMagBelow,
    pvMagClip,
    pvMagFreeze,
    pvMagMul,
    pvMagShift,
    pvMagSmear,
    pvMax,
    pvMin,
    pvMul,
} from "./spectral.ts";
export type { ConvOptions, FftOptions, PvKernelOptions } from "./spectral.ts";

export {
    DoneAction,
    Env,
    detectSilence,
    done,
    envGen,
    envToPoints,
    freeSelf,
    freeSelfWhenDone,
    line,
    pauseSelf,
    pointsToEnv,
    resolveCurve,
    xLine,
} from "./env.ts";
export type { Curve } from "./env.ts";
