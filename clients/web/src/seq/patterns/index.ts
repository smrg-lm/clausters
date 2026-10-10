// The patterns, by family (mirrors `clausters/seq/patterns/__init__.py`).
//
// Each module is one family, and each pattern says what it yields and which
// kind it is -- a **value pattern** (its stream yields values) or an **event
// pattern** (its stream yields `Event`s, and it plays) -- or, for a list or
// filter pattern, how it decides when it is built (an event pattern when what
// it holds is):
//
// - `valuepatterns` -- values computed from the arguments: a constant, an
//   arithmetic or geometric series.
// - `randompatterns` -- random values, drawn from the random context, so one
//   seed reproduces them with everything else in the script.
// - `listpatterns` -- values read from a list: in order, cycled, chosen at
//   random.
// - `filterpatterns` -- a pattern made of another: repeated.
// - `funcpatterns` -- values a function computes when the walk asks.
// - `eventpatterns` -- the patterns whose stream yields events: `pbind`.
// - `oppatterns` -- what an operator composes: `punop`, `pbinop`, `pnarop`.
//
// What a pattern is, and how one is written, is `seq/pattern.ts`.

export { pbind } from "./eventpatterns.ts";
export type { Bindings } from "./eventpatterns.ts";
export { pn } from "./filterpatterns.ts";
export type { FilterPattern } from "./filterpatterns.ts";
export { pfunc } from "./funcpatterns.ts";
export { prand, pseq, pser } from "./listpatterns.ts";
export type { ListPattern } from "./listpatterns.ts";
export { pbinop, pnarop, punop } from "./oppatterns.ts";
export { pwhite } from "./randompatterns.ts";
export { asPattern, pconst, pgeom, pseries } from "./valuepatterns.ts";
