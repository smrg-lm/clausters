// MIDI destinations and interfaces (the port of `base/_midiinterface.py`).
//
// The same RT/NRT seam as the OSC side, for MIDI. A `MidiServer` is the
// double-dispatch counterpart of the OSC `Server`: a clock + routine plays the
// *same* `Pbind` through it, and which **interface** it holds decides the
// rendering — `MidiNrtInterface` accumulates a `MidiScore` (in beats) whose
// bytes are a `.mid` or a MIDI 2.0 clip, `MidiRtInterface` sends the notes out
// a MIDI port live.
//
// A *MIDI message* is raw status/data bytes. MIDI carries no timetags: timing
// comes from the clock at emit time.
//
// The **input** side — a port decoded into message objects and demuxed to
// `MidiFunc` responders — is `MidiReceiver` at the bottom, the MIDI
// counterpart of the OSC receiving door.
//
// **What the browser shapes differently, and it is one thing.** Web MIDI hands
// a page the ports that already exist and lets it create none, so where the
// Python client's `port` names a *virtual* port to open, here it names an
// existing one to select. Everything above that — the parsing, the score, the
// event mapping, the dispatch — is the same client in two languages. A page
// also has no filesystem, so `MidiScore` hands back the file's bytes and the
// page decides what to do with them, exactly as `wavBytes` does for a take.

import { Moment } from "./moment.ts";
import { midiWriteClip, midiWriteSmf } from "./core.ts";
import type { TempoClock } from "./clock.ts";
import type { Event, EventDestination } from "../seq/event.ts";
import type { OscArg, TimedMessage } from "./osc.ts";

/**
 * One MIDI output port: whatever `send`s bytes at an optional
 * `performance.now()` deadline. A Web MIDI `MIDIOutput` is one; so is a test's
 * three-line stand-in, which is the point of naming the shape rather than the
 * browser's class.
 */
export interface MidiOutputPort {
    send(data: Uint8Array | number[], timestamp?: number): void;
    readonly name?: string | null;
    readonly id?: string;
}

/** One MIDI input port: whatever delivers `{data}` events. */
export interface MidiInputPort {
    onmidimessage: ((event: { data: Uint8Array | null }) => void) | null;
    readonly name?: string | null;
    readonly id?: string;
}

/** What `navigator.requestMIDIAccess()` resolves to, as much as is used here. */
export interface MidiPorts {
    readonly inputs: ReadonlyMap<string, MidiInputPort>;
    readonly outputs: ReadonlyMap<string, MidiOutputPort>;
}

/** A decoded channel-voice message: `{type, channel, …}`, matched by `type`. */
export interface MidiMessage {
    type: string;
    channel: number;
    [field: string]: number | string;
}

// Channel-voice status nibbles -> (message type name, data-field names). A
// parsed message is `{type, channel, …}` in the style of mido / the responder
// layer, so `MidiFunc` matches on `type`.
const CV_TYPES: Record<number, [string, string[]]> = {
    0x80: ["note_off", ["note", "velocity"]],
    0x90: ["note_on", ["note", "velocity"]],
    0xa0: ["polytouch", ["note", "value"]],
    0xb0: ["control_change", ["control", "value"]],
    0xc0: ["program_change", ["program"]],
    0xd0: ["aftertouch", ["value"]],
    0xe0: ["pitchwheel", ["pitch"]],
};

/**
 * Decodes raw channel-voice bytes into a message object (`{type, channel, …}`),
 * or `null` for a non-channel-voice / malformed message.
 *
 * `pitchwheel` combines the two 7-bit data bytes into a single 14-bit `pitch`
 * (0..16383, centre 8192); every other field is a raw 7-bit value.
 */
export function parseMidi(message: ArrayLike<number>): MidiMessage | null {
    if (message.length === 0 || message[0] < 0x80) return null;
    const kind = CV_TYPES[message[0] & 0xf0];
    if (kind === undefined) return null;
    const [name, fields] = kind;
    const d1 = message.length > 1 ? message[1] : 0;
    const d2 = message.length > 2 ? message[2] : 0;
    const msg: MidiMessage = { type: name, channel: message[0] & 0x0f };
    if (name === "pitchwheel") {
        msg.pitch = (d1 & 0x7f) | ((d2 & 0x7f) << 7);
    } else if (fields.length === 1) {
        msg[fields[0]] = d1;
    } else {
        msg[fields[0]] = d1;
        msg[fields[1]] = d2;
    }
    return msg;
}

