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

import { secs_to_samples } from "../core/clausters_core_web.js";
import { TempoMap } from "../base/time.ts";
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
    source?: (at: number) => Playhead | null;
    /**
     * The piece's starting tempo in beats per second (2.0 is 120 bpm). Ignored
     * when `tempoMap` is given.
     */
    tempo?: number;
    /**
     * The piece's {@link TempoMap}, when its tempo changes along the way — pass
     * the clock's (`TempoClock.map`) so the line and the sound read one
     * function.
     */
    tempoMap?: TempoMap;
    /** The engine's sample rate; with the tempo it fixes the beats→samples axis. */
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
    /**
     * Which counter the view's line is drawn from — `"device"` (the default:
     * the engine's sample clock, which never stops, so this class owns the
     * piece's time and anchors the line to it) or `"piece"` (the **server's
     * transport position**, so the server owns it and this class sends
     * commands). Setting `"piece"` also tells the host
     * ({@link GuiHost.headClock}), because the two are one decision and letting
     * them disagree draws a line nobody put there. A view whose axis is not
     * samples keeps `"device"`: the piece's position is measured in frames.
     */
    headClock?: "device" | "piece";
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
 *
 * **Or the server owns all of it** (`headClock: "piece"`). Then none of that
 * applies: the transport is the audio server's, the position is the engine's
 * `positionSample` — held while stopped, moved by a locate, wrapped inside a
 * loop in the engine — and this class is four commands and a read. Play, pause,
 * seek and loop stop being a line kept in step and become `/transport_play`,
 * `/transport_stop`, `/transport_locateSample` and `/transport_loop`; the anchor
 * is 0, because the counter the host draws already *is* the piece's time. That
 * is the shape a multitrack wants, where many readers follow one time
 * (`TransportPos`), and it is why an editor sends a locate and reads a position
 * back instead of computing one.
 */
/**
 * How often a rolling transport asks itself whether the pass has ended, in
 * seconds. It is not the line's frame rate — the host sweeps that from the
 * engine's clock without being told — only how sharply the cursor parks at the
 * end of the piece.
 */
const TICK = 0.05;

export class Transport {
    host: GuiHost | null;
    ids: TransportTargets;
    source: ((at: number) => Playhead | null) | null;
    /**
     * Which counter the line is drawn from: `"device"` or `"piece"`, the same
     * two words {@link GuiHost.headClock} takes. On `"piece"` the position, the
     * rolling state, the seek and the loop are all the audio server's, and this
     * class holds none of them.
     */
    headClock: "device" | "piece";
    /**
     * The piece's beat→second map. The line sweeps by engine samples from an
     * origin this places, so the origin has to come from the same function the
     * clock plays by: given only a `tempo` it is that tempo as one segment,
     * which is the affine ratio this always used.
     */
    tempoMap: TempoMap;
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
    private ticking = false; // a self-driven `update` is scheduled
    /**
     * The **tail**: `[clock beat, timeline beat]` at the moment the scan
     * drained. A scan runs out when it renders its *last item*, not when the
     * piece is over — the last clip is still sounding, and the line must go on
     * crossing it. `null` outside that stretch.
     */
    private tail: [number, number] | null = null;
    /**
     * The last answer {@link Transport.refresh} got from the server's
     * transport, on the piece. Empty until one is asked for.
     */
    private piece: { playing?: boolean; positionSample?: number } = {};

    constructor(
        host: GuiHost | null,
        ids: TransportTargets,
        {
            source,
            tempo = 1.0,
            tempoMap,
            sampleRate,
            toUnits,
            extent,
            clock = null,
            governed = false,
            headClock = "device",
        }: TransportOptions,
    ) {
        this.host = host;
        this.ids = ids;
        this.source = source ?? null;
        this.headClock = headClock;
        this.tempoMap = tempoMap?.copy() ?? new TempoMap(Number(tempo));
        this.sampleRate = Number(sampleRate);
        this.toUnits = toUnits ?? ((beats) => this.beatsToSamples(beats));
        this.extent = extent ?? null;
        this.clock = clock;
        this.governed = Boolean(governed);
        if (this.headClock === "piece") this.host?.headClock("piece");
    }

    // ---- the unit bridge ----

    /**
     * The tempo the piece **starts** at, in beats per second — a reading of
     * {@link Transport.tempoMap}. Assigning it replaces the map with that single
     * tempo.
     */
    get tempo(): number {
        return this.tempoMap.tempoAt(0.0);
    }

    set tempo(tempo: number) {
        this.tempoMap = new TempoMap(tempo);
    }

