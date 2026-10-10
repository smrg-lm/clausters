# Patterns

A **pattern** is a description of a sequence of values: `pseq([60, 62, 64])` describes three numbers, `pwhite(0, 1)` an endless run of random ones. It computes nothing until something walks it, and it can be walked as many times as you like. The model is the [Python client's](https://clausters-python.readthedocs.io/) — the same patterns under the same names, yielding the same values from the same core — and this page says it in this client's spelling.

## A description and its walks

Iterating a pattern starts a **stream** — one walk over it — and the stream yields the values:

```js
import { seq } from "clausters";

const melody = seq.pseq([60, 62, 64]);
for (const note of melody) console.log(note);   // 60, 62, 64
[...melody];                                    // [60, 62, 64] -- a new walk
```

**A pattern holds no state of its own.** Whatever a walk has to remember — where it is in a list, the value a random walk has reached — lives in that walk, so two loops over one pattern never disturb each other. Building one stores its arguments (an array it is given is copied) and starts nothing.

An **endless pattern** is bounded by whoever walks it — a `break` in the loop — and never spread whole.

## Value patterns and event patterns

There are two kinds of pattern, and **they differ in what their stream yields**:

- A **value pattern** yields values — numbers, arrays, anything a key of an event takes. `pseq`, `pwhite`, `pseries` are value patterns, and a value pattern does not play.
- An **event pattern** yields `Event`s, and it is the only kind that plays. `pbind` builds them: it binds each key to a value pattern and yields one event per step.

```js
const phrase = seq.pbind({
  degree: seq.pseq([0, 2, 4, 7]),
  amp: seq.pwhite(0.1, 0.2),
  dur: 0.25,
});
for (const event of phrase) console.log(event.get("degree"));
phrase.play(server);
```

A `pbind` ends when any key's pattern ends, holds a bare value for every event, and hands each value pattern **the event being built** as its incoming value. **Which kind a pattern is, is decided when it is built**: a list pattern over event patterns only (`seq.pseq([intro, verse])`) is an event pattern and plays; a filter pattern takes the kind of what it filters. `instanceof seq.EventPattern` answers.

## Patterns compose by operators

A pattern composes with the operators a UGen does, by the same names. TypeScript has no operator overloading, so composition is by method, as it is for a graph:

```js
seq.pseq([74, 60, 46]).add(2);                    // 76, 62, 48
seq.pseries(0, 7).mod(12).add(60).midicps();      // a cycle of fifths
seq.pseq([1, 2, 3]).mul(seq.pseq([10, 20]));      // 10, 40 -- ends with the shorter
seq.pwhite(0, 1).linlin(0, 1, 200, 800);          // random, between 200 and 800
seq.pseq([60, 64]).blend(72, 0.5);                // 66, 68
```

The whole vocabulary is there: the operators by name (`add`, `mod`, `idiv`, `bitnot`, `lt`, `eq` ...), the math methods, the range maps, the bounds (`clip(lo, hi)`, `wrap`, `fold`), sclang's value functions (`blend`, `moddif`, `bilin`, `degrad`, `not` ...) and the random operators (`rand()`, `rrand(hi)`, `coin()` ...), one draw per step from the random context.

Each value is computed by the `builtins` function of the operator's name — the one a list and a signal are computed by — so the three answer an operator with one rule. **An operator applies to each value a stream yields**; a function that reads a whole list (`builtins.normalize`) is not an operator, and is applied to the array before it becomes a pattern: `seq.pseq(builtins.normalize([3, 7, 5]))`, `seq.pseq(builtins.pyramid([60, 62, 64]))`. The functions that **reshape** an array (`mirror`, `mirror1`, `mirror2`, `stutter`, `rotate`, `pyramid`, `pyramidg`) take any items, patterns included, and `sputter` stammers over one from the random context. **An event pattern refuses an operator**: an event has no arithmetic. A stream composes the same way (`pattern.stream().add(1)`).

**A builtin given a pattern answers what the method does**: `builtins.midicps(seq.pseq([60, 72]))` is `seq.pseq([60, 72]).midicps()`, `builtins.add(2, p)` puts `p` on the right, and a function that is not an operator (`cbrt`, `degkey`) composes by name, value by value, over its first argument. A signal takes the same calls as far as a UGen has the operator; a value function no UGen computes (`cbrt`, `moddif`) is refused by name.

## Events, voices and time

Inside a `pbind` each key's pattern receives the event being built: `seq.pkey("degree")` reads a key chosen before it. `seq.pchain(a, b)` (sclang's `a <> b`) builds `a`'s events on `b`'s, `seq.pevent(p, keys)` starts every event from the same keys, and `seq.ppar([melody, bass])` plays event patterns together, merged in time. `seq.ptime`, `seq.pstep` and `seq.pseg` read the beat the walk is at — time, a step function, a curve — and need a clock that moves.

**Articulation and inflection** are two different things. An articulation is how distinct notes relate — legato, detached, a tie, a slur; an inflection is a variation inside one note — its bend, its pressure — and belongs to the note as its `expression`. A change of note is an articulation even in MPE, where a note keeps its note-on and is inflected by its bend.

