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

// The submodules as namespaces, for `ugenInputNames` at the bottom: it reads
// the builders' own parameter lists, so it needs the callables themselves and
// not only their re-exported names.
import * as buf from "./buf.ts";
import * as demand from "./demand.ts";
import * as env from "./env.ts";
import * as filter from "./filter.ts";
import * as graph from "./graph.ts";
import * as io from "./io.ts";
import * as osc from "./osc.ts";
import * as pan from "./pan.ts";
import * as spectral from "./spectral.ts";
import * as trig from "./trig.ts";

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
    transportPos,
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
    bufDelayN,
    bufDelayL,
    bufDelayC,
    bufCombN,
    bufCombL,
    bufCombC,
    bufAllpassN,
    bufAllpassL,
    bufAllpassC,
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
    bufWr,
    recordBuf,
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

// ---- the inlet labels a patcher's Def view reads ----

/**
 * Kinds whose builder parameters do **not** line up with the wire's input order
 * — a variadic run, a static field sitting between two inputs — so their names
 * would mislabel the inlets and the Def view falls back to positional ones.
 * The same set the Python client declares, and the same reasons: the
 * divergences `tests/ugen-catalog.test.ts` contrasts against the server.
 */
const INPUT_NAMES_MISALIGNED = new Set([
    "EnvGen",
    "SendReply",
    "Dseq",
    "Poll",
    "DiskIn",
    "DiskOut",
    "PV_Kernel",
]);

/** Lazily built `kind -> [parameter name, …]` — see {@link ugenInputNames}. */
let INPUT_NAMES: Map<string, string[]> | null = null;

/**
 * The positional parameter names of the builder that makes each UGen kind, read
 * from the `new Ugen("Kind", …)` literal in this module's own functions (a
 * builder is not named after its kind: `in_` builds `In`, `oscN` builds `OscN`).
 *
 * The Python client reads the same thing with `inspect`; this one reads
 * `Function.prototype.toString()`, which under node's type stripping still
 * carries the parameter list — the language's own way to the one fact, not a
 * different fact. A destructured options object is not positional and stops the
 * list, since everything past it is named rather than wired in order.
 */
function buildInputNames(): Map<string, string[]> {
    const names = new Map<string, string[]>();
    const modules = [buf, demand, env, filter, graph, io, osc, pan, spectral, trig];
    for (const value of modules.flatMap((m) => Object.values(m))) {
        if (typeof value !== "function") continue;
        const src = value.toString();
        if (/^class\s/.test(src)) continue;
        const kind = /new Ugen\(\s*"([A-Za-z_0-9]+)"/.exec(src)?.[1];
        if (kind === undefined || names.has(kind)) continue;
        names.set(kind, positionalParams(src));
    }
    return names;
}

/** The positional parameter names in a function's source, in order. */
function positionalParams(src: string): string[] {
    const open = src.indexOf("(");
    if (open < 0) return [];
    let depth = 0;
    let end = -1;
    for (let i = open; i < src.length; i++) {
        const c = src[i]!;
        if ("([{".includes(c)) depth++;
        else if (")]}".includes(c) && --depth === 0) {
            end = i;
            break;
        }
    }
    if (end < 0) return [];
    const out: string[] = [];
    let d = 0;
    let start = 0;
    const inner = src.slice(open + 1, end);
    for (let i = 0; i <= inner.length; i++) {
        const c = inner[i];
        if (i === inner.length || (c === "," && d === 0)) {
            const piece = inner.slice(start, i).trim().replace(/\s+/g, " ");
            start = i + 1;
            if (!piece) continue;
            // An options object carries named fields, not a positional input.
            if (piece.startsWith("{")) break;
            const name = piece.split("=")[0]!.trim();
            if (name) out.push(name);
        } else if ("([{".includes(c!)) d++;
        else if (")]}".includes(c!)) d--;
    }
    return out;
}

/**
 * The positional input names of the builder for UGen `kind`, or `undefined`
 * when no single builder maps to it cleanly — the generic op UGens
 * (`BinaryOpUGen`/`UnaryOpUGen`, built by the nodes' own methods) and the kinds
 * whose parameters do not line up with the wire order. `undefined` means the
 * caller labels the inlets positionally.
 *
 * What reads it is the patcher's Def view ({@link DefPatch}), which captions a
 * UGen box's inlets with the names a caller of that builder types.
 */
export function ugenInputNames(kind: string): string[] | undefined {
    if (INPUT_NAMES_MISALIGNED.has(kind)) return undefined;
    INPUT_NAMES ??= buildInputNames();
    return INPUT_NAMES.get(kind);
}
