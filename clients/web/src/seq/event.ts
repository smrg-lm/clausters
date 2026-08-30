// Events (mirrors `clausters/seq/event.py`).
//
// An `Event` is a bag of parameters with sensible defaults that knows how to
// **play itself** against a destination. The default `note` event creates a
// synth and schedules its release. Timing is the clock's job: an event emits
// at the running routine's exact logical beat (through `Server.sendBundle`),
// and the player advances by the event's `delta`.
//
// By default a note **frees** its synth after `sustain` (`/node_free`) rather
// than closing a gate — unless `hasGate` is set, in which case it sends
// `gate 0` (for defs whose envelope has a release node and a done action that
// frees the synth once the release finishes). The built-in `"default"`
// instrument is the exception: it carries such an envelope and is always
// released by its gate, so it ramps out without a click even with the global
// `hasGate` default left false.

import { cpsmidi, degreeToMidinote, midicps } from "../base/builtins.ts";
import { main } from "../base/main.ts";
import type { OscArg } from "../base/osc.ts";

/**
 * Keys that drive timing and structure, and are never sent as controls.
 * `node` and `server` are written back by `play`.
 */
const RESERVED = new Set([
    "type", "instrument", "dur", "legato", "stretch", "sustain", "delta",
    "addAction", "target", "group", "server", "hasGate",
    "midinote", "degree", "octave", "root", "scale", "node",
    // What the note says on a page. None of it is a synth control.
    "articulations", "dynamic", "ornament", "grace", "stem",
    "spelling", "accidental", "tie",
]);

/**
 * The reserved keys that say what the note is on a **page** rather than what it
 * does in the air, read by `gui.notation.sheetFromNotes` and written back by
 * `gui.notation.toTimeline`. Every one is a musical fact —
 * `articulations: ["stacc"]`, not an instruction to shorten a drawn value —
 * which is what lets the same key be read in both directions.
 */
export const NOTATION_KEYS = [
    "articulations",
    "dynamic",
    "ornament",
    "grace",
    "stem",
    "spelling",
    "accidental",
    "tie",
] as const;

/**
 * Defaults merged into every `Event`. `type` selects behaviour (`note` or
 * `rest`); `instrument` is the def name; `dur` is the beats to the next
 * event, scaled by `legato`/`stretch` into the sounding time; `amp` is linear
 * amplitude; `addAction`/`target` place the synth in the node tree; `hasGate`
 * picks release-by-free vs `gate 0`; and `octave`/`root`/`scale` define the
 * pitch space `degree` indexes.
 */
export const DEFAULTS: EventProps = {
    type: "note",
    instrument: "default",
    dur: 1.0,
    legato: 0.8,
    stretch: 1.0,
    amp: 0.1,
    addAction: 1, // tail
    target: 0, // root group
    hasGate: false, // Clausters: free on release by default
    octave: 5.0,
    root: 0.0,
    scale: [0, 2, 4, 5, 7, 9, 11], // major
};

/**
 * An event's parameters. Unknown keys are simply stored; the numeric ones
 * that are not reserved are forwarded to the synth as controls.
 */
export interface EventProps {
    [key: string]: unknown;
}

/**
 * What an `Event` can be played on: the OSC `Server`, or any destination that
 * renders one (a MIDI destination, once the client has one).
 */
export interface EventDestination {
    playEvent(event: Event): number | null;
    sendMsg(addr: string, ...args: OscArg[]): void;
}

/**
 * A note event: parameters that know how to play themselves.
 *
 * The keys split in two: a fixed **reserved** set drives timing and structure
 * (`dur`, `legato`, `stretch`, `addAction`/`target`, the pitch keys, …) and is
 * never sent to the synth; every other numeric key is forwarded as a control.
 *
 * The derived quantities compute the values actually used: `midinote` and
 * `freq` resolve pitch (an explicit `freq` wins, else `midinote`, else
 * `degree` within `octave`/`root`/`scale`), `delta` is the beats to the next
 * event and `sustain` the beats the synth sounds.
 *
 * An event may also carry what the note is **on a page**
 * ({@link NOTATION_KEYS}): `articulations`, `dynamic`, `ornament`, `grace`,
 * `stem`, `spelling`, `accidental` and `tie`. They change nothing about how the
 * event sounds — an articulation is honoured when a *score* is read, not when
 * an event is played — and they are reserved, so none of them reaches the synth
 * as a control. What reads them is `gui.notation.sheetFromNotes`.
 */
export class Event {
    readonly props: EventProps;

    constructor(props: EventProps = {}) {
        this.props = { ...DEFAULTS, ...props };
    }

    /** One parameter, or `undefined` when it is not set. */
    get(key: string): unknown {
        return this.props[key];
    }

    /** Sets parameters, as `play` writes its derived quantities back. */
    set(props: EventProps): this {
        Object.assign(this.props, props);
        return this;
    }

    private num(key: string): number {
        return Number(this.props[key]);
    }

