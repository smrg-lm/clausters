// The server's shared transport grid: the beat every client phases on
// (mirrors `clausters/defs/server/transport.py`).
//
// The grid is one conductor's to define (`setTransport`) and everyone else's to
// join. Bound to a group it stops being advisory: the engine freezes and thaws
// that subtree, so a stop is a real pause of the sound the server is generating
// rather than a convention the clients observe.
//
// A server has several transports (its `--transports`), each independent -- its
// own grid, rolling state, position, loop, end mark and governed group. The
// methods here address transport 0 on a `Server`; `transportAt` answers any
// of them as a `Transport`, an object of its own played as a routine is, which
// a `Timeline` and a playback take.
//
// A mixin, composed into `Server` beside `ServerQueries` and `ServerStreams`,
// so no attribute path moves.

import { CommandError } from "../../errors.ts";
import { encodeImmediateBundle, toBundle } from "../../base/osc.ts";
import type { MsgArg, TimedMessage } from "../../base/osc.ts";
import { nodeId } from "../node.ts";
import type { NodeLike } from "../node.ts";
import type { Server } from "./index.ts";

/** The shared grid, as `transport()` reports it. */
export interface TransportGrid {
    /** The sample the grid puts beat 0 on, on the server's sample clock. */
    originSample: number;
    /** Beats per second. Beat `b` is `originSample + b * rate / tempo`. */
    tempo: number;
}

/**
 * The rolling state, plus the grid when one is defined, as `transportState()`
 * reports it.
 *
 * `originSample` and `tempo` are `null` until a client has defined a grid --
 * the transport exists whether or not anyone has, because rolling, stopping
 * and saying where the transport is need no beats.
 */
export interface TransportState {
    /** Beat 0 of the grid on the sample clock, or `null` with no grid. */
    originSample: number | null;
    /** Beats per second, or `null` with no grid. */
    tempo: number | null;
    /** Whether the transport is rolling. */
    playing: boolean;
    /**
     * The song-position **beat**: where play starts, or where a stop left it.
     * 0 while there is no grid, since there is nothing to measure it against --
     * `positionSample` is the live one either way.
     */
    position: number;
    /** The governed group, or `null` when nothing is bound. */
    group: number | null;
    /**
     * The transport clock: samples elapsed under the transport, held while it
     * is stopped. The device clock (`/clock_query`, the taps, the streams)
     * never stops, and this one holds -- but it is monotonic all the same, so a
     * locate does not move it. For where the transport *is*, read
     * `positionSample`.
     */
    transportSample: number;
    /**
     * Where the transport stands, in samples of its own axis --
     * what a playhead draws. Not a clock: it jumps to wherever a locate puts
     * it and wraps inside `loop`. Read from the engine as of its last
     * completed block -- except right after a locate, which the server answers
     * with the place it located to until a block has applied it, so a reply in
     * the same breath as a locate (its own broadcast above all) never reports
     * the place the transport is leaving.
     */
    positionSample: number;
    /**
     * The half-open span the transport loops inside, or `null`
     * when looping is off.
     */
    loop: [number, number] | null;
    /**
     * The end mark (`transportEnd`) as `[end, back]` -- `back` `null` when the
     * position rests on the mark -- or `null` when none is set.
     */
    end: [number, number | null] | null;
    /** Which transport this is: the handle's `transportId`. */
    transport: number;
    /** The group that follows it (`transportFollow`), or `null`. */
    follow: number | null;
    /** How long a stop and a play ramp, in samples (`transportFade`); 0 for none. */
    fade: number;
}

/** A reply matcher: whether a `/transport_query.reply` is about `transport`. */
function isTransport(transport: number) {
    return (args: readonly unknown[]): boolean =>
        args.length > 12 && Number(args[12]) === transport;
}

/** Where a view keeps the server it addresses. */
const VIEWED = Symbol("viewed server");

/** Each server's transports, one object per id. */
const TRANSPORTS = new WeakMap<Server, Map<number, Transport>>();

/**
 * A `Server` addressed through one of its transports: its transport methods
 * name that transport, a `schedClear("transport")` clears that transport's
 * queue alone, and everything else is the server's own. What a
 * {@link Transport} sends its commands through, and what a timeline on it
 * plays against.
 */
