// The client-owned server configuration, and what a running server reports
// (mirrors `clausters/defs/server/options.py`).
//
// `ServerSizing` is what this client's allocators are built against, and
// `ServerInfo` is the answer to `Server.queryInfo`: what the server it is
// talking to was actually built and booted with, which is not always the same
// thing; `ServerStatus` is the answer to `Server.status`: what it is doing
// right now, which changes between two calls. The `ServerOptions` half of the
// Python module -- every flag a
// *launched* server takes, and the command line it becomes -- has no
// counterpart here: a page cannot start a process, so a web client always
// meets a server that is already running.

import { formatFixed, formatGeneral } from "../../base/format.ts";

// Server defaults, mirroring the Rust server's `DEFAULT_AUDIO_BUSES` /
// `DEFAULT_CONTROL_BUSES` (each a power of two, with no ceiling) and
// `--sample-rate`.
// They live here, on the server-configuration module, not in the bus one: how
// many buses exist is the server's property, and these are only the fallback
// when `/server_query` does not answer. The bus allocators carry no defaults
// of their own.
export const DEFAULT_AUDIO_BUSES = 1024;
export const DEFAULT_CONTROL_BUSES = 16384;
export const DEFAULT_SAMPLE_RATE = 48000;
// Boot-time pre-allocated pool sizes, mirroring the Rust server's `Limits`
// defaults (`--max-nodes`/`--max-buffers`/`--max-graph-children`/
// `--max-ugen-inputs`). 256 is the hard ceiling on UGen inputs -- a
// compile-time invariant, unlike the bus counts, which are configured
// resources with none.
export const DEFAULT_MAX_NODES = 8192;
export const DEFAULT_MAX_BUFFERS = 4096;
export const DEFAULT_MAX_GRAPH_CHILDREN = 512;
export const DEFAULT_MAX_UGEN_INPUTS = 256;

/**
 * The server's default ring count (`--taps`), when it reports none.
 *
 * An audio bus is engine memory, one block at a time, so nothing outside the
 * audio thread can see it. The server copies a bus into one of a fixed set of
 * **sample rings** when asked (`/bus_tap bus 1`), which is what lets a scope
 * see the samples of a live signal.
 *
 * **The rings are the server's own bookkeeping.** A client asks for a *bus*
 * and never for a ring: the server picks one, counts watches so two views of a
 * bus share a ring, and frees it when the last one stops. All this client
 * needs from that region is how big it is, to size a sensible default before
 * `/server_query` answers.
 */
export const DEFAULT_TAPS = 8;

/**
 * Frames per audio-tap ring on a server started with the default `--tap-frames`
 * -- the other half of `DEFAULT_TAPS`, and what sizes a window over a tap before
 * `/server_query` answers with the real shape.
 */
export const DEFAULT_TAP_FRAMES = 16384;

/** The transports a server boots with when no `--transports` says otherwise. */
export const DEFAULT_TRANSPORTS = 16;

/** The event buses a server boots with when no `--event-buses` says otherwise. */
export const DEFAULT_EVENT_BUSES = 64;

/**
 * The sizes a client's allocators need. They are a property of the *server*,
 * so `Server.attach`/`Server.boot` read them from `/server_query` rather than
 * guessing (`reconcile`);
 * pass them explicitly to skip that round trip.
 */
export interface ServerSizing {
    audioBuses: number;
    controlBuses: number;
    maxNodes: number;
    maxBuffers: number;
    /**
     * Hardware output channels -- the audio buses reserved at the bottom of
     * the space, which the allocator never hands out.
     */
    channels: number;
    /** Audio-tap rings (`--taps`); 0 on a server with no tap region. */
    taps: number;
    /**
     * Independent transports (`--transports`). Transport 0 is the one
     * addressed by number, and a multitrack's; an audio editor and every
     * sequence playing on its own each take one of the rest, which a GUI host
     * shares when there is one.
     */
    transports: number;
    /**
     * Event buses (`--event-buses`), where MIDI, event lanes and
     * `Server.busEvent` write notes and a `Voices` or plugin node reads them;
     * the top half is the MIDI bindings'.
     */
    eventBuses: number;
}

