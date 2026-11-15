/**
 * The session file: a multitrack, saved, and where its samples are.
 *
 * Private: the names are `clausters/document`'s.
 *
 * @module
 */

import {
    sessionMigrate,
} from "../core/clausters_core_web.js";
import { SESSION_APPLICATION, SESSION_FORMAT } from "./format.ts";
import { readFileAt, writeFileAt } from "../base/files.ts";
import { EventSequence } from "../seq/sequence.ts";

import { type Extra, num, unknownFields } from "./format.ts";
import { Multitrack } from "./multitrack.ts";
import { MultitrackView } from "./view.ts";


/** A file's text, where `base/files.ts` keeps files. */
async function readText(path: string): Promise<string> {
    return new TextDecoder().decode(await readFileAt(path));
}

/** Writes a file's text where {@link readText} reads it. */
async function writeText(path: string, text: string): Promise<void> {
    await writeFileAt(path, new TextEncoder().encode(text));
}

// ---- the session: the multitrack, and where its samples are ----
//
// Lifted out of `form/document.ts` rather than written again. What was worth
// keeping there was never the element-to-node conversion -- that is form's
// vocabulary and goes with it -- but this: a source table that says where
// samples are, a frozen reference for one this page does not hold, and a file
// that knows what it is missing. None of it was ever about form's primitives.

/**
 * One entry in a session's source table: where samples are, how long they live,
 * and what shape they have.
 *
 * The two fields a naive format leaves out and then cannot add are here:
 * `provenance`, a reference to whatever produced the samples, carried opaquely
 * so re-generating stays possible *without the document knowing how*; and
 * `editing`, a destructive edit that has not been confirmed -- a save never
 * blocks on a confirmation, so a saved session has to be able to say *this is a
 * working copy of that, and the person has not decided yet*.
 */
export class Source {
    /**
     * `{at: "file", path}`, `{at: "volatile"}` or `{at: "events"}` (the events
     * are {@link Source.sequence}). A relative path is resolved
     * against the session's own folder, which is what makes a session directory
     * movable; an absolute one names the user's own file, which a session must
     * never copy or rewrite.
     */
    location: Extra;
    /**
     * `"external"` (the user's own file), `"session"` (saved beside the
     * document) or `"temporary"` (a working copy that dies with the edit).
     */
    lifetime: string;
    /** Which generation of the content this is -- bumped by a destructive edit. */
    generation: number;
    channels?: number;
    frames?: number;
    /** Carried, never acted on: resampling is an edit. */
    sampleRate?: number;
    provenance?: unknown;
    /** `{from, confirmed}` while a destructive edit is open over these samples. */
    editing?: Extra;
    extra: Extra;
    /**
     * **The events, when the source is a sequence of them**: the
     * `EventSequence` itself, held rather than copied, so what an editor does
     * to it is what the next save writes.
     */
    sequence?: EventSequence;

    constructor(fields: {
        location: Extra;
        lifetime?: string;
        generation?: number;
        channels?: number;
        frames?: number;
        sampleRate?: number;
        provenance?: unknown;
        editing?: Extra;
        extra?: Extra;
        sequence?: EventSequence;
    }) {
        this.location = fields.location;
        this.lifetime = fields.lifetime ?? "session";
        this.generation = fields.generation ?? 0;
        this.channels = fields.channels;
        this.frames = fields.frames;
        this.sampleRate = fields.sampleRate;
        this.provenance = fields.provenance;
        this.editing = fields.editing;
        this.extra = fields.extra ?? {};
        this.sequence = fields.sequence;
    }

    /** Samples in a file. */
    static file(path: string, lifetime = "session"): Source {
        return new Source({ location: { at: "file", path }, lifetime });
    }

    /**
     * Samples that have not been written down. A session may hold one, because
     * saving must not be blocked by it, but a reader that finds one knows the
     * samples are not there and opens that element unresolved rather than
     * pretending.
     */
    static volatile(lifetime = "session"): Source {
        return new Source({ location: { at: "volatile" }, lifetime });
    }

