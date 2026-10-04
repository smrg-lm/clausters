// The sequencing layer (mirrors `clausters/seq/__init__.py`).
//
// - `event` -- `Event` (a note plays a synth and schedules its release).
// - `pattern` -- `Pattern` (the definition of a generator) and the value
//   patterns (`Pseq`, `Pser`, `Prand`, `Pwhite`, `Pseries`, `Pgeom`, `Pfunc`,
//   `Pn`, `Pconst`), plus `EventPattern`, what plays: `Pbind`, and a
//   `Pseq`/`Prand`/`Pn` over event patterns only.
// - `eventstream` -- `EventStreamPlayer`.
// - `timeline` -- `Timeline` (a static, editable, random-access sequence) and
//   its own transport (play/pause/stop/locate/loop) and its own tempo map.
// - `event` -- also `OscItem`/`MidiItem`, which make events of a raw message.
// - `sequence` -- `EventSequence`: events as concrete data, each with an id, in
//   beats with their tempo map -- what a notes editor edits and what a timeline
//   renders into. A handle to the document's own structure.
// - `curves` -- `CurveEmitter`: what sends the curves of the events a server
//   plays, a stretch at a time.

export { CurveEmitter } from "./curves.ts";
export { DEFAULTS, Event, MidiItem, NOTATION_KEYS, OscItem, rest } from "./event.ts";
export type { EventDestination, EventProps } from "./event.ts";
export { EventStreamPlayer } from "./eventstream.ts";
export {
    EventPattern,
    INF,
    Pattern,
    Pbind,
    Pconst,
    Pfunc,
    Pgeom,
    Pn,
    Prand,
    Pser,
    Pseq,
    Pseries,
    Pwhite,
    asPattern,
} from "./pattern.ts";
export type { Bindings } from "./pattern.ts";
export { Entry, Timeline, itemData, itemFromData } from "./timeline.ts";
export { EventSequence, SeqAutomation, SeqEvent, SeqEvents } from "./sequence.ts";
export type { PlayDestination, TimelineItem } from "./timeline.ts";