/** The static configuration a running server reports over `/server_query`. */
export interface ServerInfo extends ServerSizing {
    blockSize: number;
    nominalSampleRate: number;
    actualSampleRate: number;
    inputChannels: number;
    maxGraphChildren: number;
    maxUgenInputs: number;
    /** Audio-tap region shape; 0/0 when the server has no segment. */
    taps: number;
    tapFrames: number;
    /** The stream-transport frame ceiling in bytes. */
    maxFrame: number;
    /**
     * How many control buses one `/bus_stream` subscription may list
     * (`--max-stream-buses`), **as it applies to this client's carrier**: the
     * server's configured ceiling clamped by what one reply carries over the
     * transport asking. A subscription is one client's whole live picture -- a
     * page of many canvases asks for a bus per meter -- so a client that draws
     * a lot reads this rather than assuming. Falls back to the historical 128
     * against a server too old to report it.
     */
    maxStreamBuses: number;
    /**
     * How many transports the server has (`--transports`), addressed `0` to
     * `transports - 1` (`Server.transportAt`). Falls back to the one transport
     * a server too old to report it had.
     */
    transports: number;
    /**
     * How many event buses the server has (`--event-buses`). Falls back to
     * the default against a server too old to report it.
     */
    eventBuses: number;
}

/**
 * The server's configuration as the readable block `print` shows in the Python
 * client (`ServerInfo.__str__`) -- same fields, same order, same wording.
 *
 * A free function rather than a method for the reason the record formatters in
 * `defs/info.ts` are: `ServerInfo` is an interface, so it carries none.
 */
export function formatServerInfo(info: ServerInfo): string {
    const g = formatGeneral;
    const drift = info.actualSampleRate === info.nominalSampleRate
        ? ""
        : ` (nominal ${g(info.nominalSampleRate)})`;
    const taps = info.taps
        ? `${info.taps} x ${info.tapFrames} frames`
        : "none (no segment)";
    return [
        `server ${g(info.actualSampleRate)} Hz${drift}, ` +
            `${info.blockSize}-sample blocks, ` +
            `${info.channels} out / ${info.inputChannels} in`,
        `  buses   ${info.audioBuses} audio, ${info.controlBuses} control`,
        `  limits  ${info.maxNodes} nodes, ${info.maxBuffers} buffers, ` +
            `${info.maxGraphChildren} graph children, ` +
            `${info.maxUgenInputs} ugen inputs`,
        `  taps    ${taps}`,
        `  frame   ${info.maxFrame} bytes max`,
        `  stream  ${info.maxStreamBuses} buses per /bus_stream`,
        `  transports ${info.transports}`,
        `  event buses ${info.eventBuses}`,
    ].join("\n");
}

/**
 * The live counters a running server reports over `/server_status` -- the
 * answer to {@link Server.status}.
 *
 * The reply carries exactly these fields, in this order: the four counts, the
 * two CPU meters, the two sample rates and `lateBlocks`.
 *
 * `avgCpu` and `peakCpu` are the audio thread's per-block processing time as a
 * **percentage of the block budget**, not of a core: the average is an
 * exponential moving average with a ~1 s time constant, the peak is the worst
 * single block **since the previous call**, so every call reports the peak of
 * its own interval and reading it resets the window. Expect the peak to sit
 * well above the average -- the callback must fit its worst block, not its
 * mean. In an offline render both measure render speed, since there is no
 * callback.
 *
 * **A server with no clock reports none of the three**: `avgCpu`, `peakCpu`
 * and `lateBlocks` are `null` -- not zero, which would read as an idle engine.
 * An engine in a page is that server.
 */
