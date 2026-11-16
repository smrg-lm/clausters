/**
 * The symbolic score: notation as data, edited and engraved with no window
 * (mirrors `clausters/notation/__init__.py`).
 *
 * A **symbolic score** is notation -- staves, notes and their engraving, held
 * as MEI -- and not an OSC score (the timed bundles an offline render runs).
 * Everything here works in a page that opens no window and holds no GUI host:
 * a score is read, edited, written, read back into sound and engraved into a
 * display list through the core alone. The score editor is the application
 * over the same {@link Score} (`ScoreEditor`), and putting a page on screen is
 * the GUI's (`gui.scoreView`, `gui.pageJson`, `PlayheadSync.forScore`).
 *
 * The whole layer is **shared**: the score model, the SVG walk and the MEI
 * encoder are `clausters_notation`, reached here through the core's wasm door,
 * and the engraver is the pinned verovio compiled to wasm with the same
 * options and the same importers the native library is built with. This
 * module is the TypeScript shell over that, as `clausters.notation` is the
 * Python one -- a second client rebinds the same core rather than
 * reimplementing any of it.
 *
 * The engraver is staged beside the wasm bundles (`dist/vendor/verovio/`) and
 * **loaded on demand**: a page that never engraves never downloads it.
 *
 * There are three ways in: typed score text (ABC/PAE/MEI/MusicXML) handed to
 * {@link engrave} or `Score.open`; {@link fromNotes}, {@link fromTimeline} and
 * {@link sheetFromEvents}, which read the client's own `seq` data into a score
 * (the inverse direction, data->score); and {@link svgToDisplayList}, the
 * adapter the first two both flow through.
 *
 * `sheet` is the **score model** underneath all of that: notation as data,
 * operations as data over it, and the reading that turns it back into sound
 * ({@link toNotes}, {@link interpretation}, and {@link toTimeline} beside it).
 * A sheet is a plain object a caller holds, an operation is a payload it
 * sends, and the whole vocabulary lives in Rust -- which is what lets a
 * standalone host with no client language edit the same score through the
 * same one door.
 *
 * @module
 */

export { Score, engrave, svgToDisplayList } from "./engraver.ts";
export type { EngraveOptions, Page } from "./engraver.ts";
export {
    fromNotes,
    fromTimeline,
    sheetFromEvents,
    sheetFromNotes,
    sheetFromTimeline,
    toSequence,
    toTimeline,
} from "./mei.ts";
export type { Placed, PlaybackOptions, ReadOptions, Slot } from "./mei.ts";
export {
    addControl,
    addSpanner,
    apply,
    concat,
    del,
    enter,
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
    readEvents,
    removeControl,
    removeMeasures,
    removeSpanner,
    renderEvents,
    repeat,
    retrograde,
    setBarline,
    setBreak,
    setClef,
    setDur,
    setEnding,
    setGroups,
    setHeader,
    setKey,
    setMark,
    setMarks,
    setMeter,
    setMultirests,
    setPage,
    setPitches,
    setRepeat,
    setStaff,
    silence,
    stack,
    stretch,
    tie,
    toMei,
    toNotes,
    toVoice,
    transpose,
    TRANSCRIPTION_KEYS,
} from "./sheet.ts";
export type {
    HeaderFields,
    Interpretation,
    MarkOptions,
    Op,
    OpSpec,
    PerformedNote,
    Ratio,
    ReadSequence,
    RenderedSequence,
    Sheet,
    TextPlace,
    Transcription,
    TransposeOptions,
} from "./sheet.ts";
