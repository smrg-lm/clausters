// Session: an explicit, isolated environment (mirrors `clausters/session.py`).
//
// A `Session` is the unit of isolation: it bundles a `Server` and a
// `TempoClock` into one handle with `play` / `run`, and the two factories pick
// sensible defaults for the two carriers a page has. Because each session owns
// its own state, **several coexist** — one against this page's engine beside
// one against a `--ws` server — without touching each other.
//
// That is why this arrived late and matters now. A page used to hold one
// engine and one GUI host, so wiring them by hand was three lines and the
// singletons *were* the environment. Since the hosts and engines became
// instances (`engine()`, `newGuiHost()`), the environment is a thing a page
// has several of, and a `Session` is the handle that keeps one coherent: this
// clock, this server, this GUI host, this random root.
//
// The counterpart is the **default session**, `defaultSession` (`base/main.ts`
// — `main` is its short name): the ambient environment used whenever no
// session was named. An explicit `Session` is simply a *named* environment
// that never touches the default one.
//
// ```ts
// const s = await Session.embed();
// s.clock.setTempo(2.0);
// s.play(new Pbind({ instrument: "default", freq: Pseq([440, 550]), dur: 0.5 }));
// ```
//
// **What the Python client has here and this one does not, yet.** Its `nrt`
// factory and `render` need an offline drive, which is its own milestone. The
// factories are named for the carriers this package already names them by —
// `page`/`connect`, as on `Server` and `GuiHost` — rather than for Python's
// `embed`/`live`, whose parameters (a host, a port, a process to boot) a page
// has none of.

import { TempoClock } from "./base/clock.ts";
import { LogicalTimebase, MonotonicTimebase } from "./base/timebase.ts";
import type { Timebase } from "./base/timebase.ts";
import { pageConnection, ScoreConnection, WsConnection } from "./base/connection.ts";
import type { Connection } from "./base/connection.ts";
import { OscDestination } from "./base/destination.ts";
import { Environment } from "./base/environment.ts";
import { main } from "./base/main.ts";
import type { IdShare } from "./base/ids.ts";
import { loadCore } from "./base/core.ts";
import type { RenderOptions, RenderStats } from "./render.ts";
import { Server } from "./defs/server/index.ts";
import type { ServerOptions } from "./defs/server/index.ts";
import type { ClaustersServer } from "./engine/server.ts";
import { engine as engineInstance, server as pageEngine } from "./engine/server.ts";
import { GuiHost } from "./gui/host.ts";
import { newGuiHost, pageGuiConnection } from "./gui/page.ts";
import type { ClaustersGui } from "./gui/page.ts";
import type { EventStreamPlayer } from "./seq/eventstream.ts";
import type { Pattern } from "./seq/pattern.ts";
import { play as playVerb } from "./play.ts";

/** What both factories take on top of their carrier's own options. */
export interface SessionOptions {
    /**
     * The session's clock, on the session's timebase. Omitted, the session
     * makes one at tempo 1.0; the tempo is the clock's, so it is set there
     * (`session.clock.setTempo(2.0)`).
     */
    clock?: TempoClock;
    /**
     * The time every clock of the session is made on, fixed for the session's
     * life. Left unset (and with no `clock`) it is **its server's sample
     * clock**, which is sample-accurate and drift-free — and in the page it is
     * exact, the engine and the `AudioContext` being one clock — and a server
     * that does not answer it throws, since a clock's timebase is fixed when
     * it is made and there is nothing to fall back to afterwards. Pass
     * `new MonotonicTimebase()` for wall-clock timetags.
     */
    timebase?: Timebase;
    /** Seconds added to each event's timetag; see `Server.latency`. */
    latency?: number;
    /** How long a reply is waited for when a call does not say. */
    timeout?: number;
    /**
     * The slice of the server's client id space this session allocates from,
     * when the engine underneath has **more than one client** — one client
     * authoring over a carrier of its own while the page holds a session on
     * that same engine. Both legs take it, the audio server's node, bus and
     * buffer ids and the GUI host's widget ids alike, so a session is one
     * share of everything rather than of one space. See `IdShare`; the
     * default takes the whole space.
     */
    share?: IdShare;
}