function addressed(server: Server, id: number): Server {
    if (id === 0) return server;
    return new Proxy(server, {
        get(target, prop, receiver) {
            if (prop === "transportId") return id;
            if (prop === VIEWED) return target;
            return Reflect.get(target, prop, receiver);
        },
    });
}

/** The shared transport grid. Composed into `Server`; never used alone. */
export class ServerTransport {
    /**
     * The transport this handle addresses: 0 on a `Server`, the one it was
     * made for on what `transportAt` answers.
     */
    get transportId(): number {
        return 0;
    }

    /**
     * **Transport `transport` of this server, as an object**: a
     * {@link Transport}, the same one every time it is asked for. Its verbs --
     * `play`, `pause`, `locate`, ... -- are that transport's, and it goes
     * wherever a transport is taken: `timeline.transport =
     * server.transportAt(n)`. The methods on the server itself address
     * transport 0, and that is the one transport a page names by number on
     * its own: every other is taken by what plays -- a sequence, an audio
     * editor, a GUI host's monitor -- so a number picked by hand may be
     * somebody's. {@link ServerTransport.transportNew} takes a free one; this
     * addresses one already known, an editor's `transport.id` say. An id past
     * the server's `--transports` fails when a command is sent, not here.
     */
    transportAt(this: Server, transport: number): Transport {
        const server = ((this as unknown as Record<symbol, Server>)[VIEWED] ?? this) as Server;
        let held = TRANSPORTS.get(server);
        if (held === undefined) {
            held = new Map();
            TRANSPORTS.set(server, held);
        }
        const id = Math.trunc(transport);
        let found = held.get(id);
        if (found === undefined) {
            found = new Transport(server, id);
            held.set(id, found);
        }
        return found;
    }

    /**
     * **A transport nobody holds, taken from this server's**, as a
     * {@link Transport}: for a timeline of its own, a group to govern apart
     * from everything else that plays. It is the caller's until
     * {@link Transport.free} gives it back.
     *
     * Throws when every transport is taken -- a server has a fixed number of
     * them (`--transports`), and a GUI host sharing the server takes half.
     */
    transportNew(this: Server): Transport {
        const server = ((this as unknown as Record<symbol, Server>)[VIEWED] ?? this) as Server;
        const transport = server.transportAt(server.ids.alloc("transports", 1));
        transport.taken = true;
        return transport;
    }

    /** `/transport_query` for this handle's transport, answered by the reply about it. */
    async queryTransport(this: Server, timeout?: number) {
        const msg = await this.request("/transport_query", [["i", this.transportId]], {
            expect: ["/transport_query.reply", "/fail"],
            match: isTransport(this.transportId),
            timeout,
        });
        if (msg.addr === "/fail") {
            throw new CommandError(`/transport_query failed: ${msg.args.join(" ")}`);
        }
        return msg;
    }

    /**
     * The server's shared transport grid (`/transport_query`), or `null` if
     * none is set. The grid lets several clients phase-align on the master
     * sample clock.
     */
    async transport(this: Server, timeout?: number): Promise<TransportGrid | null> {
        const msg = await this.queryTransport(timeout);
        if (!Number(msg.args[2])) return null;
        return {
            originSample: Number(msg.args[0]),
            tempo: Number(msg.args[1]),
        };
    }

    /**
     * Defines the server's shared transport grid (`/transport_set`): beat 0 at
     * `originSample` on the sample clock, advancing at `tempo` beats per
     * second. One client (the conductor) sets it; the others read it. Last
     * writer wins, and defining the grid resets the rolling state to stopped at
     * position 0 -- so a bound group freezes with it.
     */
    async setTransport(
        this: Server,
        originSample: number,
        tempo: number,
        timeout?: number,
    ): Promise<Server> {
        await this.command(
            "/transport_set",
            [["i", this.transportId], ["h", Math.trunc(originSample)], ["d", tempo]],
            timeout,
        );
        return this;
    }

