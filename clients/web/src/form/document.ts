// The arrangement to and from the **document** — the shared model in
// `crates/clausters-document` (mirrors `clausters/form/document.py`).
//
// The document is the single authoritative model of a composition, and it lives
// in a Rust crate so that every deployment mode binds one of it: this client,
// the Python client, and a `standalone` GUI host with no language attached at
// all. This module is the bridge, and it is a **round trip through the format**
// rather than a binding: the tree here is converted to the document's JSON and
// back, so the crate stays the normative shape without this client giving up its
// own objects.
//
// What crosses, and what cannot
// -----------------------------
//
// The document holds **where things are** — the tree, the placements, the
// grouping. It holds a leaf's configuration as an **opaque payload it never
// interprets**, which is not a limitation to work around but the reason one
// document can serve three languages: a generator *is code*, in the language of
// whoever wrote it, and no format owns that.
//
// So the conversion is lossless for **concrete data** (events, placements, sets,
// buffers by reference) and carries a **generator by reference**, exactly as a
// project file references a plugin rather than serializing it. Coming back, a
// leaf whose configuration names an object this process no longer has resolves
// through `resolve`; without one it comes back as the reference itself, which a
// `Generator` already accepts (it wraps a def *name* as readily as a def
// object). That is the frozen case, and it is the floor rather than a failure: a
// host with no interpreter shows what was rendered.
//
// Identity
// --------
//
// The document addresses nodes by id and the arrangement does not, so
// `toDocument` assigns one per element and **stamps it on the element**, reusing
// whatever is already there. Converting the same tree twice therefore yields the
// same ids, which is what lets an edit made against one conversion still name
// the right node in the next.
//
// The stamp itself is the one place this module reads differently from the
// Python one, and only in shape: Python sets a `_doc_id` attribute on the
// object, and a page keeps the same association in a `WeakMap` — `docIdOf` /
// `setDocId` below — because a timeline item is an `Event` this module does not
// own and must not grow a field on.
//
// The id is on the *element*, so placing one element at two offsets writes two
// nodes with one id, and an edit naming that id cannot say which of the two it
// means. Give each appearance its own element over the same source — two
// `Vector` leaves over one server buffer — until the addressing settles.

import { Event as SeqEvent } from "../seq/event.ts";
import { MIDI_KEY, OSC_KEY, Timeline, itemData, itemFromData } from "../seq/timeline.ts";
import { pointsToEnv } from "../defs/ugens/index.ts";
import { CONCRETE, LOGICAL, Aggregate, Member } from "./aggregate.ts";
import type { BusSpec } from "./aggregate.ts";
import {
    Clang,
    Element,
    Generator,
    Segments,
    Sequence,
    Track,
    Vector,
} from "./element.ts";
import type { SourceLike } from "./element.ts";

/**
 * A document node, as `serde` reads it. Deliberately open: a body this build
 * does not know is carried through whole rather than dropped.
 */
export type DocNode = Record<string, any>;

/** A whole document: the version it is at, and its root node. */
export interface DocumentJson {
    version: number;
    root: DocNode;
    /**
     * **Content the tree reads rather than places**: the nodes a window names
     * (`{"node": id}`). Absent in every document that shares nothing.
     */
    content?: DocNode[];
}

/** A session: the document, plus the table that says where its source is. */
export interface SessionJson {
    format: number;
    document: DocumentJson;
    sources: Record<string, unknown>;
    provenance?: unknown;
}

/**
 * What a caller supplies for a leaf the document only names — the port of the
 * Python bridge's `resolve(kind, config)`.
 */
export type Resolver = (kind: string, config: any) => unknown;

/**
 * The node ids this conversion stamped, keyed by the object they name — an
 * element, a member handle, or a timeline item.
 *
 * A `WeakMap` rather than an attribute (Python's `_doc_id`) because the objects
 * stamped are not all ours: a `Track`'s items are `Event`s the sequencing layer
 * owns, and this bridge has no business growing a field on one.
 */
const docIds = new WeakMap<object, number>();

/** The node id stamped on `obj` by a conversion, or `null` for none. */
export function docIdOf(obj: unknown): number | null {
    if (obj === null || typeof obj !== "object") return null;
    return docIds.get(obj as object) ?? null;
}

/**
 * Stamps `id` as the node id of `obj` — what a conversion does, and what an
 * editor does for a node it mints itself (a note added by a gesture).
 */
export function setDocId(obj: unknown, id: number): void {
    if (obj !== null && typeof obj === "object") docIds.set(obj as object, id);
}

/**
 * Takes the stamp off `obj` — for material that **stops** being content: a join
 * makes two windows one element again, and a timeline still carrying the id it
 * had as content would send the next edit to a node the document no longer has.
 */
export function clearDocId(obj: unknown): void {
    if (obj !== null && typeof obj === "object") docIds.delete(obj as object);
}

/**
 * The version an unedited document carries.
 *
 * One rather than zero, because zero is what an edit means by *unstated* when it
 * names the state it was made against — the same reservation the GUI host's
 * sequence numbers make. An unedited document is a real state an editor must be
 * able to name, so it cannot share a number with "I cannot say".
 */
export const FIRST_VERSION = 1;

/** The session format this client writes (the crate's `session::FORMAT`). */
export const SESSION_FORMAT = 1;

/**
 * The whole arrangement as a document, ready for `serde`.
 *
 * `version` is the document version to stamp (see the crate: the document's half
 * of the two counters); zero means *unstated* and is never a document's own
 * version.
 */
export function toDocument(
    element: Element,
    { version = FIRST_VERSION }: { version?: number } = {},
): DocumentJson {
    const ids = new Ids(element);
    const shared = sharedContent(element, ids);
    const out: DocumentJson = {
        version: Math.trunc(version),
        root: node(element, ids, null, shared),
    };
    if (shared.size > 0) {
        // **Content is written once, and the tree names it.** Its nodes are in
        // the same id space as the tree's, which is what lets a window and an
        // intent name the same thing.
        out.content = [...shared.values()].map((entry) => entry.node);
    }
    return out;
}

/** One piece of shared content: the node it was written as, and its id. */
interface Content {
    id: number;
    node: DocNode;
}

/**
 * The material **more than one element reads**, as content nodes.
 *
 * A window onto samples costs nothing to repeat because the samples are not in
 * the document; a window onto a timeline of notes is not so lucky — the notes
 * are nodes, so writing two windows as two tracks writes every note twice, with
 * the same ids in each, and a reopened piece gets two timelines that drift apart
 * from the first edit. So a timeline **two elements hold** is written once,
 * here, and each of its readers becomes a window naming it.
 *
 * A timeline only one element holds is written exactly as it always was: the
 * table exists for sharing, not for tracks.
 */