/**
 * One `Server` plus one `TempoClock`, bundled into a single handle.
 *
 * This is the client's ergonomic entry point. Rather than wiring a connection,
 * a server, a clock and a timebase together yourself, you take a `Session`
 * that owns them and drives them as a unit — `play` a pattern on it, `run` it
 * for some seconds, `gui()` a host wired to its own engine.
 *
 * Prefer the factories to the constructor: `embed` opens a session on the
 * server inside this tab and `live` one on a `--ws` server, each with sensible
 * defaults — the reference client's two names, for the same two situations. The constructor is for the uncommon case of supplying your own
 * `Server` and clock.
 *
 * Which factory you call is the *only* thing that differs between the two
 * carriers: that difference lives in the `Server`'s connection, not in the
 * pattern or the clock. So the same `play` drives either, and both can run
 * side by side in one page.
 *
 * It is an `Environment` — the same base the default session extends — so a
 * named session and the default one are the same kind of thing. That makes it
 * its **own random context** (`seed` / `rng`): `session.seed(n)` reproduces
 * *this* session's events without touching another's. Anything created
 * while the session drives (`play`) or inside `use()` draws from this root.
 */
export class Session extends Environment {
    override server: Server;
    /** The clock that sequences this session's server. */
    readonly clock: TempoClock;

    private gui_: GuiHost | null = null;
    /**
     * The page host `gui()` booted, if it booted one — the wasm instance
     * behind the client, which nothing else on the page holds and which
     * therefore has to be released with the session.
     */
    private ownedGui: ClaustersGui | null = null;
    private ownedEngine: ClaustersServer | null = null;
    private readonly destinations: OscDestination[] = [];

    /**
     * Drives `server` on `clock` — a fresh one at tempo 1.0, on the session's
     * timebase, when omitted. A clock that already belongs to another session,
     * or is on another timebase than `timebase`, throws.
     *
     * The clock gets a back-reference to this session, so a play running on it
     * resolves *this* session's server and random root — which is what keeps
     * several sessions isolated from each other and from the default one.
     *
     * `gui` is a host this session drives instead of opening one — the visual
     * half of taking a `Server` the session did not open, and the way a session
     * adopts a host reached with `GuiHost.connect`. `gui()` then returns it
     * rather than opening anything.
     *
     * `timebase` is the physical time every clock of this session paces
     * against, fixed for the session's life. Omitted it is `clock`'s when one
     * is given, else a `LogicalTimebase` for an offline server — the only one
     * an offline session can have — and the page's monotonic clock otherwise
     * (`embed` and `live` default to the server's sample clock).
     */
    constructor(server: Server, clock?: TempoClock, gui?: GuiHost, timebase?: Timebase) {
        super();
        this.server = server;
        if (clock !== undefined) {
            if (timebase !== undefined && timebase !== clock.timebase) {
                throw new Error(
                    `this clock is on a ${clock.timebase.constructor.name} and the session was `
                    + `asked for a ${timebase.constructor.name}: every clock of a session is on `
                    + `the session's timebase, and a clock's is fixed when it is made`,
                );
            }
            timebase = clock.timebase;
        }
        if (server.connection instanceof ScoreConnection) {
            if (timebase === undefined) {
                timebase = new LogicalTimebase();
            } else if (!(timebase instanceof LogicalTimebase)) {
                throw new Error(
                    `an offline session is on a LogicalTimebase, and this one was given a `
                    + `${timebase.constructor.name}: an offline run has no physical time to wait `
                    + `on, so logical time is the only time its clocks can have`,
                );
            }
        } else if (timebase === undefined) {
            timebase = new MonotonicTimebase();
        }
        this.timebaseHeld = timebase;
        this.clocks_ = [];
        // Made with this session in force, so it takes the session's timebase
        // and is kept here -- whatever session is active around this constructor.
        this.clock = this.adopt(clock ?? this.use(() => new TempoClock()));
        this.gui_ = gui ?? null;
    }

    private clocks_!: TempoClock[];
    private readonly timebaseHeld: Timebase;