/**
 * What a figure that comes from timing reads as when the server has no clock
 * to time with: an engine in a page is the case.
 */
const NOT_TIMED = "not available (this server cannot time itself)";

/**
 * One role's share of the server's time, as `/server_load` reports it (the
 * result of {@link Server.load}).
 *
 * The server times **itself** rather than asking the operating system, so the
 * reading is the same on every platform: each piece of work brackets itself
 * with a monotonic clock and adds the elapsed time to its role's slot. What is
 * measured is therefore the *work* -- a block, a stage of a parallel group, a
 * serving turn, an NRT job, a compilation -- and not the thread that ran it.
 *
 * `busy` is time the work was **in progress**, not per cent of a core: a DSP
 * worker spinning for its next stage is burning a core and is idle by this
 * reading.
 *
 * **A server with no clock reports no time.** `busy` is then `null` and
 * `share` stays unset: not zero seconds, which would read as an idle role,
 * but no reading. `calls` is still there, since a run is counted and not
 * timed. An engine in a page is that server -- it runs in an AudioWorklet,
 * whose scope has no clock fine enough to time a block.
 */
export interface Load {
    /**
     * `audio` (the callback's block), `dsp` (a worker thread), `net` (the
     * serving turn), `nrt` (the job queue), `faust` (compiling), or `disk-in`
     * and `disk-out` (streaming: every `DiskIn` node's reading together, and
     * every `DiskOut` node's writing).
     */
    role: string;
    /** Which instance, for a role that has several. Only `dsp` does today. */
    index: number;
    /**
     * Seconds of work since the server booted, or `null` from a server that
     * cannot time itself.
     */
    busy: number | null;
    /** Times the work ran: blocks, stages, turns, jobs, compilations. */
    calls: number;
    /**
     * Fraction of wall time busy **since this client's previous call**, or
     * `undefined` on the first one, which has no interval to measure -- and on
     * every one from a server that cannot time itself.
     */
    share?: number;
}

/**
 * `role` for a single-instance role, `role index` otherwise.
 *
 * @param load - the row to name.
 */
export function loadName(load: Load): string {
    return load.role === "dsp" ? `${load.role} ${load.index}` : load.role;
}

/**
 * The readable block {@link Server.load}'s rows print as, one line per role --
 * the same lines, in the same order, as the Python client's `format_load`.
 *
 * A free function for the same reason {@link formatServerStatus} is one.
 *
 * @param rows - the reading to render.
 */
export function formatLoad(rows: Load[]): string {
    if (rows.length > 0 && rows.every((row) => row.busy === null)) {
        // No clock on the server: say so once, and keep what was counted.
        const lines = [`server load: ${NOT_TIMED}`];
        for (const row of rows) {
            lines.push(`  ${loadName(row).padEnd(8)} ${row.calls} calls`);
        }
        return lines.join("\n");
    }
    const lines = ["server load"];
    for (const row of rows) {
        const share = row.share === undefined
            ? "     -"
            : `${formatFixed(row.share * 100, 1).padStart(5)}%`;
        const busy = row.busy === null
            ? "        -  "
            : `${formatFixed(row.busy, 3).padStart(9)} s`;
        lines.push(`  ${loadName(row).padEnd(8)} ${share}  ${busy}  ${row.calls} calls`);
    }
    return lines.join("\n");
}

/**
 * One row of a {@link Profile}: a phase, a synth, a def, a command, a reply or
 * an NRT job, and what it cost over the profile's window.
 *
 * One shape for the six sections, so a reader goes through them alike: `name`
 * says what the row is and `count` how many times it ran or went by; the
 * fields a section has no use for keep their defaults.
 */