    /**
     * The full shared transport state. **Always answers**: the transport exists
     * whether or not a grid does, so the grid fields are `null` rather than the
     * whole state being. Read the grid alone with `transport`, which still
     * answers `null` when none is set.
     *
     * `group` is the governed group (`transportGroup`) or `null` when nothing
     * is bound, and `transportSample` is the transport clock.
     */
    async transportState(this: Server, timeout?: number): Promise<TransportState> {
        const msg = await this.queryTransport(timeout);
        const defined = Boolean(Number(msg.args[2]));
        const group = Number(msg.args[5]);
        const loopStart = Number(msg.args[8]);
        const loopEnd = Number(msg.args[9]);
        const end = msg.args.length > 10 ? Number(msg.args[10]) : -1;
        const back = msg.args.length > 11 ? Number(msg.args[11]) : -1;
        return {
            originSample: defined ? Number(msg.args[0]) : null,
            tempo: defined ? Number(msg.args[1]) : null,
            playing: Boolean(Number(msg.args[3])),
            position: Number(msg.args[4]),
            group: group < 0 ? null : group,
            transportSample: Number(msg.args[6]),
            positionSample: Number(msg.args[7]),
            loop: loopEnd > loopStart ? [loopStart, loopEnd] : null,
            end: end < 0 ? null : [end, back < 0 ? null : back],
            transport: this.transportId,
            follow: msg.args.length > 13 && Number(msg.args[13]) >= 0
                ? Number(msg.args[13])
                : null,
            fade: msg.args.length > 14 ? Number(msg.args[14]) : 0,
        };
    }

    /**
     * Binds the group the transport governs (`/transport_group`), or unbinds
     * with `null`.
     *
     * This is what gives the transport its teeth. With no group bound it is a
     * shared beat grid plus a rolling state that clients obey by choice. With
     * one bound, the **engine** enforces it: `transportStop` freezes that
     * subtree and the server's transport clock, `transportPlay` thaws them.
     * Every node in the subtree keeps its internal state across the freeze, so
     * a resume continues the sound rather than restarting it -- which is the
     * only thing a pause can mean for sound the server generates itself.
     *
     * Freeing the group unbinds the transport, and unbinding thaws whatever it
     * governed, so no frozen subtree is left with nobody to resume it.
     */
    async transportGroup(
        this: Server,
        group: NodeLike | null,
        timeout?: number,
    ): Promise<Server> {
        const id = group === null ? -1 : nodeId(group);
        await this.command("/transport_group", [["i", this.transportId], ["i", id]], timeout);
        return this;
    }

    /**
     * Makes **event lane** `lane` on this transport (`/lane_new`): notes and
     * messages the transport plays by its position, as a reader plays a take
     * -- a locate moves them, a loop plays them again on every pass, a stop
     * releases them, with nothing sent per pass. `lane` is an id the caller
     * picks, like a buffer's; its notes are made at the tail of `target`,
     * which should be a group this transport does **not** govern: a stop
     * releases the notes, and a voice frozen there would sound again,
     * mid-release, on the next play. An existing lane of that id is freed
     * first.
     */
    async laneNew(this: Server, lane: number, target: NodeLike, timeout?: number): Promise<Server> {
        await this.command(
            "/lane_new",
            [["i", this.transportId], ["i", Math.trunc(lane)], ["i", nodeId(target)]],
            timeout,
        );
        return this;
    }

    /**
     * Replaces event lane `lane`'s data whole (`/lane_set`): `{notes: [[start,
     * end, voice, {control: value}, "gate" | "free"], ...], messages:
     * [[position, address, ...args], ...], midi: [[position, ...bytes],
     * ...], ump: [[position, ...words], ...]}`, every position a sample of
     * the transport's position and every
     * list optional. A note's `voice` is a def's name, or `{graph: id, slot:
     * name}` for one more of a slot of a running graph instance, the controls
     * its ports -- how a note carries the curves that shape it. A note is
     * released at `end` by `gate 0` or by a free; a
     * message is a command the server takes in a timed bundle, run as
     * written; a MIDI message plays as though it had reached the server's MIDI
     * input there, through its channel's `/midi_bind` binding, and a `ump`
     * entry -- one MIDI 2.0 packet's words -- the same way at its own
     * resolution, its per-note messages reaching the note on their channel
     * and key. What sounds
     * keeps its release, and
     * the new data is heard from where the position is.
     * The notes editor's playback writes it from an `EventSequence`.
     */
    async laneSet(
        this: Server,
        lane: number,
        data: Record<string, unknown>,
        timeout?: number,
    ): Promise<Server> {
        await this.command("/lane_set", [["i", Math.trunc(lane)], ["s", JSON.stringify(data)]], timeout);
        return this;
    }

