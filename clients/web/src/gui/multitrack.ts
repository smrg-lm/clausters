// `Multitrack`: the piece a `multitrack` widget draws, kept here.
//
// The widget owns the lanes and the clips and reports **the piece as it now
// stands** after any gesture; this is the object that holds that on the
// client's side, so a script says what the piece *is* and never what a hand did
// to it.
//
// **It wires nothing that a script would otherwise have to.** `attach`
// subscribes once, to one widget, and turns both edit-backs into this object's
// own lists — so there is no handler per clip, no widget id anywhere, and
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
     * Silenced. Carried by the host, never interpreted — what a solo does to
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
 * `start` is the source frame the box's own time zero reads — so trimming the
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

    constructor(
        name: string,
        lane: string,
        at = 0.0,
        dur = 0.0,
        start = 0.0,
        label = "",
    ) {
        this.name = name;
        this.lane = lane;
        this.at = at;
        this.dur = dur;
        this.start = start;
        this.label = label;
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
    | readonly [string, string, number?, number?, number?, string?];

/** What `attach` needs of a widget: one subscription and one way to set. */
export interface MultitrackWidget {
    onEvent(handler: (tag: string, ...vals: unknown[]) => void): unknown;
    set(props: Record<string, unknown>): unknown;
}

export interface MultitrackOptions {
    lanes?: readonly LaneLike[];
    clips?: readonly ClipLike[];
    /** The drag grid in axis units; `0` is no grid. */
    snap?: number;
    /**
     * `onChange(what)` after a hand edited the piece, with `what` being
     * `"clips"` or `"lanes"`. It is called *after* this object's lists are
     * already the new ones, so a handler reads them rather than parsing
     * anything.
     */
    onChange?: (what: string) => void;
}

/**
 * The piece: its lanes, its clips, and the one widget that draws them.
 */
export class Multitrack {
    /** The rows, top to bottom. */
    lanes: Lane[];
    /** The boxes. */
    clips: Clip[];
    /** The drag grid in axis units. */
    snap: number;
    /** Called after a hand edited the piece. */
    onChange: ((what: string) => void) | null;

    private widget: MultitrackWidget | null = null;

    constructor({ lanes = [], clips = [], snap = 0.0, onChange }: MultitrackOptions = {}) {
        this.lanes = lanes.map((l) => (l instanceof Lane ? l : new Lane(...l)));
        this.clips = clips.map((c) => (c instanceof Clip ? c : new Clip(...c)));
        this.snap = snap;
        this.onChange = onChange ?? null;
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
     * Where the piece ends: the furthest clip end, `0` for none.
     *
     * The **end**, not the last onset — a clip dragged past everything else
     * lengthens the piece by its whole length.
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
     * Take a lane away. **The clips on it are kept** — they name a lane that is
     * not there, are drawn nowhere, and come back to be re-homed; losing them
     * silently is the one thing a removal must not do.
     */
    removeLane(name: string): this {
        this.lanes = this.lanes.filter((l) => l.name !== name);
        return this.pushed("lanes");
    }

    /**
     * Put a clip where you say — adding it, or moving the one of that name. The
     * verb is one because *the piece is a statement*: what you hand over is
     * where the clip is, not how it got there.
     */
    place(
        name: string,
        lane: string,
        at: number,
        dur: number,
        start = 0.0,
        label = "",
    ): this {
        const found = this.clip(name);
        if (found === null) {
            this.clips.push(new Clip(name, lane, at, dur, start, label));
        } else {
            found.lane = lane;
            found.at = at;
            found.dur = dur;
            found.start = start;
            found.label = label;
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
     * The `multitrack` widget drawing this piece, built from what this object
     * holds. Any widget prop (`name`, `weight`, `link`, `ruler`, `sampleRate`,
     * `playheadAt`…) passes through.
     */
    view(props: Record<string, unknown> = {}): GuiNode {
        return multitrackView({
            snap: this.snap,
            ...props,
            lanes: this.laneTuples(),
            clips: this.clipTuples(),
        });
    }

    /**
     * Subscribe to the widget drawing this piece.
     *
     * **One subscription, for the whole piece.** The widget reports what it now
     * holds, so this replaces the lists and calls `onChange`; there is nothing
     * per clip to register and no id for a script to carry.
     */
    attach(widget: MultitrackWidget): this {
        this.widget = widget;
        widget.onEvent((tag, ...vals) => this.edited(tag, vals));
        return this;
    }

    /** Both edit-backs, each the whole list — the piece as it now stands. */
    private edited(tag: string, vals: unknown[]): void {
        if (tag === "clips") {
            this.clips = six(vals).map(([n, lane, at, dur, start, label]) =>
                new Clip(String(n), String(lane), Number(at), Number(dur),
                    Number(start), String(label)));
        } else if (tag === "lanes") {
            this.lanes = six(vals).map(([n, label, h, m, s, g]) =>
                new Lane(String(n), String(label), Number(h), Boolean(Number(m)),
                    Boolean(Number(s)), Number(g)));
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

    private clipTuples(): [string, string, number, number, number, string][] {
        return this.clips.map((c) => [c.name, c.lane, c.at, c.dur, c.start, c.label]);
    }
}

/**
 * The flat payload as sextuples; a trailing partial group is dropped rather
 * than half-read, the rule every flat payload here follows.
 */
function six(vals: unknown[]): unknown[][] {
    const out: unknown[][] = [];
    for (let i = 0; i + 6 <= vals.length; i += 6) out.push(vals.slice(i, i + 6));
    return out;
}
