// `Multitrack`: the multitrack a `multitrack` widget draws, kept here.
//
// The widget owns the lanes and the clips and reports **the multitrack as it now
// stands** after any gesture; this is the object that holds that on the
// client's side, so a script says what the multitrack *is* and never what a hand did
// to it.
//
// **It wires nothing that a script would otherwise have to.** `attach`
// subscribes once, to one widget, and turns both edit-backs into this object's
// own lists -- so there is no handler per clip, no widget id anywhere, and
// nothing to keep in step by hand. Identity is your own name: a clip is placed,
// drawn and reported by the same word you called it.
//
// **The words here are the picture's.** A `Lane` is a row of the view and a
// `Clip` is a box on it, which is what the protocol has always called them; the
// model's words for the same things are `multitrack.ts`'s `Track`, `Lane` and
// `Region`, and a region is not a clip. This object is the view's side.
//
// The Python client's `clausters/gui/multitrack.py`, call for call.

import { multitrack as multitrackView } from "./guidef.ts";
import type { GuiNode } from "./guidef.ts";

/** One row of the view: what it is called, how thick it is, and its strip. */
export class Lane {
    /** Its identity, and your own word for it. */
    name: string;
    /** What the header draws; the name when empty. */
    label: string;
    /** Its thickness in logical pixels. */
    height: number;
    /**
     * Silenced. Carried by the host, never interpreted -- what a solo does to
     * *other* lanes is the mixer's rule, and the mixer is yours.
     */
    mute: boolean;
    solo: boolean;
    /** The fader, over `[0, 1]`. */
    gain: number;

    constructor(
        name: string,
        label = "",
        height = 96.0,
        mute = false,
        solo = false,
        gain = 1.0,
    ) {
        this.name = name;
        this.label = label;
        this.height = height;
        this.mute = mute;
        this.solo = solo;
        this.gain = gain;
    }
}

/**
 * One box: which lane it is on, and where it sits there.
 *
 * `at`, `dur` and `start` are in the axis' own unit (timeline samples), and
 * `start` is the source frame the box's own time zero reads -- so trimming the
 * left edge moves `at`, `dur` and `start` together, which is what makes a trim
 * hide frames instead of compressing them.
 */
export class Clip {
    name: string;
    lane: string;
    at: number;
    dur: number;
    start: number;
    label: string;
    /**
     * The **server buffer** this box is a window onto; a negative number (the
     * default) draws an empty box. A number and not samples: they are the
     * server's, and the host maps or fetches them, so two clips over one take
     * cost one download. **Negative and not zero**, because buffer 0 is a
     * buffer -- the first one an allocator hands out.
     */
    source: number;

    constructor(
        name: string,
        lane: string,
        at = 0.0,
        dur = 0.0,
        start = 0.0,
        label = "",
        source = -1,
    ) {
        this.name = name;
        this.lane = lane;
        this.at = at;
        this.dur = dur;
        this.start = start;
        this.label = label;
        this.source = source;
    }

    /** Where it ends on the axis. */
    get end(): number {
        return this.at + this.dur;
    }
}

/** A lane as a script types one: the object, or the tuple it takes. */
export type LaneLike =
    | Lane
    | readonly [string, string?, number?, boolean?, boolean?, number?];

/** A clip as a script types one: the object, or the tuple it takes. */
export type ClipLike =
    | Clip
    | readonly [string, string, number?, number?, number?, string?, number?];

/** What `attach` needs of a widget: one subscription and one way to set. */
export interface MultitrackWidget {
    onEvent(handler: (tag: string, ...vals: unknown[]) => void): unknown;
    set(props: Record<string, unknown>): unknown;
}

/** What `attach` needs of a window: a way to find a widget by name. */
export interface MultitrackWindow {
    widget(name: string): MultitrackWidget;
}