    /**
     * Frees event lane `lane` (`/lane_free`): what it queued is dropped and the
     * notes it is sounding are released.
     */
    async laneFree(this: Server, lane: number, timeout?: number): Promise<Server> {
        await this.command("/lane_free", [["i", Math.trunc(lane)]], timeout);
        return this;
    }

    /**
     * Has `group` **follow** the transport (`/transport_follow`), or ends that
     * with `null`.
     *
     * Its nodes read the transport -- its position, whether it rolls -- as a
     * governed group's do, and nothing freezes them. It is for the part of an
     * application that must go on running while its transport is stopped, an
     * output with its meter and its declick, and still has to know that
     * transport. The governed group may sit inside it. A transport has one
     * following group, as it has one governed group, and a group is bound to
     * one transport either way.
     */
    async transportFollow(
        this: Server,
        group: NodeLike | null,
        timeout?: number,
    ): Promise<Server> {
        const id = group === null ? -1 : nodeId(group);
        await this.command("/transport_follow", [["i", this.transportId], ["i", id]], timeout);
        return this;
    }

    /**
     * Schedules `messages` at an absolute sample on the **transport** axis
     * (`/sched_atTransport`), the counterpart of `sendBundle`'s device axis.
     *
     * Declaring the axis is not about disambiguation -- classification is
     * deterministic, and a client that bound the group knows which of its nodes
     * are governed. It is about **verification**: the server compares the
     * declaration against its own classification and fails when they disagree,
     * instead of playing the bundle in the wrong place. Needs a group bound.
     */
    async schedAtTransport(
        this: Server,
        target: number,
        messages: readonly TimedMessage[],
        timeout?: number,
    ): Promise<Server> {
        const inner = encodeImmediateBundle(toBundle(messages));
        await this.command(
            "/sched_atTransport",
            [["i", this.transportId], ["h", Math.trunc(target)], ["b", inner]],
            timeout,
        );
        return this;
    }

    /**
     * Starts the shared transport rolling (`/transport_play`). With `position`
     * playback starts from that song-position beat; without it, from where it
     * last stopped or located. The server broadcasts the change to every
     * `/server_notify` client, so all following playheads roll together. Needs
     * a grid defined (`setTransport`).
     */
    async transportPlay(
        this: Server,
        position?: number,
        timeout?: number,
    ): Promise<Server> {
        const args: MsgArg[] = [["i", this.transportId]];
        if (position !== undefined) args.push(["d", position]);
        await this.command("/transport_play", args, timeout);
        return this;
    }

    /**
     * Stops the shared transport (`/transport_stop`); every following playhead
     * halts and a governed subtree freezes. Broadcast to `/server_notify`
     * clients.
     */
    async transportStop(this: Server, timeout?: number): Promise<Server> {
        await this.command("/transport_stop", [["i", this.transportId]], timeout);
        return this;
    }

    /**
     * Sets the shared transport's song position (`/transport_locate`) -- where
     * play starts, or where it seeks to while playing. Every following playhead
     * locates to it. Broadcast to `/server_notify` clients.
     *
     * It moves the position, never the state of a node: a governed subtree
     * stays exactly where it is, since a generator's position *is* its state.
     */
    async transportLocate(
        this: Server,
        position: number,
        timeout?: number,
    ): Promise<Server> {
        await this.command(
            "/transport_locate",
            [["i", this.transportId], ["d", position]],
            timeout,
        );
        return this;
    }

    /**
     * Seeks on the transport's own **sample** axis (`/transport_locateSample`).
     *
     * The sibling of `transportLocate`, which takes a beat: a sequencer locates
     * by beat and an audio editor by frame, and converting either into the
     * other on the client is how a rounding error gets into a seek. The beat
     * position follows, so both readings of `transportState` agree. Needs a
     * grid defined; a negative sample clamps to 0.
     */
    async transportLocateSample(
        this: Server,
        sample: number,
        timeout?: number,
    ): Promise<Server> {
        await this.command(
            "/transport_locateSample",
            [["i", this.transportId], ["h", Math.trunc(sample)]],
            timeout,
        );
        return this;
    }