    /**
     * The physical time every clock of this session paces against, fixed for
     * the session's life: the server's sample clock for an embedded or live
     * session, a `LogicalTimebase` for an offline one.
     *
     * A `TempoClock` made while this session is active is made on it and kept
     * here, so the clocks of one session never disagree about what "now" is.
     */
    get timebase(): Timebase {
        return this.timebaseHeld;
    }

    /** @internal */
    timebaseForClock(timebase: Timebase | undefined): Timebase {
        if (timebase === undefined || timebase === this.timebaseHeld) return this.timebaseHeld;
        throw new Error(
            `a clock made while a session is active is on the session's timebase `
            + `(${this.timebaseHeld.constructor.name}), and this one asked for a `
            + `${timebase.constructor.name}; make it with no timebase, or with no session active`,
        );
    }

    /**
     * Every clock this session owns, the default ({@link Session.clock})
     * first.
     *
     * A session is one server and *as many clocks as the piece has tempos*.
     * {@link Session.adopt} is what puts one here, and a clock built while this
     * session is ambient adopts it by itself, so ten hand-made clocks are
     * already the session's without a line saying so.
     */
    get clocks(): readonly TempoClock[] {
        return this.clocks_;
    }

    /**
     * Takes `clock` into this session and returns it.
     *
     * Sets the clock's `session` back-reference, which is what an ambient play
     * follows from inside a routine. A clock this session already holds is not
     * taken twice.
     *
     * Called for you: a `TempoClock` made while this session is active is
     * adopted at construction. Call it by hand for a clock made with no session
     * active, on this session's timebase.
     *
     * A clock that belongs to another session throws, and so does one on
     * another timebase: a clock's timebase is fixed when it is made, so it
     * cannot join a session that paces against another time.
     */
    adopt(clock: TempoClock): TempoClock {
        const previous = clock.session;
        if (previous != null && previous !== this) {
            throw new Error(
                "this clock already belongs to another session; a clock is kept by the "
                + "session it was made in",
            );
        }
        if (clock.timebase !== this.timebaseHeld) {
            throw new Error(
                `this clock is on a ${clock.timebase.constructor.name} and the session on a `
                + `${this.timebaseHeld.constructor.name}: every clock of a session is on the `
                + `session's timebase`,
            );
        }
        clock.session = this;
        if (!this.clocks_.some((held) => held === clock)) this.clocks_.push(clock);
        return clock;
    }

    /**
     * Drops `clock` from this session: it is no longer closed with it and no
     * longer answers an ambient play. Returns the clock.
     *
     * The **default clock cannot be released** — a session without one has no
     * answer for `play` — so releasing it throws rather than leaving the
     * session in a state nothing checks for.
     */
    release(clock: TempoClock): TempoClock {
        if (clock === this.clock) {
            throw new Error(
                "the default clock belongs to its session: adopt another clock "
                + "and stop this one, rather than releasing it",
            );
        }
        this.clocks_ = this.clocks_.filter((held) => held !== clock);
        if (clock.session === this) clock.session = null;
        return clock;
    }

    // ---- the factories (the "defaults", explicit) ----

    /**
     * An **offline** session: its `Server` writes a timestamped score instead
     * of sending anything, and `render` turns that score into samples through
     * the engine's own renderer running as fast as it can.
     *
     * No `AudioContext`, no gesture, no socket and no server process — which
     * is why this is the one factory that is not asynchronous past loading the
     * codec. What is *not* different is everything above the carrier: the same
     * patterns, defs and routines play into it, because only the connection
     * underneath the `Server` changed.
     *
     * Its timebase is a `LogicalTimebase`, and it is the only one it can have:
     * every clock made while it is active is on that logical time. `clock` is
     * the session's clock, made on a `LogicalTimebase` (omitted, one at tempo
     * 1.0 on the session's); `timebase` is the session's `LogicalTimebase`
     * (omitted, a new one; any other kind throws).
     */
    static async nrt(
        { clock, timebase }: { clock?: TempoClock; timebase?: Timebase } = {},
    ): Promise<Session> {
        await loadCore();
        // Neither booted nor attached: a score has no server to bring up and
        // none to reach, so the handle is the bare one the reference client
        // builds (`Server(interface=OscNrtInterface())`) and the allocators keep
        // the compiled sizing, which is the whole truth about an offline run.
        const server = new Server({ connection: new ScoreConnection() });
        return new Session(server, clock, undefined, timebase);
    }