    /**
     * A sequence of events -- an `EventSequence` -- held in the session file
     * itself. A region over it is a window onto its beats (in seconds, through
     * its tempo map), and it draws the notes.
     */
    static events(sequence: EventSequence, lifetime = "session"): Source {
        return new Source({ location: { at: "events" }, lifetime, sequence });
    }

    /** Its shape, for a caller that knows it. */
    shaped(channels: number, frames: number, sampleRate: number): Source {
        this.channels = channels;
        this.frames = frames;
        this.sampleRate = sampleRate;
        return this;
    }

    /** Where the file is, when the samples are in one. */
    get path(): string | undefined {
        if (this.location.at !== "file") return undefined;
        return (this.location.path as string) || undefined;
    }

    /**
     * Whether the samples are somewhere a reader could find them. A sequence
     * is in the file itself, so it always is.
     */
    get isResolvable(): boolean {
        return this.sequence !== undefined || this.path !== undefined;
    }

    /** Whether a destructive edit is open and undecided over these samples. */
    get isBeingEdited(): boolean {
        return Boolean(this.editing) && !this.editing!.confirmed;
    }

    write(): Extra {
        const out: Extra = {
            location:
                this.sequence === undefined
                    ? this.location
                    : { at: "events", sequence: this.sequence.data() },
            lifetime: this.lifetime,
            generation: this.generation,
        };
        if (this.channels !== undefined) out.channels = this.channels;
        if (this.frames !== undefined) out.frames = this.frames;
        if (this.sampleRate !== undefined) out.sample_rate = this.sampleRate;
        if (this.provenance !== undefined) out.provenance = this.provenance;
        if (this.editing !== undefined) out.editing = this.editing;
        return { ...out, ...this.extra };
    }

    static read(written: Extra): Source {
        const { sequence, ...location } = (written.location as Extra) ?? { at: "volatile" };
        return new Source({
            location,
            sequence: location.at === "events" ? EventSequence.fromData(sequence ?? {}) : undefined,
            lifetime: String(written.lifetime ?? "session"),
            generation: num(written.generation),
            channels: written.channels as number | undefined,
            frames: written.frames as number | undefined,
            sampleRate: written.sample_rate as number | undefined,
            provenance: written.provenance,
            editing: written.editing as Extra | undefined,
            extra: unknownFields(written, "location", "lifetime", "generation", "channels",
                        "frames", "sample_rate", "provenance", "editing"),
        });
    }
}

/**
 * A source a session names and this page does not hold.
 *
 * Reading a session written elsewhere gives a reference and not an object.
 * Rather than losing it, a region's window holds this: the same `bufnum` a real
 * buffer answers with, plus what the table said about where the samples are and
 * what shape they have, so a re-save keeps every location it was given.
 *
 * Without it, a multitrack opened with no way to read its files would be written back
 * with every source marked volatile, which is a format that loses its own
 * contents on the second save.
 */
export class FrozenSource {
    bufnum: number;
    lifetime = "session";
    generation = 0;
    path?: string;
    frames = 0;
    channels = 0;
    sampleRate = 0;

    constructor(id: number, entry?: Source) {
        this.bufnum = id;
        this.locate(entry);
    }

    /** Take where and what this source is from a session's table entry. */
    locate(entry?: Source): void {
        if (!entry) return;
        this.path = entry.path;
        this.lifetime = entry.lifetime;
        this.generation = entry.generation;
        this.frames = entry.frames ?? 0;
        this.channels = entry.channels ?? 0;
        this.sampleRate = entry.sampleRate ?? 0;
    }
}


/**
 * A session: the multitrack, saved, and where its samples are.
 *
 * An {@link Multitrack} says *what plays when* and deliberately does not say
 * where a source lives, because inside a running system a source is a server
 * buffer, a mapped file or a rendered result and the multitrack has no business
 * knowing which. A session is the multitrack plus exactly that missing half.
 *
 * Not the client's `SessionFile`, which is a connection to a running server. Two
 * nouns, two modules; this one is a **file**.
 */
