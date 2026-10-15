# Patterns

A **pattern** is a description of a sequence of values: `pseq([60, 62, 64])` describes three numbers, `pwhite(0, 1)` an endless run of random ones. It computes nothing until something walks it, and it can be walked as many times as you like. This page is about what a pattern is, the two kinds there are, how they compose, and how to write one of your own; the patterns themselves are listed by family at the end and documented in the API reference (`clausters.seq.patterns`).

## A description and its walks

Iterating a pattern starts a **stream** — one walk over it — and the stream yields the values:

```python
from clausters.seq import pseq, pseries

melody = pseq([60, 62, 64])
for note in melody:
    print(note)            # 60, 62, 64
list(melody)               # [60, 62, 64] -- a second walk starts again
```

**A pattern holds no state of its own.** Whatever a walk has to remember — where it is in a list, the value a random walk has reached, a seed — lives in that walk, so two loops over the same pattern never disturb each other, and a pattern can be handed to two players at once. That is also why building one is cheap: it stores its arguments (a list it is given is copied, so editing the list afterwards does not change the pattern) and starts nothing.

An **endless pattern** is bounded by whoever walks it, never read whole:

```python
from itertools import islice

list(islice(pseries(0, 2), 4))   # [0, 2, 4, 6]
for x in pseries(0, 2):
    if x > 10:
        break
```

The loop is the first way to see what a pattern yields, and the quickest: no clock, no server.

## Value patterns and event patterns

There are two kinds of pattern, and **they differ in what their stream yields**:

- A **value pattern** yields values — numbers, lists, anything a key of an event takes. `pseq`, `pwhite`, `pseries` are value patterns. A value pattern does not play: there is nothing in a number to sound.
- An **event pattern** yields `Event`s, and it is the only kind that plays. `pbind` is the one that builds events: it binds each key to a value pattern and yields one event per step, each key's value the next of its pattern.

```python
from clausters.seq import pbind, pseq, pwhite

phrase = pbind(degree=pseq([0, 2, 4, 7]), amp=pwhite(0.1, 0.2), dur=0.25)
for event in phrase:
    print(event["degree"], event["amp"])
phrase.play()              # a value pattern has no play
```

A `pbind` ends when any of its keys' patterns ends, and a bare value (`dur=0.25`) is held for every event. Each value pattern given to it receives **the event being built** as it is asked for its value — the incoming value, below — so a key can read what the keys before it chose.

**Which kind a pattern is, is decided when it is built.** A list pattern over event patterns only is an event pattern: `pseq([intro, verse, verse])`, where each is a `pbind`, is a phrase of phrases and plays. Over values, or a mix, it is a value pattern. A filter pattern takes the kind of what it filters (`pn(phrase, 4)` plays). `isinstance(p, EventPattern)` answers.

## Patterns compose by operators

A pattern composes with **the same operators a UGen does**, by the same names, and the result is a pattern:

```python
pseq([74, 60, 46]) + 2                 # 76, 62, 48
(pseries(0, 7) % 12 + 60).midicps()    # a cycle of fifths, as frequencies
pwhite(0, 1).linlin(0, 1, 200, 800)    # random, between 200 and 800
pseq([0, 0.5, 1.5]).fold(0, 1)         # 0, 0.5, 0.5
pseq([60, 64]).blend(72, 0.5)          # 66, 68
pseq([1, 2, 3]) * pseq([10, 20])       # 10, 40 -- ends with the shorter
```

