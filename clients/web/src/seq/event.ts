// Events (mirrors `clausters/seq/event.py`).
//
// An `Event` is a bag of parameters with sensible defaults that knows how to
// **play itself** against a destination. The default `note` event creates a
// synth and schedules its release. Timing is the clock's job: an event emits
// at the running routine's exact logical beat (through `Server.sendBundle`),
// and the player advances by the event's `delta`.
//
// By default a note **frees** its synth after `sustain` (`/node_free`) rather
// than closing a gate -- unless `hasGate` is set, in which case it sends
// `gate 0` (for defs whose envelope has a release node and a done action that
// frees the synth once the release finishes). The built-in `"default"`
// instrument is the exception: it carries such an envelope and is always
// released by its gate, so it ramps out without a click even with the global
// `hasGate` default left false.

import {
    event_delta as coreDelta,
    event_sustain as coreSustain,
    level_resolve as coreLevelResolve,
    level_set as coreLevelSet,
    pitch_resolve as corePitchResolve,
    pitch_set as corePitchSet,
    split_degree as coreSplitDegree,
} from "../core/clausters_core_web.js";
import { main } from "../base/main.ts";
import type { OscArg } from "../base/osc.ts";

/**
 * Keys that drive timing and structure, and are never sent as controls.
 * `node` and `server` are written back by `play`.
 */
const RESERVED = new Set([
    "type", "instrument", "dur", "legato", "stretch", "sustain", "delta",
    "addAction", "target", "group", "server", "hasGate",
    "midinote", "degree", "alter", "octave", "root", "scale", "node", "db",
    // What the note says on a page. None of it is a synth control.
    "articulations", "dynamic", "ornament", "grace", "stem",
    "spelling", "accidental", "tie",
]);

/**
 * The reserved keys that say what the note is on a **page** rather than what it
 * does in the air, read by `gui.notation.sheetFromNotes` and written back by
 * `gui.notation.toTimeline`. Every one is a musical fact --
 * `articulations: ["stacc"]`, not an instruction to shorten a drawn value --
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
 * The pitch keys in the order the core carries them; `scale` is the seventh,
 * passed beside them. Also the order a caller's keys are written in when an
 * event is built, lowest first: where a caller states two spellings, the last
 * one -- `freq` -- is the one the others follow.
 */
const PITCH_KEYS = ["freq", "midinote", "degree", "alter", "octave", "root", "scale"];
const PITCH_ORDER = ["scale", "root", "octave", "degree", "alter", "midinote", "freq"];
/** The level keys in the order the core carries them, and their build order. */
const LEVEL_KEYS = ["amp", "velocity", "db"];
const LEVEL_ORDER = ["db", "velocity", "amp"];
/** The pitch keys an edit to another pitch key can rewrite. */
const SPELLINGS = ["freq", "midinote", "degree"];

const held = (value: unknown): boolean => value !== undefined && value !== null;

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
 * (`dur`, `legato`, `stretch`, `addAction`/`target`, the pitch keys, ...) and is
 * never sent to the synth; every other numeric key is forwarded as a control.
 *
 * The derived quantities compute the values actually used: `midinote` and
 * `freq` resolve pitch (an explicit `freq` wins, else `midinote`, else
 * `degree` altered by `alter` within `octave`/`root`/`scale`), `amp` and
 * `velocity` the level (an explicit `amp` wins, else `velocity`, else `db`),
 * `delta` is the beats to the next event and `sustain` the beats the synth
 * sounds. The rules are the shared core's, so every client's event sounds the
 * same.
 *
 * **A family's keys stay coherent.** `freq`, `midinote` and `degree` +
 * `alter` are spellings of one pitch, and `amp`, `velocity` and `db` of one
 * level: writing one of them rewrites the others the event holds, so a note
 * moved by `midinote` does not go on sounding the `freq` it was written with.
 * A key the event does not hold is not added. Built with two spellings of one
 * family, the event takes `freq` over `midinote` over the degree, and `amp`
 * over `velocity` over `db`.
 *
 * **A degree is altered by `alter`**, in semitones (real, so a microtone is
 * one too). SuperCollider's fraction (`degree: 1.1` for degree 1 sharp) and a
 * pair (`degree: [1, 1]`) are both read as the two keys.
 *
 * An event may also carry what the note is **on a page**
 * ({@link NOTATION_KEYS}): `articulations`, `dynamic`, `ornament`, `grace`,
 * `stem`, `spelling`, `accidental` and `tie`. They change nothing about how the
 * event sounds -- an articulation is honoured when a *score* is read, not when
 * an event is played -- and they are reserved, so none of them reaches the synth
 * as a control. What reads them is `gui.notation.sheetFromNotes`.
 */
export class Event {
    readonly props: EventProps;

    constructor(props: EventProps = {}) {
        this.props = { ...DEFAULTS };
        for (const [key, value] of Object.entries(props)) {
            if (!PITCH_KEYS.includes(key) && !LEVEL_KEYS.includes(key)) {
                this.props[key] = value;
            }
        }
        for (const key of [...PITCH_ORDER, ...LEVEL_ORDER]) {
            if (key in props) this.setKey(key, props[key]);
        }
    }