`seq.pvoice(instrument, bindings, { artic })` is the pattern of articulation: notes of one voice, each note's `legato` saying how it goes on to the next (with `artic`, it joins while `legato >= 1`; without, always; a rest breaks the line). The join is one datum that each destination realizes: on the server one node held across the joined notes and set by each, on MIDI overlapping notes, and rendered into an `EventSequence` a `tie` where a joined note keeps its pitch and a slur over each run that moves — the sequence keeps both the `legato` and the page's marks.

```js
const line = seq.pvoice(
  "lead",
  { midinote: seq.pseq([60, 62, 64, 64, 67]), dur: 0.5, legato: seq.pseq([1, 1, 1, 0.6, 0.8]) },
  { artic: true },
);
const data = (await line.renderEvents()).data(); // a tie on the first 64, a slur from 1 to 4
```

## Writing a pattern

A pattern is written as a **generator function** under `seq.pattern`; the result is the function that builds it:

```ts
const pcount = seq.pattern(function* pcount(inval: unknown, n: number) {
  for (let i = 0; i < n; i++) inval = yield i;
  return inval;
});

[...pcount(3)];                     // [0, 1, 2]
pcount(4).mul(10);                  // composed like any pattern
```

The generator receives the **incoming value** first — `inval`, the event a `pbind` is building when the pattern is one of its keys, else `undefined` — and then its arguments; each `yield` gives a value and receives the next incoming one, and the generator returns the last. To put another pattern's values in place, embed it: `inval = yield* seq.embed(item, inval)`. `seq.pattern(fn, { events: true })` builds an event pattern.

## The families

One module per family under `seq/patterns/`, all re-exported from `seq`, each pattern's description saying what it yields and which kind it is:

| Family | What it yields | Patterns |
|---|---|---|
| `valuepatterns` | values computed from the arguments | `pconst`, `pvalue`, `pseries`, `pgeom` |
| `randompatterns` | random values, from the random context | `pwhite`, `pexprand`, `plprand`, `phprand`, `pmeanrand`, `pbeta`, `pcauchy`, `pgauss`, `ppoisson`, `pbrown`, `pgbrown`, `pprob` |
| `listpatterns` | values read from a list | `pseq`, `pser`, `prand`, `pxrand`, `pwrand`, `pshuffle`, `pslide`, `pwalk`, `place`, `placep`, `ptuple`, `pswitch`, `pswitch1`, `pfsm`, `pdfsm` |
| `filterpatterns` | another pattern, transformed | `pn`, `pgate`, `pcollect`, `pselect`, `preject`, `pdrop`, `plen`, `pstutter`, `platch`, `pwhile`, `pwrap`, `pclump`, `pflatten`, `pdiff`, `pprorate`, `pavaroh`, `pdelta`, `pdur`, `ptrace` |
| `funcpatterns` | values a function computes when asked | `pfunc`, `pfuncn`, `prout`, `plazy`, `pif`, `pproduct` |
| `eventpatterns` | events | `pbind`, `pkey`, `pchain`, `pevent`, `pvoice`, `ppar` |
| `timepatterns` | values of the beat the walk is at | `ptime`, `pstep`, `pseg` |
| `oppatterns` | an operator over patterns | `punop`, `pbinop`, `pnarop` |

A list is read in many ways: `pseq`/`pser` in order from an offset, `prand`/`pxrand`/`pwrand` at random (never twice in a row, by weights), `pshuffle` in one kept random order, `pslide` as a sliding window, `pwalk` stepping across it, `place`/`placep` lacing, `ptuple` side by side, `pswitch`/`pswitch1` by an index pattern, and `pfsm`/`pdfsm` by a state machine; each is an event pattern when its items are.

A filter pattern is made of another: `pcollect`/`pselect`/`preject` map and filter by a function, `pdrop`/`plen` cut, `pstutter` repeats, `platch` holds, `pclump`/`pflatten` group and open, `pdiff` steps, `pprorate` splits a duration, `pavaroh` reads a raga's two scales, `pdur`/`pdelta` cut and delay a phrase of events, `pn` with a `key` marks where each repeat starts so a `pgate` takes a new value there, and `ptrace` (or `.trace()`) prints what passes.

A function stands in for a list: `pfunc` computes a value per step (with the event being built, when it declares an argument), `pfuncn` embeds what it returns, `prout` walks a generator function, `plazy` builds its pattern when the walk starts, `pif` chooses between two patterns by a pattern of conditions, and `pproduct` runs nested loops over several.

An argument may itself be a pattern, read one value per step: `seq.pwhite(seq.pseq([0, 12]), seq.pseq([7, 19]))` draws from two ranges in turn. The distributions say where values gather: `pwhite` evenly, `pexprand` evenly per octave, `plprand`/`phprand`/`pmeanrand` toward an end or the middle, `pgauss`/`pcauchy` around a centre, `pbeta` by two weights, `ppoisson` counting events, `pbrown`/`pgbrown` as a walk from the last value, and `pprob` by a shape you draw as a list of chances.

The random patterns draw from the random context — see [Routines and clocks](routines-and-clocks.md#reproducible-randomness).

`examples/basics/patterns.html` puts this page together: a `pyramid` fed to a `pseq`, an operator over it, a `pvoice` whose joins glide on the server and come back as ties and slurs, and a counter-line beside it under `ppar`, rendered offline.