/** Coerces whatever a caller calls a MIDI message into the three raw bytes. */
function messageBytes(message: ArrayLike<number>): [number, number, number] {
    return [
        message.length > 0 ? message[0] & 0xff : 0,
        message.length > 1 ? message[1] & 0xff : 0,
        message.length > 2 ? message[2] & 0xff : 0,
    ];
}

/**
 * Accumulated MIDI events ordered by **beat**. Beats are clock-agnostic; the
 * PPQ chosen at write time maps them to file ticks.
 */
export class MidiScore {
    /** `[beat, bytes]` in insertion order; `sorted` is what a write reads. */
    readonly events: [number, Uint8Array][] = [];

    add(beat: number, message: ArrayLike<number>): void {
        this.events.push([Number(beat), Uint8Array.from(messageBytes(message))]);
    }

    /** Beat order, stable — a note-off keeps its place before a re-trigger. */
    sorted(): [number, Uint8Array][] {
        return this.events
            .map((e, i) => [e, i] as const)
            .sort((a, b) => a[0][0] - b[0][0] || a[1] - b[1])
            .map(([e]) => e);
    }

    /** The flat `(ticks, msgs)` pair the core's writers take. */
    private ticked(ppq: number): [Uint32Array, Uint8Array] {
        const events = this.sorted();
        const ticks = new Uint32Array(events.length);
        const msgs = new Uint8Array(3 * events.length);
        events.forEach(([beat, bytes], i) => {
            ticks[i] = Math.max(0, Math.round(beat * ppq));
            msgs.set(bytes, 3 * i);
        });
        return [ticks, msgs];
    }

    /**
     * Standard MIDI File (`.mid`) bytes, written by the shared core.
     *
     * A page has no filesystem, so this hands the bytes back rather than
     * taking a path — the same split `render`/`wavBytes` already makes for a
     * take. The writer is `clausters-midi`'s, the one the Python client calls,
     * so the two produce the same file.
     */
    toSmf(ppq: number): Uint8Array {
        const [ticks, msgs] = this.ticked(ppq);
        return midiWriteSmf(ticks, msgs, ppq);
    }

    /**
     * MIDI 2.0 Clip File (SMF2CLIP) bytes — note velocities at 16-bit
     * resolution — on the same terms as `toSmf`.
     */
    toClip(ppq: number): Uint8Array {
        const [ticks, msgs] = this.ticked(ppq);
        return midiWriteClip(ticks, msgs, ppq);
    }
}

/** Where a `MidiServer`'s messages go: a score offline, a port live. */
export interface MidiInterface {
    readonly isRealtime: boolean;
    emit(beat: number, message: ArrayLike<number>): void;
    close(): void;
}

/**
 * Non-real-time MIDI: accumulates `(beat, message)` into a `MidiScore` to
 * write offline.
 */
export class MidiNrtInterface implements MidiInterface {
    readonly isRealtime = false;
    readonly score = new MidiScore();

    emit(beat: number, message: ArrayLike<number>): void {
        this.score.add(beat, message);
    }

    close(): void {}
}

/**
 * How a real-time interface or a receiver picks its port. `P` is the port's
 * direction — an output for `MidiRtInterface`, an input for `MidiReceiver`.
 */
export interface MidiPortOptions<P = MidiOutputPort> {
    /**
     * The port: one already obtained, or a name to look for (a
     * case-insensitive substring of the port's name, or its exact id).
     * Omitted, the first port the browser offers.
     */
    port?: string | P;
    /** The ports to look in; `requestMidiPorts()` is called if absent. */
    access?: MidiPorts;
}

/** Resolves a port out of a `MidiPorts`, by name, by id, or first. */
function pickPort<T extends { name?: string | null; id?: string }>(
    ports: ReadonlyMap<string, T>,
    want: string | undefined,
    what: string,
): T {
    const all = [...ports.values()];
    if (all.length === 0) throw new Error(`no MIDI ${what} port is available`);
    if (want === undefined) return all[0];
    const needle = want.toLowerCase();
    const found = all.find(
        (p) => p.id === want || (p.name ?? "").toLowerCase().includes(needle),
    );
    if (found === undefined) {
        const names = all.map((p) => p.name ?? p.id ?? "?").join(", ");
        throw new Error(`no MIDI ${what} port matching "${want}"; have: ${names}`);
    }
    return found;
}