function sharedContent(root: Element, ids: Ids): Map<Timeline, Content> {
    const holders = new Map<Timeline, Track[]>();
    const scan = (element: Element): void => {
        if (element instanceof Track) {
            const found = holders.get(element.timeline) ?? [];
            found.push(element);
            holders.set(element.timeline, found);
        }
        for (const child of children(element)) scan(child as Element);
    };
    scan(root);
    const shared = new Map<Timeline, Content>();
    for (const [timeline, tracks] of holders) {
        if (tracks.length < 2) {
            // **Not content, and it has to stop being it.** A join makes two
            // windows one element again, and a timeline still carrying the id it
            // had as content would send the next note edit to a node this
            // document no longer has.
            if (docIdOf(timeline) !== null) clearDocId(timeline);
            continue;
        }
        // The content node carries the notes, and the id is the **timeline's**
        // own — stamped on it, so a second conversion names the same node and
        // every intent recorded against a note still lands.
        const id = ids.of(timeline as unknown as Element);
        shared.set(timeline, {
            id,
            node: {
                id,
                kind: "aggregate",
                grouping: CONCRETE,
                members: timelineItems(timeline).map(([beat, item]) =>
                    timelineMember(beat, item, ids),
                ),
                config: { form: FORM_TRACK },
            },
        });
    }
    return shared;
}

/**
 * The arrangement as a **session**: the document, plus the table that says where
 * its source is.
 *
 * A document says *what plays when* and deliberately not where a source lives,
 * because inside a running system a source is a server buffer, a mapped file or
 * a rendered result and the tree has no business knowing which. A session is the
 * document plus exactly that missing half, so the thing can be closed and opened
 * again — by this client, or by a `standalone` host with no language attached,
 * which is why the format lives in the crate and not here.
 *
 * `sources` is `{sourceId: entry}` — each entry as the crate's `session::Source`
 * (`location`, `lifetime`, `generation`, and optionally `channels`/`frames`/
 * `sample_rate`/`provenance`/`editing`). A source with an **open destructive
 * edit** carries `editing` and reopens that way: a save never blocks on a
 * confirmation. `provenance` is an opaque reference to whatever produced the
 * session — the scripts behind it — carried and never interpreted, which is what
 * makes re-generating possible without the format knowing how.
 */
export function toSession(
    element: Element,
    {
        sources,
        version = FIRST_VERSION,
        provenance,
    }: {
        sources?: Record<number | string, unknown> | Map<number, unknown> | null;
        version?: number;
        provenance?: unknown;
    } = {},
): SessionJson {
    const document = toDocument(element, { version });
    const entries =
        sources instanceof Map ? [...sources.entries()] : Object.entries(sources ?? {});
    const table: Record<string, unknown> = {};
    for (const [key, value] of entries) table[String(key)] = value;
    const covered = new Set([...Object.keys(table)].map((k) => Number(k)));
    const missing = [...sourceIds(document.root)]
        .filter((id) => !covered.has(id))
        .sort((a, b) => a - b);
    if (missing.length > 0) {
        // A session whose table does not cover its own document reopens with
        // that source unresolved -- the take draws nothing and nothing says
        // why, which is a defect found the only way it can be: by looking at a
        // window two saves later. The table is caller data (what a location
        // *means* is the caller's), but whether it covers the tree is checkable
        // here, and it is the difference between an error now and a silent hole
        // later. It bites hardest where the ids move under you: reopening
        // resolves source into new buffers, so a table built once at startup
        // stops matching the composition it is saved with.
        throw new Error(
            "the source table does not cover this document: no entry for " +
                `${missing.join(", ")}. Build it from the arrangement being ` +
                "saved (each buffer element's current source), not from the " +
                "source the script started with.",
        );
    }
    const session: SessionJson = {
        format: SESSION_FORMAT,
        document,
        sources: table,
    };
    if (provenance !== undefined && provenance !== null) session.provenance = provenance;
    return session;
}

/**
 * Every source id the document names, so a session can be checked against its
 * own table before it is written.
 */
function sourceIds(root: DocNode): Set<number> {
    const found = new Set<number>();
    const stack: unknown[] = [root];
    while (stack.length > 0) {
        const current = stack.pop();
        if (current === null || typeof current !== "object") continue;
        const node = current as DocNode;
        const source = node.source;
        if (source !== null && typeof source === "object" && "source" in source) {
            found.add(Math.trunc(Number(source.source)));
        }
        // A `segments` node names one source per segment, and a session whose
        // table covered only the first would reopen with the rest of the
        // source missing.
        for (const seg of (node.segments as unknown[]) ?? []) {
            if (seg !== null && typeof seg === "object") stack.push(seg);
        }
        for (const member of (node.members as DocNode[]) ?? []) {
            if (member !== null && typeof member === "object") stack.push(member.node);
        }
        stack.push(node.rendered);
    }
    return found;
}

/**
 * Opens a session: the root element, and its source table as written.
 *
 * The table is handed back as data rather than resolved, because what a source
 * *is* (a server buffer to allocate, a file to map) is the caller's to decide and
 * depends on what is running.
 *
 * Throws if the file was written in a format this build cannot read. A newer
 * *field* is not a version change — it is ignored on the way through, the way an
 * unknown body is carried rather than dropped — so this only fires when reading
 * it wrongly is the alternative.
 */
export function fromSession(
    session: SessionJson,
    { resolve }: { resolve?: Resolver } = {},
): { element: Element; sources: Map<number, unknown> } {
    const format = Math.trunc(Number(session.format ?? SESSION_FORMAT));
    if (format > SESSION_FORMAT) {
        throw new Error(
            `session format ${format} is newer than this build reads (${SESSION_FORMAT})`,
        );
    }
    const sources = new Map<number, unknown>();
    for (const [key, value] of Object.entries(session.sources ?? {})) {
        sources.set(Number(key), value);
    }
    const element = fromDocument(session.document, { resolve });
    // A source nothing resolved comes back as a reference, and the table is what
    // says where it is — so the reference is given it. Otherwise opening a
    // session and saving it again wrote every unresolved take as volatile, and
    // the format lost its own contents on the second save.
    for (const src of sourceObjects(element)) {
        if (src instanceof FrozenSource) {
            src.locate((sources.get(src.bufnum) ?? null) as DocNode | null);
        }
    }
    return { element, sources };
}

/**
 * A `resolve` for {@link fromSession}, **over the session's own table**.
 *
 * {@link fromSession} rebuilds the tree; this is what makes the tree hold
 * something. A document names a source by number and says nothing about where it
 * is — the table says that — so reopening a piece into a running system is two
 * steps, and this is the second one:
 *
 * - **A take** (a `Vector` or a `Segments` window) whose source the table
 *   locates in a **file** is read onto the server, once per source id no matter
 *   how many windows name it: two clips over one take are two windows onto
 *   **one** buffer, and reading it twice would give them two buffers that drift
 *   apart on the first edit. A source the table calls *volatile* existed only in
 *   the run that wrote it, so it comes back as a {@link FrozenSource} — drawn,
 *   placed, silent — rather than as a lie.
 * - **A generator** (or a pattern) is code, and a document carries a *reference*
 *   to code and never the code. So it is looked up in `defs`, and a name nothing
 *   supplies is left frozen with whatever it last **rendered** as its floor —
 *   which is already the format's contract and is the whole of what a host with
 *   no language attached can show.
 *
 * `folder` is the session file's own folder: a **relative** path in the table is
 * resolved against it, which is what makes a session directory movable, and an
 * absolute one names the user's own file and is left exactly as written. `defs`
 * is what supplies the code a leaf names — a record from reference to object, or
 * a `defs(kind, reference)` — and anything it does not have is frozen rather
 * than an error.
 *
 * Reading a file is asynchronous here and not in the Python client (a page's
 * `Buffer.read` goes to the worker that owns the filesystem), so the *takes are
 * read while the resolver is built* and the resolver itself is the same
 * synchronous function on both sides — the `await` is the language's, not a
 * different call:
 *
 * ```ts
 * const resolve = await sessionResolver(data, { folder });
 * const { element, sources } = fromSession(data, { resolve });
 * ```
 */
