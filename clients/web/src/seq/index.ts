// The sequencing layer (mirrors `clausters/seq/__init__.py`).
//
// - `event` -- `Event` (a note plays a synth and schedules its release).
// - `pattern` -- `Pattern` (the definition of a generator) and the value
//   patterns (`Pseq`, `Pser`, `Prand`, `Pwhite`, `Pseries`, `Pgeom`, `Pfunc`,
//   `Pn`, `Pconst`), plus `EventPattern`, what plays: `Pbind`, and a
//   `Pseq`/`Prand`/`Pn` over event patterns only.
// - `eventstream` -- `EventStreamPlayer`.
// - `automation` -- `Automation` (a break-point control curve rendered as a
//   control vector) and the lane def it plays through.
// - `timeline` -- `Timeline` (a static, editable, random-access sequence) and
//   its own transport (play/pause/stop/locate/loop) and its own tempo map, plus the `OscItem`/`MidiItem` raw-message
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
export { Entry, MidiItem, OscItem, Timeline, itemData, itemFromData } from "./timeline.ts";
export type { PlayDestination, TimelineItem } from "./timeline.ts";
