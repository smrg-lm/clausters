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
// const s = await Session.embed({ tempo: 2.0 });
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
import type { EventDestination } from "./seq/event.ts";
import type { EventStreamPlayer } from "./seq/eventstream.ts";
import type { Pattern } from "./seq/pattern.ts";

/** What both factories take on top of their carrier's own options. */
export interface SessionOptions {
    /** The clock's tempo, in beats per second. */
    tempo?: number;
    /**
     * The clock's pacing source. Left unset the session **anchors to its
     * server's sample clock**, which is sample-accurate and drift-free — and
     * in the page it is exact, the engine and the `AudioContext` being one
     * clock. Pass `new MonotonicTimebase()` to keep wall-clock timetags.
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
     * Drives `server` on `clock` (a fresh one at tempo 1.0 when omitted).
     *
     * The clock gets a back-reference to this session, so a play running on it
     * resolves *this* session's server and random root — which is what keeps
     * several sessions isolated from each other and from the default one.
     */
    /**
     * `gui` is a host this session drives instead of opening one — the visual
     * half of taking a `Server` the session did not open, and the way a session
     * adopts a host reached with `GuiHost.connect`. `gui()` then returns it
     * rather than opening anything.
     */
    constructor(server: Server, clock?: TempoClock, gui?: GuiHost) {
        super();
        this.server = server;
        this.clock = clock ?? new TempoClock();
        this.clock.session = this;
        this.gui_ = gui ?? null;
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
     * The clock's tempo is what maps the piece's beats onto the render's
     * seconds; at the default 1.0 a beat is a second.
     */
    static async nrt({ tempo = 1.0 }: { tempo?: number } = {}): Promise<Session> {
        await loadCore();
        // Neither booted nor attached: a score has no server to bring up and
        // none to reach, so the handle is the bare one the reference client
        // builds (`Server(interface=OscNrtInterface())`) and the allocators keep
        // the compiled sizing, which is the whole truth about an offline run.
        const server = new Server({ connection: new ScoreConnection() });
        return new Session(server, new TempoClock(tempo));
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
        tempo = 1.0,
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
                { tempo, timebase, latency },
                "boot",
            );
        }
        const options = channels === undefined ? {} : { channels };
        const audio = await engineInstance(options);
        const session = await Session.over(
            { connection: await pageConnection(audio), timeout, share },
            { tempo, timebase, latency },
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
        { tempo = 1.0, timebase, latency, timeout, share }: SessionOptions = {},
    ): Promise<Session> {
        await loadCore();
        // `attach`: nothing here started that server, and a WebSocket that
        // connects proves a listener rather than a server — so the session
        // refuses to be built against silence instead of dropping every later
        // message into it.
        return Session.over(
            { transport: "ws", url, timeout, share },
            { tempo, timebase, latency },
            "attach",
        );
    }

    /** Builds the session around a server described by `options`. */
    private static async over(
        options: ServerOptions,
        { tempo, timebase, latency }: SessionOptions,
        how: "boot" | "attach",
    ): Promise<Session> {
        // Not the default session's by being built: `activate()` is the verb
        // for that, the way the reference client's `Session.live` passes
        // `adopt_default=False` and leaves the slot to whoever asks.
        const server = new Server(options);
        await server[how]({ adoptDefault: false });
        if (latency !== undefined) server.latency = latency;
        const session = new Session(server, new TempoClock(tempo, { timebase }));
        // With no explicit timebase, anchor to the server's own sample clock:
        // a session is sample-accurate out of the box. Graceful — a server
        // that does not answer leaves the clock on wall-clock time.
        if (timebase === undefined) await session.lockToServer();
        return session;
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
        // The session's share governs both legs: a session that is one of two
        // clients on an engine is one of two on its host as well.
        const share = this.server.share;
        if (this.ownedEngine) {
            // A host of this session's own, wired to this session's engine —
            // and this session's to close, unlike the page's shared one.
            this.ownedGui = await newGuiHost({ engine: this.ownedEngine });
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
        const previous = main.currentSession;
        main.currentSession = this;
        try {
            return body(this);
        } finally {
            main.currentSession = previous;
        }
    }

    /**
     * Plays an event pattern on this session's clock and server.
     *
     * @param quant the beat grid the player starts on; omitted, it starts now.
     */
    play(pattern: Pattern<unknown>, quant?: number): EventStreamPlayer {
        return this.use(() =>
            pattern.play(this.server as unknown as EventDestination, {
                clock: this.clock,
                quant,
            }));
    }

    /**
     * Anchors this session's clock to its server's sample clock — the
     * sample-accurate, drift-free timebase, with the server as the master.
     * Returns `this`, so it chains after a factory.
     *
     * Safe when the server is not a reachable master: the clock simply stays
     * on wall-clock time (`Server.sampleTimebase` says so and warns).
     */
    async lockToServer(): Promise<this> {
        await this.clock.lockTo(this.server, { timeout: this.server.timeout });
        return this;
    }

    /**
     * Joins this session's server's shared transport, so a `quant`-ed pattern
     * starts on the same beat as every other client on it (see
     * `TempoClock.joinTransport`). Resolves with `this`, so it chains after
     * the other anchoring verb: `await (await session.lockToServer())
     * .joinTransport()` — lock first, and the alignment is sample-exact. A
     * server with no transport defined leaves the clock's own grid alone.
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
     * Starts the clock so scheduled events fire in real time; returns `this`.
     * A restart **resumes** at the beat `stop` left the clock on.
     */
    start(): this {
        this.clock.start();
        return this;
    }

    /**
     * Stops the clock; returns `this`. Nothing further fires while it is
     * stopped, but the schedule is kept and the beat is held: this is a
     * transport, and a later `start` picks the music up where it was
     * (`session.clock.clear()` is what drops what is queued).
     */
    stop(): this {
        this.clock.stop();
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
        this.clock.close();
        this.server.close();
        void this.ownedEngine?.close();
        this.ownedEngine = null;
        if (main.server === this.server) main.server = null;
        if (main.currentSession === this) main.currentSession = null;
    }
}