export async function sessionResolver(
    session: SessionJson,
    { server = null, folder = null, defs = null }: SessionResolverOptions = {},
): Promise<Resolver> {
    const table = new Map<number, DocNode>();
    for (const [key, value] of Object.entries(session.sources ?? {})) {
        table.set(Number(key), (value ?? {}) as DocNode);
    }
    // **Read once per source id, before the tree asks.** The table covers
    // exactly what the document names (`toSession` refuses one that does not),
    // so there is nothing here that the piece does not use.
    const buffers = new Map<number, SourceLike | null>();
    for (const [sourceId, entry] of table) {
        const location = (entry.location as DocNode) ?? {};
        const path = location.at === "file" ? String(location.path ?? "") : "";
        buffers.set(sourceId, path ? await readTake(path, folder, server) : null);
    }
    return (kind: string, config: unknown): unknown => {
        const src = (config ?? {}) as DocNode;
        if (kind === "vector") {
            return buffers.get(Math.trunc(Number(src.source ?? -1))) ?? null;
        }
        const reference = src[kind] ?? src.generator;
        if (defs === null || typeof reference !== "string") return null;
        if (typeof defs === "function") return defs(kind, reference);
        return defs[reference] ?? null;
    };
}

/** What {@link sessionResolver} needs to turn a table into things. */
export interface SessionResolverOptions {
    /** The server the takes are read onto; `null` takes the ambient one. */
    server?: unknown;
    /** The session file's own folder, a relative path is resolved against it. */
    folder?: string | null;
    /** What supplies the code a leaf names. */
    defs?: Record<string, unknown> | ((kind: string, reference: string) => unknown) | null;
}

/**
 * One source's file onto the server, or `null` when it cannot be read.
 *
 * A missing file is **not** an error here. Half a session is worth opening — the
 * piece still draws, the other lanes still sound, and the element that could not
 * be resolved comes back frozen the way an unresolved generator does. Throwing
 * instead would make one moved file the difference between a piece and nothing.
 */
async function readTake(
    path: string,
    folder: string | null,
    server: unknown,
): Promise<SourceLike | null> {
    const { Buffer } = await import("../defs/buffer.ts");
    const full = folder && !path.startsWith("/") ? `${folder}/${path}` : path;
    try {
        return await Buffer.read(full, { server: server as never });
    } catch {
        return null;
    }
}

/**
 * The **source table** for an arrangement, built from what its takes actually
 * hold — the table {@link toSession} demands and refuses to guess.
 *
 * Its error message says to build the table "from the arrangement being saved,
 * each buffer element's current source", and this is that sentence as a
 * function, in one place rather than in every script. Each take's buffer is
 * asked where it is: a `Buffer` read from a file knows its `path` and is written
 * as that file, and one allocated in this run is written as **volatile** — it
 * existed only while the page did, and a session that claimed otherwise would
 * reopen with silence where it promised samples. A {@link FrozenSource} reports
 * what the document it came from said, so a session opened and saved again keeps
 * every location it was given.
 *
 * `folder` is the session file's own folder: a path inside it is written
 * **relative**, which is what makes the pair of files movable together, and one
 * outside it stays absolute, because a session must never claim to own the
 * user's own file.
 */
export function sourcesOf(
    element: Element,
    { folder = null }: { folder?: string | null } = {},
): Map<number, DocNode> {
    const table = new Map<number, DocNode>();
    for (const src of sourceObjects(element)) {
        const entry: DocNode = {
            location: locationOf(src, folder),
            lifetime: src.lifetime ?? "session",
            generation: Math.trunc(Number(src.generation ?? 0)),
        };
        if (src.channels) entry.channels = Math.trunc(Number(src.channels));
        if (src.frames) entry.frames = Math.trunc(Number(src.frames));
        if (src.sampleRate) entry.sample_rate = Number(src.sampleRate);
        table.set(Math.trunc(Number(src.bufnum ?? 0)) || 0, entry);
    }
    return table;
}

/** Where one source's samples are, as the crate's `session::Location`. */
function locationOf(src: SourceLike, folder: string | null): DocNode {
    const path = src.path;
    if (typeof path !== "string" || !path) return { at: "volatile" };
    if (folder) {
        const base = folder.endsWith("/") ? folder : `${folder}/`;
        if (path.startsWith(base)) return { at: "file", path: path.slice(base.length) };
    }
    return { at: "file", path };
}

/**
 * Every take's source in this arrangement, in the order the walk meets them, one
 * entry per source id — a take placed twice is one source.
 */
function sourceObjects(element: Element): SourceLike[] {
    const found = new Map<number, SourceLike>();
    const stack: Element[] = [element];
    while (stack.length > 0) {
        const current = stack.pop() as Element;
        for (const src of sourcesIn(current)) {
            const id = Math.trunc(Number(src.bufnum ?? 0)) || 0;
            if (!found.has(id)) found.set(id, src);
        }
        if (current instanceof Aggregate) {
            for (const handle of current.handles) stack.push(handle.element);
        } else if (current instanceof Sequence && Array.isArray(current.wraps)) {
            for (const item of current.wraps) {
                if (item instanceof Element) stack.push(item);
            }
        } else if (current instanceof Generator && current.rendered !== null) {
            stack.push(current.rendered);
        }
    }
    return [...found.values()];
}

/**
 * The sources one element names itself — a take's buffer, or one per window of a
 * `Segments`.
 */
function sourcesIn(element: Element): SourceLike[] {
    if (element instanceof Vector) {
        return element.wraps === null || element.wraps === undefined
            ? []
            : [element.wraps as SourceLike];
    }
    if (element instanceof Segments) {
        return element.segments
            .filter((seg) => seg.buffer !== null && seg.buffer !== undefined)
            .map((seg) => seg.buffer);
    }
    return [];
}

/**
 * Rebuilds an arrangement from a document — what {@link toDocument} produces, or
 * what the crate wrote.
 *
 * `resolve(kind, config)` supplies the leaves whose configuration *names*
 * something this process must supply — a generator's def, a pattern. Returning
 * `null` (or passing no resolver) leaves the reference itself in place, which is
 * the frozen case rather than an error.
 */