The whole vocabulary is there: the arithmetic, comparison and bitwise operators (`+`, `%`, `//`, `~`, `<`, and `eq`/`ne` by name, since `==` stays Python's equality), the math methods (`.sqrt()`, `.midicps()`, `.round(0.5)` ...), the range maps (`.linlin`, `.lincurve` ...), the bounds (`.clip(lo, hi)`, `.wrap`, `.fold`), sclang's value functions (`.blend`, `.moddif`, `.bilin`, `.degrad` ...) and the random operators (`.rand()`, `.rrand(hi)`, `.coin()` ...), which draw one value per step from the random context.

Each value is computed by the `clausters.base.builtins` function of the operator's name — the same function a list is computed by, and the same the server runs for a UGen — so `pseq([69]).midicps()`, `midicps([69])` and `sine(...).midicps()` answer with one rule. An operand that is not a pattern is the same value at every step; two patterns are walked together, one value from each per step, and the composition ends when either does. A composed pattern is a pattern like any other: it iterates, embeds, repeats under `pn`.

**An operator applies to each value a stream yields.** That is what makes it a pattern's: a stream may never end, and an operator never needs to see more than the value in hand. A function that reads a **whole list** — `normalize` needs the minimum and the maximum, `normalizesum` the sum — is not an operator and a pattern does not have it; apply it to the list before the list becomes a pattern:

```python
from clausters.base.builtins import mirror1, normalize, pyramid

pseq(normalize([3, 7, 5]))             # 0.0, 1.0, 0.5
pseq(pyramid([60, 62, 64]))            # 60, 60 62, 60 62 64
pseq(mirror1([60, 64, 67, 72]), 4)     # up and down, looping without a seam
```

The functions that **reshape** a list — `mirror`, `mirror1`, `mirror2`, `stutter`, `rotate`, `pyramid` and `pyramidg` (its runs as separate lists), sclang's ten pyramid types among them — take any items, not only numbers: `pyramid([phrase_a, phrase_b])` is a list of patterns, and `pseq` of it plays them in that order. `sputter` (in the random context, beside `scramble`) stammers over a list, repeating an item while a coin lands.

**An event pattern refuses an operator.** It yields events, and an event has no arithmetic; `pbind(...) + 2` raises and says so. What composes is the value patterns inside it.

A **stream** composes the same way: `pseq([1, 2]).stream() + 1` is a stream of 2, 3, and so is a `Routine` times a number.

**A builtin given a pattern answers what the method does.** `midicps(pseq([60, 72]))` is `pseq([60, 72]).midicps()`, `add(2, p)` is `2 + p`, and a function of `clausters.base.builtins` that is not an operator — `cbrt`, `degkey`, `snap` — composes by name, value by value: `degkey(pseq([0, 2, 4]), major)` is a pattern of keys. It composes over its first argument, as a method does. The same holds for a signal, as far as a signal has the operator: `midicps(sine(1))` is a `UnaryOpUGen`, `sine(1).degrad()` a product, and a value function no UGen computes (`cbrt`, `moddif`) is refused by name rather than missing.

## Events: keys from keys, patterns together, and time

Inside a `pbind` each key's pattern receives the event being built, so **a key can be read by a later one**: `pkey("degree")` is the degree this event already chose, and `pbind(degree=pseq([0, 7, 14]), octave=pkey("degree") // 7 + 4)` computes one from the other. `pchain(a, b)` — sclang's `a <> b` — runs `b` first and hands each of its events to `a` as the event to build on, so `b` gives defaults that `a` overrides; `pevent(p, {"amp": 0.1})` starts every event of `p` from the same keys.

`ppar([melody, bass])` plays event patterns **together**: their events merged in time, each one's `delta` the time to the next event of any of them. A pattern can also read **the time itself**: `ptime()` is the beats since it started, `pstep([60, 67], [2, 1])` holds each value for its duration on a grid of its own, and `pseg([0.1, 0.5, 0.1], [4, 4], "exp")` follows a curve, read at the beat each event falls on. They need a clock that moves — as keys of a pattern that plays or renders; a plain loop does not advance the beat.

## A voice: articulation and inflection

Two things shape how a line of notes sounds, and they are different in kind.

- An **articulation** is how *distinct notes relate*: a legato that carries one note into the next, a detached note, a tie that holds one pitch across two, a slur over a phrase. It is a relation between notes.
- An **inflection** is a variation *inside one note*: its pitch bending, its pressure swelling, any control moving while it sounds. It belongs to the note — it is its `expression`, the curves an `Event` carries from its start.

It is the difference in a spoken voice between how syllables are joined and how one syllable is inflected. In MPE, too, a note keeps the note-on it started with and is inflected by its bend; a change of note is a new note — an articulation — whatever the bend does.

`pvoice` is the pattern of articulation. It binds keys as `pbind` does, but its events are **notes of one voice**, and each note's `legato` says how it goes on to the next:

```python
from clausters.seq import pseq, pvoice

line = pvoice("lead",
              midinote=pseq([60, 62, 64, 64, 67]),
              dur=0.5,
              legato=pseq([1.0, 1.0, 1.0, 0.6, 0.8]),
              artic=True)
```

With `artic`, a note joins the next while `legato >= 1` and the next one re-attacks when it is less — sclang's `PmonoArtic`; without it every note goes on into the next, sclang's `Pmono`. A rest always breaks the line. So the line above is joined from 60 through the second 64, which articulates, and 67 starts afresh.

**The articulation is one datum, and each destination realizes it in its own terms.** The data is the notes and their `legato`; what a join *becomes* is the instrument's:

- **on the server**, the voice holds one node: the first note starts a synth, each joined note sets that synth's controls instead of starting another, and only the last releases it — so a control with a lag glides, and nothing re-attacks;
- **on a node** — an `instrument` that is a `Voices` node or a plugin's (below) — each joined note-on joins the note before it, which keeps its voice and moves to the new pitch;
- **on MIDI**, joined notes overlap, which a synthesizer in mono mode plays legato;
- **on a page**, rendered into an `EventSequence`, a joined note that keeps its pitch is a **tie** and a run of joined notes that moves is a **slur**, from its first note to the one that articulates.

```python
data = line.render_events().data()
[e["data"].get("tie", False) for e in data["events"]]   # [False, False, True, False, False]
data["notation"]["spanners"]                            # [{"kind": "slur", "from": 1, "to": 4}]
```

The rendered sequence keeps **both shapes**: each note's `legato`, and the tie and slur — the slur in the sequence's notation, where a score reads it. And it keeps what it plays as: each joined note's `joins` names the note it goes on from, so the sequence played on the server is the same held line the pattern played. Nothing is lost on the way.

What varies inside one of those notes is not the voice's business: give the note an `expression` and it is inflected however it is articulated. `voice=` writes which voice of the staff the line is, so two voices played together (`ppar([upper, lower])`) stay apart on the page.

## An instrument that is a node

An event's `instrument` is a def's name — a synth of that def per note — or **a node**: a `Voices` node, which holds N voices of one def and gives them to notes, or a plugin's node, which plays its own. A note on a node makes no node; it is given to that node, identified by a note id the server handle writes on the event (`note_id`), and its end is that note's note-off:

```python
from clausters import Voices
from clausters.seq import pbind, pseq

keys = Voices("default", 8)                       # eight voices, one node
pbind(instrument=keys, midinote=pseq([72, 76, 79]), dur=0.25).play()
```

Every other number the event holds is a control of the note's own voice. A node reads its notes from an **event bus** too — its `notes` control names one, as `in` names an audio bus — so the same instrument plays from MIDI (`Server.midi_bind` routes a channel to a bus) and from `Server.bus_event` alike. A note on a node carries no curves. `basics/voices` in the client's examples plays one `Voices` node from MIDI, a pattern, a `pvoice` line and the client.

## Writing a pattern

A pattern is written as a **generator function** under `pattern`, as a UGen is a function returning a graph node. The name becomes a function that builds the pattern; the generator runs once per walk:

```python
from clausters.seq import embed, pattern, pseq

@pattern
def pcount(inval, n):
    "0, 1, ... n - 1."
    for i in range(n):
        inval = yield i
    return inval

list(pcount(3))                  # [0, 1, 2]
list(pseq([pcount(2), 9]))       # [0, 1, 9] -- embedded like any pattern
pcount(4) * 10                   # and composed like one
```

The generator receives the **incoming value** first — `inval` — and then the arguments the pattern was built with. The incoming value is what whoever walks the pattern hands it at each step: for a key of a `pbind`, the event being built; for a plain loop, `None`. Each `yield` gives a value and receives the next incoming value, and the generator returns the last one when it ends. A pattern that does not need it passes it along unread, as `pcount` does.

To put another pattern's values in place, **embed** it: `inval = yield from embed(item, inval)` yields every value of `item` (or `item` itself, if it is a plain value) and hands back the incoming value where it stopped — which is how `pseq` walks its items.

`pattern(events=True)` builds an event pattern, whose generator yields events; `events` may also be a function of the arguments, which is how a list pattern decides when it is built.

## The families

The patterns live in `clausters.seq.patterns`, one module per family, and are all re-exported from `clausters.seq`. Each pattern's description says what it yields and which kind it is.

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

**An argument may itself be a pattern.** A bound, a step or a parameter is read one value per step, as sclang reads it: `pwhite(pseq([0, 12]), pseq([7, 19]))` draws from two ranges in turn, and `pseries(0, pseq([2, 2, 1]))` steps by a pattern and ends when the steps do.

**A list can be read many ways.** `pseq` in order and `pser` cycling to a count, both from an `offset`; `prand` at random, `pxrand` never the same item twice in a row, `pwrand` by weights; `pshuffle` in one random order, kept; `pslide` as a window sliding along it (`pslide([1, 2, 3, 4, 5], 3, 3, 1)` is `1 2 3  2 3 4  3 4 5`); `pwalk` stepping back and forth across it; `place` lacing lists into a line (`place([1, [2, 3]], 4)` is `1 2 1 3 1 2 1 3`) and `placep` interlacing patterns; `ptuple` reading several side by side into one list per step; `pswitch` and `pswitch1` choosing an item by an index pattern; and `pfsm`/`pdfsm` letting a state machine choose what comes next. Each of them is an event pattern when its items are.

**A filter pattern is made of another.** It reads its source and changes what comes out: `pcollect`, `pselect` and `preject` map and filter by a function (which may take the incoming value as a second argument); `pdrop` and `plen` cut the start or the length; `pstutter` repeats each value, `platch` holds one while a pattern of booleans says so, `pclump` and `pflatten` group values into lists and open them again, `pdiff` gives the steps of a line, `pprorate` splits a duration into parts, and `pavaroh` reads degrees with a raga's ascending and descending scales. On events, `pdur` cuts a phrase to a duration and `pdelta` starts it late. `pn` repeats, and with a `key` it marks the event each repeat starts on — so `pbind(n=pn(pseq([0, 1]), INF, "g"), x=pgate(pseries(), INF, "g"))` takes a new `x` only when `n` starts over. `ptrace` (or `.trace()` on any pattern) prints each value as it passes, which is the quickest way to see what a pattern deep inside a `pbind` yields.

**A function can stand in for a list.** `pfunc` calls a function for each value — with the event being built, when it takes an argument, so `pbind(degree=..., octave=pfunc(lambda e: 4 + e["degree"] // 7))` computes one key from another — and `pfuncn` embeds what it returns, so a function may return a pattern. `prout` walks a generator function as a pattern (a routine's body, yielding values instead of waits), `plazy` builds its pattern only when the walk starts, `pif` takes each value from one pattern or another by a pattern of conditions, and `pproduct` runs nested loops over several patterns, every combination in turn.

**Choosing a distribution** is choosing where the values gather. `pwhite` spreads them evenly; `pexprand` evenly per octave, which is what a frequency or a duration wants; `plprand`, `phprand` and `pmeanrand` lean toward the low end, the high end or the middle; `pgauss` and `pcauchy` gather around a centre, Cauchy with tails that reach far now and then; `pbeta` gathers at both ends or in the middle, by its two weights; `ppoisson` counts events. `pbrown` and `pgbrown` are not independent draws but a **walk**: each value moves from the last, so a line wanders instead of jumping. And `pprob` takes the shape itself, drawn as a list of relative chances: `pprob([0, 1, 4, 1, 0], 48, 72)` is a range of notes most likely in its middle.

The random patterns draw from the **random context**, so one seed reproduces them with everything else in a script — see [Routines and clocks](routines-and-clocks.md#the-random-context-one-seed-per-session).

`basics/patterns` in the client's examples puts this page together: a `pyramid` fed to a `pseq`, an operator over it, a `pvoice` whose joins glide on the server and come back as ties and slurs, and a counter-line beside it under `ppar`, rendered offline.
