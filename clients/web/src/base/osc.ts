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
    osc_encode_message,
    osc_decode_packet,
} from "../core/clausters_core_web.js";

/// One typed argument: the tag keeps the int/float distinction explicit
/// ("i" int32, "h" int64, "f" float32, "d" float64, "s" string, "b" blob).
export type OscArg =
    | ["i" | "h" | "f" | "d", number]
    | ["h", bigint]
    | ["s", string]
    | ["b", Uint8Array];

/// A decoded message: plain values (numbers/strings/Uint8Array/bool/null).
export interface OscMessage {
    addr: string;
    args: (number | string | Uint8Array | boolean | null)[];
}

/// Loads the core wasm once (later calls reuse it). `source` overrides the
/// default URL-relative lookup with raw module bytes (the node path).
export function loadOsc(source?: BufferSource): Promise<void> {
    return loadCore(source);
}

/// Encodes one OSC message: `encodeMessage("/s_new", [["s","default"],
/// ["i",1000]])`. Requires a prior `loadOsc()`.
export function encodeMessage(addr: string, args: OscArg[] = []): Uint8Array {
    return osc_encode_message(addr, args);
}

/// Decodes one packet into its messages (bundles flattened, in order).
/// Requires a prior `loadOsc()`.
export function decodePacket(bytes: Uint8Array): OscMessage[] {
    return osc_decode_packet(bytes) as unknown as OscMessage[];
}