export interface ProfileRow {
    /**
     * The phase (`nodes`), the def of a synth or the def itself, the OSC
     * address of a command or a reply, or the command of an NRT job.
     */
    name: string;
    /**
     * Seconds of work over the window, or `null` where there is none to
     * measure -- a reply, or any row from a server that cannot time itself.
     */
    busy: number | null;
    /** Blocks, slices, turns, runs, messages or jobs over the window. */
    count: number;
    /** `busy` as a fraction of the window's wall time. */
    share?: number;
    /** The role a phase belongs to (`audio` or `net`). */
    role: string;
    /** The node id of a synth row. */
    node: number | null;
    /** How many synths of a def ran in the window. */
    nodes: number;
    /** Bytes a command or a reply carried over the window. */
    bytes: number;
    /** Seconds an NRT job waited in the queue before running. */
    waited: number | null;
}

/**
 * What the server spent its time on over a window, in detail (the result of
 * {@link Server.profile}).
 *
 * {@link Server.load} says which role spent the time; this says what inside
 * it: which phase of the block or of the serving turn, which synth and which
 * def, which command, how much each reply carried and how long each NRT job
 * ran and waited. Each section is sorted costliest first and lists only what
 * moved during the window.
 *
 * **What the window cost** is the difference between two of the server's
 * cumulative reports, so several clients can profile one server at once. The
 * synth timing is the one switch: the server runs it only while some client
 * asks, because it costs the audio thread two clock reads per synth per
 * block. `timedSynths` says whether it was on for the whole window.
 */
export interface Profile {
    /**
     * The window, in seconds of the server's clock, or `null` from a server
     * that cannot time itself.
     */
    seconds: number | null;
    /** Whether every synth was timed throughout the window. */
    timedSynths: boolean;
    /** Where the block (`audio`) and the serving turn (`net`) went. */
    phases: ProfileRow[];
    /** The synths still in the tree at the end, with their node ids. */
    synths: ProfileRow[];
    /** Every def whose synths ran, those freed during the window included. */
    defs: ProfileRow[];
    /** Every OSC address the server received. */
    commands: ProfileRow[];
    /** Every OSC address the server sent, pushes and streams included. */
    replies: ProfileRow[];
    /** Every command that ran on the NRT queue. */
    jobs: ProfileRow[];
}

/** The fields of each `/server_profile.reply` section, in order. */
const PROFILE_WIDTHS = [4, 4, 4, 4, 3, 4];

type Arg = unknown;

/** `/server_profile.reply`'s arguments cut into its six sections. */
function profileSections(args: readonly Arg[]): {
    uptime: number | null;
    timed: boolean;
    sections: Arg[][][];
} {
    const uptime = args[0] === null ? null : Number(args[0]);
    let at = 2;
    const sections: Arg[][][] = [];
    for (const width of PROFILE_WIDTHS) {
        const n = Number(args[at]);
        at += 1;
        const rows: Arg[][] = [];
        for (let i = 0; i < n; i++) {
            rows.push(args.slice(at + width * i, at + width * (i + 1)));
        }
        at += width * n;
        sections.push(rows);
    }
    return { uptime, timed: Number(args[1]) !== 0, sections };
}

/**
 * The {@link Profile} of the window between two `/server_profile.reply`
 * argument lists.
 *
 * @internal
 * @param first - the report at the window's start.
 * @param second - the report at its end.
 */
