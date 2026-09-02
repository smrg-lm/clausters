/**
 * Editing: the subdomain of the GUI where a picture writes back.
 *
 * Everything that turns a gesture into a change of the data, and the change back
 * into a picture. It is a subpackage rather than a module because it is four
 * collaborators and two editors, and because the boundaries between them are the
 * whole design:
 *
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
 * - {@link FormEditor} — the arrangement's editor: `Editor` plus a held document,
 *   the node index, several views of one composition, the lanes and clips, and
 *   the transport. {@link FormEditing} is its context.
 *
 * The two names are the point of the split: `Editor` is what a person calls to
 * edit a buffer, a curve or a timeline, and `FormEditor` is what edits a piece.
 *
 * And {@link edit} is how a person calls it: one verb over the three fundamental
 * structures, dispatching on what the structure is — {@link SamplesEditor} over
 * a `Buffer`, {@link PointsEditor} over an `Automation`, {@link NotesEditor}
 * over a `Timeline`. Each is `Editor` with its own domain and view in it and
 * nothing else, which is what the split was for.
 *
 * {@link View} here is **not** `gui/guidef.ts`'s `View`, and only this one is
 * reached through this module: the guidef one is a tree you can open, this one is
 * the picture of a structure plus the registry that resolves an event back to it.
 * `gui/index.ts` goes on exporting the guidef `View`, so nothing a page writes
 * changes.
 *
 * @module
 */

export { Editing, FIRST_VERSION, contexts } from "./context.ts";
export type { Adopting } from "./context.ts";
export { Domain } from "./domain.ts";
export { edit } from "./edit.ts";
export type { EditOptions } from "./edit.ts";
export { NotesDomain, NotesEditor, NotesView, quintuples } from "./events.ts";
export type { CrateEvent, Note } from "./events.ts";
export { PointsDomain, PointsEditor, PointsView, quads } from "./points.ts";
export type { CratePoint } from "./points.ts";
export { SamplesDomain, SamplesEditor, SamplesView } from "./samples.ts";
export { Echo } from "./echo.ts";
export type { Correction } from "./echo.ts";
export { Editor, NOT_AN_EDIT, resolveEditorHost } from "./editor.ts";
export type { GenericEditorOptions } from "./editor.ts";
export { FormEditing, FormEditor, MEASURES } from "./formeditor.ts";
export type { EditorOptions, Indexed, Measure } from "./formeditor.ts";
export { View } from "./view.ts";
