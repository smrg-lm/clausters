// Responders: `OscFunc` over the reply stream and `MidiFunc` over an incoming
// MIDI port — the port of the reference client's `responders.py`.
//
// The **input** side of the client. Everything else here builds OSC and sends
// it; a responder registers a self-filtering callback that fires when a
// matching message *arrives*, and that callback may send onward — so the
// client is a hub, not only a mouth. Same object as sclang's `OSCFunc`, same
// surface as the reference client's: enabled the moment it is created, matched
// by address and optionally by sender and by argument template, freed with
// `free` (or suspended with `disable`, and `enable`d again).
//
// ```js
// import { OscFunc, Synth } from "clausters";
//
// // Every node the server starts, as it starts.
// const started = new OscFunc((msg) => console.log("node", msg[1]), "/node_start");
//
// // The address a def's SendReply reports on, narrowed to one trigger id.
// new OscFunc(([, , id, value]) => draw(value), "/tr", { argTemplate: [null, 7] });
//
// started.free();
// ```
//
// **What is the same, and what a page changes.** The callback signature is the
// reference's — `func(msg, time, src)` with `msg` the message as
// `[addr, ...args]`, `time` the containing bundle's Unix seconds (`null` for
// an immediate or bare message) and `src` the sender — and so are `path`,
// `src`, `argTemplate`, `oneShot` and the `oscfunc` builder. What differs is
// underneath, in the receiver (see `base/receiver.ts`): a page has no socket
// of its own to bind, so the door is the connection the client already has,
// and `src` names a carrier rather than a `(host, port)`.
//
// The default receiver follows from that. There, it is a lazily-bound
// ephemeral UDP port; here it is the **ambient session's server** — the same
// thing the ambient verbs resolve, created on first use, so `new OscFunc(fn,
// "/done")` in a page that has played something needs no arguments. Name a
// receiver explicitly (`{ recv }`) for anything else, exactly as there.
//
// The golden rule survives the change of language: a callback runs on the
// page's one thread as the packet arrives. Keep it quick; to *sequence* in
// response, schedule a routine on a clock rather than looping inside the
// callback — or give the receiver a clock, which dispatches its handlers
// through it.

import { OscReceiver } from "./base/receiver.ts";
import type { OscHandler } from "./base/receiver.ts";
import type { OscMessage } from "./base/osc.ts";
import { MidiReceiver } from "./base/midi.ts";
import type { MidiMessage } from "./base/midi.ts";
import { main } from "./base/main.ts";

/** One decoded argument, as a callback sees it. */
export type OscValue = OscMessage["args"][number];

/**
 * A message as a responder's callback receives it: the address first, then the
 * arguments — the reference client's list, which is what makes
 * `msg[1]` the first argument in both clients.
 */
export type ResponderMessage = [string, ...OscValue[]];

/**
 * The callback: the message, the containing bundle's Unix time (`null` for an
 * immediate or bare message), and the sender.
 */
export type OscCallback = (msg: ResponderMessage, time: number | null, src: string) => void;

/**
 * One `argTemplate` slot: a predicate, `null`/`undefined` (matches anything),
 * or a literal compared for equality.
 */
export type ArgMatcher = ((value: OscValue) => boolean) | OscValue | null | undefined;

/** One `argTemplate` slot against one incoming value. */
function matches(template: ArgMatcher, value: OscValue): boolean {
    if (typeof template === "function") return Boolean(template(value));
    return template === null || template === undefined || template === value;
}

// ---- the module-default receiver (opt-in convenience) ----

let defaultReceiver: OscReceiver | null = null;

/**
 * The default receiver: the **ambient session's server**'s own, which is where
 * a page's messages arrive.
 *
 * It resolves the ambient server the way the ambient verbs do, and fails the
 * same way when there is none — so importing this module opens nothing, as
 * there. Two deliberate differences from the reference client's module
 * default, both following from a page having no socket of its own: it is the
 * *server's* receiver rather than a second listener on the same carrier, and
 * it is resolved per call rather than cached, so a page holding two sessions
 * (each with its own server) gets each one's messages from each one's
 * responders. `setDefaultOscReceiver` pins one anyway — to listen on a
 * particular server whatever is ambient, or to attach a clock.
 */
export function defaultOscReceiver(): OscReceiver {
    return defaultReceiver ?? main.resolveServer().receiver;
}

/**
 * Installs `receiver` as the module default returned by
 * `defaultOscReceiver`.
 */
export function setDefaultOscReceiver(receiver: OscReceiver): OscReceiver {
    defaultReceiver = receiver;
    return receiver;
}

// ---- OSC ----

