// `form` -- a frozen algebra for placing elements in time (mirrors
// `clausters/form/__init__.py`).
//
// A recursive algebra of elements for composing music: the five primitives
// (`Clang`, `Sequence`, `Vector` -- with `Segments`, the same primitive over
// several windows -- `Track`, `Generator`) as thin adornments over the objects
// the client already has, and `Aggregate` -- the one new structure -- placing
// elements recursively with an offset and deriving their temporal relation. An
// element is *generated* (the rendered thing: random-access, editable) or a
// *generator* (the algorithm that renders it: forward-only), and evaluating the
// second into the first is the **change of state** rendering performs. Pure and
// transport-agnostic, and no view draws it.
//
// See `./formelement.ts` for the primitives and the temporal *character*,
// `./aggregate.ts` for grouping and the temporal *relation*, and `./render.ts`
// for the change of state to sound.
//
// **This module has no door to the shared document.** It had one -- a bridge
// that converted these elements to the crate's JSON -- and it was removed on
// 2026-09-06 with the turn that made the multitrack a model of its own. What a
// multitrack is written with now is `../document.ts`, which is also the door to
// the crate. Nothing here converts, and nothing here is
// designed around.

export {
    ABSTRACT,
    BEATS,
    SECONDS,
    Clang,
    FormElement,
    Generator,
    PUNCTUAL,
    RELATIVE,
    SEGMENT,
    Segment,
    Segments,
    Sequence,
    Track,
    Vector,
    take,
    temporalCharacter,
    toBeats,
} from "./formelement.ts";
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
} from "./formelement.ts";
export {
    Aggregate,
    CONCRETE,
    LOGICAL,
    MIXED,
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
    Member,
    PlacedMember,
    TemporalRelation,
} from "./aggregate.ts";
export { flatten, render, renderLogical, toTimeline } from "./render.ts";
export type { Flat, RenderOptions, RenderResult } from "./render.ts";