export class SessionFile {
    /**
     * The format version this build writes -- {@link SESSION_FORMAT}, the crate's
     * `session::FORMAT`. A session **read** in an older format is migrated to
     * this one first ({@link SessionFile.read}), since its numbers are read
     * differently: format 2 placed the multitrack in beats.
     */
    format = SESSION_FORMAT;
    /**
     * What the file says about itself: `{ application: "multitrack" }`, the
     * application of the host it belongs to. A file written before it was
     * named belongs to the multitrack, and one another application's metadata
     * names is refused by {@link SessionFile.read}.
     */
    metadata: Extra = { application: SESSION_APPLICATION };
    /**
     * The multitrack. Always present, possibly empty -- which mirrors the crate,
     * where an absent arrangement reads as an empty one rather than as nothing.
     */
    multitrack = new Multitrack();
    /**
     * How the multitrack was being **looked at**: one entry per window. Carried for
     * the reason every program in the field carries it -- reopening a multitrack into
     * the window it was left in is what a person expects -- and a reader that
     * ignores it opens the same multitrack.
     */
    views: MultitrackView[] = [];
    /**
     * **Where a pass over the multitrack ends**, as a playback's `end` says
     * it: `null` (it rolls on, the default), `"contents"` (where the last
     * region ends) or a number of seconds, an end marker. The transport's own
     * switch, kept so a session opened again stops where it stopped before --
     * not in a view, since it changes what is heard. It bounds no axis: how
     * far a window shows or zooms out reads nothing here.
     */
    end: "contents" | number | null = null;
    /**
     * The general tree, beside the multitrack and never inside it: a
     * multitrack's regions are windows onto sources, and no region places a
     * tree.
     */
    document?: Extra;
    /** Where each source is, keyed by source id. */
    sources = new Map<number, Source>();
    /**
     * What produced the session as a whole -- the scripts behind it -- carried
     * opaquely.
     */
    provenance?: unknown;
    extra: Extra = {};
    /**
     * The file this session was opened from or last saved to, or `null`. Not
     * written into the file: where a session is is not what it says.
     */
    path: string | null = null;

    /**
     * A session read from the file at `path`, remembering where it came from:
     * the multitrack application reads relative paths against that file's
     * folder (`edit`), and {@link SessionFile.save} writes back to it.
     *
     * The path is on the filesystem `Buffer.read` names -- the disk under node,
     * and the page's own storage (`opfs`) in a tab.
     */
    static async open(path: string): Promise<SessionFile> {
        const session = SessionFile.read(JSON.parse(await readText(path)) as Extra);
        session.path = path;
        return session;
    }

    /**
     * Writes the session to `path`, or back to the file it was opened from or
     * last saved to, and answers where it went. Rejects when there is no `path`
     * and the session was never opened or saved.
     */
    async save(path?: string): Promise<string> {
        const target = path ?? this.path;
        if (target === null) {
            throw new Error("clausters: this session has nowhere to save to: give it a path");
        }
        await writeText(target, JSON.stringify(this.write(), null, 1));
        this.path = target;
        return target;
    }

    /** The source a reference names, if the table has it. */
    source(id: number): Source | undefined {
        return this.sources.get(id);
    }

    /**
     * The sources that are sequences of events: source id -> `EventSequence`,
     * the handles the table holds.
     */
    sequences(): Map<number, EventSequence> {
        const out = new Map<number, EventSequence>();
        for (const [id, source] of this.sources) {
            if (source.sequence !== undefined) out.set(id, source.sequence);
        }
        return out;
    }

    /**
     * Sources whose samples are not written down anywhere -- what a save
     * consults before promising the file is complete.
     */
    volatile(): number[] {
        return [...this.sources.entries()]
            .filter(([, source]) => !source.isResolvable)
            .map(([id]) => id)
            .sort((a, b) => a - b);
    }