/**
 * Responder for incoming OSC messages.
 *
 * Registers `func` to fire when a message matching `path` arrives. The
 * callback is called `func(msg, time, src)` — `msg` the message as
 * `[addr, arg1, …]`, `time` the containing bundle's Unix time (`null` for an
 * immediate or bare message), `src` the carrier it arrived on.
 *
 * - `src` — respond only to that sender (a socket's URL, or `"page"` for the
 *   in-page engine).
 * - `argTemplate` — matched against the arguments by position; an entry is a
 *   literal (compared equal), a predicate, or `null` (matches anything).
 *   Shorter than the message is fine: only the listed positions are checked.
 * - `recv` — the `OscReceiver` to register with; the module default otherwise.
 *
 * Enabled on creation. Call `free` (or `disable`) when done.
 */
export class OscFunc {
    /** The callback this responder fires. */
    func: OscCallback;
    /** The OSC address it matches (a leading `/` is added if missing). */
    readonly path: string;
    /** The sender it is narrowed to, if any. */
    readonly src: string | null;
    /** The argument template it matches by position, if any. */
    readonly argTemplate: ArgMatcher[] | null;
    /** The receiver it is registered with. */
    readonly recv: OscReceiver;
    /** Whether it is currently responding. */
    enabled = false;

    private readonly handler: OscHandler;

    constructor(
        func: OscCallback,
        path: string,
        {
            src = null,
            argTemplate = null,
            recv,
        }: {
            src?: string | null;
            argTemplate?: ArgMatcher[] | null;
            recv?: OscReceiver;
        } = {},
    ) {
        this.func = func;
        this.path = path.startsWith("/") ? path : `/${path}`;
        this.src = src;
        this.argTemplate = argTemplate;
        this.recv = recv ?? defaultOscReceiver();
        this.handler = (addr, args, time, from) => {
            if (addr !== this.path) return;
            if (this.src !== null && from !== this.src) return;
            if (this.argTemplate !== null) {
                const checked = Math.min(this.argTemplate.length, args.length);
                for (let i = 0; i < checked; i++) {
                    if (!matches(this.argTemplate[i], args[i]!)) return;
                }
            }
            this.func([addr, ...args], time, from);
        };
        this.enable();
    }

    /** Starts responding (registers the handler with the receiver). */
    enable(): this {
        if (!this.enabled) {
            this.recv.add(this.handler);
            this.enabled = true;
        }
        return this;
    }

    /** Stops responding without discarding the object (re-`enable`-able). */
    disable(): this {
        if (this.enabled) {
            this.recv.remove(this.handler);
            this.enabled = false;
        }
        return this;
    }

    /** Disables permanently; call when finished with this responder. */
    free(): void {
        this.disable();
    }

    /** Frees the responder after its first match — a one-time action. */
    oneShot(): this {
        const inner = this.func;
        this.func = (msg, time, src) => {
            this.free();
            inner(msg, time, src);
        };
        return this;
    }

    toString(): string {
        return `OscFunc(${this.path}, src=${this.src}, argTemplate=${JSON.stringify(
            this.argTemplate,
        )})`;
    }
}

/**
 * Builds an `OscFunc` over a callback — the reference client's decorator form,
 * which in TypeScript is the same curried shape:
 *
 * ```js
 * const resp = oscfunc("/play")((msg, time, src) => console.log(msg));
 * ```
 */
export function oscfunc(
    path: string,
    options: {
        src?: string | null;
        argTemplate?: ArgMatcher[] | null;
        recv?: OscReceiver;
    } = {},
): (func: OscCallback) => OscFunc {
    if (typeof path !== "string") {
        throw new TypeError("oscfunc needs the OSC address path as a string");
    }
    return (func: OscCallback) => new OscFunc(func, path, options);
}

// ---- MIDI ----

/** A `MidiFunc`'s callback: the decoded message and the port it arrived on. */
export type MidiCallback = (message: MidiMessage, src: string) => void;

/** One `argTemplate` slot for a MIDI field: a literal, a predicate, or `null`. */
export type MidiMatcher =
    | ((value: number | string | undefined) => boolean)
    | number
    | string
    | null
    | undefined;

let defaultMidiRecv: MidiReceiver | null = null;

/**
 * The module-default `MidiReceiver`, as installed by
 * `setDefaultMidiReceiver`.
 *
 * **This is the one default a page cannot create for you**, and the reason is
 * the platform rather than the design: the reference client's default opens a
 * virtual input port on the spot, while Web MIDI hands out only the ports that
 * already exist and asks the user's permission to do it — a grant, and an
 * `await`. So a page starts one receiver explicitly and pins it:
 *
 * ```js
 * setDefaultMidiReceiver(await new MidiReceiver({ port: "Keystation" }).start());
 * ```
 *
 * after which `new MidiFunc(fn, "note_on")` needs no arguments, exactly as
 * there. Naming the receiver per responder (`{ recv }`) skips the default
 * entirely.
 */
