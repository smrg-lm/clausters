// `Transport`: play, pause, stop and locate, with the view's playhead in step
// (mirrors `clausters/gui/transport.py`).
//
// Every time view the host draws — a lane, a piano-roll, an engraved page —
// shows the same line, and every script that plays into one needs the same four
// buttons. This is that logic, once, independent of which widget it drives.
//
// **The line is the host's, not the client's.** `playhead_at` is a single
// anchor: the sample-clock value the view's time 0 maps to. The host reads the
// engine's clock every frame and draws the line from there, so a pass costs
// *one* message, not one per frame. A transport that is not playing is the other
// half of that number — `playhead_at` goes negative and the static `playhead`
// holds the cursor where the music was left, which is what makes pause look like
// pause.
//
// **Two axes meet here.** The anchor lives on the engine's sample clock
// (samples, always); the static cursor lives on the *view's* own axis — timeline
// samples for a lane, milliseconds for an engraved page. `Transport` converts to
// the first itself and takes `toUnits` for the second, which is the whole of
// what a view has to say about its units.
//
// **A pass ends by itself.** `seq.Playhead` reports the end of its scan, so
// `update` parks the cursor at the piece's end without the script timing it.

import { beats_to_secs, secs_to_samples } from "../core/clausters_core_web.js";
import type { TempoClock } from "../base/clock.ts";
import { ReplyTimeout } from "../errors.ts";
import type { GuiHost } from "./host.ts";
import type { Playhead } from "../seq/timeline.ts";
import type { Server } from "../defs/server/index.ts";

/** The widgets a transport draws its line on. */
export type TransportTargets = number | readonly number[] | (() => number | readonly number[]);

/** What {@link Transport} is built with. */
export interface TransportOptions {
    /**
     * `source(at)` starts a pass at beat `at` and returns the playing
     * `Playhead` (`null` when there is nothing to play). It is called afresh on
     * every play, so what sounds is always the piece as it now stands.
     */
    source: (at: number) => Playhead | null;
    /** The clock's tempo in beats per second (2.0 is 120 bpm). */
    tempo: number;
    /** The engine's sample rate; with `tempo` it fixes the beats→samples axis. */
    sampleRate: number;
    /**
     * `toUnits(beats)` → the view's own units, for the static cursor. Defaults
     * to beats→samples, which is what the timeline views use; an engraved page
     * passes its beats→milliseconds.
     */
    toUnits?: (beats: number) => number;
    /**
     * `extent()` → the piece's length in beats, where {@link Transport.update}
     * parks the cursor when a pass ends. Read on each use, so a piece that grew
     * (a clip dragged past the end) ends where it now ends.
     */
    extent?: () => number;
    /** The clock the pass runs on. A governed pause freezes it. */
    clock?: TempoClock | null;
    /**
     * Whether a **server** transport governs the piece (its transport group is
     * bound). Governed, a pause freezes the server's subtree and this clock
     * rather than stopping the playhead, so `resume` continues the sound where
     * it stopped instead of re-rendering it.
     */
    governed?: boolean;
}

/**
 * Drive a `seq.Playhead` and a view's playhead line together.
 *
 * `host` may be `null` and set later (a view drawn before it is opened), and
 * `ids` is one widget id, several, or a callable returning either — for a view
 * that redraws, whose lanes are new widgets the transport must find again.
 *
 * **Anchoring is asynchronous here and synchronous in the Python client**, for
 * the reason every request is: the anchor asks the server for its clock, and a
 * page waits for an answer instead of blocking on one. `play`, `resume` and
 * `anchor` hand back promises; a script that does not await them still gets the
 * pass — what arrives late is the line, not the sound.
 */
export class Transport {
    host: GuiHost | null;
    ids: TransportTargets;
    source: (at: number) => Playhead | null;
    tempo: number;
    sampleRate: number;
    toUnits: (beats: number) => number;
    extent: (() => number) | null;
    /** The clock the pass runs on, when there is one. */
    clock: TempoClock | null;
    /** Whether a server transport governs the piece. */
    governed: boolean;
    /**
     * The server the anchor queries for its clock — the destination of the last
     * `play`, or whatever `anchor` was given.
     */
    server: Server | null = null;

    private head: Playhead | null = null;
    private atBeat = 0.0; // the beat the cursor waits at while stopped
    private ended = false; // the end of a pass was already parked (send it once)
    /**
     * The **tail**: `[clock beat, timeline beat]` at the moment the scan
     * drained. A scan runs out when it renders its *last item*, not when the
     * piece is over — the last clip is still sounding, and the line must go on
     * crossing it. `null` outside that stretch.
     */
    private tail: [number, number] | null = null;