    /**
     * Sets -- or clears, with `null` -- the span the transport loops
     * inside (`/transport_loop`), in samples.
     *
     * The span is **half-open**: `[0, n]` over an `n`-sample take plays every
     * frame exactly once and joins its own start with no repeated frame.
     * Turning a loop on does not move the transport; it keeps playing and wraps
     * when it first reaches the end, in the engine, so nothing has to be sent
     * once a pass completes. An empty or inverted span fails. What a loop
     * toggle remembers is the caller's to keep: clearing forgets the span.
     */
    async transportLoop(
        this: Server,
        span: [number, number] | null = null,
        timeout?: number,
    ): Promise<Server> {
        const args: MsgArg[] = [["i", this.transportId]];
        if (span !== null) args.push(["h", Math.trunc(span[0])], ["h", Math.trunc(span[1])]);
        await this.command("/transport_loop", args, timeout);
        return this;
    }

    /**
     * Sets -- or clears, with `null` -- the transport's **end mark**
     * (`/transport_end`), in samples: where a rolling transport stops, and
     * `back`, where it is located once it has; `null` leaves it on the mark.
     *
     * The stop is the engine's, on the mark's exact sample, and it is a stop
     * like `transportStop`: the governed group and the transport clock freeze,
     * and every `/server_notify` client is told. The mark stays set, so the
     * next play from `back` ends at the same place, and the transport never
     * rolls past it: a play from at or past the mark stops at once. **A loop
     * wins** -- while one is set the position wraps and never reaches the mark.
     */
    async transportEnd(
        this: Server,
        end: number | null = null,
        back: number | null = null,
        timeout?: number,
    ): Promise<Server> {
        const args: MsgArg[] = [["i", this.transportId]];
        if (end !== null) args.push(["h", Math.trunc(end)]);
        if (end !== null && back !== null) args.push(["h", Math.trunc(back)]);
        await this.command("/transport_end", args, timeout);
        return this;
    }

    /**
     * How long a stop and a play **ramp** (`/transport_fade`), in samples; 0,
     * the default, is no ramp.
     *
     * With a ramp a stop is a **stopping phase**: the governed group goes on
     * running and the position goes on advancing while the ramp falls to zero,
     * and then they freeze -- the position rests where the readers stopped
     * reading. A play thaws and ramps up. What reads the ramp is
     * `transportFade`, in a group that follows the transport
     * (`transportFollow`): an output multiplies what the readers wrote by it,
     * and neither edge clicks. The end mark starts its ramp that long before
     * the mark, so the pass still ends on it; a loop's wrap and a locate are
     * not ramped.
     */
    async transportFade(
        this: Server,
        samples: number,
        timeout?: number,
    ): Promise<Server> {
        await this.command(
            "/transport_fade",
            [["i", this.transportId], ["h", Math.trunc(samples)]],
            timeout,
        );
        return this;
    }
}

/**
 * What is loaded on a transport and plays through it: the playback of a
 * sequence, whose verbs speak its beats.
 *
 * @internal
 */
export interface TransportDriver {
    playing(): Promise<boolean>;
    play(at?: number): Promise<void>;
    pause(): Promise<void>;
    stop(): Promise<void>;
    locate(at: number): Promise<void>;
    span: [number, number] | null;
    looping: boolean;
    setSpan(span: readonly [number, number] | null): Promise<void>;
    setLooping(on: boolean): Promise<void>;
    end: null | "contents" | number;
    setEnd(end: null | "contents" | number): Promise<void>;
    free(): Promise<void>;
}

/**
 * **One of a server's transports, as an object**: what `Server.transportAt`
 * answers, and what `play(sequence)` answers for the transport the sequence
 * took -- one of its own, so two sequences play together, each driven by the
 * object its `play` answered.
 *
 * It is played the way a routine or a timeline is: `play`, `pause`, `stop`,
 * `locate`, `loop` and `unloop`, `playing`, and `wait`, which a page awaits or
 * not. Its positions are those of **what is loaded on it**: the beats of the
 * sequence a `play(sequence)` put there, and with nothing loaded, the
 * transport's own seconds.
 *
 * A sequence keeps its transport until it is freed: `free` releases what
 * sounds, frees its lane and gives the transport back to the server's, which
 * has a fixed number of them (`--transports`). They go with the server's
 * handle when it is closed.
 *
 * The transport's other commands are here by their own names too: `group`
 * and `follow` bind the groups it governs and leads, `fade` sets how a stop
 * and a play ramp, `locateSample` seeks on its sample axis, and `state` is what
 * the engine says of it. A server's own transport methods (`transportPlay`,
 * ...) address transport 0; this object is how any other is addressed.
 */