export interface MultitrackOptions {
    lanes?: readonly LaneLike[];
    clips?: readonly ClipLike[];
    /** The drag grid in axis units; `0` is no grid. */
    snap?: number;
    /**
     * `onChange(what)` after a hand edited the multitrack, with `what` being
     * `"clips"` or `"lanes"`. It is called *after* this object's lists are
     * already the new ones, so a handler reads them rather than parsing
     * anything.
     */
    onChange?: (what: string) => void;
    /**
     * `onLocate(at)` when a click placed the window's cursor on this widget's
     * axis, in axis units. It is **not** an edit -- the multitrack did not change --
     * but it arrives here because the widget owns the axis, so this hands it on
     * rather than swallowing it.
     */
    onLocate?: (at: number) => void;
}

/**
 * The multitrack: its lanes, its clips, and the one widget that draws them.
 */
export class Multitrack {
    /** The rows, top to bottom. */
    lanes: Lane[];
    /** The boxes. */
    clips: Clip[];
    /** The drag grid in axis units. */
    snap: number;
    /** Called after a hand edited the multitrack. */
    onChange: ((what: string) => void) | null;
    /** Called when a click placed the window's cursor. */
    onLocate: ((at: number) => void) | null;

    private widget: MultitrackWidget | null = null;
    private builtAs: string | null = null;

    constructor(
        { lanes = [], clips = [], snap = 0.0, onChange, onLocate }: MultitrackOptions = {},
    ) {
        this.lanes = lanes.map((l) => (l instanceof Lane ? l : new Lane(...l)));
        this.clips = clips.map((c) => (c instanceof Clip ? c : new Clip(...c)));
        this.snap = snap;
        this.onChange = onChange ?? null;
        this.onLocate = onLocate ?? null;
    }

    // ---- reading it ----

    /** The lane of this name, or `null`. */
    lane(name: string): Lane | null {
        return this.lanes.find((l) => l.name === name) ?? null;
    }

    /** The clip of this name, or `null`. */
    clip(name: string): Clip | null {
        return this.clips.find((c) => c.name === name) ?? null;
    }

    /** The clips on `lane`, in the order they are drawn. */
    on(lane: string): Clip[] {
        return this.clips.filter((c) => c.lane === lane);
    }

    /**
     * Where the multitrack ends: the furthest clip end, `0` for none.
     *
     * The **end**, not the last onset -- a clip dragged past everything else
     * lengthens the multitrack by its whole length.
     */
    get extent(): number {
        return this.clips.reduce((far, c) => Math.max(far, c.end), 0.0);
    }

    // ---- changing it ----

    /** Append a lane (a `Lane`, its tuple, or a bare name). */
    addLane(lane: LaneLike | string): this {
        const made = typeof lane === "string"
            ? new Lane(lane)
            : lane instanceof Lane
                ? lane
                : new Lane(...lane);
        this.lanes.push(made);
        return this.pushed("lanes");
    }

    /**
     * Take a lane away. **The clips on it are kept** -- they name a lane that is
     * not there, are drawn nowhere, and come back to be re-homed; losing them
     * silently is the one thing a removal must not do.
     */
    removeLane(name: string): this {
        this.lanes = this.lanes.filter((l) => l.name !== name);
        return this.pushed("lanes");
    }

    /**
     * Put a clip where you say -- adding it, or moving the one of that name. The
     * verb is one because *the multitrack is a statement*: what you hand over is
     * where the clip is, not how it got there.
     */
    place(
        name: string,
        lane: string,
        at: number,
        dur: number,
        start = 0.0,
        label = "",
        source = -1,
    ): this {
        const found = this.clip(name);
        if (found === null) {
            this.clips.push(new Clip(name, lane, at, dur, start, label, source));
        } else {
            found.lane = lane;
            found.at = at;
            found.dur = dur;
            found.start = start;
            found.label = label;
            found.source = source;
        }
        return this.pushed("clips");
    }

    /** Take a clip away. */
    remove(name: string): this {
        this.clips = this.clips.filter((c) => c.name !== name);
        return this.pushed("clips");
    }

