/**
 * The multitrack's window state: what a window shows of a multitrack, saved
 * in its session file beside it (mirrors `clausters/gui/editing/multitrack_view.py`).
 *
 * Presentation, parallel to the model and never inside it: nothing here can
 * change what plays, and a multitrack round trips the same whether a view of it
 * exists or not. It is the multitrack application's, so it lives with it.
 *
 * @module
 */

import { type Extra, num, unknownFields } from "../../document/format.ts";
import { Span } from "../../document/timeline.ts";
import type { Multitrack, TakeLane, Track } from "../../document/multitrack.ts";

/** How one track is drawn. */
export class TrackView {
    /**
     * How tall its row is, in the window's own units. Absent is the window's
     * default, which is what a track nobody resized has.
     */
    height?: number;
    /** Whether the row is collapsed to its header. */
    collapsed = false;
    /**
     * Whether the track's other take lanes are shown under the one that plays --
     * comping open, in a word. Closed by default: a track with six takes on it
     * is one row until somebody asks to see them.
     */
    takeLanesShown = false;
    /** The colour the track is drawn in, carried and never read. */
    color?: string;
    extra: Extra = {};

    write(): Extra {
        const out: Extra = {};
        if (this.height !== undefined) out.height = this.height;
        if (this.collapsed) out.collapsed = true;
        if (this.takeLanesShown) out.take_lanes_shown = true;
        if (this.color !== undefined) out.color = this.color;
        return { ...out, ...this.extra };
    }

    static read(written: Extra): TrackView {
        const view = new TrackView();
        if (written.height !== undefined) view.height = num(written.height);
        view.collapsed = written.collapsed === true;
        view.takeLanesShown = written.take_lanes_shown === true;
        if (written.color !== undefined) view.color = String(written.color);
        view.extra = unknownFields(written, "height", "collapsed", "take_lanes_shown", "color");
        return view;
    }
}

/** How one lane is drawn. */
export class TakeLaneView {
    /** How tall its row is when the track's lanes are shown. */
    height?: number;
    extra: Extra = {};

    write(): Extra {
        const out: Extra = {};
        if (this.height !== undefined) out.height = this.height;
        return { ...out, ...this.extra };
    }

    static read(written: Extra): TakeLaneView {
        const view = new TakeLaneView();
        if (written.height !== undefined) view.height = num(written.height);
        view.extra = unknownFields(written, "height");
        return view;
    }
}

/**
 * One window's picture of one multitrack: where it is looking, how far it is zoomed,
 * what the hand is holding, how tall each track is drawn.
 *
 * None of that is what the multitrack *is* -- a selection and a zoom are each
 * window's and never the multitrack's -- and all of it is state a person loses
 * on a reopen unless something writes it down. A session carries a **list** of
 * these, because a multitrack drawn in two windows has two views and they disagree
 * on purpose.
 *
 * Nothing here ever reaches the document or the history: a view is not edited
 * through an intent, and an undo never puts a scroll back.
 */
export class MultitrackView {
    /** What the window is called, when a person named it. */
    name?: string;
    /**
     * The stretch of the timeline on screen, in seconds -- the zoom and the
     * horizontal scroll, which are one fact and not two. Absent shows the whole
     * multitrack.
     */
    visible?: Span;
    /** How far down the tracks the window is scrolled, in its own units. */
    scroll = 0;
    /**
     * The grid this window snaps to, in beats. Zero snaps nothing. It is here
     * rather than in the multitrack because two windows over one multitrack may snap
     * differently -- the arranger to a bar, the editor below it to a sixteenth.
     * A musical grid over a multitrack in seconds, taken through its tempo map by
     * the window: the ruler's configuration, not a unit of the placement.
     */
    quant = 0;
    /**
     * Whether the window follows its content. `false` says the window is the
     * reader's, and nothing moves it, which is what an editor wants.
     */
    autofit = true;
    /** The time range the hand swept, in seconds, when it swept one. */
    selection?: Span;
    /**
     * What the hand is holding: regions, take lanes or tracks, by id. One list
     * rather than one per kind, because the multitrack has one id space.
     */
    selected: number[] = [];
    /** What a keystroke is aimed at, which is not the same as what is selected. */
    focused?: number;
    /** The region the detail editor below is showing, when the window has one. */
    detail?: number;
    /** How each track is drawn, by the track's id. */
    tracks = new Map<number, TrackView>();
    /** How each lane is drawn, by the lane's id. */
    takeLanes = new Map<number, TakeLaneView>();
    extra: Extra = {};

    /** How `track` is drawn, or the default when nobody touched it. */
    track(track: Track): TrackView {
        return this.tracks.get(track.ident) ?? new TrackView();
    }