    /**
     * A session on an **embedded** server — the audio server compiled to wasm
     * and running in this tab's AudioWorklet, with no process and no socket
     * anywhere.
     *
     * The reference client's `Session.embed`, and the same thing it names: the
     * server inside this program rather than one it talks to. There it is the
     * bundled native library in this process; here it is wasm in this tab, and
     * either way the client shares memory with it — which is what lets a whole
     * take go into a buffer in one copy.
     *
     * `boot`s an engine of this session's own, which is the reference
     * client's default and the reason there is no flag here: ownership is the
     * verb's to say. So its nodes, buses and buffers share nothing with the
     * rest of the document — the case several sessions in one page exist for.
     * Pass `engine` to drive one that is already open instead; an engine this
     * session opened is closed with it, one handed in is not.
     *
     * The `AudioContext` needs a user gesture to start, so call this from a
     * click rather than at load.
     */
    static async embed({
        engine,
        channels,
        clock,
        timebase,
        latency,
        timeout,
        share,
    }: SessionOptions & {
        /**
         * An engine to drive rather than open — the reference client's
         * `server=`, and the same rule: one handed in is not this session's to
         * close.
         */
        engine?: ClaustersServer;
        /**
         * The engine's output channel count. The reference reads this from the
         * server's own options or its config file; a page has neither, and an
         * `AudioContext`'s output width is fixed when it is created, so the
         * only place to say it is here.
         */
        channels?: number;
    } = {}): Promise<Session> {
        await loadCore();
        // Handed an engine, this session drives that one and does not own it —
        // the reference client's `server=`. Otherwise `boot` brings up one of
        // its own, which is the reference's default and the reason there is no
        // flag here: ownership is the verb's to say.
        if (engine !== undefined) {
            return Session.over(
                { connection: await pageConnection(engine), timeout, share },
                { clock, timebase, latency },
                "boot",
            );
        }
        const options = channels === undefined ? {} : { channels };
        const audio = await engineInstance(options);
        const session = await Session.over(
            { connection: await pageConnection(audio), timeout, share },
            { clock, timebase, latency },
            "boot",
        );
        session.ownedEngine = audio;
        return session;
    }

    /**
     * A session on a **live server** — one running as its own process,
     * reached over a WebSocket (`clausters --ws`).
     *
     * The reference client's `Session.live`, minus the half a tab cannot do:
     * there, `live` **boots** a server when none answers; here it can only
     * `attach`, since a page starts no process. The address defaults the same
     * way, but nobody answering it is an error rather than something this call
     * fixes by starting one.
     *
     * The browser's only network carrier, and the one that reaches a server
     * with the whole def catalogue (the embedded engine is the `synth,embed`
     * build, with no Faust JIT).
     */
    static async live(
        url = "ws://127.0.0.1:57120",
        { clock, timebase, latency, timeout, share }: SessionOptions = {},
    ): Promise<Session> {
        await loadCore();
        // `attach`: nothing here started that server, and a WebSocket that
        // connects proves a listener rather than a server — so the session
        // refuses to be built against silence instead of dropping every later
        // message into it.
        return Session.over(
            { transport: "ws", url, timeout, share },
            { clock, timebase, latency },
            "attach",
        );
    }

    /** Builds the session around a server described by `options`. */
    private static async over(
        options: ServerOptions,
        { clock, timebase, latency }: SessionOptions,
        how: "boot" | "attach",
    ): Promise<Session> {
        // Not the default session's by being built: `activate()` is the verb
        // for that, the way the reference client's `Session.live` passes
        // `adopt_default=False` and leaves the slot to whoever asks.
        const server = new Server(options);
        await server[how]({ adoptDefault: false });
        if (latency !== undefined) server.latency = latency;
        // With no clock and no timebase, the session is made on the server's
        // own sample clock: sample-accurate out of the box. A server that does
        // not answer throws, since a clock's timebase is fixed when it is made.
        if (clock === undefined && timebase === undefined) {
            timebase = await server.sampleTimebase({ timeout: server.timeout });
        }
        return new Session(server, clock, undefined, timebase);
    }

    // ---- the GUI leg ----