export function fromDocument(
    document: DocumentJson,
    { resolve }: { resolve?: Resolver } = {},
): Element {
    // **Content first, because the tree names it.** A window onto a node reads
    // material the document holds once and several windows share, so it is built
    // before the tree and handed down — and every window over one node gets the
    // *same* object, which is the whole point: two halves of a cut edit one
    // timeline, and reopening a piece must not hand them two.
    const content = new Map<number, Element>();
    for (const held of document.content ?? []) {
        const built = fromNode(held, resolve ?? null);
        content.set(Number(held.id), built);
        setDocId(built.wraps ?? built, Number(held.id));
    }
    return fromNode(document.root, resolve ?? null, false, content);
}

// ---- arrangement -> document ----

/**
 * Node ids for one conversion: whatever an element already carries, and a fresh
 * number past all of them for one that does not.
 *
 * Allocating past the maximum already stamped is what keeps a second conversion
 * stable — a new element added between two conversions cannot take an id an
 * existing element is still using.
 *
 * **An id names one element, and this is where that is enforced.** The number is
 * stamped on the element object, and numbering starts at 1 for every root, so
 * two arrangements built in one script both hold 1, 2, 3 — and source authored
 * in one and used in the other arrives carrying a number a different element
 * here already holds. Nothing downstream survives that: an intent naming the id
 * reaches whichever node the crate's lookup finds first while the editor's index
 * keeps the last, so one gesture writes two places. The walk therefore **claims**
 * each id for the object it first meets carrying it, and an object that turns up
 * with an id already claimed by another is renumbered.
 *
 * Two things it deliberately does not do. It does not touch the *same* object
 * appearing twice — two placements of one element are one node with one id,
 * which is a question about what an id identifies and is open in the document
 * crate's plan, not something to settle by accident here. And it does not
 * renumber the first claimant, so a tree converted on its own is numbered
 * exactly as it always was.
 *
 * The cost of renumbering, stated because it is real: a log entry recorded
 * earlier against the moved element's old number no longer names it. It happens
 * only when source crosses between trees, it stamps a number nothing else in
 * this tree holds, and the editor re-derives its index from the document on
 * every edit — so what is at risk is undo of an edit made before the crossing,
 * not the current one.
 */
class Ids {
    next = 1;
    private owner = new Map<number, object>();
    private renumber = new Set<object>();
    /**
     * Elements already met as a placement, so a second one is checked against
     * what may be placed twice at all.
     */
    placed = new Set<object>();

    constructor(root: Element) {
        this.scan(root, null);
    }

    private scan(element: Element, member: Member | null): void {
        const holder: object = member ?? element;
        const existing = docIdOf(holder);
        if (existing !== null) {
            const owner = this.owner.get(existing);
            if (owner === undefined) {
                this.owner.set(existing, holder);
                this.next = Math.max(this.next, existing + 1);
            } else if (owner === holder) {
                this.next = Math.max(this.next, existing + 1);
            } else {
                // Another object in this tree claimed the number first, so this
                // one was numbered against a tree that is not this one.
                this.renumber.add(holder);
            }
        }
        if (element instanceof Aggregate) {
            for (const handle of element.handles) this.scan(handle.element, handle);
            return;
        }
        for (const child of children(element)) this.scan(child as Element, null);
    }

    /**
     * The id of the node this element occupies — **the placement's** when it is
     * placed, since a clip is a window onto source and what an edit names is the
     * window.
     *
     * An element reached any other way (the root, a rendered subtree, an item of
     * a sequence) carries its own, which is the same rule read where there is no
     * placement to name.
     */
    of(element: object, member: Member | null = null): number {
        const holder: object = member ?? element;
        const existing = docIdOf(holder);
        if (existing !== null && !this.renumber.has(holder)) return existing;
        const assigned = this.next;
        this.next += 1;
        this.owner.set(assigned, holder);
        this.renumber.delete(holder);
        setDocId(holder, assigned);
        return assigned;
    }
}

/**
 * What below this element carries an id of its own.
 *
 * A `Track`'s timeline items are not `Element`s, but they *are* nodes in the
 * document (a note is addressable, or no edit could name it and no log could
 * invert it), so they take ids the same way — and the scan has to see them, or a
 * second conversion would hand them numbers the first did not.
 */
function children(element: Element): object[] {
    if (element instanceof Aggregate) return element.handles.map((m) => m.element);
    if (element instanceof Track) {
        return timelineItems(element.wraps).map(([, item]) => item as object);
    }
    if (element instanceof Sequence && Array.isArray(element.wraps)) {
        return (element.wraps as unknown[]).filter((item) => item instanceof Element);
    }
    if (element instanceof Generator && element.rendered !== null) {
        // The last rendered result is ordinary tree, so its nodes take ids like
        // any others -- and the scan has to see them, or a second conversion
        // would renumber a subtree the first had already stamped.
        return [element.rendered];
    }
    return [];
}

/**
 * One element as a document node: the temporal metadata every node has, plus the
 * body that says what it is.
 */
function node(
    element: Element,
    ids: Ids,
    member: Member | null = null,
    shared: Map<Timeline, Content> | null = null,
): DocNode {
    const out: DocNode = { id: ids.of(element, member) };
    if (typeof element.name === "string" && element.name) {
        // A referenceable label, never a second identity -- the server's own
        // rule for an aggregate's name, and the reason a reopened piece can
        // still label its lanes the way it was authored.
        out.name = element.name;
    }
    if (element.onset !== null) out.onset = Number(element.onset);
    if (element.duration !== null) out.duration = Number(element.duration);
    if (element.resident) out.resident = true;
    Object.assign(out, body(element, ids, shared));
    return out;
}

/** The keys `node` writes itself; a preserved body must not restate them. */
const TEMPORAL = new Set(["id", "name", "onset", "duration", "resident"]);

/**
 * What a `Track` is, in the set body's opaque config. The document has one set
 * kind and goes on having one -- a track is *a set with the restrictions of a
 * multitrack view*, and the tree deliberately carries no view. But a writer that
 * has such a set must get it back, or a round trip turns every track into a
 * plain set and the piece reopens with a level of nesting nobody wrote. So the
 * restriction travels the way a leaf's code does: carried, uninterpreted.
 */
export const FORM_TRACK = "track";

