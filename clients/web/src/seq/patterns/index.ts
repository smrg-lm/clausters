// The patterns, by family (mirrors `clausters/seq/patterns/__init__.py`).
//
// Each module is one family, and each pattern says what it yields and which
// kind it is -- a **value pattern** (its stream yields values) or an **event
// pattern** (its stream yields `Event`s, and it plays) -- or, for a list or
// filter pattern, how it decides when it is built (an event pattern when what
// it holds is):
//
// - `valuepatterns` -- values computed from the arguments: a constant, a value
//   once, an arithmetic or geometric series.
// - `randompatterns` -- random values, drawn from the random context, so one
//   seed reproduces them with everything else in the script: uniform,
//   exponential, toward an end or the middle, beta, Cauchy, Gaussian, Poisson,
//   a random walk, a drawn probability shape.
// - `listpatterns` -- values read from a list: in order, cycled, chosen at
//   random (never twice in a row, or weighted), shuffled, a sliding window, a
//   walk, laced, switched by an index, or driven by a state machine.
// - `filterpatterns` -- a pattern made of another: repeated, gated, mapped,
//   filtered, cut, stuttered, latched, clumped or flattened, differenced,
//   prorated, read through a raga's two scales, delayed, cut to a duration,
//   traced.
// - `funcpatterns` -- values a function computes when the walk asks.
// - `eventpatterns` -- the patterns whose stream yields events: `pbind`.
// - `oppatterns` -- what an operator composes: `punop`, `pbinop`, `pnarop`.
//
// What a pattern is, and how one is written, is `seq/pattern.ts`.

export { pbind } from "./eventpatterns.ts";
export type { Bindings } from "./eventpatterns.ts";
export {
    pavaroh, pclump, pcollect, pdelta, pdiff, pdrop, pdur, pflatten, pgate, platch, plen, pn,
    pprorate, preject, pselect, pstutter, ptrace, pwhile, pwrap,
} from "./filterpatterns.ts";
export type { FilterPattern } from "./filterpatterns.ts";
export { pfunc } from "./funcpatterns.ts";
export {
    pdfsm, pfsm, place, placep, prand, pseq, pser, pshuffle, pslide, pswitch, pswitch1, ptuple,
    pwalk, pwrand, pxrand,
} from "./listpatterns.ts";
export type { DfsmState, ListItems, ListPattern } from "./listpatterns.ts";
export { pbinop, pnarop, punop } from "./oppatterns.ts";
export {
    pbeta, pbrown, pcauchy, pexprand, pgauss, pgbrown, phprand, plprand, pmeanrand, ppoisson,
    pprob, pwhite,
} from "./randompatterns.ts";
export { asPattern, pconst, pgeom, pseries, pvalue } from "./valuepatterns.ts";
export type { NumArg } from "./valuepatterns.ts";