export function profileDifference(first: readonly Arg[], second: readonly Arg[]): Profile {
    const before = profileSections(first);
    const after = profileSections(second);
    const seconds = before.uptime === null || after.uptime === null
        ? null
        : after.uptime - before.uptime;
    const secondsOf = (value: Arg): number | null => (value === null || value === undefined ? null : Number(value));
    const less = (now: number | null, was: number | null): number | null =>
        now === null ? null : now - (was ?? 0);
    const counted = (now: Arg, was: Arg[] | undefined, at: number): number =>
        Number(now) - (was ? Number(was[at]) : 0);
    const row = (fields: Partial<ProfileRow> & Pick<ProfileRow, "name" | "busy" | "count">): ProfileRow => ({
        role: "",
        node: null,
        nodes: 0,
        bytes: 0,
        waited: null,
        ...fields,
    });
    const section = (
        index: number,
        key: (r: Arg[]) => string,
        build: (r: Arg[], was: Arg[] | undefined) => ProfileRow,
    ): ProfileRow[] => {
        const was = new Map(before.sections[index].map((r) => [key(r), r]));
        const rows: ProfileRow[] = [];
        for (const r of after.sections[index]) {
            const made = build(r, was.get(key(r)));
            if (made.count > 0 || (made.busy ?? 0) > 0 || made.bytes > 0) {
                if (made.busy !== null && seconds) made.share = made.busy / seconds;
                rows.push(made);
            }
        }
        rows.sort((a, b) =>
            (b.busy ?? 0) - (a.busy ?? 0) ||
            b.bytes - a.bytes ||
            b.count - a.count ||
            (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
        return rows;
    };
    const busyAt = (r: Arg[], was: Arg[] | undefined, at: number): number | null =>
        less(secondsOf(r[at]), was ? secondsOf(was[at]) : null);
    const synths = section(1, (r) => `${r[0]} ${r[1]}`, (r, was) => row({
        name: String(r[1]),
        node: Number(r[0]),
        busy: busyAt(r, was, 2),
        count: counted(r[3], was, 3),
    }));
    const defs = section(2, (r) => String(r[0]), (r, was) => row({
        name: String(r[0]),
        busy: busyAt(r, was, 1),
        count: counted(r[2], was, 2),
        nodes: counted(r[3], was, 3),
    }));
    // A def's synths in the window are the ones that ran in it, not the
    // difference of two counts: one alive throughout counts in both. So it is
    // the synths that began in the window (the count's difference) plus those
    // alive at both ends that moved, plus those alive at the start and gone by
    // the end. The server lists at most its costliest synths, so in a tree
    // larger than that this is a floor.
    const key = (node: Arg, def: Arg): string => `${node} ${def}`;
    const started = new Set(before.sections[1].map((b) => key(b[0], b[1])));
    const ended = new Set(after.sections[1].map((r) => key(r[0], r[1])));
    for (const def of defs) {
        const moved = synths.filter((s) => s.name === def.name && started.has(key(s.node, s.name))).length;
        const gone = before.sections[1]
            .filter((b) => String(b[1]) === def.name && !ended.has(key(b[0], b[1]))).length;
        def.nodes += moved + gone;
    }
    return {
        seconds,
        timedSynths: after.timed,
        phases: section(0, (r) => `${r[0]} ${r[1]}`, (r, was) => row({
            name: String(r[1]),
            role: String(r[0]),
            busy: busyAt(r, was, 2),
            count: counted(r[3], was, 3),
        })),
        synths,
        defs,
        commands: section(3, (r) => String(r[0]), (r, was) => row({
            name: String(r[0]),
            busy: busyAt(r, was, 1),
            count: counted(r[2], was, 2),
            bytes: counted(r[3], was, 3),
        })),
        replies: section(4, (r) => String(r[0]), (r, was) => row({
            name: String(r[0]),
            busy: null,
            count: counted(r[1], was, 1),
            bytes: counted(r[2], was, 2),
        })),
        jobs: section(5, (r) => String(r[0]), (r, was) => row({
            name: String(r[0]),
            busy: busyAt(r, was, 1),
            count: counted(r[2], was, 2),
            waited: busyAt(r, was, 3),
        })),
    };
}

/**
 * The readable report {@link Server.profile}'s result prints as: every phase,
 * then the `limit` costliest rows of each other section -- the same lines as
 * the Python client's `format_profile`.
 *
 * A free function, as {@link formatLoad} is.
 *
 * @param profile - the profile to render.
 * @param limit - how many rows of each section past the phases.
 */
export function formatProfile(profile: Profile, limit = 10): string {
    const seconds = (value: number | null): string =>
        value === null ? "        -  " : `${formatFixed(value * 1000, 2).padStart(8)} ms`;
    const share = (value: number | undefined): string =>
        value === undefined ? "     -" : `${formatFixed(value * 100, 1).padStart(5)}%`;
    const window = profile.seconds === null ? NOT_TIMED : `${formatFixed(profile.seconds, 2)} s`;
    const lines = [`server profile over ${window}`];
    if (!profile.timedSynths) {
        lines.push("  (synths were not timed for the whole window)");
    }
    lines.push("  phases");
    for (const row of profile.phases) {
        lines.push(`    ${`${row.role} ${row.name}`.padEnd(22)} ${share(row.share)}  ` +
            `${seconds(row.busy)}  ${row.count} runs`);
    }
    const sections: [string, ProfileRow[], (r: ProfileRow) => string][] = [
        ["synths", profile.synths, (r) => `node ${r.node}`],
        ["defs", profile.defs, (r) => `${r.nodes} synths`],
        ["commands", profile.commands, (r) => `${r.bytes} bytes`],
        ["replies", profile.replies, (r) => `${r.bytes} bytes`],
        ["jobs", profile.jobs, (r) => `waited ${seconds(r.waited).trim()}`],
    ];
    for (const [title, rows, extra] of sections) {
        if (rows.length === 0) continue;
        lines.push(`  ${title}`);
        for (const row of rows.slice(0, limit)) {
            lines.push(`    ${row.name.padEnd(22)} ${share(row.share)}  ${seconds(row.busy)}  ` +
                `${row.count} x  ${extra(row)}`);
        }
        if (rows.length > limit) {
            lines.push(`    ... ${rows.length - limit} more`);
        }
    }
    return lines.join("\n");
}

export interface ServerStatus {
    /** Live UGen instances across every playing node. */
    ugens: number;
    /** Playing synth nodes. */
    synths: number;
    /** Groups in the node tree, the root group included. */
    groups: number;
    /** Defs loaded, both families together. */
    defs: number;
    /**
     * Percentage of the block budget, averaged (~1 s time constant); `null`
     * from a server that cannot time itself.
     */
    avgCpu: number | null;
    /**
     * Percentage of the block budget, worst block since the previous call;
     * `null` from a server that cannot time itself.
     */
    peakCpu: number | null;
    /** The rate the server was asked for. */
    nominalSampleRate: number;
    /** The rate the device actually runs at; it drifts from the nominal one. */
    actualSampleRate: number;
    /**
     * Blocks that missed their budget since boot -- cumulative. An occasional
     * increment is a warning, a steady climb is audible trouble. `0` against a
     * server too old to report it, `null` from one that cannot time itself.
     */
    lateBlocks: number | null;
}

/**
 * The server's live counters as the readable block `print` shows in the Python
 * client (`ServerStatus.__str__`) -- same fields, same order, same wording.
 *
 * A free function for the same reason {@link formatServerInfo} is one.
 */
export function formatServerStatus(status: ServerStatus): string {
    const g = formatGeneral;
    const drift = status.actualSampleRate === status.nominalSampleRate
        ? ""
        : ` (nominal ${g(status.nominalSampleRate)})`;
    let cpu = NOT_TIMED;
    if (status.avgCpu !== null && status.peakCpu !== null) {
        const late = status.lateBlocks ? `, ${status.lateBlocks} late` : "";
        cpu = `${formatFixed(status.avgCpu, 1)}% avg, `
            + `${formatFixed(status.peakCpu, 1)}% peak${late}`;
    }
    return [
        `server ${g(status.actualSampleRate)} Hz${drift}`,
        `  playing ${status.synths} synths in ${status.groups} groups, ` +
            `${status.ugens} ugens`,
        `  loaded  ${status.defs} defs`,
        `  cpu     ${cpu}`,
    ].join("\n");
}
