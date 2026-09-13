/**
 * Editing: the subdomain of the GUI where a picture writes back.
 *
 * Everything that turns a gesture into a change of the data, and the change back
 * into a picture. It is a subpackage rather than a module because it is five
 * collaborators and four editors, and because the boundaries between them are
 * the whole design:
 *
 * - {@link Application} — the window set: the host, the widget-id space, the
 *   acknowledgement and the walk of the undo order. Everything true of a
 *   **session on screen** rather than of one structure, so several editors can
 *   share one — and an editor handed none is an application of one.
 * - {@link Editor} — the generic one. It edits **one structure** and imports
 *   nothing from the arrangement: it opens a window through its {@link View},
 *   turns a gesture into a payload through its {@link Domain}, answers the host
 *   through its {@link Echo}, and records in the {@link Editing} context the data
 *   owns.
 * - {@link View} — the `GuiDef` of one structure, and the registry from widget id
 *   to what it shows. The only per-domain thing on the graphic side.
 * - {@link Domain} — the data adapter: gesture → payload, payload → the client
 *   object, the label and the coalesce key. It does not know how an edit inverts
 *   (that is the crate's `history::Editable`) and it does not draw.
 * - {@link Echo} — the acknowledgement protocol: the stamp, the version, the
 *   floor, the corrections and the reason. Entirely generic, and testable with no
 *   structure at all.
 * - {@link Editing} — the editing context: the history, the version, and the
 *   views to tell. An editor **asks for it and never builds one**, which is what
 *   makes two windows over one thing walk one undo order.
 * - `trace` — the path said out loud, at five points: an event routed, an entry
 *   recorded, a step of the pile, a publish, an acknowledgement. Silent unless
 *   asked (`CLAUSTERS_LOG=gui.editing`, or {@link watch}), because what a window
 *   in front of a person does wrong is otherwise visible to nobody.
 * {@link edit} is how a person calls it: one verb over the fundamental
 * structures, dispatching on what the structure is — {@link SamplesEditor} over
 * a `Buffer`, {@link PointsEditor} over an `Automation`, {@link NotesEditor}
 * over a `Timeline`, {@link MultitrackEditor} over a `Multitrack`. Each is
 * `Editor` with its own domain and view in it and nothing else, which is what
 * the split was for.
 *
 * {@link View} here is **not** `gui/guidef.ts`'s `View`, and only this one is
 * reached through this module: the guidef one is a tree you can open, this one is
 * the picture of a structure plus the registry that resolves an event back to it.
 * `gui/index.ts` goes on exporting the guidef `View`, so nothing a page writes
 * changes.
 *
 * @module
 */

export { Application, BASE_ID } from "./application.ts";
export type { Drawing } from "./application.ts";
export { Editing, FIRST_VERSION, contexts } from "./context.ts";
export type { Adopting, Applier } from "./context.ts";
export { Domain } from "./domain.ts";
export { edit } from "./edit.ts";
export type { EditOptions } from "./edit.ts";
export { NotesDomain, NotesEditor, NotesView } from "./events.ts";
export type { CrateEvent, NotesEditorOptions, Note } from "./events.ts";
export {
    MultitrackDomain,
    MultitrackEditor,
    MultitrackView,
    Sources,
} from "./multitrack.ts";
// `Bridge` is a type here and not a value: `MultitrackDomain.bridge` is public,
// so the reference has to resolve, but a page builds one no more than the Python
// client's does — `MultitrackEditor` makes it.
export type { Bridge, MultitrackEditorOptions } from "./multitrack.ts";
export { Playback } from "./playback.ts";
// The plan's own shapes are the crate's and were only ever restated here to
// read it; what a page sees now is the reconciler's answer.
export type { Step, StepArg } from "./playback.ts";
export { PointsDomain, PointsEditor, PointsView, quads } from "./points.ts";
export type { CratePoint } from "./points.ts";
export { MEASURES, SamplesDomain, SamplesEditor, SamplesView, measures } from "./samples.ts";
export type { Measure, SamplesEditorOptions } from "./samples.ts";
export { Echo } from "./echo.ts";
export type { Answer, Correction, Envelope, Turn } from "./echo.ts";
export { Editor, notAnEdit, resolveEditorHost } from "./editor.ts";
export type { GenericEditorOptions, Leg } from "./editor.ts";
export { log, watch } from "./trace.ts";
export { View } from "./view.ts";
