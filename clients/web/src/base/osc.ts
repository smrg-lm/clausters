// The OSC codec, through the shared native core.
//
// Encoding and decoding go through `clausters-core` compiled to wasm (the
// `core/` staged bundle), so the bytes are identical to the server's and the
// Python client's by construction — the parity vectors in `tests/` hold this.
// This module replaces the interim hand-written page codec the B milestones
// used (temporary from day one, removed with the consolidation into this
// package).
//
// The wasm module must be loaded once before the sync codec calls:
// `await loadOsc()` (idempotent) — the codec's name for `base/core.ts`'s one
// core load, which everything else core-backed shares.

import { loadCore } from "./core.ts";
import {
    osc_encode_bundle,
    osc_encode_immediate_bundle,
    osc_encode_message,
    osc_encode_score_bundle,
    osc_decode_packet,
    osc_decode_packet_timed,
} from "../core/clausters_core_web.js";

/**
 * One typed argument: the tag keeps the int/float distinction explicit
 * ("i" int32, "h" int64, "f" float32, "d" float64, "s" string, "b" blob).
 */
export type OscArg =
    | ["i" | "h" | "f" | "d", number]
    | ["h", bigint]
    | ["s", string]
    | ["b", Uint8Array];

/** A decoded message: plain values (numbers/strings/Uint8Array/bool/null). */
export interface OscMessage {
    addr: string;
    args: (number | string | Uint8Array | boolean | null)[];
}

/**
 * Loads the core wasm once (later calls reuse it). `source` overrides the
 * default URL-relative lookup with raw module bytes (the node path).
 */
export function loadOsc(source?: BufferSource): Promise<void> {
    return loadCore(source);
}

/**
 * Encodes one OSC message: `encodeMessage("/synth_new", [["s","default"],
 * ["i",1000]])`. Requires a prior `loadOsc()`.
 */
export function encodeMessage(addr: string, args: OscArg[] = []): Uint8Array {
    return osc_encode_message(addr, args);
}

/**
 * Decodes one packet into its messages (bundles flattened, in order).
 * Requires a prior `loadOsc()`.
 */
export function decodePacket(bytes: Uint8Array): OscMessage[] {
    return osc_decode_packet(bytes) as unknown as OscMessage[];
}

/**
 * A decoded message plus the time of the bundle that carried it, in Unix
 * seconds — `null` for a bare message and for an immediate bundle, which says
 * "now" rather than an instant. A nested bundle's messages carry the innermost
 * timetag.
 */
export interface TimedOscMessage extends OscMessage {
    time: number | null;
}

/**
 * `decodePacket` keeping each message's bundle time — what the responder layer
 * reads, so a callback is handed the same `time` the Python client hands its
 * own. Requires a prior `loadOsc()`.
 */
export function decodePacketTimed(bytes: Uint8Array): TimedOscMessage[] {
    return osc_decode_packet_timed(bytes) as unknown as TimedOscMessage[];
}

/** One message inside a bundle: its address and its typed arguments. */
export interface BundleMessage {
    addr: string;
    args: OscArg[];
}

const asEntries = (messages: readonly BundleMessage[]): unknown[] =>
    messages.map((m) => [m.addr, m.args]);

/**
 * One message of a timed bundle: an address and its arguments, tagged by the
 * same rule `sendMsg` uses (an explicit `[tag, value]` pair where the guess
 * would be wrong).
 */
export type TimedMessage = [string, ...MsgArg[]];

/** The bundle form the codec takes. */
export const toBundle = (messages: readonly TimedMessage[]): BundleMessage[] =>
    messages.map(([addr, ...args]) => ({ addr, args: args.map(oscArg) }));

/**
 * Encodes a bundle stamped at `unixSecs` — the wall clock the server reads as
 * an NTP timetag, which is how a message gets a *time*. A message on its own
 * has none: it means "now".
 */
export function encodeBundle(
    unixSecs: number,
    messages: readonly BundleMessage[],
): Uint8Array {
    return osc_encode_bundle(unixSecs, asEntries(messages));
}

/**
 * Encodes a bundle stamped at `secs` **from the start of a render** — the
 * bundle an NRT score is made of.
 *
 * The same packing as `encodeBundle` on a different epoch: a score's time is
 * not a wall clock, so nothing is added to it. Sharing the core's packing is
 * what makes a score written here byte-identical to the Python client's.
 */
export function encodeScoreBundle(
    secs: number,
    messages: readonly BundleMessage[],
): Uint8Array {
    return osc_encode_score_bundle(secs, asEntries(messages));
}

/**
 * Encodes a bundle with the **immediate** timetag: what rides inside
 * `/sched_at`, whose own absolute sample carries the time.
 */
export function encodeImmediateBundle(
    messages: readonly BundleMessage[],
): Uint8Array {
    return osc_encode_immediate_bundle(asEntries(messages));
}

/**
 * A plain value a message argument may take, or an explicit `[tag, value]`
 * pair when the inferred type is wrong.
 */
export type MsgArg = number | string | boolean | bigint | Uint8Array | OscArg;

/**
 * One argument tagged **by inference**: an integral number rides as an int32,
 * a fractional one as a float32, a boolean as the 1/0 the wire carries (OSC
 * has no bool), a string as a string and bytes as a blob. A JS number is a
 * double with no int/float distinction, so this is a guess — the clients tag
 * what they know by position instead, and pass an explicit `[tag, value]`
 * pair wherever the guess would be wrong.
 */
export function oscArg(value: MsgArg): OscArg {
    if (Array.isArray(value) && value.length === 2 && typeof value[0] === "string") {
        return value as OscArg;
    }
    if (typeof value === "bigint") return ["h", value];
    if (typeof value === "string") return ["s", value];
    if (typeof value === "boolean") return ["i", value ? 1 : 0];
    if (value instanceof Uint8Array) return ["b", value];
    return Number.isInteger(value) ? ["i", value as number] : ["f", value as number];
}