function kindBody(element: Element, ids: Ids, shared: Map<Timeline, Content> | null = null): DocNode {
    const kept = preserved(element);
    if (kept !== null) {
        // A body this build does not know, on its way back out untouched.
        return Object.fromEntries(
            Object.entries(kept).filter(([key]) => !TEMPORAL.has(key)),
        );
    }
    if (element instanceof Track) {
        const entry = shared?.get(element.timeline);
        if (entry !== undefined) {
            // **This timeline is content, so this element is a window onto it.**
            // More than one element reads it, and writing the notes once per
            // reader would write one identity twice — so the notes are in
            // `content` and each reader names the node. The *element's* length
            // stays the node's own, absent when nobody stated one; the window's
            // length is how much of the material it can show, which is what a
            // reader that does not resolve the content lays the clip out with.
            const start = Number(element.start);
            const length =
                element.duration !== null
                    ? Number(element.duration)
                    : Math.max(0.0, Number(element.timeline.duration()) - start);
            return withConfig(
                {
                    kind: "segments",
                    segments: [
                        { source: { node: entry.id }, start, duration: length },
                    ],
                },
                { form: FORM_TRACK },
            );
        }
        // A Set with the restrictions of a multitrack view, and its items are
        // placed elements like any others -- which is what makes a note in a
        // roll addressable, and therefore editable and undoable. Which
        // restrictions those are is the client's own business, so it rides in
        // the body's opaque config and the document never reads it.
        return withConfig(
            {
                kind: "aggregate",
                grouping: CONCRETE,
                members: timelineItems(element.wraps).map(([beat, item]) =>
                    timelineMember(beat, item, ids),
                ),
            },
            // The **window** onto the timeline, written only when there is one
            // — the beats counterpart of a vector's `start`, and through the
            // same door: the config carries what the document does not
            // interpret. A track saying nothing about a window reads its
            // timeline from the beginning, which is every track written before
            // windows existed.
            element.start ? { form: FORM_TRACK, start: element.start } : { form: FORM_TRACK },
        );
    }
    if (element instanceof Aggregate) {
        const body = {
            kind: "aggregate",
            grouping: element.kind === LOGICAL ? LOGICAL : CONCRETE,
            members: element.handles.map((handle) => placement(handle, ids, shared)),
        };
        // A logical aggregate's **declared buses** ride in the body's opaque
        // config, the same door a `Track`'s restrictions use: they are the
        // writer's own wiring, carried and never read. Without this a patch lost
        // its buses on every round trip — the cords survived (a member's
        // controls are in its own config) while the buses they name did not, so
        // a reopened patcher drew the connections and could render none of them.
        // And an edit no format carries is an edit no history can invert.
        const buses = element.busSpecList;
        return buses.length > 0 ? withConfig(body, { buses }) : body;
    }
    if (element instanceof Clang) {
        return withConfig({ kind: "clang" }, plain(itemConfig(element.wraps)) as DocNode);
    }
    if (element instanceof Sequence) {
        const items = element.wraps;
        if (Array.isArray(items) && items.every((i) => i instanceof Element)) {
            return {
                kind: "sequence",
                // A sequence's items are *elements in order*, not placements —
                // there is no handle to name, so each node's id is its own.
                members: (items as Element[]).map((i) => ({
                    offset: 0.0,
                    node: node(i, ids),
                })),
            };
        }
        // A pattern, or a list of values the client owns: a reference, not a
        // serialization. A leaf with no name is written with no reference:
        // frozen, and the same bytes on every run of the same script.
        return withConfig(
            { kind: "sequence" },
            named({ sequence: reference(items, element) }),
        );
    }
    if (element instanceof Segments) {
        // Several windows read as one: the source is the **list**, each entry
        // naming its own source and its own window into it. One node, because
        // what this element is is one thing to play.
        const out: DocNode = {
            kind: "segments",
            segments: element.segments.map((seg) => ({
                source: source(seg.buffer),
                start: Number(seg.start),
                duration: Number(seg.duration),
            })),
        };
        const config: DocNode = {};
        if (element.instrument !== null) {
            const instrument = reference(element.instrument);
            if (instrument !== null) config.instrument = instrument;
        }
        if (Object.keys(element.controls).length > 0) {
            config.controls = plain(element.controls);
        }
        return withConfig(out, Object.keys(config).length > 0 ? config : null);
    }
    if (element instanceof Vector) {
        const out: DocNode = { kind: "vector", source: source(element.buffer) };
        const config: DocNode = {};
        if (element.instrument !== null) {
            const instrument = reference(element.instrument);
            if (instrument !== null) config.instrument = instrument;
        }
        if (Object.keys(element.controls).length > 0) {
            config.controls = plain(element.controls);
        }
        // The **window** onto the source, written only when it is not the whole
        // of it: a document saying nothing about a window means one that reads
        // the buffer from its first frame, which is every take written before
        // windows existed.
        if (element.start) config.start = Number(element.start);
        if (element.loop) config.loop = true;
        return withConfig(out, Object.keys(config).length > 0 ? config : null);
    }
    if (element instanceof Generator) {
        const config: DocNode = named({
            generator: reference(element.wraps, element),
        });
        if (element.controls && Object.keys(element.controls).length > 0) {
            config.controls = plain(element.controls);
        }
        if (element.maps && Object.keys(element.maps).length > 0) {
            config.maps = plain(element.maps);
        }
        const out: DocNode = { kind: "generator" };
        if (element.rendered !== null) {
            // What the generator last produced, as ordinary tree. A host with no
            // language attached has nothing to run the generator with, so this
            // is the whole of what it can show.
            out.rendered = node(element.rendered, ids);
        }
        return withConfig(out, config);
    }
    // A base `Element` wrapping something this module has no body for. It
    // becomes an opaque leaf rather than an error, which is the format's own
    // rule read from this side: **what a writer does not understand, it
    // preserves**. The alternative was found by routing the editor's own edits
    // through the document -- an arrangement is free to hold an element kind the
    // conversion predates, and refusing to convert would make the whole
    // composition uneditable because one leaf in it is unfamiliar.
    // Under the **same config key** a `Generator` writes, because this is the
    // same body kind and the key is what a reader resolves on: writing a second
    // name for it made a round trip change the leaf's key (`element` on the way
    // out of a hand-written tree, `generator` on the way out of the one that
    // came back), so a resolver that recognized the source once stopped
    // recognizing it on the second open.
    return withConfig(
        { kind: "generator" },
        named({
            generator: reference(element.wraps, element),
            points: pointsOf(element.wraps),
        }),
    );
}

/**
 * The raw node a {@link fromDocument} kept for a body this build cannot name, or
 * `null` for an element it understands.
 */
function preserved(element: Element): DocNode | null {
    if (Object.getPrototypeOf(element) !== Element.prototype) return null;
    // **A raw node is a plain object, and nothing else is.** The Python client
    // spells this `isinstance(element.wraps, dict)`; `typeof x === "object"` is
    // not that test — it is true of every instance there is, so a base
    // `Element` wrapping an `Automation` (the arrangement's own way of holding
    // a curve) was written out as *the automation's fields* instead of as the
    // generator node with its break-points. The two clients wrote different
    // documents for one composition, and the edit that follows the write had
    // nothing to configure.
    const wraps = element.wraps as object | null;
    if (wraps === null || typeof wraps !== "object") return null;
    const proto = Object.getPrototypeOf(wraps);
    return proto === Object.prototype || proto === null ? (wraps as DocNode) : null;
}

/**
 * One placement: where it sits, and the node it holds — whose id is the
 * **handle's**, so one element placed twice is two windows and not one
 * ambiguous name.
 */
function placement(
    handle: Member,
    ids: Ids,
    shared: Map<Timeline, Content> | null = null,
): DocNode {
    placeableTwice(handle, ids);
    const out: DocNode = {
        offset: Number(handle.offset),
        node: node(handle.element, ids, handle, shared),
    };
    if (handle.dur !== null) out.dur = Number(handle.dur);
    return out;
}