    /**
     * How `track` is drawn, to be edited -- created on first use, which is
     * what makes "nobody has touched it" cost nothing to store.
     */
    trackView(track: Track): TrackView {
        let view = this.tracks.get(track.ident);
        if (!view) {
            view = new TrackView();
            this.tracks.set(track.ident, view);
        }
        return view;
    }

    /** How `lane` is drawn, or the default. */
    takeLane(lane: TakeLane): TakeLaneView {
        return this.takeLanes.get(lane.ident) ?? new TakeLaneView();
    }

    /** How `lane` is drawn, to be edited. See {@link MultitrackView.trackView}. */
    takeLaneView(lane: TakeLane): TakeLaneView {
        const id = lane.ident;
        let view = this.takeLanes.get(id);
        if (!view) {
            view = new TakeLaneView();
            this.takeLanes.set(id, view);
        }
        return view;
    }

    /**
     * Drops everything this view says about objects the multitrack no longer holds,
     * and answers whether anything went.
     *
     * **State goes when the thing goes.** Keeping it is worse than losing it: a
     * height kept for a track that is not the same track is a defect that looks
     * like a feature.
     */
    prune(multitrack: Multitrack): boolean {
        const held = new Set<number>();
        for (const track of multitrack.tracksWritten()) {
            held.add(num(track.id));
            for (const lane of (track.take_lanes as Extra[] | undefined) ?? []) {
                held.add(num(lane.id));
                for (const region of (lane.regions as Extra[] | undefined) ?? []) held.add(num(region.id));
            }
            for (const curve of (track.automation as Extra[] | undefined) ?? []) held.add(num(curve.id));
        }
        const before = [this.tracks.size, this.takeLanes.size, this.selected.length,
                        this.focused, this.detail].join(",");
        for (const id of [...this.tracks.keys()]) {
            if (!held.has(id)) this.tracks.delete(id);
        }
        for (const id of [...this.takeLanes.keys()]) {
            if (!held.has(id)) this.takeLanes.delete(id);
        }
        this.selected = this.selected.filter((id) => held.has(id));
        if (this.focused !== undefined && !held.has(this.focused)) {
            this.focused = undefined;
        }
        if (this.detail !== undefined && !held.has(this.detail)) {
            this.detail = undefined;
        }
        return before !== [this.tracks.size, this.takeLanes.size,
                           this.selected.length, this.focused,
                           this.detail].join(",");
    }

    /** The view as the crate's JSON. Nothing said is nothing written. */
    write(): Extra {
        const out: Extra = {};
        if (this.name !== undefined) out.name = this.name;
        if (this.visible) out.visible = this.visible.write();
        if (this.scroll) out.scroll = this.scroll;
        if (this.quant) out.quant = this.quant;
        if (!this.autofit) out.autofit = false;
        if (this.selection) out.selection = this.selection.write();
        if (this.selected.length) out.selected = [...this.selected];
        if (this.focused !== undefined) out.focused = this.focused;
        if (this.detail !== undefined) out.detail = this.detail;
        if (this.tracks.size) {
            const table: Extra = {};
            for (const id of [...this.tracks.keys()].sort((a, b) => a - b)) {
                table[String(id)] = this.tracks.get(id)!.write();
            }
            out.tracks = table;
        }
        if (this.takeLanes.size) {
            const table: Extra = {};
            for (const id of [...this.takeLanes.keys()].sort((a, b) => a - b)) {
                table[String(id)] = this.takeLanes.get(id)!.write();
            }
            out.take_lanes = table;
        }
        return { ...out, ...this.extra };
    }

    /** A view from the crate's JSON. */
    static read(written: Extra): MultitrackView {
        const view = new MultitrackView();
        if (written.name !== undefined) view.name = String(written.name);
        if (written.visible) view.visible = Span.read(written.visible as Extra);
        view.scroll = num(written.scroll);
        view.quant = num(written.quant);
        view.autofit = written.autofit !== false;
        if (written.selection) view.selection = Span.read(written.selection as Extra);
        view.selected = ((written.selected as number[]) ?? []).map(Number);
        if (written.focused !== undefined) view.focused = num(written.focused);
        if (written.detail !== undefined) view.detail = num(written.detail);
        for (const [id, entry] of Object.entries((written.tracks as Extra) ?? {})) {
            view.tracks.set(Number(id), TrackView.read(entry as Extra));
        }
        for (const [id, entry] of Object.entries((written.take_lanes as Extra) ?? {})) {
            view.takeLanes.set(Number(id), TakeLaneView.read(entry as Extra));
        }
        view.extra = unknownFields(written, "name", "visible", "scroll", "quant",
                          "autofit", "selection", "selected", "focused",
                          "detail", "tracks", "take_lanes");
        return view;
    }
}