export function defaultMidiReceiver(): MidiReceiver {
    if (defaultMidiRecv === null) {
        throw new Error(
            "no default MidiReceiver: a page cannot open a MIDI port without a " +
                "user grant, so start one and pin it -- " +
                "setDefaultMidiReceiver(await new MidiReceiver().start())",
        );
    }
    return defaultMidiRecv;
}

/** Installs `receiver` as the module default `defaultMidiReceiver` returns. */
export function setDefaultMidiReceiver(receiver: MidiReceiver): MidiReceiver {
    defaultMidiRecv = receiver;
    return receiver;
}

/**
 * Responder for incoming MIDI messages.
 *
 * Registers `func` to fire on channel-voice messages of a given type. The
 * callback is called `func(message, src)` — `message` an object
 * (`{type, channel, …}`, see `parseMidi`) and `src` the port's name.
 *
 * - `chan` — respond only on that channel (0..15).
 * - `argTemplate` — a `{field: matcher}` object matched against the message's
 *   fields; a matcher is a literal, a predicate, or `null` (matches anything).
 * - `recv` — the `MidiReceiver` to register with; the module default
 *   otherwise.
 *
 * Enabled on creation. Call `free` (or `disable`) when done.
 */
export class MidiFunc {
    /** The callback this responder fires. */
    func: MidiCallback;
    /** The message types it answers. */
    readonly types: string[];
    readonly chan: number | null;
    readonly argTemplate: Record<string, MidiMatcher> | null;
    readonly recv: MidiReceiver;
    enabled = false;

    private readonly handler = (message: MidiMessage, src: string): void => {
        if (!this.types.includes(message.type)) return;
        if (this.chan !== null && message.channel !== this.chan) return;
        if (this.argTemplate !== null) {
            for (const [field, template] of Object.entries(this.argTemplate)) {
                const value = message[field];
                if (typeof template === "function") {
                    if (!template(value)) return;
                } else if (
                    template !== null &&
                    template !== undefined &&
                    template !== value
                ) {
                    return;
                }
            }
        }
        this.func(message, src);
    };

    constructor(
        func: MidiCallback,
        midiMsg: string | string[],
        options: {
            chan?: number | null;
            argTemplate?: Record<string, MidiMatcher> | null;
            recv?: MidiReceiver;
        } = {},
    ) {
        this.func = func;
        this.types = typeof midiMsg === "string" ? [midiMsg] : [...midiMsg];
        this.chan = options.chan ?? null;
        this.argTemplate = options.argTemplate ?? null;
        this.recv = options.recv ?? defaultMidiReceiver();
        this.enable();
    }

    /** Starts responding (registers the handler with the receiver). */
    enable(): this {
        if (!this.enabled) {
            this.recv.add(this.handler);
            this.enabled = true;
        }
        return this;
    }

    /** Stops responding without discarding the object (re-`enable`-able). */
    disable(): this {
        if (this.enabled) {
            this.recv.remove(this.handler);
            this.enabled = false;
        }
        return this;
    }

    /** Disables permanently; call when finished with this responder. */
    free(): void {
        this.disable();
    }

    /** Frees the responder after its first match — a one-time action. */
    oneShot(): this {
        const inner = this.func;
        this.func = (message, src) => {
            this.free();
            inner(message, src);
        };
        return this;
    }

    toString(): string {
        return `MidiFunc(${JSON.stringify(this.types)}, chan=${this.chan})`;
    }
}

/**
 * Builds a `MidiFunc` over a callback — the decorator-shaped door, spelled as
 * a call in a language without decorators.
 *
 * ```js
 * midifunc(["note_on", "note_off"])((message, src) => console.log(message, src));
 * ```
 */
export function midifunc(
    midiMsg: string | string[],
    options: {
        chan?: number | null;
        argTemplate?: Record<string, MidiMatcher> | null;
        recv?: MidiReceiver;
    } = {},
): (func: MidiCallback) => MidiFunc {
    if (typeof midiMsg !== "string" && !Array.isArray(midiMsg)) {
        throw new TypeError("midifunc needs a MIDI type string or list of them");
    }
    return (func: MidiCallback) => new MidiFunc(func, midiMsg, options);
}

export { MidiReceiver, OscReceiver };
