/**
 * The multitrack's session file: the envelope every application's file shares,
 * with what the multitrack keeps in it (mirrors
 * `clausters/gui/editing/multitrack_session.py`).
 *
 * What is the multitrack's is three things: the {@link Multitrack} itself, how
 * each window was looking at it ({@link MultitrackView}) and where a pass over
 * it ends. They are read out of the envelope's contents and written back into
 * it, so the JSON is the one the format has always had -- `multitrack`, `views`
 * and `end` beside `sources` at the top level -- and a file another application
 * writes over the same envelope is read by that application, not by this one.
 *
 * @module
 */

import { Multitrack } from "../../document/multitrack.ts";
import type { Extra } from "../../document/format.ts";
import { SessionFile } from "../../document/session-file.ts";
import { MultitrackView } from "./multitrack-view.ts";

/**
 * **A multitrack, saved**: the {@link SessionFile} envelope, the multitrack's
 * tracks, its windows' views and where a pass over it ends.
 *
 * An {@link Multitrack} says *what plays when* and deliberately does not say
 * where a source lives, because inside a running system a source is a server
 * buffer, a mapped file or a rendered result and the multitrack has no business
 * knowing which. The envelope's source table is that missing half.
 */
export class MultitrackSession extends SessionFile {
    /** The application a multitrack's session file belongs to, as its metadata names it. */
    static readonly APPLICATION = "multitrack";

    override metadata: Extra = { application: MultitrackSession.APPLICATION };
    /**
     * The multitrack. Always present, possibly empty -- which mirrors the crate,
     * where an absent multitrack reads as an empty one rather than as nothing.
     */
    multitrack = new Multitrack();
    /**
     * How the multitrack was being **looked at**: one entry per window. Carried
     * because reopening a multitrack into the window it was left in is what a
     * person expects, and a reader that ignores it opens the same multitrack.
     */
    views: MultitrackView[] = [];
    /**
     * **Where a pass over the multitrack ends**, as a playback's `end` says it:
     * `null` (it rolls on, the default), `"contents"` (where the last region
     * ends) or a number of seconds, an end marker. The transport's own switch,
     * kept so a session opened again stops where it stopped before -- not in a
     * view, since it changes what is heard. It bounds no axis: how far a window
     * shows or zooms out reads nothing here.
     */
    end: "contents" | number | null = null;

    /**
     * Sources the multitrack names but the table does not hold -- what an
     * opening reader reports rather than discovering one element at a time.
     *
     * **Every** take lane is walked and not only the ones that play: an
     * alternate take names its source whether or not anyone has chosen it yet.
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

    protected override writtenContents(): Extra {
        const out: Extra = {};
        const multitrack = this.multitrack.write();
        if (Object.keys(multitrack).length) out.multitrack = multitrack;
        if (this.views.length) out.views = this.views.map((v) => v.write());
        if (this.end !== null) out.end = this.end;
        return out;
    }

    /**
     * Takes the multitrack's fields out of the contents read.
     *
     * @throws Error when the file's metadata names another application than
     *   the multitrack.
     */
    protected override readContents(): void {
        if (!this.belongsTo(MultitrackSession.APPLICATION)) {
            throw new Error(`the file belongs to the ${String(this.metadata.application)} `
                + "application, not to the multitrack");
        }
        const { multitrack, arrangement, views, end, ...carried } = this.contents;
        // `arrangement` is what the field was called before 2026-09-08, read so
        // a session written then still opens.
        this.multitrack = Multitrack.read((multitrack as Extra) ?? (arrangement as Extra) ?? {});
        this.views = ((views as Extra[]) ?? []).map(MultitrackView.read);
        this.end = (end as "contents" | number | null | undefined) ?? null;
        this.contents = carried;
    }
}
