// The sequencing layer (mirrors `clausters/seq/__init__.py`).
//
// - `event` -- `Event` (a note plays a synth and schedules its release).
// - `pattern` -- `Pattern` (the definition of a generator), `EventPattern` (a
//   pattern whose stream yields events, and what plays) and `pattern`, which
//   makes one out of a generator function.
// - `patterns` -- the patterns, by family: `pseq`, `pser`, `prand`, `pwhite`,
//   `pseries`, `pgeom`, `pfunc`, `pn`, `pconst` and the event pattern `pbind`.
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
    PatternStream,
    embed,
    pattern,
} from "./pattern.ts";
export type { PatternGenerator, PatternOptions } from "./pattern.ts";
export * from "./patterns/index.ts";
export { Entry, Timeline, itemData, itemFromData } from "./timeline.ts";
export { EventSequence, SeqAutomation, SeqEvent, SeqEvents } from "./sequence.ts";
export type { PlayDestination, TimelineItem } from "./timeline.ts";