    /**
     * Set a lane's strip. What a solo does to the other lanes is **your** rule:
     * the host carries the flag and never reads it.
     */
    mix(
        lane: string,
        { mute, solo, gain }: { mute?: boolean; solo?: boolean; gain?: number } = {},
    ): this {
        const found = this.lane(lane);
        if (found === null) return this;
        if (mute !== undefined) found.mute = Boolean(mute);
        if (solo !== undefined) found.solo = Boolean(solo);
        if (gain !== undefined) found.gain = Number(gain);
        return this.pushed("lanes");
    }

    // ---- the widget ----

    /**
     * The `multitrack` widget drawing this multitrack, built from what this object
     * holds. Any widget prop (`name`, `weight`, `link`, `ruler`, `sampleRate`,
     * `playheadAt`…) passes through.
     */
    view(props: Record<string, unknown> = {}): GuiNode {
        // The name is remembered so `attach` needs only the window: this object
        // built the node, so it is the one that knows what it called it.
        if (typeof props.name === "string") this.builtAs = props.name;
        return multitrackView({
            snap: this.snap,
            ...props,
            lanes: this.laneTuples(),
            clips: this.clipTuples(),
        });
    }

    /**
     * Subscribe to the widget drawing this multitrack: pass the **window**
     * `view().open()` gave back, or the widget handle itself.
     *
     * It is a second step because a view is a *definition* and an id names a
     * *live* widget -- one view opens as many times as you like, each window
     * with ids of its own -- so which opened window this multitrack is watching has
     * to be said. What does not have to be said again is the name: `view`
     * remembered it.
     *
     * **One subscription, for the whole multitrack.** The widget reports what it now
     * holds, so this replaces the lists and calls `onChange`; there is nothing
     * per clip to register and no id for a script to carry.
     */
    attach(where: MultitrackWidget | MultitrackWindow): this {
        let widget: MultitrackWidget;
        if ("onEvent" in where) {
            widget = where;
        } else {
            if (this.builtAs === null) {
                throw new Error(
                    "this multitrack's view was built with no name, so a window " +
                    "cannot be searched for it: pass the widget handle, or " +
                    "build the view with a name",
                );
            }
            widget = where.widget(this.builtAs);
        }
        this.widget = widget;
        widget.onEvent((tag, ...vals) => this.edited(tag, vals));
        return this;
    }

    /** Both edit-backs, each the whole list -- the multitrack as it now stands. */
    private edited(tag: string, vals: unknown[]): void {
        if (tag === "clips") {
            this.clips = groups(vals, 7).map(
                ([n, lane, at, dur, start, label, source]) =>
                    new Clip(String(n), String(lane), Number(at), Number(dur),
                        Number(start), String(label), Math.trunc(Number(source))));
        } else if (tag === "lanes") {
            this.lanes = groups(vals, 6).map(([n, label, h, m, s, g]) =>
                new Lane(String(n), String(label), Number(h), Boolean(Number(m)),
                    Boolean(Number(s)), Number(g)));
        } else if (tag === "locate") {
            // Not an edit: one cursor, and it is the transport's. It lands on
            // this widget because this widget owns the axis.
            if (vals.length) this.onLocate?.(Number(vals[0]));
            return;
        } else {
            return;
        }
        this.onChange?.(tag);
    }

    /** Send a changed list to the widget, when there is one to send to. */
    private pushed(what: string): this {
        if (this.widget === null) return this;
        this.widget.set(
            what === "clips" ? { clips: this.clipTuples() } : { lanes: this.laneTuples() },
        );
        return this;
    }

    private laneTuples(): [string, string, number, boolean, boolean, number][] {
        return this.lanes.map((l) => [l.name, l.label, l.height, l.mute, l.solo, l.gain]);
    }

    private clipTuples(): [string, string, number, number, number, string, number][] {
        return this.clips.map(
            (c) => [c.name, c.lane, c.at, c.dur, c.start, c.label, c.source],
        );
    }
}

/**
 * The flat payload in groups of `n`; a trailing partial group is dropped rather
 * than half-read, the rule every flat payload here follows.
 */
function groups(vals: unknown[], n: number): unknown[][] {
    const out: unknown[][] = [];
    for (let i = 0; i + n <= vals.length; i += n) out.push(vals.slice(i, i + n));
    return out;
}