/**
 * Refuses a *second* placement of an element whose source is in the node.
 *
 * Two windows share source only when the node **references** it — a buffer names
 * a source, a generator names a recipe, and both placements point at the one
 * thing. A clang, a track or an aggregate carries its source *inside* the node,
 * so a second placement is a second **copy**: they diverge on the first edit,
 * which is the answer the open decision rejected. Refused with the distinction
 * rather than copied in silence.
 */
function placeableTwice(handle: Member, ids: Ids): void {
    const element = handle.element;
    if (!ids.placed.has(element)) {
        ids.placed.add(element);
        return;
    }
    if (
        element instanceof Vector ||
        element instanceof Generator ||
        (element instanceof Sequence && !Array.isArray(element.wraps))
    ) {
        return; // a window onto source the node only names
    }
    throw new Error(
        `${element.constructor.name} is placed more than once, and its source ` +
            "is in the node rather than named by it — two placements would be " +
            "two copies that diverge on the first edit. Place a leaf that " +
            "*references* its source (a Vector over one server buffer, a " +
            "Generator over one recipe), or give each placement its own element.",
    );
}

/**
 * A timeline item as a placed clang, with an id stamped on the item itself so it
 * survives to the next conversion.
 *
 * **Whatever the item is.** A clang is "parameters or actions that happen
 * together" and its configuration is the client's own terms, which is what an
 * OSC marker and a raw MIDI message are as much as a note — so all three travel
 * as the one description {@link itemData} writes, and come back as themselves.
 * Handing the item over raw wrote a marker as the *name* that answered for it,
 * which reopened as a note with no parameters: a lane a piece could draw and not
 * save.
 */
function timelineMember(beat: number, item: unknown, ids: Ids): DocNode {
    const out: DocNode = { id: ids.of(item as object) };
    Object.assign(out, withConfig({ kind: "clang" }, plain(itemConfig(item)) as DocNode));
    return { offset: Number(beat), node: out };
}

/**
 * One timeline item as the config a clang carries.
 *
 * A note is its own parameters and {@link plain} already knows how to spell
 * them; a marker names itself, through the one shared description
 * ({@link itemData}). Anything else is written as the reference it always was.
 */
function itemConfig(item: unknown): unknown {
    if (item instanceof SeqEvent) return item;
    return itemData(item) ?? item;
}

/**
 * `[beat, item]` pairs from a `seq.Timeline`, or nothing when a `Track` wraps
 * something else.
 */
function timelineItems(timeline: unknown): [number, unknown][] {
    if (timeline === null || timeline === undefined) return [];
    if (!(timeline instanceof Timeline)) return [];
    return [...timeline];
}

/**
 * The mixing keys a node's configuration carries, and their defaults. A
 * configuration is written **whole**, so a key that is not there is the default
 * — audible, unsoloed, at unit gain.
 */
export const MIXING: Record<string, boolean | number> = {
    mute: false,
    solo: false,
    level: 1.0,
};

/**
 * One element's body — what kind of thing it is — with the **mixing** the
 * composition holds over it laid into its configuration.
 *
 * Mute, solo and level go through the same opaque door a leaf's code and a
 * track's restrictions use: the document carries them and never reads them,
 * because what a level *means* is the client's. They ride in the config rather
 * than beside the temporal keys so that {@link leafConfig} picks them up — a
 * `Configure` intent replaces a configuration whole, and one that started from a
 * config without them would silence-then-unsilence a lane on every curve edit.
 */
function body(element: Element, ids: Ids, shared: Map<Timeline, Content> | null = null): DocNode {
    const out = kindBody(element, ids, shared);
    const mixing = mixingOf(element);
    if (Object.keys(mixing).length > 0) {
        out.config = { ...((out.config as DocNode) ?? {}), ...mixing };
    }
    return out;
}

/**
 * What of {@link MIXING} this element states — only what differs from the
 * default, so an ordinary element writes no mixing at all and a file written
 * before mixing existed reads back identical.
 */
export function mixingOf(element: Element): DocNode {
    const stated: DocNode = {};
    if (element.mute) stated.mute = true;
    if (element.solo) stated.solo = true;
    if (Number(element.level) !== 1.0) stated.level = Number(element.level);
    return stated;
}

/**
 * Writes a node's mixing onto the element, **whole**: a key the configuration
 * does not carry is the default, which is the same rule every other `Configure`
 * follows.
 */
export function setMixing(element: Element, config: DocNode): void {
    element.mute = Boolean(config?.mute ?? false);
    element.solo = Boolean(config?.solo ?? false);
    element.level = Number(config?.level ?? 1.0);
}

/**
 * A source a document names and this process does not hold.
 *
 * A `Vector` element wraps a `Buffer`; reading a document written elsewhere (or
 * written here before the buffer was allocated) gives the reference and not the
 * object. Rather than losing it, the element wraps this: the same `bufnum` a
 * real buffer answers with, plus the lifetime and generation the document
 * carried, so a re-conversion is faithful and a caller that *can* resolve it does
 * so through `resolve`.
 */
export class FrozenSource implements SourceLike {
    readonly bufnum: number;
    lifetime: string;
    generation: number;
    /**
     * What the **session's table** said about this source, when it was read from
     * one: where the samples are, and their shape. It is what makes opening a
     * session and saving it again keep every location it was given — without it,
     * a piece opened with no resolver (or with one that could not read a file)
     * would be written back with every take marked volatile, which is a format
     * that loses its own contents on the second save.
     */
    path: string | null = null;
    frames = 0;
    channels = 0;
    sampleRate = 0;

    constructor(src: DocNode, entry: DocNode | null = null) {
        this.bufnum = Math.trunc(Number(src?.source ?? 0));
        this.lifetime = String(src?.lifetime ?? "session");
        this.generation = Math.trunc(Number(src?.generation ?? 0));
        this.locate(entry);
    }

    /** Takes where and what this source is from a session table entry. */
    locate(entry: DocNode | null | undefined): void {
        if (!entry) return;
        const location = (entry.location as DocNode) ?? {};
        if (location.at === "file" && location.path) this.path = String(location.path);
        this.lifetime = String(entry.lifetime ?? this.lifetime);
        this.generation = Math.trunc(Number(entry.generation ?? this.generation)) || 0;
        this.frames = Math.trunc(Number(entry.frames ?? 0)) || 0;
        this.channels = Math.trunc(Number(entry.channels ?? 0)) || 0;
        this.sampleRate = Number(entry.sample_rate ?? 0) || 0;
    }
}

/**
 * A buffer element's source. A server buffer the user allocated is **session**
 * source -- neither the external-file rule nor a scratch copy -- and a
 * {@link FrozenSource} reports whatever the document said instead.
 */
function source(buffer: SourceLike): DocNode {
    return {
        source: Math.trunc(Number(buffer?.bufnum ?? 0)) || 0,
        lifetime: buffer?.lifetime ?? "session",
        generation: Math.trunc(Number(buffer?.generation ?? 0)),
    };
}

