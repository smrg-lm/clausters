// The sequencing layer (mirrors `clausters/seq/__init__.py`).
//
// - `event` — `Event` (a note plays a synth and schedules its release).
// - `pattern` — `Pattern` and the value patterns (`Pseq`, `Pser`, `Prand`,
//   `Pwhite`, `Pseries`, `Pgeom`, `Pfunc`, `Pn`, `Pconst`) plus `Pbind` (an
//   event pattern).
// - `eventstream` — `EventStreamPlayer`.
// - `automation` — `Automation` (a break-point control curve rendered as a
//   control vector) and the lane def it plays through.
// - `timeline` — `Timeline` (a static, editable, random-access sequence) and
//   `Playhead` (play/stop/locate/loop over it), plus the `OscEvent`/`MidiEvent` raw-message
//   item.

export {
    Automation,
    DEFAULT_FRAMES,
    LANE_DEF,
    addAutomationDef,
    autoLaneDef,
} from "./automation.ts";
export type { AutomationTarget, AutomationTargets } from "./automation.ts";
export { DEFAULTS, Event, NOTATION_KEYS, rest } from "./event.ts";
export type { EventDestination, EventProps } from "./event.ts";
export { EventStreamPlayer } from "./eventstream.ts";
export {
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
export { Entry, MidiEvent, OscEvent, Playhead, Timeline } from "./timeline.ts";
export type { PlayDestination, TimelineItem } from "./timeline.ts";