    /**
     * The GUI host this session draws on, opened once and wired to **this
     * session's** engine, so a bound widget reaches this server and not the
     * page's. The browser parallel of the Python client's `session.gui()`,
     * which boots a `clausters-gui` process pointed at its session's server.
     *
     * Idempotent: repeated calls return the same `GuiHost`. It is owned by
     * the session and released on `close`. A session **given** a host (the
     * constructor's `gui`) is already settled: this returns that host and
     * opens nothing — the same way a `Server` is taken rather than opened when
     * the constructor is used. That is how a session drives a native
     * `clausters-gui --ws` host: `new Session(server, clock, await
     * new GuiHost(await WsConnection.open(url)).attach())`.
     *
     * A session on the page's shared engine gets the page's host (canvas in
     * `<body>` included); one holding its own engine gets a host of its own,
     * which appends no canvas — pass yours to the def, as a component does.
     */
    /**
     * The GUI host this session already has, or `null` when none was opened —
     * what the ambient visual verbs read, so they draw on a session's host
     * rather than opening a second one, without opening one themselves.
     */
    get guiHost(): GuiHost | null {
        return this.gui_;
    }

    async gui(): Promise<GuiHost> {
        if (this.gui_) return this.gui_;
        // The session's share of the host's widget ids is the one it has on
        // its engine: a session that is one of two clients on an engine is one
        // of two on its host as well.
        const share = this.server.share;
        if (this.ownedEngine) {
            // A host of this session's own, wired to this session's engine —
            // and this session's to close, unlike the page's shared one. **It
            // allocates on that engine too** (its voices, its take monitor, the
            // piece it plays), so the session's ids are split with it: the
            // session keeps the first half and the host takes the second.
            this.ownedGui = await newGuiHost({
                engine: this.ownedEngine,
                idShare: this.server.splitShare(),
            });
            this.gui_ = await new GuiHost({ gui: this.ownedGui, share }).boot();
        } else {
            this.gui_ = await new GuiHost({ share }).attach();
        }
        return this.gui_;
    }

    // ---- driving ----

    /**
     * Makes this session the ambient one for the duration of `body`, so
     * anything created in it (a played routine, a bare `new Synth`) resolves
     * to *this* session's server, clock and random root rather than the
     * default session's.
     *
     * The counterpart of the Python client's `with session:` block. It
     * restores the previous session afterwards, so nesting is safe — but it is
     * **synchronous by design**: an `await` inside would let another task run
     * while this session is ambient, and the page's one thread has no way to
     * scope that. Do the awaiting outside and the creating inside.
     */
    use<T>(body: (session: this) => T): T {
        const previous = main.sessionContext;
        main.currentSession = this;
        try {
            return body(this);
        } finally {
            main.sessionContext = previous;
        }
    }

    /**
     * Plays an event pattern on this session's clock and server. A value
     * pattern does not play, and throws.
     *
     * @param quant the beat grid the player starts on; omitted, it starts now.
     */
    play(pattern: Pattern<unknown>, quant?: number): EventStreamPlayer {
        return this.use(() =>
            playVerb(pattern, { server: this.server, clock: this.clock, quant }) as EventStreamPlayer);
    }

    /**
     * Joins this session's server's shared transport, so a `quant`-ed pattern
     * starts on the same beat as every other client on it (see
     * `TempoClock.joinTransport`). Resolves with `this`, so it chains after a
     * factory; on a session made on the sample clock (the default) the
     * alignment is sample-exact. A server with no transport defined leaves the
     * clock's own grid alone.
     */
    async joinTransport(): Promise<this> {
        await this.clock.joinTransport(this.server, this.server.timeout);
        return this;
    }

    /**
     * Drains the clock and renders the accumulated score (offline sessions
     * only).
     *
     * Advances the clock logically, with no real-time waiting, so everything
     * scheduled lands in the score, and then renders that score. Schedule a
     * closing event — freeing the root group, or whatever ends the piece — so
     * the render has a defined length: the renderer stops when the score does,
     * and commands do not sound.
     *
     * `until` bounds the drain in beats, which an endless source needs (an
     * infinite pattern never drains on its own).
     */
    async render(options: RenderOptions & { until?: number } = {}): Promise<RenderStats> {
        const { until, ...rest } = options;
        this.use(() => {
            this.clock.render(until);
        });
        return this.server.render(rest);
    }