    /** Sources with a destructive edit still open and undecided. */
    openEdits(): number[] {
        return [...this.sources.entries()]
            .filter(([, source]) => source.isBeingEdited)
            .map(([id]) => id)
            .sort((a, b) => a - b);
    }

    /**
     * Sources the multitrack names but the table does not hold -- what an opening
     * reader reports rather than discovering one element at a time.
     *
     * **Every** take lane is walked and not only the ones that play: an alternate
     * take names its source whether or not anyone has chosen it yet.
     */
    dangling(): number[] {
        const missing: number[] = [];
        for (const region of this.multitrack.regions()) {
            const named = (region.content.window ?? {}).source as Extra | undefined;
            const id = named?.source;
            if (typeof id === "number" && !this.sources.has(id) && !missing.includes(id)) {
                missing.push(id);
            }
        }
        return missing;
    }

    /**
     * Promotes a temporary working copy to one saved beside the document,
     * **leaving the edit open**. What a save mid-edit does: auto-confirming
     * would turn a save into an edit, and refusing until the edit is settled
     * would make the safest habit in the program the one that is blocked.
     */
    promote(id: number): boolean {
        const source = this.sources.get(id);
        if (!source || source.lifetime !== "temporary") return false;
        source.lifetime = "session";
        return true;
    }

    /**
     * Confirms the edit open over a source: the working copy becomes the
     * samples, and there is nothing left undecided about it.
     */
    confirm(id: number): boolean {
        const source = this.sources.get(id);
        if (!source || !source.editing) return false;
        source.editing = { ...source.editing, confirmed: true };
        return true;
    }

    /** The session as the crate's JSON. */
    write(): Extra {
        const out: Extra = { format: this.format, metadata: { ...this.metadata } };
        const multitrack = this.multitrack.write();
        if (Object.keys(multitrack).length) out.multitrack = multitrack;
        if (this.views.length) out.views = this.views.map((v) => v.write());
        if (this.end !== null) out.end = this.end;
        if (this.document !== undefined) out.document = this.document;
        if (this.sources.size) {
            const table: Extra = {};
            for (const id of [...this.sources.keys()].sort((a, b) => a - b)) {
                table[String(id)] = this.sources.get(id)!.write();
            }
            out.sources = table;
        }
        if (this.provenance !== undefined) out.provenance = this.provenance;
        return { ...out, ...this.extra };
    }

    /**
     * A session from the crate's JSON, migrated first when it was written in an
     * older format.
     *
     * The migration is the crate's (`sessionMigrate`), so an old file opens the
     * same here, in the Python client and in the GUI host.
     *
     * @throws Error when the file's metadata names another application than
     *   the multitrack.
     */
    static read(written: Extra): SessionFile {
        if (num(written.format, 1) < SESSION_FORMAT) {
            written = JSON.parse(sessionMigrate(JSON.stringify(written))) as Extra;
        }
        const metadata: Extra = {
            application: SESSION_APPLICATION,
            ...((written.metadata as Extra | undefined) ?? {}),
        };
        if (metadata.application !== SESSION_APPLICATION) {
            throw new Error(`the file belongs to the ${String(metadata.application)} `
                + "application, not to the multitrack");
        }
        const session = new SessionFile();
        session.format = num(written.format, 1);
        session.metadata = metadata;
        // `arrangement` is what the field was called before 2026-09-08, read so a
        // session written then still opens.
        session.multitrack = Multitrack.read(
            (written.multitrack as Extra) ?? (written.arrangement as Extra) ?? {});
        session.views = ((written.views as Extra[]) ?? []).map(MultitrackView.read);
        session.end = (written.end as "contents" | number | null | undefined) ?? null;
        if (written.document !== undefined) session.document = written.document as Extra;
        for (const [id, entry] of Object.entries((written.sources as Extra) ?? {})) {
            session.sources.set(Number(id), Source.read(entry as Extra));
        }
        if (written.provenance !== undefined) session.provenance = written.provenance;
        session.extra = unknownFields(written, "format", "metadata", "multitrack", "arrangement", "views",
                             "end", "document", "sources", "provenance");
        return session;
    }
}