    constructor(
        host: GuiHost | null,
        ids: TransportTargets,
        {
            source,
            tempo,
            sampleRate,
            toUnits,
            extent,
            clock = null,
            governed = false,
        }: TransportOptions,
    ) {
        this.host = host;
        this.ids = ids;
        this.source = source;
        this.tempo = Number(tempo);
        this.sampleRate = Number(sampleRate);
        this.toUnits = toUnits ?? ((beats) => this.beatsToSamples(beats));
        this.extent = extent ?? null;
        this.clock = clock;
        this.governed = Boolean(governed);
    }

    // ---- the unit bridge ----

    /**
     * Beats → samples of the engine clock, through the core's own time
     * arithmetic (the seconds→samples rounding every client shares).
     */
    beatsToSamples(beats: number): number {
        return secs_to_samples(
            beats_to_secs(this.tempo, 0.0, 0.0, Number(beats)),
            this.sampleRate,
        );
    }

    private targets(): number[] {
        const ids = typeof this.ids === "function" ? this.ids() : this.ids;
        return typeof ids === "number" ? [ids] : [...ids];
    }

    // ---- the transport ----

    /** The `Playhead` of the pass in flight, or `null` before the first play. */
    get playhead(): Playhead | null {
        return this.head;
    }

    /**
     * Whether the piece is sounding: a pass is rolling, **or** its scan has
     * drained and the last item is still ringing (the tail). It goes false on
     * its own at the end of the piece — where the last item ends, not where it
     * started — which is what {@link Transport.update} decides.
     *
     * The tail counts as playing because everything a caller does with this
     * answer is true of it: a pause holds where the music is, a seek starts a
     * fresh pass from there, and a button reads "pause" rather than "play".
     */
    get playing(): boolean {
        return (this.head !== null && this.head.playing) || this.tail !== null;
    }

    /**
     * The transport's position in beats: where the playhead is while it plays,
     * where it got to while the last item is still ringing, and where the next
     * `play` starts when neither.
     */
    get position(): number {
        if (this.head !== null && this.head.playing) return this.head.position();
        const tail = this.tailPosition();
        return tail === null ? this.atBeat : tail;
    }

    /**
     * Where the line is between the scan draining and the piece ending: the last
     * item's beat plus what the clock has advanced since, never past the end.
     * `null` when there is no tail to be in.
     *
     * The clock is the **pass's own**, and it has to be *rolling*: an offline
     * render computes the whole piece in an instant and its beat is the queue's,
     * not the wall's, so there is no tail to sweep and the cursor parks straight
     * away.
     */
    private tailPosition(): number | null {
        if (this.tail === null) return null;
        const [since, beat] = this.tail;
        const clock = this.passClock();
        if (clock === null || !clock.rolling) return beat;
        const end = this.extent === null ? beat : Number(this.extent());
        return Math.min(beat + (clock.beats() - since), Math.max(end, beat));
    }

    /** The clock the pass in flight runs on: the playhead's own, else ours. */
    private passClock(): TempoClock | null {
        return this.head?.clock ?? this.clock;
    }

    /**
     * The beat a bare `play` starts from — where a pause, a locate or the end of
     * a pass left the transport. It is *not* {@link Transport.position}: a play
     * while already playing restarts from here, not from where the music got to.
     */
    get at(): number {
        return this.atBeat;
    }

    /**
     * Play (or resume) from beat `at` — the transport's position by default —
     * and anchor the line to the engine clock. `server` is where the anchor's
     * clock query goes (remembered for later passes).
     *
     * The pass starts before the promise settles: what is awaited is the anchor.
     */
    async play(
        server: Server | null = null,
        { at }: { at?: number } = {},
    ): Promise<Playhead | null> {
        if (server !== null) this.server = server;
        const beat = at === undefined ? this.atBeat : Number(at);
        this.halt();
        this.atBeat = beat;
        this.ended = false;
        this.head = this.source(beat);
        this.cursor(null); // the clock's line takes over from the cursor
        await this.anchor(null, { at: beat });
        return this.head;
    }

    /**
     * Halt where we are: the cursor stays on what the music stopped on, and
     * `play` resumes from there. What is already sounding keeps sounding —
     * stopping a playhead is not a panic button (the script owns its voices).
     * Answers the position it stopped at.
     *
     * **Governed** (a server transport holds the piece), the playhead is not
     * stopped at all — it is starved of time. `/transport_stop` freezes the
     * server's subtree and its queue, the clock freezes with them, and the scan
     * simply stops making progress. That is what lets `resume` continue the
     * sound rather than start it again.
     */
    pause(): number {
        // Where the music stopped — including inside the tail, where the scan
        // has drained but the last clip is still sounding.
        this.atBeat = this.position;
        if (this.governed) {
            void this.server?.transportStop();
            this.clock?.freeze();
        } else {
            this.halt();
        }
        this.cursor(this.atBeat);
        return this.atBeat;
    }