    /** One parameter, or `undefined` when it is not set. */
    get(key: string): unknown {
        return this.props[key];
    }

    /**
     * Sets parameters, each as its family's coherence writes it -- as `play`
     * writes its derived quantities back.
     */
    set(props: EventProps): this {
        for (const [key, value] of Object.entries(props)) this.setKey(key, value);
        return this;
    }

    private num(key: string): number {
        return Number(this.props[key]);
    }

    private setKey(key: string, value: unknown): void {
        if (!held(value)) {
            this.props[key] = value;
        } else if (PITCH_KEYS.includes(key)) {
            this.setPitch(key, value);
        } else if (LEVEL_KEYS.includes(key)) {
            this.setLevel(key, Number(value));
        } else {
            this.props[key] = value;
        }
    }

    private pitchKeys(): Float64Array {
        return Float64Array.from(
            PITCH_KEYS.slice(0, 6),
            (k) => (held(this.props[k]) ? Number(this.props[k]) : NaN),
        );
    }

    private scale(): Float32Array {
        return Float32Array.from((this.props.scale as number[] | undefined) ?? []);
    }

    private setPitch(key: string, value: unknown): void {
        if (key === "degree" && Array.isArray(value)) {
            this.setPitch("degree", value[0]);
            this.setPitch("alter", value[1]);
            return;
        }
        if (key === "scale") this.props.scale = value;
        const others = SPELLINGS.some((k) => k !== key && held(this.props[k]));
        if (!others) {
            // Nothing else spells this pitch, so nothing follows: only a
            // fractional degree has to be split.
            if (key === "degree" && typeof value === "number" && !Number.isInteger(value)) {
                const [degree, alter] = coreSplitDegree(value);
                this.props.degree = degree;
                this.props.alter = alter;
            } else if (key !== "scale") {
                this.props[key] = value;
            }
            return;
        }
        const keys = corePitchSet(
            this.pitchKeys(),
            PITCH_KEYS.indexOf(key),
            key === "scale" ? 0 : Number(value),
            this.scale(),
            this.props.spelling === "flat" ? -1 : 0,
        );
        keys.forEach((v, i) => {
            if (!Number.isNaN(v)) this.props[PITCH_KEYS[i]] = v;
        });
    }

    private levelKeys(): Float64Array {
        return Float64Array.from(
            LEVEL_KEYS,
            (k) => (held(this.props[k]) ? Number(this.props[k]) : NaN),
        );
    }

    private setLevel(key: string, value: number): void {
        const keys = coreLevelSet(this.levelKeys(), LEVEL_KEYS.indexOf(key), value);
        keys.forEach((v, i) => {
            if (!Number.isNaN(v)) this.props[LEVEL_KEYS[i]] = v;
        });
    }

    // ---- derived quantities ----

    /**
     * The MIDI note number this event sounds: an explicit `freq` inverted,
     * else `midinote`, else `degree` altered by `alter` within
     * `octave`/`root`/`scale`, else middle C.
     */
    midinote(): number {
        return corePitchResolve(this.pitchKeys(), this.scale())[0];
    }

    /**
     * The frequency in Hz this event sounds: an explicit `freq` if given,
     * otherwise `midinote` in equal temperament.
     */
    freq(): number {
        return corePitchResolve(this.pitchKeys(), this.scale())[1];
    }

    /**
     * The linear amplitude this event sounds at: an explicit `amp`, else its
     * `velocity`, else its `db`.
     */
    amp(): number {
        return coreLevelResolve(this.levelKeys())[0];
    }

    /**
     * The velocity a note-on of this event carries (1..127): an explicit
     * `velocity`, else its amplitude's.
     */
    velocity(): number {
        return coreLevelResolve(this.levelKeys())[1];
    }

    /**
     * Beats until the next event: an explicit `delta` if given, otherwise
     * `dur * stretch`. As in SuperCollider, the key overrides the calculation.
     */
    delta(): number {
        const delta = this.props.delta;
        return coreDelta(this.num("dur"), this.num("stretch"), held(delta) ? Number(delta) : NaN);
    }

    /**
     * Beats the synth sounds: an explicit `sustain` if given, otherwise
     * `dur * legato * stretch`.
     */
    sustain(): number {
        const sustain = this.props.sustain;
        return coreSustain(
            this.num("dur"),
            this.num("legato"),
            this.num("stretch"),
            held(sustain) ? Number(sustain) : NaN,
        );
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
     * The `name value ...` control tail this event sends to the synth: `freq`
     * and `amp` always, `out` when set, then every other numeric key that is
     * not reserved.
     */
    controlArgs(): OscArg[] {
        const args: OscArg[] = [
            ["s", "freq"], ["f", this.freq()],
            ["s", "amp"], ["f", this.amp()],
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
     * on/off -- without the clock or the routine knowing which.
     *
     * Returns **this event, with its keys completed**: the derived quantities
     * are written in (`midinote`, `freq`, `delta`, `sustain` -- the values
     * actually used) along with `node` (the synth's node id; `null` for a
     * rest) and `server` (the destination), so the note stays actionable
     * after the fact -- `free` cuts it, `release` ends it musically. The
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
 * beats -- a rest in the sequence.
 */
export const rest = (dur = 1.0): Event => new Event({ type: "rest", dur });
