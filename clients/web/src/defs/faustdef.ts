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
import type { Server } from "./server/index.ts";
import { resolveServer } from "./wire.ts";
import { Signal } from "./signals.ts";
import type { SignalNode } from "./signals.ts";
import type { PatchViewOptions, PatchWindow } from "../plot.ts";
import type { ControlInfo } from "./info.ts";

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

    /**
     * @internal — how {@link DefPatch} reads a signal tree back out. The twin of
     * the Python client's `_payload`: private to the package, not surface.
     */
    get patchPayload(): unknown {
        return this.payload;
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
        server?: Server,
        { wait = true, timeout = 10.0 }: { wait?: boolean; timeout?: number } = {},
    ): Promise<string> {
        const target = resolveServer(server);
        const payload: MsgArg[] = [this.name, this.dumpDef()];
        if (!wait) {
            target.sendMsg("/def_send", "faust", ...payload);
            return this.name;
        }
        await target.command("/def_send", ["faust", ...payload], timeout);
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
     * Open this def's **structure** as a directed `patch` view in its own
     * window on the ambient GUI host — the level-2 patcher drawn from the def's
     * signal graph (every signal op a box, every operand a cord), the host
     * laying the boxes out as an inverted tree. One window per call, the `plot`
     * posture; this shows the def's *structure*, where `plot(this)` renders its
     * *sound*.
     *
     * A **signal-tree** def ({@link FaustDef.fromSignals}) decodes node for
     * node; a **box-tree** or **source** def is opaque and draws as a single box
     * (its internals are the Faust compiler's, not reconstructable
     * client-side). `label` captions the patch panel (defaults to `"faustdef"`
     * — the panel names *what* is drawn, not the def's name); `host` is an
     * explicit `GuiHost`, absent resolves the ambient one. Resolves with a
     * `PatchWindow` (`close()`).
     */
    async plotDef(options: PatchViewOptions = {}): Promise<PatchWindow> {
        const { DefPatch } = await import("./patch.ts");
        const { openPatchView } = await import("../plot.ts");
        return openPatchView(DefPatch.fromFaustdef(this), {
            label: "faustdef",
            title: this.name,
            ...options,
        });
    }

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

    /**
     * This def's control surface as `ControlInfo` entries, in tree order — the
     * shape all three def families answer with.
     *
     * A Faust control is the one that **brings its own range**: an
     * `hslider`/`vslider`/`nentry` declares `init`, `min`, `max` and `step`
     * where it is written, so a GUI control built from one needs nothing else.
     * A `button`/`checkbox` is a 0/1 control and says so. The reserved
     * `in`/`out` bus controls are not included.
     */
    controls(): ControlInfo[] {
        const params: ControlInfo[] = [];
        if (this.kind !== "source") collectParams(this.payload, params);
        return params;
    }

    /** One control by name, as a `ControlInfo` — `fd.control("cutoff")`. */
    control(name: string): ControlInfo {
        for (const info of this.controls()) {
            if (info.name === name) return info;
        }
        throw new Error(
            `'${this.name}' declares no control '${name}' ` +
                `(it has: ${this.controlNames().join(", ") || "none"})`,
        );
    }
}

/**
 * Every UI control in a signal/box payload, as `ControlInfo` in tree order.
 *
 * Faust puts the range where the control is declared, so this is a read rather
 * than a lookup: an `hslider` carries `init`/`min`/`max`/`step`, a
 * `button`/`checkbox` is 0/1 with no step.
 */
function collectParams(node: unknown, out: ControlInfo[]): void {
    if (Array.isArray(node)) {
        for (const item of node) collectParams(item, out);
        return;
    }
    if (node === null || typeof node !== "object") return;
    const record = node as Record<string, unknown>;
    const op = record.op;
    const label = record.label;
    if (
        typeof op === "string" && CONTROL_OPS.has(op) && typeof label === "string"
        && !out.some((p) => p.name === label)
    ) {
        if (op === "button" || op === "checkbox") {
            out.push({ name: label, default: 0.0, rate: "kr", min: 0.0, max: 1.0 });
        } else {
            const info: ControlInfo = {
                name: label,
                default: Number(record.init ?? 0.0),
                rate: "kr",
            };
            if (typeof record.min === "number") info.min = record.min;
            if (typeof record.max === "number") info.max = record.max;
            if (typeof record.step === "number") info.step = record.step;
            out.push(info);
        }
    }
    for (const value of Object.values(record)) collectParams(value, out);
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