    /**
     * Beats → samples of the engine clock, through the piece's time map (and the
     * core's seconds→samples rounding every client shares).
     *
     * Where the line's origin comes from, so it must be the map and not a ratio:
     * the host sweeps the playhead by engine samples, and a beat placed by a
     * frozen tempo would be crossed at a time the clock never plays it at.
     */
    beatsToSamples(beats: number): number {
        return secs_to_samples(this.tempoMap.secsAt(Number(beats)), this.sampleRate);
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
     *
     * On the **piece** it is the engine's last answer ({@link
     * Transport.refresh}) and none of the above: the transport is rolling or it
     * is not, and nothing here has an opinion.
     */
    get playing(): boolean {
        if (this.headClock === "piece") return Boolean(this.piece.playing);
        return (this.head !== null && this.head.playing) || this.tail !== null;
    }

    /**
     * Ask the server where the piece is, and remember it.
     *
     * **The read is separate from the answer** because asking is a round trip
     * and {@link Transport.position} is not: a counter refreshes on its own
     * tick, a button reads what is already known, and the *line* refreshes
     * neither — the host draws it straight from the engine, every frame, with
     * nothing sent. On a device-clock transport this does nothing, since the
     * position is here.
     *
     * (A promise here and a plain call in the Python client, for the reason
     * every request is: a page waits for an answer instead of blocking on one.
     * The call is the same call.)
     */
    async refresh(): Promise<this> {
        if (this.headClock === "piece" && this.server !== null) {
            const state = await this.server.transportState();
            this.piece = {
                playing: state.playing,
                positionSample: state.positionSample,
            };
        }
        return this;
    }

    /**
     * **What the engine was just told**, on the piece: whether it rolls and
     * where it stands, remembered as {@link Transport.refresh} would have
     * answered, and every target's line drawn from the piece's position again.
     *
     * For a caller that sent the transport's commands itself — a playback whose
     * verbs are the shared crate's — so the answers this object gives before
     * the next refresh are the ones the commands made true.
     */
    reported({ playing, positionSample }: { playing?: boolean; positionSample?: number }): this {
        if (playing !== undefined) this.piece.playing = playing;
        if (positionSample !== undefined) this.piece.positionSample = positionSample;
        this.pieceAnchor();
        return this;
    }

    /**
     * Samples of the piece → beats, through the same map
     * {@link Transport.beatsToSamples} goes the other way — so what the engine
     * reports and what the ruler draws are one function read in two directions.
     */
    samplesToBeats(samples: number): number {
        const secs = this.sampleRate > 0 ? Number(samples) / this.sampleRate : 0.0;
        return this.tempoMap.beatsAt(secs);
    }

    /**
     * Draw every target's line straight from the piece's position: the anchor
     * is 0, because the counter the host reads already is that time.
     *
     * Re-applied rather than set once, because a view that redraws has new
     * widgets and they come up with no line at all.
     */
    private pieceAnchor(): void {
        if (this.host === null) return;
        for (const id of this.targets()) {
            this.host.set(id, { playhead_at: 0.0, playhead: -1.0 });
        }
    }

    /**
     * The transport's position in beats: where the playhead is while it plays,
     * where it got to while the last item is still ringing, and where the next
     * `play` starts when neither.
     *
     * On the **piece** it is what the engine last said ({@link
     * Transport.refresh}), not something kept here, which is the whole point: a
     * wrap at a loop's end and a seek some other client sent are both where it
     * says, and neither passed through this object. Asking is a round trip and
     * this is not, so a caller that wants it current refreshes first — the
     * *line* needs neither, since the host draws it straight from the engine
     * every frame.
     */
    get position(): number {
        if (this.headClock === "piece") {
            return this.samplesToBeats(this.piece.positionSample ?? 0);
        }
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
     *
     * On the **piece** it is `/transport_play`, and a bare one: the engine keeps
     * where it stopped, so resuming is the same verb as starting and nothing is
     * re-rendered. Given an `at` it seeks there first.
     */
    async play(
        server: Server | null = null,
        { at }: { at?: number } = {},
    ): Promise<Playhead | null> {
        if (server !== null) this.server = server;
        if (this.headClock === "piece") {
            if (at !== undefined) this.locate(at);
            this.pieceAnchor();
            // A `source` is still called, and it is the **events** half: what
            // follows the transport by itself (a reader on `TransportPos`)
            // needs no pass, and what fires voices does. So the two halves of a
            // piece meet here — the engine's readers, and a client pass the
            // transport's verbs cue.
            this.halt();
            this.head = this.source?.(at === undefined ? this.position : at) ?? null;
            await this.server?.transportPlay();
            this.piece.playing = true;
            return this.head;
        }
        const beat = at === undefined ? this.atBeat : Number(at);
        this.halt();
        this.atBeat = beat;
        this.ended = false;
        this.head = this.source?.(beat) ?? null;
        this.cursor(null); // the clock's line takes over from the cursor
        this.watch();
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
        if (this.headClock === "piece") {
            // Nothing to park and nothing to compute: the engine holds the
            // position where it froze, and the line holds with it.
            void this.server?.transportStop();
            this.piece.playing = false;
            // Governed, the pass is starved of time rather than stopped, so
            // resuming continues it; ungoverned there is nothing to starve.
            if (this.governed) this.clock?.freeze();
            else this.halt();
            return this.position;
        }
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
        this.watch();
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
     *
     * On the **piece** it is one `/transport_locateSample`, playing or not: the
     * seek happens in the engine, so nothing is re-cued and what is already
     * sounding carries on from there rather than being cut and started again.
     */
    locate(beat: number): this {
        const at = Math.max(Number(beat), 0.0);
        if (this.headClock === "piece") {
            const sample = Math.trunc(this.beatsToSamples(at));
            void this.server?.transportLocateSample(sample);
            this.piece.positionSample = sample;
            this.pieceAnchor();
            // The readers seek in the engine and need nothing; a pass of voices
            // has to be cued again, which is the one re-cue the piece keeps —
            // on a locate, and not on every edit.
            if (this.playing && this.source !== null) {
                this.halt();
                this.head = this.source(at);
            }
            return this;
        }
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
     * Have the end of the pass noticed, without anyone asking.
     *
     * {@link Transport.update} is the question "has it ended yet", and somebody
     * has to ask it. That used to be the caller's own loop — which is how every
     * example came to have one — and it is now the host's
     * {@link AppClock}, the same loop the window's gestures arrive on. A
     * transport with no host (a view built but never opened) simply keeps
     * `update` as the manual call it always was.
     */
    /**
     * The span of the piece the position wraps inside, in beats — or, with no
     * arguments (or `null`), looping off.
     *
     * **The piece's only**, because it is the only one the engine can wrap: the
     * wrap happens on its exact sample, so a pass repeats with no seam and no
     * client in the loop. A device-clock transport folds the *drawn* line
     * instead (`playhead_loop_*`), which is a different thing and stays the
     * view's.
     */
    loop(start: number | null = null, end: number | null = null): this {
        if (this.headClock !== "piece") {
            throw new Error(
                'a loop is the piece\'s: build the transport with headClock: "piece"',
            );
        }
        if (start === null || end === null) {
            void this.server?.transportLoop(null);
        } else {
            void this.server?.transportLoop([
                Math.trunc(this.beatsToSamples(Math.max(start, 0.0))),
                Math.trunc(this.beatsToSamples(Math.max(end, 0.0))),
            ]);
        }
        return this;
    }

    private watch(): void {
        if (this.ticking) return;
        const clock = this.host?.clock;
        if (clock === undefined) return;
        this.ticking = true;
        clock.sched(TICK, this.tick);
    }

    /**
     * One look, then another in `TICK` seconds while there is still something to
     * notice.
     *
     * Returning a number is how the clock reschedules, so this is a periodic
     * task with no loop of its own; returning nothing ends it.
     *
     * "Still something to notice" is **not** `playing`: a scan that has just run
     * out is not playing and is exactly the moment `update` exists for, so
     * stopping there would leave the cursor sweeping off the end forever. It is
     * the piece sounding, or a drained scan that has not been parked yet — and a
     * `pause`, which keeps its playhead without ending it, stops the asking
     * until the next `play`.
     *
     * A bound property rather than a method, because the clock keys what it has
     * queued by identity and a fresh closure per schedule would be unreachable
     * to `unsched`.
     */
    private readonly tick = (): number | undefined => {
        const head = this.head;
        if (head === null || this.ended || !(this.playing || head.finished)) {
            this.ticking = false;
            return undefined;
        }
        this.update();
        return TICK;
    };

    /**
     * Park the cursor when the pass ends by itself; it answers whether the piece
     * just ended.
     *
     * **Nothing has to call this.** A `play` schedules it on the host's
     * application clock for as long as the piece is sounding, so a caller
     * neither loops nor ticks. It stays public because a transport built before
     * its host exists has no clock to schedule on, and because asking the
     * question once more is always legal.
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