    /**
     * Runs the clock for `seconds` and then stops it; resolves with `this`.
     *
     * Where the Python client blocks a thread, this one awaits — the page has
     * one thread and has to keep running, which is the same rule the rest of
     * this client follows.
     */
    async run(seconds: number): Promise<this> {
        this.start();
        await new Promise((resolve) => setTimeout(resolve, seconds * 1000));
        return this.stop();
    }

    /**
     * Starts **every** clock the session owns, so scheduled events fire in real
     * time; returns `this`. A restart **resumes** at the beat `stop` left each
     * clock on.
     *
     * A piece with one clock reads exactly as it always did. A piece with
     * several starts them together, which is what makes them start together —
     * starting ten clocks in a loop staggers them by whatever the loop costs.
     */
    start(): this {
        for (const clock of this.clocks_) clock.start();
        return this;
    }

    /**
     * Stops **every** clock the session owns; returns `this`. Nothing further
     * fires while they are stopped, but each schedule is kept and each beat is
     * held: this is a transport, and a later `start` picks the music up where
     * it was (`session.clock.clear()` is what drops what is queued).
     */
    stop(): this {
        for (const clock of this.clocks_) clock.stop();
        return this;
    }

    /**
     * Makes this the ambient session, and leaves it there; returns `this`.
     *
     * The unscoped form of {@link Session.use}. A block is the right shape when
     * the session's life is the block's, and the wrong one for an environment
     * that outlives every statement that uses it — which in a page is the
     * ordinary case, not the exception: event handlers, a console, a timer, an
     * `await` in the middle of a setup routine. After this, anything created
     * with no session named (`play(...)`, a bare `new Synth`) resolves to *this*
     * session's server, clock and random root.
     *
     * The reference client's `Session.activate`, and its reason for existing is
     * the same one written there: there is no block to be inside of.
     */
    activate(): this {
        main.currentSession = this;
        return this;
    }

    /**
     * Gives up being the ambient session; returns `this`.
     *
     * The counterpart of {@link Session.activate}, and a no-op when some
     * *other* session is ambient — giving up a slot one does not hold would
     * silently unseat the session that does.
     */
    deactivate(): this {
        if (main.currentSession === this) main.currentSession = null;
        return this;
    }


    /**
     * An external OSC application as a destination, living as long as this
     * session (`close` closes it).
     *
     * What it sends is standard OSC — a message, or a bundle timetagged at the
     * ambient `Moment`, so a sequence sent to another application keeps the
     * same logical timing as one sent to the server. What it does not send is
     * anything of ours: no `Server.latency`, no sample-accurate `/sched_at`.
     *
     * The carrier is a WebSocket, the page having no UDP socket to open —
     * that is the only difference from the Python client's, whose `host`/
     * `port` open one directly. The connection is this session's and is
     * closed with it.
     */
    async destination(url: string): Promise<OscDestination> {
        const dest = await OscDestination.open(url);
        this.destinations.push(dest);
        return dest;
    }

    /**
     * Releases everything this session owns: its GUI host — the client, and
     * the wasm host under it when `gui()` booted one — its destinations, its
     * clock, its server client, and an engine it opened for itself (the page's
     * shared host and engine are not this session's to stop). If it had
     * adopted the default slots, it gives them up.
     */
    close(): void {
        this.gui_?.stop();
        this.gui_ = null;
        // The client detaches; the wasm host itself is released only when this
        // session is the one that booted it (the page's is shared page state).
        this.ownedGui?.close();
        this.ownedGui = null;
        // Each closes the carrier it opened, which is the destination's own
        // rule now rather than a list of sockets this session keeps beside it.
        for (const dest of this.destinations) dest.close();
        this.destinations.length = 0;
        // Every clock the session owns, not just the default.
        for (const clock of this.clocks_) clock.close();
        this.clocks_.length = 0;
        this.server.close();
        void this.ownedEngine?.close();
        this.ownedEngine = null;
        if (main.server === this.server) main.server = null;
        if (main.currentSession === this) main.currentSession = null;
    }
}