export class Transport {
    readonly #server: Server;
    readonly #id: number;
    /** What its commands are sent through. @internal */
    readonly view: Server;
    /** What is loaded on it and plays through it, when something is. @internal */
    driver: TransportDriver | null = null;
    /**
     * Whether `transportNew` took it for a page, whose `free` then gives it
     * back.
     *
     * @internal
     */
    taken = false;
    #rate: number | null = null;
    /** The span and the loop switch with nothing loaded, in seconds. */
    #span: [number, number] | null = null;
    #looping = false;

    /** @internal */
    constructor(server: Server, id: number) {
        this.#server = server;
        this.#id = id;
        this.view = addressed(server, id);
    }

    /** The server whose transport this is. */
    get server(): Server {
        return this.#server;
    }

    /** Which of the server's transports it is. */
    get id(): number {
        return this.#id;
    }

    async #samples(secs: number): Promise<number> {
        this.#rate ??= (await this.#server.queryInfo()).nominalSampleRate;
        return Math.round(secs * this.#rate);
    }

    // ---- played as a routine is ----

    /** The transport as the engine has it (`/transport_query`). */
    state(): Promise<TransportState> {
        return this.view.transportState();
    }

    /**
     * Whether the transport is rolling, as the engine answers -- a pass that
     * ended on its own stopped with nobody here saying so. A method here, the
     * reference client's property: asking the engine is a round trip.
     */
    async playing(): Promise<boolean> {
        if (this.driver !== null) return this.driver.playing();
        return (await this.state()).playing;
    }

    /**
     * Rolls -- from `at` when given, else from where it was paused, or located,
     * or from the start.
     */
    async play(at?: number): Promise<this> {
        if (this.driver !== null) {
            await this.driver.play(at);
            return this;
        }
        if (at !== undefined) await this.locate(at);
        await this.view.transportPlay();
        return this;
    }

    /** Stops where it stands: a `play` carries on from there. */
    async pause(): Promise<this> {
        if (this.driver !== null) await this.driver.pause();
        else await this.view.transportStop();
        return this;
    }

    /**
     * Stops and goes back to where the pass started. What is sounding is
     * released, and rings out.
     */
    async stop(): Promise<this> {
        if (this.driver !== null) {
            await this.driver.stop();
            return this;
        }
        await this.view.transportStop();
        await this.view.transportLocateSample(0);
        return this;
    }

