// The **arrangement** -- the client-side layer under the multitrack editor
// (mirrors `clausters/form/__init__.py`).
//
// A recursive algebra of elements for composing music: the five primitives
// (`Clang`, `Sequence`, `Vector` -- with `Segments`, the same primitive over
// several windows -- `Track`, `Generator`) as thin adornments over the objects
// the client already has, and `Aggregate` -- the one new structure -- placing
// elements recursively with an offset and deriving their temporal relation. An
// element is *generated* (the rendered thing: random-access, editable) or a
// *generator* (the algorithm that renders it: forward-only), and evaluating the
// second into the first is the **change of state** rendering performs. Pure and
// transport-agnostic.
//
// See `./element.ts` for the primitives and the temporal *character*,
// `./aggregate.ts` for grouping and the temporal *relation*, and `./render.ts`
// for the change of state to sound.
//
// **This module has no door to the shared document.** It had one -- a bridge
// that converted these elements to the crate's JSON -- and it was removed on
// 2026-09-06 with the turn that made the arrangement a model of its own. What a
// multitrack is written with now is `../arrangement.ts`, and the crate is
// reached through `../document.ts`. Nothing here converts, and nothing here is
// designed around.

export {
    ABSTRACT,
    BEATS,
    SECONDS,
    Clang,
    Element,
    Generator,
    PUNCTUAL,
    RELATIVE,
    SEGMENT,
    BufferSegments,
    NoteSegments,
    Segment,
    SegmentRun,
    Segments,
    singleWindow,
    Sequence,
    Track,
    Vector,
    take,
    temporalCharacter,
    toBeats,
} from "./element.ts";
export type {
    Beats,
    ElementOptions,
    EventControls,
    GeneratorOptions,
    SegmentSpec,
    SegmentsOptions,
    TrackOptions,
    SourceLike,
    TemporalCharacter,
    TimeUnit,
    VectorOptions,
} from "./element.ts";
export {
    Aggregate,
    CONCRETE,
    LOGICAL,
    MIXED,
    Member,
    SIMULTANEOUS,
    SUCCESSIVE,
} from "./aggregate.ts";
export type {
    AggregateKind,
    AggregateOptions,
    Bus,
    BusRate,
    BusSpec,
    ChildSpec,
    PlacedMember,
    TemporalRelation,
} from "./aggregate.ts";
export { flatten, render, renderLogical, toTimeline } from "./render.ts";
export type { Flat, RenderOptions, RenderResult } from "./render.ts";