/**
 * The configuration a leaf's node carries, exactly as {@link toDocument} writes
 * it.
 *
 * Public because an **editor** needs it: a `Configure` intent replaces a leaf's
 * configuration *whole*, so an editor that wants to change one field of it has
 * to start from the rest — and re-deriving that here rather than in the editor
 * is what keeps one description of what a leaf's config is.
 */
export function leafConfig(element: Element): DocNode {
    return { ...((body(element, new Ids(element)).config as DocNode) ?? {}) };
}

/**
 * A leaf's **whole node body**, exactly as {@link toDocument} writes it — its
 * kind, its source and its configuration, with no id (the id belongs to the
 * placement that holds it).
 *
 * Public for the same reason {@link leafConfig} is, one step further out: an
 * edit that replaces *what a placement holds* — a run of clips joined into one
 * element — states the result as a member list, and a member carries the node.
 * Re-deriving that in the editor would be a second description of what a leaf is
 * written as.
 */
export function leafNode(element: Element): DocNode {
    const out = { ...body(element, new Ids(element)) };
    delete out.id;
    return out;
}

/**
 * The first node id no element in this arrangement holds.
 *
 * What an editor mints from when it has to name a node the conversion has not
 * seen yet — a note added by a gesture. It follows the conversion's own rule
 * (past the maximum already stamped), so a minted id and a converted one cannot
 * collide.
 */
export function nextNodeId(element: Element): number {
    return new Ids(element).next;
}

/**
 * A curve's break-points, when the leaf is one — or `null`.
 *
 * **The document has to carry these, and not only draw them.** A curve is a leaf
 * like any other and its configuration is opaque, but an edit to it is a
 * `Configure` intent, and an intent's inverse is *the previous value read out of
 * the document*: with nothing there, a dragged break-point had nothing to invert
 * and could not be undone. Carrying them also makes an edited curve survive a
 * save, which it did not — reopening resolved the automation by name and took
 * whatever envelope that object happened to hold.
 */
function pointsOf(wrapped: unknown): number[] | null {
    const toPoints = (wrapped as { toPoints?: unknown } | null)?.toPoints;
    if (typeof toPoints !== "function") return null;
    let points: number[];
    try {
        points = [...(toPoints.call(wrapped) as Iterable<unknown>)].map((v) => Number(v));
    } catch {
        // A leaf is opaque, and reading one must never be able to take a save
        // down: an object that answers to the name and not to the shape is
        // carried by reference like any other, with no points.
        return null;
    }
    if (points.some((v) => Number.isNaN(v))) return null;
    return points.length > 0 ? points : null;
}

/**
 * A config with the keys whose value is `null` dropped — a reference nothing
 * could name is left out rather than written as null, so an unnamed leaf and a
 * leaf named nothing are the same file.
 */
function named(config: DocNode): DocNode {
    return Object.fromEntries(
        Object.entries(config).filter(([, value]) => value !== null && value !== undefined),
    );
}

function withConfig(out: DocNode, config: DocNode | null): DocNode {
    if (config !== null && Object.keys(config).length > 0) out.config = config;
    return out;
}

/**
 * What names an object the document does not own — or `null` when nothing does,
 * which is the honest answer.
 *
 * A leaf is opaque by decision: the document carries a *reference* to an
 * algorithm and never the algorithm, so reopening hands the reference to a
 * resolver and takes back whatever that resolver has. The reference therefore
 * has to be something a caller **can produce**. Three sources, in order: the
 * object's own name (a def, an `Automation`), the element's `name` (what an
 * author writes for source that has none of its own — a `Pbind` is code and
 * carries no name), and nothing.
 *
 * **Nothing is better than a printed object**: an identity nothing can resolve
 * is unresolvable by construction *and* different between two runs of the same
 * script, so it would break the format's determinism to hand a resolver a key
 * that could never match. An unnamed leaf is written with no reference at all
 * and comes back frozen — drawn, placed, silent — which is what a composition
 * means where its language is not running.
 */
function reference(obj: unknown, element: Element | null = null): string | null {
    if (typeof obj === "string") return obj;
    const own = (obj as { name?: unknown } | null)?.name;
    if (typeof own === "string" && own) return own;
    const label = element?.name;
    return typeof label === "string" && label ? label : null;
}

/**
 * A value as plain JSON-able data, leaving anything else as its reference.
 *
 * A `seq.Event` travels as the object of its parameters, under the **document's
 * own spelling** of the two keys this language renamed: the file says
 * `add_action`/`has_gate`, as the wire and the Python client do, so one
 * composition reads the same in both clients. Every other key is a control name
 * and belongs to the def, so it crosses untouched.
 */
function plain(value: unknown): any {
    if (value instanceof SeqEvent) return plain(eventProps(value.props));
    if (Array.isArray(value)) return value.map((v) => plain(v));
    if (value === null || value === undefined) return null;
    const type = typeof value;
    if (type === "string" || type === "number" || type === "boolean") return value;
    if (type === "object" && Object.getPrototypeOf(value) === Object.prototype) {
        return Object.fromEntries(
            Object.entries(value as Record<string, unknown>).map(([k, v]) => [
                String(k),
                plain(v),
            ]),
        );
    }
    return reference(value);
}

/**
 * The two event keys whose spelling differs between the clients, as
 * `thisLanguage -> the document`. Nothing else in an event is renamed: the rest
 * are the def's control names, which are one string in every language.
 */
const EVENT_KEYS: Record<string, string> = {
    addAction: "add_action",
    hasGate: "has_gate",
};

/** The reverse of {@link EVENT_KEYS}, for a document on its way back in. */
const EVENT_KEYS_BACK: Record<string, string> = Object.fromEntries(
    Object.entries(EVENT_KEYS).map(([k, v]) => [v, k]),
);

function eventProps(props: Record<string, unknown>): Record<string, unknown> {
    return Object.fromEntries(
        Object.entries(props).map(([k, v]) => [EVENT_KEYS[k] ?? k, v]),
    );
}

/** A document's event config, back in this language's spelling. */
function eventConfig(config: DocNode): Record<string, unknown> {
    return Object.fromEntries(
        Object.entries(config).map(([k, v]) => [EVENT_KEYS_BACK[k] ?? k, v]),
    );
}

// ---- document -> arrangement ----