    /** Puts the position at `at`: a rolling transport goes on from there, a stopped one starts there next. */
    async locate(at: number): Promise<this> {
        if (this.driver !== null) await this.driver.locate(at);
        else await this.view.transportLocateSample(await this.#samples(at));
        return this;
    }

    /**
     * **The time range** `[start, end]` a pass plays and a loop repeats, or
     * `null`. With a sequence loaded it is the range a sweep leaves on its
     * roll -- set it here and the roll draws it, sweep it there and it reads
     * here -- and `play` plays it, from its start to its end, as the space bar
     * does. Kept while stopped. Set with {@link Transport.setSpan}.
     */
    get span(): [number, number] | null {
        return this.driver !== null ? this.driver.span : this.#span;
    }

    /** Sets the time range: see {@link Transport.span}. */
    async setSpan(span: readonly [number, number] | null): Promise<this> {
        if (this.driver !== null) {
            await this.driver.setSpan(span);
            return this;
        }
        this.#span = span === null ? null : [span[0], span[1]];
        if (this.#looping) await this.#loopRaw();
        return this;
    }

    /** Whether the loop switch is on. */
    get looping(): boolean {
        return this.driver !== null ? this.driver.looping : this.#looping;
    }

    /**
     * **Loops**: with `start` and `end`, sets the span to them first; then turns
     * the loop on over the span -- or, with none, over every note of the
     * sequence loaded. A rolling transport follows at once, a stopped one on its
     * next `play`. The `L` key over a roll is the same switch.
     */
    async loop(start?: number, end?: number): Promise<this> {
        if (start !== undefined && end !== undefined) await this.setSpan([start, end]);
        if (this.driver !== null) {
            await this.driver.setLooping(true);
            return this;
        }
        this.#looping = true;
        await this.#loopRaw();
        return this;
    }

    /** Turns the loop off; the span stays. */
    async unloop(): Promise<this> {
        if (this.driver !== null) {
            await this.driver.setLooping(false);
            return this;
        }
        this.#looping = false;
        await this.view.transportLoop(null);
        return this;
    }

    async #loopRaw(): Promise<void> {
        if (this.#span === null) return;
        await this.view.transportLoop([await this.#samples(this.#span[0]), await this.#samples(this.#span[1])]);
    }

    /**
     * **Where a pass ends**: `null`, the transport rolling on until it is
     * stopped; a position, an end marker; or, with a sequence loaded,
     * `"contents"`, where its last note ends -- what `play(sequence)` sets. A
     * pass that reaches its end stops there, and `wait` resolves. A method
     * here, the reference client's property: asking the engine is a round
     * trip.
     */
    async end(): Promise<null | "contents" | number> {
        if (this.driver !== null) return this.driver.end;
        const end = (await this.state()).end;
        if (end === null) return null;
        this.#rate ??= (await this.#server.queryInfo()).nominalSampleRate;
        return end[0] / this.#rate;
    }

    /** Sets where a pass ends: see {@link Transport.end}. */
    async setEnd(end: null | "contents" | number): Promise<this> {
        if (this.driver !== null) await this.driver.setEnd(end);
        else if (end === "contents") throw new Error("a transport with nothing loaded has no contents");
        else await this.view.transportEnd(end === null ? null : await this.#samples(end));
        return this;
    }

    /**
     * **Frees what is loaded on it, and gives it back**: the sequence a
     * `play(sequence)` put there stops, its notes released, its lane is freed,
     * and the transport goes back to the server's for something else to take
     * -- this object is then a transport with nothing loaded. One
     * `transportNew` answered goes back the same way. A transport addressed by
     * number has nothing to free.
     */
    async free(): Promise<this> {
        if (this.driver !== null) await this.driver.free();
        else if (this.taken) {
            this.taken = false;
            this.#server.ids.release("transports", this.#id, 1);
        }
        return this;
    }

    /**
     * Resolves when the transport stops -- a pass that ends where its contents
     * do stops on its own -- or after `timeout` seconds; answers whether it
     * stopped. A page that plays and goes on awaits it; a live one does not.
     */
    async wait(timeout?: number): Promise<boolean> {
        const deadline = timeout === undefined ? Infinity : performance.now() + timeout * 1000;
        while (await this.playing()) {
            if (performance.now() >= deadline) return false;
            await new Promise((resolve) => setTimeout(resolve, 50));
        }
        return true;
    }

    // ---- the transport's other commands ----

    /** Binds the group this transport governs: see `Server.transportGroup`. */
    async group(group: NodeLike | null): Promise<this> {
        await this.view.transportGroup(group);
        return this;
    }

    /** Has `group` follow this transport: see `Server.transportFollow`. */
    async follow(group: NodeLike | null): Promise<this> {
        await this.view.transportFollow(group);
        return this;
    }

    /** How long a stop and a play ramp, in samples: see `Server.transportFade`. */
    async fade(samples: number): Promise<this> {
        await this.view.transportFade(samples);
        return this;
    }

    /** Seeks on the transport's own sample axis: see `Server.transportLocateSample`. */
    async locateSample(sample: number): Promise<this> {
        await this.view.transportLocateSample(sample);
        return this;
    }

    /**
     * Makes event lane `lane` on this transport: see `Server.laneNew`. Its data
     * and its end are the server's (`laneSet`, `laneFree`).
     */
    async laneNew(lane: number, target: NodeLike): Promise<this> {
        await this.view.laneNew(lane, target);
        return this;
    }

    /** Drops what is queued on this transport's clock (`/sched_clear "transport"`). */
    schedClear(): this {
        this.view.schedClear("transport");
        return this;
    }

    toString(): string {
        return `Transport(${this.#id})`;
    }
}