    /**
     * Continue from where `pause` left off, **without re-rendering**.
     *
     * The difference from `play` is MIDI's `continue` versus `start`: play reads
     * the composition as it now stands and starts it again from `at`, resume
     * picks the frozen sound back up. Governed, the server still holds every
     * node's internal state and every scheduled bundle, so what comes back is
     * the same sound carried on. Ungoverned there is nothing frozen to continue,
     * so this falls back to `play`.
     */
    async resume(): Promise<Playhead | null> {
        if (!this.governed) return this.play();
        await this.server?.transportPlay();
        this.clock?.thaw();
        this.ended = false;
        await this.anchor(null, { at: this.position });
        return this.head;
    }

    /** Halt and go back to the top. */
    stop(): this {
        this.pause();
        return this.locate(0.0);
    }

    /**
     * Seek: put the transport at `beat`. Playing, it starts a fresh pass from
     * there (so a seek also picks up any edit); stopped, it just moves the
     * cursor the view draws. This is what a click on a ruler does.
     */
    locate(beat: number): this {
        const at = Math.max(Number(beat), 0.0);
        if (this.playing) {
            void this.play(null, { at });
        } else {
            this.tail = null;
            this.atBeat = at;
            this.head?.locate(at); // the pass no longer ended *here*
            this.ended = false;
            this.cursor(at);
        }
        return this;
    }

    /**
     * Park the cursor when the pass ends by itself. Call it once per pass of the
     * script's loop; it answers whether the piece just ended.
     *
     * The playhead says when its scan ran out, so the end needs no timing here:
     * the cursor stops at the piece's extent rather than sweeping off the view,
     * and stays there — the transport is *at the end*, so it is a locate (a
     * rewind) that goes back to the top.
     */
    update(): boolean {
        const head = this.head;
        if (this.ended || head === null || !head.finished) return false;
        const end = this.extent === null ? head.position() : Number(this.extent());
        const clock = this.passClock();
        if (end > head.position() && clock !== null && clock.rolling) {
            if (this.tail === null) {
                // From the moment the last item was *rendered* — which is a loop
                // pass or two before anyone noticed — not from now.
                const since = head.scannedAt;
                this.tail = [since ?? clock.beats(), head.position()];
            }
            if ((this.tailPosition() ?? end) < end) return false; // still ringing
        }
        this.ended = true;
        this.tail = null;
        this.atBeat = Math.max(end, 0.0);
        this.cursor(this.atBeat);
        return true;
    }

    // ---- the line: anchored to the clock, or a static cursor ----

    /**
     * Anchor the view's playhead to the engine clock, so the line starts at beat
     * `at` and sweeps on with the audio. Answers whether it could.
     *
     * The anchor is a **query**: it asks the server for its clock, and a server
     * that does not answer leaves the view without a line — so the failure is
     * reported, not swallowed (a playhead that silently never appears is the
     * worst of both). A destination with no engine clock — an NRT score — has
     * nothing to anchor to and answers false.
     */
    async anchor(
        server: Server | null = null,
        { at = 0.0 }: { at?: number } = {},
    ): Promise<boolean> {
        if (server !== null) this.server = server;
        const target = this.server;
        if (this.host === null || target === null) return false;
        // NRT: there is no engine clock to anchor to.
        if (target.scoring) return false;
        let reply;
        try {
            reply = await target.request("/clock_query", [], {
                expect: ["/clock_query.reply"],
            });
        } catch (error) {
            // A live server that did not answer: no line, and it shows.
            if (error instanceof ReplyTimeout) return false;
            throw error;
        }
        const args = reply.args;
        if (args.length === 0) return false;
        // Items sound `latency` ahead of the time they were played at, so the
        // clock value beat 0 maps to is *now* plus that latency, less what has
        // already been played.
        const now = Number(args[0]) + target.latency * this.sampleRate;
        const origin = now - this.beatsToSamples(at);
        for (const id of this.targets()) this.host.set(id, { playheadAt: origin });
        return true;
    }

    /**
     * Take the sweeping line off the view (the static cursor stays). The host's
     * anchored playhead *tracks the engine clock*, so a line left anchored keeps
     * sweeping after the music stopped.
     */
    unanchor(): this {
        this.cursor(this.atBeat);
        return this;
    }

    /**
     * Draw (or clear) the static cursor — the located position of a transport
     * that is not playing. `null` clears it, which is what the clock anchor does
     * when a pass takes the line over.
     */
    cursor(beat: number | null): this {
        if (this.host === null) return this;
        const pos = beat === null ? -1.0 : this.toUnits(beat);
        for (const id of this.targets()) {
            this.host.set(id, { playheadAt: -1.0, playhead: pos });
        }
        return this;
    }

    /** Stop the pass in flight, if any, without touching the cursor. */
    private halt(): void {
        this.tail = null;
        if (this.head !== null && this.head.playing) this.head.stop();
    }
}