function fromNode(
    src: DocNode,
    resolve: Resolver | null,
    placed = false,
    content: Map<number, Element> | null = null,
): Element {
    const kind = src.kind;
    const config: DocNode = src.config ?? {};
    const onset = src.onset ?? null;
    const duration = src.duration ?? null;
    let built: Element;

    if (kind === "aggregate" && config.form === FORM_TRACK) {
        // A set the author wrote as a `Track`, said by the body's own config.
        // Rebuilding it as an `Aggregate` is what made a reopened piece grow a
        // level of nesting nobody wrote, and left the editor drawing a lane of
        // clips where there had been a roll.
        const timeline = new Timeline();
        for (const member of (src.members as DocNode[]) ?? []) {
            const child = member.node as DocNode;
            const leaf = fromNode(child, resolve);
            // A timeline holds the client's own sequencing items, not elements:
            // what went out as a placed event comes back as the event itself.
            const item = leaf.wraps ?? leaf;
            if ("id" in child) {
                // The id belongs to the item, which is what the conversion
                // stamped on the way out -- so a note keeps its number across a
                // save, and an intent recorded against it still names it.
                setDocId(item, Math.trunc(Number(child.id)));
            }
            timeline.add(Number(member.offset ?? 0.0), item);
        }
        built = new Track(timeline, onset, duration,
                          { start: Number(config.start ?? 0) || 0 });
    } else if (kind === "aggregate") {
        const aggregate = new Aggregate(null, src.grouping === LOGICAL ? LOGICAL : CONCRETE, {
            onset,
            duration,
            buses: (config.buses as BusSpec[] | undefined) ?? undefined,
        });
        for (const member of (src.members as DocNode[]) ?? []) {
            const child = member.node as DocNode;
            const handle = aggregate.add(
                fromNode(child, resolve, true, content),
                Number(member.offset ?? 0.0),
                member.dur === undefined || member.dur === null ? null : Number(member.dur),
            );
            if ("id" in child) {
                // The placement's id, on the placement: a second window onto the
                // same source is a second handle with a number of its own.
                setDocId(handle, Math.trunc(Number(child.id)));
            }
        }
        built = aggregate;
    } else if (kind === "clang") {
        // An OSC marker and a raw MIDI message are clangs too, and each names
        // itself in its config -- see `timelineMember`.
        const named = OSC_KEY in config || MIDI_KEY in config;
        built = new Clang(
            named
                ? (itemFromData(config as Record<string, unknown>) as SeqEvent)
                : new SeqEvent(eventConfig(config)),
            onset,
            duration,
        );
    } else if (kind === "sequence") {
        const members = src.members as DocNode[] | undefined;
        const items =
            members && members.length > 0
                ? members.map((m) => fromNode(m.node as DocNode, resolve, false, content))
                : (resolved(resolve, "sequence", config) || config.sequence);
        built = new Sequence(items, onset, duration);
    } else if (kind === "segments") {
        const windows = (src.segments as DocNode[]) ?? [];
        const overNodes = windows.filter(
            (w) => typeof w.source === "object" && w.source !== null &&
                "node" in (w.source as DocNode),
        );
        if (overNodes.length > 0) {
            // **A window onto content**: the material is a node of this
            // document, built once and shared by every window that names it, so
            // this element is a `Track` reading that timeline from `start`. Its
            // own length is the node's — absent when nobody stated one — and the
            // window's is how much of the material it can show, which is what a
            // reader that does not resolve the content lays it out with.
            if (overNodes.length > 1) {
                throw new Error(
                    "a run of windows onto several nodes is not built yet: the " +
                        "element for it is `NoteSegments` (../segments.ts), and " +
                        "what makes one is a join across timelines",
                );
            }
            const window = overNodes[0] as DocNode;
            const named = Number((window.source as DocNode).node);
            const held = content?.get(named);
            const timeline = held?.wraps;
            if (!(timeline instanceof Timeline)) {
                throw new Error(
                    `a window names content node ${named}, which this document ` +
                        "does not hold: a window and the material it reads are " +
                        "written together",
                );
            }
            built = new Track(timeline, onset, duration, {
                start: Number(window.start ?? 0.0),
            });
        } else {
        built = new Segments(
            ((src.segments as DocNode[]) ?? []).map(
                (seg) =>
                    [
                        (resolved(resolve, "vector", seg.source) as SourceLike) ||
                            new FrozenSource((seg.source as DocNode) ?? {}),
                        Number(seg.start ?? 0.0),
                        Number(seg.duration ?? 0.0),
                    ] as const,
            ),
            onset,
            duration,
            {
                instrument: (config.instrument as string) ?? null,
                controls: (config.controls as Record<string, unknown>) ?? null,
            },
        );
        }
    } else if (kind === "vector") {
        built = new Vector(
            (resolved(resolve, "vector", src.source) as SourceLike) ||
                new FrozenSource((src.source as DocNode) ?? {}),
            onset,
            duration,
            {
                instrument: (config.instrument as string) ?? null,
                controls: (config.controls as Record<string, unknown>) ?? null,
                start: Number(config.start ?? 0.0),
                loop: Boolean(config.loop ?? false),
            },
        );
    } else if (kind === "generator") {
        const rendered = src.rendered as DocNode | undefined;
        const supplied = resolved(resolve, "generator", config);
        applyPoints(supplied, config.points as number[] | undefined);
        built = new Generator(
            // `element` is what this client wrote for a leaf it had no body for
            // before the two keys became one; a file carrying it still opens.
            supplied || config.generator || config.element || null,
            onset,
            duration,
            {
                controls: (config.controls as Record<string, unknown>) ?? null,
                maps: (config.maps as Record<string, string>) ?? null,
                rendered:
                    rendered === undefined || rendered === null
                        ? null
                        : fromNode(rendered, resolve),
            },
        );
    } else {
        // A body this build does not know. The document preserves it whole and
        // so does this side: it comes back as an abstract element carrying the
        // payload, so a round trip through an older client does not lose it.
        built = new Element({ ...src }, onset, duration);
    }

    // The document is the authority on temporal metadata: an element's own
    // constructor may derive a duration (a `Clang` takes the event's `dur` when
    // none is given), and letting that win would make a document say something
    // the document did not say.
    built.onset = onset === null ? null : Number(onset);
    built.duration = duration === null ? null : Number(duration);
    if (src.resident) built.resident = true;
    // The composition's mixing, restored the way it was written: whole, so a
    // document that says nothing says the audible default.
    setMixing(built, config);
    const name = src.name;
    if (typeof name === "string" && name) {
        // A label, not an identity: it says what the node is and nothing
        // addresses by it, so restoring it is what lets a reopened piece label
        // its lanes the way it was authored.
        built.name = name;
    }
    if ("id" in src && !placed) {
        // An element reached as a placement takes no id of its own: the number
        // is the window's, and its handle is what carries it.
        setDocId(built, Math.trunc(Number(src.id)));
    }
    return built;
}

/**
 * Puts a carried curve back onto the source that was handed to us.
 *
 * The document is the authority for what it holds: a resolver returns the
 * `seq.Automation` this process has, and the envelope *in the file* is the one
 * that was saved — without this, reopening a session showed the curve the script
 * last built rather than the curve the piece was left with.
 */
function applyPoints(supplied: unknown, points: number[] | undefined): void {
    if (!points || points.length === 0) return;
    if (supplied === null || supplied === undefined) return;
    if (!(typeof supplied === "object" && "env" in supplied)) return;
    (supplied as { env: unknown }).env = pointsToEnv([...points]);
}

/**
 * Whatever the caller supplies for a leaf the document only names, or `null`
 * when nobody can supply it — the frozen case.
 */
function resolved(resolve: Resolver | null, kind: string, config: unknown): unknown {
    return resolve === null ? null : (resolve(kind, config) ?? null);
}
