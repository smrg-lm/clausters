// FaustDef: a named Faust definition ready for `/def_send faust` (mirrors
// `clausters/defs/faustdef.py`).
//
// Wraps a graph built with `./signals.ts` (the **signal tree** form), a raw
// **box tree** (the form the Python client's box API emits — machine-built
// here, so it is accepted as JSON rather than rebuilt), or a Faust **source**
// string: the three payloads the server's `/def_send faust` accepts, on equal
// footing (it sniffs which by the first byte). They are three ways of writing
// Faust, not a main road and two detours.
//
// Sending and instantiating is the `Server`'s job; this only builds the
// payload and exposes the declared control names (UI labels), plus the
// reserved `in`/`out` bus controls the server adds.
//
// The in-page engine is the `synth,embed` build with no LLVM JIT, so a
// FaustDef reaches a **native** server only (over the WebSocket carrier);
// against the in-page engine `/def_send faust` fails. That is a property of the
// build, not of this class — nothing here names a carrier.

import type { MsgArg } from "../base/osc.ts";
import type { Server } from "./server.ts";
import { Signal } from "./signals.ts";
import type { SignalNode } from "./signals.ts";

/** Which of the three payload forms a def carries. */
export type FaustDefKind = "signals" | "box" | "source";

const CONTROL_OPS = new Set(["hslider", "vslider", "nentry", "button", "checkbox"]);

export class FaustDef {
    /** The def name (also what `/def_send faust` replies with on success). */
    readonly name: string;
    readonly kind: FaustDefKind;
    private readonly payload: unknown;

    constructor(name: string, payload: unknown, kind: FaustDefKind) {
        this.name = name;
        this.payload = payload;
        this.kind = kind;
    }

    // --- constructors ---

    /** One output per argument (a `Signal` or a number). */
    static fromSignals(name: string, ...outputs: (Signal | number)[]): FaustDef {
        if (outputs.length === 0) {
            throw new TypeError("a signal def needs at least one output");
        }
        const nodes: SignalNode[] = outputs.map((o) =>
            o instanceof Signal ? o.toJSON() : o
        );
        return new FaustDef(name, { signals: nodes }, "signals");
    }

    /** From Faust source, verbatim. */
    static fromSource(name: string, src: string): FaustDef {
        return new FaustDef(name, src, "source");
    }

    /**
     * From a raw box-tree object — the JSON the Python client's box API
     * emits, and what a machine-generated graph produces.
     */
    static fromBox(name: string, box: unknown): FaustDef {
        return new FaustDef(name, box, "box");
    }

    // --- serialization ---

    /**
     * Sends this def to the server via `/def_send faust` and returns its name.
     *
     * `/def_send faust` JIT-compiles on the server's network thread, and reaches a
     * **native** server only: the in-page engine is the `synth,embed` build
     * with no LLVM, and answers `/fail`.
     *
     * `wait: true` (the default) resolves on `/done` and rejects with
     * `CommandError` on `/fail`; `wait: false` only sends, to be sequenced
     * with the server's `sync` before anything relies on the def.
     */
    async send(
        server: Server,
        { wait = true, timeout = 10.0 }: { wait?: boolean; timeout?: number } = {},
    ): Promise<string> {
        const payload: MsgArg[] = [this.name, this.dumpDef()];
        if (!wait) {
            server.sendMsg("/def_send", "faust", ...payload);
            return this.name;
        }
        await server.command("/def_send", ["faust", ...payload], timeout);
        return this.name;
    }

    /**
     * The def serialized to text — the `/def_send faust <name> <payload>` wire
     * payload: a JSON signal/box tree, or the Faust source string verbatim.
     */
    dumpDef(): string {
        if (this.kind === "source") return this.payload as string;
        return JSON.stringify(this.payload);
    }

    // --- controls ---

    /** bus-selecting controls every Faust synth also accepts. */
    static readonly reserved = ["out", "in"] as const;

    /**
     * The control names this def declares (UI labels), in tree order. The
     * reserved `in`/`out` bus controls (added by the server) are not
     * included; see `FaustDef.reserved`.
     */
    controlNames(): string[] {
        const names: string[] = [];
        if (this.kind !== "source") collectLabels(this.payload, names);
        return names;
    }
}

function collectLabels(node: unknown, out: string[]): void {
    if (Array.isArray(node)) {
        for (const item of node) collectLabels(item, out);
    } else if (node !== null && typeof node === "object") {
        const record = node as Record<string, unknown>;
        if (typeof record.op === "string" && CONTROL_OPS.has(record.op)) {
            const label = record.label;
            if (typeof label === "string" && !out.includes(label)) out.push(label);
        }
        for (const value of Object.values(record)) collectLabels(value, out);
    }
}