/**
 * The browser's MIDI ports.
 *
 * A page cannot create a port, so this is where every live MIDI leg starts:
 * `navigator.requestMIDIAccess()`, which asks the user once and then hands over
 * the inputs and outputs that exist. Pass the result to `MidiRtInterface.open`
 * or `MidiReceiver` to reuse one grant.
 */
export async function requestMidiPorts(
    options: { sysex?: boolean } = {},
): Promise<MidiPorts> {
    const request = (
        navigator as unknown as {
            requestMIDIAccess?: (o: { sysex: boolean }) => Promise<MidiPorts>;
        }
    ).requestMIDIAccess;
    if (request === undefined) {
        throw new Error("this browser has no Web MIDI (navigator.requestMIDIAccess)");
    }
    return await request.call(navigator, { sysex: options.sysex ?? false });
}

/**
 * Real-time MIDI output through a browser port.
 *
 * Each message is sent at its beat. Where the Python client sleeps a future
 * note-off onto the clock, the browser takes the deadline directly:
 * `MIDIOutput.send(data, timestamp)` schedules against `performance.now()`, so
 * the driver hands over the deadline it has already computed. Timing is still
 * best-effort by design — the port is the OS's, not ours.
 */
export class MidiRtInterface implements MidiInterface {
    readonly isRealtime = true;
    readonly output: MidiOutputPort;
    /** The port's name, as the browser reports it. */
    readonly port: string;

    constructor(output: MidiOutputPort) {
        this.output = output;
        this.port = output.name ?? output.id ?? "midi-out";
    }

    /** Resolves a port (asking for access if none is passed) and opens on it. */
    static async open(options: MidiPortOptions = {}): Promise<MidiRtInterface> {
        const want = options.port;
        if (want !== undefined && typeof want !== "string") {
            return new MidiRtInterface(want);
        }
        const access = options.access ?? (await requestMidiPorts());
        return new MidiRtInterface(pickPort(access.outputs, want, "output"));
    }

    emit(beat: number, message: ArrayLike<number>): void {
        const now = Moment.current();
        const bytes = Uint8Array.from(messageBytes(message));
        if (now.clock !== null && beat > now.beat + 1e-9) {
            this.output.send(bytes, deadlineMs(now.at(beat - now.beat)));
        } else {
            this.output.send(bytes);
        }
    }

    close(): void {
        // A stop leaves any note-off scheduled past it unsent, which would hang
        // notes on the destination. Send an "all notes off" (CC 123) on every
        // channel -- the standard MIDI panic, so a partial run ends silent.
        for (let ch = 0; ch < 16; ch++) {
            this.output.send(Uint8Array.of(0xb0 | ch, 0x7b, 0));
        }
    }
}

/**
 * A logical moment as a `performance.now()` deadline, which is the clock Web
 * MIDI schedules against. `timeOrigin` is that clock's Unix zero, so the
 * conversion is exact rather than sampled.
 */
function deadlineMs(when: Moment): number {
    return when.instant() * 1000 - performance.timeOrigin;
}

/** How a `MidiServer` renders events. */
export interface MidiServerOptions {
    /** The interface; a fresh `MidiNrtInterface` (a score) by default. */
    interface?: MidiInterface;
    /** The channel notes are played on (0..15). */
    channel?: number;
    /** Ticks per quarter note a written file uses. */
    ppq?: number;
}

/**
 * A MIDI destination for event patterns — the double-dispatch counterpart of
 * the OSC `Server`.
 *
 * A `Pbind` played on a clock with this as the destination renders each `Event`
 * as a note on/off pair, handed to the held interface (an NRT score or a live
 * port). Note number from `event.midinote()`, velocity from `amp` (0..1 →
 * 0..127).
 */
export class MidiServer implements EventDestination {
    readonly interface: MidiInterface;
    readonly channel: number;
    readonly ppq: number;

    constructor(options: MidiServerOptions = {}) {
        this.interface = options.interface ?? new MidiNrtInterface();
        this.channel = (options.channel ?? 0) & 0x0f;
        this.ppq = options.ppq ?? 480;
    }

    /** The accumulated `MidiScore`, or `null` on a real-time interface. */
    get score(): MidiScore | null {
        const held = this.interface;
        return held instanceof MidiNrtInterface ? held.score : null;
    }

