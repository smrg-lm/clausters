// The **arrangement** — the client-side layer under the multitrack editor
// (mirrors `clausters/form/__init__.py`).
//
// A recursive algebra of elements for composing music: the five primitives
// (`Clang`, `Sequence`, `Vector` — with `Segments`, the same primitive over
// several windows — `Track`, `Generator`) as thin adornments over the objects
// the client already has, and `Aggregate` — the one new structure — placing
// elements recursively with an offset and deriving their temporal relation. An
// element is *generated* (the rendered thing: random-access, editable) or a
// *generator* (the algorithm that renders it: forward-only), and evaluating the
// second into the first is the **change of state** rendering performs. Pure and
// transport-agnostic.
//
// See `./element.ts` for the primitives and the temporal *character*,
// `./aggregate.ts` for grouping and the temporal *relation*, `./render.ts` for
// the change of state to sound, and `./document.ts` for the bridge to the shared
// document model.

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
    Segment,
    Segments,
    Sequence,
    Track,
    Vector,
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
export {
    FIRST_VERSION,
    FORM_TRACK,
    FrozenSource,
    SESSION_FORMAT,
    docIdOf,
    fromDocument,
    fromSession,
    leafConfig,
    leafNode,
    nextNodeId,
    setDocId,
    toDocument,
    toSession,
} from "./document.ts";
export type { DocNode, DocumentJson, Resolver, SessionJson } from "./document.ts";
export { flatten, render, renderLogical, toTimeline } from "./render.ts";
export type { Flat, RenderOptions, RenderResult } from "./render.ts";