    // ---- derived quantities ----

    /**
     * The MIDI note number this event sounds. An explicit `freq` (Hz) is
     * inverted through `cpsmidi`; otherwise it comes from `midinote`, or from
     * `degree` within `octave`/`root`/`scale`.
     */
    midinote(): number {
        if (this.props.freq !== undefined) return cpsmidi(this.num("freq"));
        if (this.props.midinote !== undefined) return this.num("midinote");
        if (this.props.degree === undefined) return 60;
        // Pitch-space resolution is the core's shared rule (floored octave
        // wrapping), so every client's Event resolves degrees identically.
        return degreeToMidinote(
            this.num("degree"),
            this.num("octave"),
            this.num("root"),
            (this.props.scale as number[]) ?? [],
        );
    }

    /**
     * The frequency in Hz this event sounds: an explicit `freq` if given,
     * otherwise `midinote` converted through the core's `midicps`.
     */
    freq(): number {
        if (this.props.freq !== undefined) return this.num("freq");
        return midicps(this.midinote());
    }

    /**
     * Beats until the next event: an explicit `delta` if given, otherwise
     * `dur * stretch`. As in SuperCollider, the key overrides the calculation.
     */
    delta(): number {
        if (this.props.delta !== undefined) return this.num("delta");
        return this.num("dur") * this.num("stretch");
    }

    /**
     * Beats the synth sounds: an explicit `sustain` if given, otherwise
     * `dur * legato * stretch`.
     */
    sustain(): number {
        if (this.props.sustain !== undefined) return this.num("sustain");
        return this.num("dur") * this.num("legato") * this.num("stretch");
    }

    /**
     * Whether this event releases by closing a gate. The built-in `"default"`
     * instrument carries a gated, self-freeing envelope, so it does even
     * though the global default is `false`.
     */
    releasesByGate(): boolean {
        return Boolean(this.props.hasGate) || this.props.instrument === "default";
    }

    /**
     * The `name value …` control tail this event sends to the synth: `freq`
     * and `amp` always, `out` when set, then every other numeric key that is
     * not reserved.
     */
    controlArgs(): OscArg[] {
        const args: OscArg[] = [
            ["s", "freq"], ["f", this.freq()],
            ["s", "amp"], ["f", this.num("amp")],
        ];
        if (this.props.out !== undefined) {
            args.push(["s", "out"], ["f", this.num("out")]);
        }
        for (const [key, value] of Object.entries(this.props)) {
            if (RESERVED.has(key) || key === "freq" || key === "amp" || key === "out") {
                continue;
            }
            if (typeof value === "number") args.push(["s", key], ["f", value]);
        }
        return args;
    }

    // ---- play ----

    /**
     * Plays this event on `destination` (double dispatch): the OSC `Server`
     * turns it into `/synth_new` plus a release, a MIDI destination into note
     * on/off — without the clock or the routine knowing which.
     *
     * Returns **this event, with its keys completed**: the derived quantities
     * are written in (`midinote`, `freq`, `delta`, `sustain` — the values
     * actually used) along with `node` (the synth's node id; `null` for a
     * rest) and `server` (the destination), so the note stays actionable
     * after the fact — `free` cuts it, `release` ends it musically. The
     * scheduled self-release still arrives regardless.
     *
     * Outside a clock the note plays immediately; inside a routine it emits
     * at the routine's logical beat.
     */
    play(destination?: EventDestination): this {
        const target = destination
            ?? (main.resolveServer() as unknown as EventDestination);
        const midinote = this.midinote();
        const freq = this.freq();
        this.set({ midinote, freq, delta: this.delta(), sustain: this.sustain() });
        this.props.node = target.playEvent(this);
        this.props.server = target;
        return this;
    }

    /**
     * Cuts the played note **now** (`/node_free`), without waiting for its
     * sustain. A no-op when the event has not sounded (a rest, or never
     * played). The release already scheduled at play time still arrives and
     * is harmless.
     */
    free(): void {
        const node = this.props.node;
        const server = this.props.server as EventDestination | undefined;
        if (typeof node === "number" && server) {
            server.sendMsg("/node_free", ["i", node]);
        }
    }

    /**
     * Ends the played note **musically**, now: `gate 0` when it releases by
     * gate, a plain `/node_free` otherwise. Same no-op rule as `free`.
     */
    release(): void {
        const node = this.props.node;
        const server = this.props.server as EventDestination | undefined;
        if (typeof node !== "number" || !server) return;
        if (this.releasesByGate()) {
            server.sendMsg("/node_set", ["i", node], ["s", "gate"], ["f", 0]);
        } else {
            server.sendMsg("/node_free", ["i", node]);
        }
    }
}

/**
 * A silent `Event` that sounds nothing but still advances time by `dur`
 * beats — a rest in the sequence.
 */
export const rest = (dur = 1.0): Event => new Event({ type: "rest", dur });
