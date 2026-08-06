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
// const s = await Session.page({ tempo: 2.0 });
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
import type { IdShare } from "./base/core.ts";
import { loadOsc } from "./base/osc.ts";
import type { RenderOptions, RenderStats } from "./render.ts";
import { Server } from "./defs/server/index.ts";
import type { ClaustersServer } from "./engine/server.ts";
import { engine as engineInstance, server as pageEngine } from "./engine/server.ts";
import { GuiHost } from "./gui/host.ts";
import { newGuiHost } from "./gui/page.ts";
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
     * Require the server to answer before the session is built, instead of
     * carrying on against the compiled sizing when it does not (see
     * `Server.open`'s `verify`). Worth passing on {@link Session.connect}: a
     * WebSocket that connects proves a listener, not a server.
     */
    verify?: boolean;
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
 * Prefer the factories to the constructor: `page` opens a session on an
 * in-page engine and `connect` one on a `--ws` server, each with sensible
 * defaults. The constructor is for the uncommon case of supplying your own
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
 * *this* session's material without touching another's. Material created
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
    private readonly destinations: { dest: OscDestination; connection: Connection }[] = [];

    /**
     * Drives `server` on `clock` (a fresh one at tempo 1.0 when omitted).
     *
     * The clock gets a back-reference to this session, so a play running on it
     * resolves *this* session's server and random root — which is what keeps
     * several sessions isolated from each other and from the default one.
     */
    constructor(server: Server, clock?: TempoClock) {
        super();
        this.server = server;
        this.clock = clock ?? new TempoClock();
        this.clock.session = this;
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
        await loadOsc();
        const connection = new ScoreConnection();
        const server = await Server.open(connection, { sizing: {}, notify: false });
        return new Session(server, new TempoClock(tempo));
    }

    /**
     * A session on an **in-page engine** — the audio server compiled to wasm
     * in this tab's AudioWorklet, with no process and no socket anywhere.
     *
     * Defaults to the page's shared engine, which is what a page wants: its
     * components play into one mix. Pass `own: true` for an engine of this
     * session's own, so its nodes, buses and buffers share nothing with the
     * rest of the document — the case several sessions in one page exist for.
     * An engine this session opened is closed with it; the page's is not.
     *
     * The `AudioContext` needs a user gesture to start, so call this from a
     * click rather than at load.
     */
    static async page({
        own = false,
        engine,
        channels,
        tempo = 1.0,
        timebase,
        latency,
        timeout,
        share,
    }: SessionOptions & {
        own?: boolean;
        engine?: ClaustersServer;
        channels?: number;
    } = {}): Promise<Session> {
        await loadOsc();
        const options = channels === undefined ? {} : { channels };
        const audio = engine ?? (own ? await engineInstance(options) : await pageEngine(options));
        await audio.resume();
        const session = await Session.over(
            await pageConnection(audio),
            { tempo, timebase, latency, timeout, share },
        );
        // `own` is what makes the engine this session's to close — whether it
        // opened one or was handed one that belongs to it. The page's shared
        // engine is never anyone's to stop, and that is the default.
        if (own) session.ownedEngine = audio;
        return session;
    }

    /**
     * A session on a **native server** over a WebSocket (`clausters --ws`) —
     * the browser's only network carrier, and the one that reaches a server
     * with the whole def catalogue (the in-page engine is the `synth,embed`
     * build, with no Faust JIT).
     */
    static async connect(
        url = "ws://127.0.0.1:57120",
        { tempo = 1.0, timebase, latency, timeout, share, verify }: SessionOptions = {},
    ): Promise<Session> {
        await loadOsc();
        return Session.over(await WsConnection.open(url), {
            tempo, timebase, latency, timeout, share, verify,
        });
    }

    /** Opens a server over `connection` and builds the session around it. */
    private static async over(
        connection: Connection,
        { tempo, timebase, latency, timeout, share, verify }: SessionOptions,
    ): Promise<Session> {
        const server = await Server.open(connection, { timeout, share, verify });
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
     * the session and released on `close`.
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
            this.gui_ = await GuiHost.page(this.ownedGui, { share });
        } else {
            this.gui_ = await GuiHost.page(undefined, { share });
        }
        return this.gui_;
    }

    /**
     * A `GuiHost` driving a **native** `clausters-gui --ws` host instead, for
     * a session whose windows belong on the desktop. Installed as this
     * session's host when it has none yet, so `gui()` returns it afterwards;
     * a session that already has one keeps it, and the connected host is
     * returned all the same.
     */
    async connectGui(url?: string): Promise<GuiHost> {
        const host = await GuiHost.connect(url);
        this.gui_ ??= host;
        return host;
    }

    // ---- driving ----

    /**
     * Makes this session the ambient one for the duration of `body`, so
     * material created in it (a played routine, a bare `new Synth`) resolves
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
        this.clock.timebase = await this.server.sampleTimebase({
            timeout: this.server.timeout,
        });
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
     * Lends this session's **server** to the default session, first-wins —
     * the browser counterpart of a free-standing `Server.boot()` adopting the
     * default in the Python client. After it, `play(...)` and a bare
     * `new Synth(...)` work with no session named.
     *
     * It lends the server and **not the clock**, which is the reference
     * client's split and not an oversight: the default session's clock is
     * created by the first ambient play and started right there, so lending a
     * stopped one would hand `play()` a clock nothing ever starts. A
     * session's own clock is reached through `session.play` or inside
     * `session.use`.
     *
     * Returns `this`. A server already adopted is left alone: whichever
     * session claimed the slot first keeps it.
     */
    adoptDefault(): this {
        main.server ??= this.server;
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
        const connection = await WsConnection.open(url);
        const dest = new OscDestination(connection);
        this.destinations.push({ dest, connection });
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
        for (const { connection } of this.destinations) connection.close();
        this.destinations.length = 0;
        this.clock.close();
        this.server.close();
        void this.ownedEngine?.close();
        this.ownedEngine = null;
        if (main.server === this.server) main.server = null;
        if (main.currentSession === this) main.currentSession = null;
    }
}