    playEvent(event: Event): number | null {
        if (event.get("type") === "rest") return null;
        const beat = Moment.current().beat;
        const note = Math.round(event.midinote()) & 0x7f;
        const amp = Math.min(1, Math.max(0, Number(event.get("amp") ?? 0)));
        const velocity = Math.round(amp * 127) & 0x7f;
        const ch = this.channel;
        this.interface.emit(beat, [0x90 | ch, note, velocity]);
        this.interface.emit(beat + event.sustain(), [0x80 | ch, note, 0]);
        return null;
    }

    /**
     * Emits a raw MIDI message at the running routine's logical beat — the MIDI
     * counterpart of `Server.sendBundle` for a raw OSC message, and what
     * `MidiEvent` renders through.
     */
    sendMessage(message: ArrayLike<number>): null {
        this.interface.emit(Moment.current().beat, message);
        return null;
    }

    /**
     * The two OSC verbs a destination is asked for. A MIDI port carries no
     * OSC, so both are errors rather than silent no-ops: an `OscEvent` on a
     * `MidiServer` is a mistake, and one that says so is better than one that
     * plays nothing.
     */
    sendMsg(addr: string, ..._args: OscArg[]): void {
        throw new TypeError(`a MidiServer carries no OSC (${addr})`);
    }

    /** As `sendMsg`, for a bundle. */
    sendBundle(messages: readonly TimedMessage[]): void {
        throw new TypeError(
            `a MidiServer carries no OSC (${messages.length} messages)`,
        );
    }

    close(): void {
        this.interface.close();
    }
}

/** How a `MidiReceiver` opens and dispatches. */
export interface MidiReceiverOptions extends MidiPortOptions<MidiInputPort> {
    /** Dispatch through this clock instead of inline on the message event. */
    clock?: TempoClock | null;
}

/** What a receiver hands a handler: the decoded message and the port's name. */
export type MidiHandler = (message: MidiMessage, src: string) => void;

/**
 * A MIDI **input** port that demuxes to registered handlers — the transport
 * under `MidiFunc`.
 *
 * It takes one of the browser's input ports (the page cannot make one), decodes
 * each message with `parseMidi`, and calls every registered handler with
 * `(message, src)` — `src` being the port's name. Dispatch is inline on the
 * message event by default, or through `clock.sched` when a clock is given; the
 * golden rule holds either way, a handler must not block its thread.
 */
export class MidiReceiver {
    /** The port's name once started, the requested one before that. */
    port: string;
    clock: TempoClock | null;
    private readonly want: MidiReceiverOptions;
    private input: MidiInputPort | null = null;
    private handlers: MidiHandler[] = [];

    constructor(options: MidiReceiverOptions = {}) {
        this.want = options;
        this.clock = options.clock ?? null;
        this.port = typeof options.port === "string" ? options.port : "";
    }

    /** Resolves the port (asking for access if none was passed) and listens. */
    async start(): Promise<this> {
        if (this.input !== null) return this;
        const want = this.want.port;
        if (want !== undefined && typeof want !== "string") {
            this.input = want;
        } else {
            const access = this.want.access ?? (await requestMidiPorts());
            this.input = pickPort(access.inputs, want, "input");
        }
        this.port = this.input.name ?? this.input.id ?? "midi-in";
        this.input.onmidimessage = (event) => {
            if (event.data === null) return;
            const msg = parseMidi(event.data);
            if (msg !== null) this.dispatch(msg);
        };
        return this;
    }

    /** Stops listening. The port itself belongs to the browser, not to us. */
    stop(): this {
        if (this.input !== null) {
            this.input.onmidimessage = null;
            this.input = null;
        }
        return this;
    }

    /** `stop`, under the name every other closable thing here answers to. */
    close(): this {
        return this.stop();
    }

    /**
     * Registers `handler(message, src)`, called for every decoded channel-voice
     * message. Returns the handler so it can later be `remove`d.
     */
    add(handler: MidiHandler): MidiHandler {
        this.handlers.push(handler);
        return handler;
    }

    remove(handler: MidiHandler): void {
        const at = this.handlers.indexOf(handler);
        if (at >= 0) this.handlers.splice(at, 1);
    }

    private dispatch(msg: MidiMessage): void {
        for (const handler of [...this.handlers]) {
            if (this.clock !== null) {
                this.clock.sched(0, () => {
                    handler(msg, this.port);
                });
            } else {
                handler(msg, this.port);
            }
        }
    }
}
