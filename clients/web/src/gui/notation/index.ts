// Engrave a score into the host's `score` display list (mirrors
// `clausters/gui/notation/__init__.py`).
//
// This is the client-side rendering step: an engraver lays a digital score (MEI,
// MusicXML, ABC or Plaine & Easie) out into SVG, and that SVG is walked into the
// flat, resolution-independent display list the GUI host's `score` widget
// consumes — a SMuFL glyph-outline table plus placed primitives in page units,
// each carrying the MEI `xml:id` it was engraved from. The host tessellates it;
// **the engraver lives on the client, never in the host**, so any language
// client reuses the same host renderer by sending the same display list.
//
// The whole layer is **shared**: the score model, the SVG walk and the MEI
// encoder are `clausters_core::notation`, reached here through the core's wasm
// door, and the engraver is the pinned verovio compiled to wasm with the same
// options and the same importers the native library is built with. This module
// is the TypeScript shell over that, as `clausters.gui.notation` is the Python
// one — a second client rebinds the same core rather than reimplementing any of
// it.
//
// The engraver is staged beside the wasm bundles (`dist/vendor/verovio/`) and
// **loaded on demand**: a page that never engraves never downloads it.
//
// There are three ways in: typed score text (ABC/PAE/MEI/MusicXML) handed to
// `engrave`/`Score.open`; `fromNotes`/`fromTimeline`, which turn the client's
// own `seq` data into MEI (the inverse direction, data→score); and
// `svgToDisplayList`, the adapter the first two both flow through. `scoreView`
// and `transport` are the two helpers that put a page on screen and *play* it.
//
// `sheet` is the **score model** underneath all of that: notation as data,
// operations as data over it, and the reading that turns it back into sound
// (`toNotes`, `interpretation`, and `toTimeline` beside it). A sheet is a plain object a caller holds, an
// operation is a payload it sends, and the whole vocabulary lives in Rust —
// which is what lets a standalone host with no client language edit the same
// score through the same one door.

export { Score, engrave, pageJson, svgToDisplayList } from "./engraver.ts";
export type { EngraveOptions, Page } from "./engraver.ts";
export {
    fromNotes,
    fromTimeline,
    sheetFromNotes,
    sheetFromTimeline,
    toTimeline,
} from "./mei.ts";
export type { MeiOptions, PlaybackOptions, Slot } from "./mei.ts";
export {
    addSpanner,
    apply,
    concat,
    del,
    fromMei as sheetFromMei,
    fromVoice as sheetFromVoice,
    header,
    insert,
    insertMeasures,
    interpretation,
    invert,
    itemId,
    marks,
    measures,
    moveSteps,
    ops,
    pitch,
    removeMeasures,
    removeSpanner,
    repeat,
    retrograde,
    setBarline,
    setBreak,
    setDur,
    setHeader,
    setMarks,
    setMeter,
    setPitches,
    silence,
    stack,
    stretch,
    tie,
    toMei,
    toNotes,
    toVoice,
    transpose,
} from "./sheet.ts";
export type {
    HeaderFields,
    Interpretation,
    MarkOptions,
    Op,
    OpSpec,
    PerformedNote,
    Ratio,
    Sheet,
    TransposeOptions,
} from "./sheet.ts";
export { scoreView, transport } from "./view.ts";
export type { ScoreViewOptions } from "./view.ts";
export { setEngraverUrl } from "./_verovio.ts";
