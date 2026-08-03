// SynthDef: a named UGen graph ready for `/def_send synth` (mirrors
// `clausters/defs/synthdef.py`, emitting the same JSON `SynthDefSpec`).
//
// The UGen-graph counterpart of `FaustDef`: it wraps one or more root `Ugen`
// nodes (built with the lowercase callables in `./ugens.ts`), walks the graph
// and serializes the `{name, controls, ugens}` JSON the server compiles.
//
// ```ts
// const freq = control("freq", 440.0);
// const amp = control("amp", 0.2);
// const sig = sine(freq).mul(amp);
// const def = new SynthDef("beep", out(0.0, sig), out(1.0, sig));  // stereo
// await def.send(server);                                   // /def_send synth
// ```
//
// **Instance-based build (no globals).** The walk is a plain post-order
// traversal of the root nodes: a UGen is emitted only after its inputs, so
// the `ugens` list is topologically ordered (every `{"ugen": w}` reference
// points at an earlier node, as the server requires) and shared sub-graphs
// are emitted once (dedup by object identity). Controls are gathered in
// first-seen order; reusing the same name with a different default is an
// error.

import type { MsgArg } from "../base/osc.ts";
import type { Server } from "./server/index.ts";
import { resolveServer } from "./wire.ts";
import { ChannelList, Control, Ugen } from "./ugens/index.ts";
import type { Channel } from "./ugens/index.ts";

/**
 * One serialized UGen input: a reference to an earlier UGen, to a control,
 * or a constant.
 */
export type SpecInput =
    | { ugen: number }
    | { control: number }
    | { const: number };

export interface ControlSpec {
    name: string;
    default: number;
    rate?: string;
    lag?: number;
    lag_down?: number;
}

export interface UgenSpec {
    kind: string;
    inputs: SpecInput[];
    rate?: string;
    op?: string;
    label?: string;
    [field: string]: unknown;
}

/** The `SynthDefSpec` the server's `/def_send synth` compiles. */
export interface SynthDefSpec {
    name: string;
    controls: ControlSpec[];
    ugens: UgenSpec[];
}

/**
 * A named UGen graph. Pass the graph's **root** UGens — normally the outputs
 * (`out(...)`/`replaceOut(...)`, and any `localOut(...)` to keep feedback
 * writes in the graph), but a root can equally be a side-effect UGen with no
 * audio output (`sendTrig(...)`/`sendReply(...)`/`poll(...)`): a def may
 * consist only of those and no `out` at all. Every root must be a UGen; a
 * def needs at least one (the server rejects an empty graph). A def with no
 * output UGen is simply silent on the server.
 */
export class SynthDef {
    readonly name: string;
    readonly roots: Ugen[];

    constructor(name: string, ...roots: (Ugen | ChannelList)[]) {
        const flat: Ugen[] = [];
        for (const root of roots) {
            // A multichannel root (out(bus, dup(sig)) returns a ChannelList
            // of Outs) contributes one root per channel.
            if (root instanceof ChannelList) {
                for (const item of root.items) {
                    if (!(item instanceof Ugen)) {
                        throw new TypeError("SynthDef roots must be UGens");
                    }
                    flat.push(item);
                }
            } else {
                flat.push(root);
            }
        }
        if (flat.length === 0) {
            throw new TypeError(
                "a SynthDef needs at least one root UGen (an output like " +
                    "out(bus, signal), or a side-effect UGen like sendTrig(...))",
            );
        }
        for (const root of flat) {
            if (!(root instanceof Ugen)) {
                throw new TypeError("SynthDef roots must be UGens");
            }
        }
        this.name = String(name);
        this.roots = flat;
    }

    /** The `SynthDefSpec` object the server's `/def_send synth` compiles. */
    spec(): SynthDefSpec {
        const ordered: Ugen[] = []; // UGens in topological order
        const wire = new Map<Ugen, number>(); // ugen -> its index in `ordered`
        const controls: Control[] = []; // controls in first-seen order
        const ctlIndex = new Map<string, number>();

        const visit = (node: unknown): void => {
            if (node instanceof Ugen) {
                if (wire.has(node)) return;
                for (const input of node.inputs) visit(input);
                wire.set(node, ordered.length);
                ordered.push(node);
            } else if (node instanceof Control) {
                const seen = ctlIndex.get(node.name);
                if (seen === undefined) {
                    ctlIndex.set(node.name, controls.length);
                    controls.push(node);
                } else if (controls[seen]!.signature() !== node.signature()) {
                    throw new TypeError(
                        `control '${node.name}' used with conflicting ` +
                            "definitions (default/type/lag differ)",
                    );
                }
            } else if (node instanceof ChannelList) {
                throw new TypeError(
                    "a channel list cannot feed a single-channel input -- " +
                        "index it (chans.at(0)) or mix() it down; per-argument " +
                        "multichannel expansion is not implemented",
                );
            } else if (typeof node !== "number") {
                throw new TypeError(`not a UGen graph node: ${String(node)}`);
            }
            // a plain number is a constant: nothing to gather here
        };

        for (const root of this.roots) visit(root);

        const serInput = (input: Channel): SpecInput => {
            if (input instanceof Ugen) return { ugen: wire.get(input)! };
            if (input instanceof Control) return { control: ctlIndex.get(input.name)! };
            return { const: Number(input) };
        };

        const serControl = (c: Control): ControlSpec => {
            const d: ControlSpec = { name: c.name, default: c.default };
            if (c.rate !== undefined) d.rate = c.rate;
            if (c.lag !== undefined) d.lag = c.lag;
            if (c.lagDown !== undefined) d.lag_down = c.lagDown;
            return d;
        };

        const serUgen = (u: Ugen): UgenSpec => {
            const d: UgenSpec = { kind: u.kind, inputs: u.inputs.map(serInput) };
            if (u.rate !== undefined) d.rate = u.rate;
            if (u.op !== undefined) d.op = u.op;
            if (u.label !== undefined) d.label = u.label;
            if (u.staticFields) Object.assign(d, u.staticFields);
            return d;
        };

        return {
            name: this.name,
            controls: controls.map(serControl),
            ugens: ordered.map(serUgen),
        };
    }

    /**
     * Sends this def to the server via `/def_send synth` and returns its name.
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
        const payload: MsgArg[] = [this.dumpDef()];
        if (!wait) {
            target.sendMsg("/def_send", "synth", ...payload);
            return this.name;
        }
        await target.command("/def_send", ["synth", ...payload], timeout);
        return this.name;
    }

    /**
     * The def serialized to text — the `/def_send synth` wire payload. Useful to
     * inspect the built graph before sending it.
     */
    dumpDef(): string {
        return JSON.stringify(this.spec());
    }

    /** The control names this def declares, in first-seen order. */
    controlNames(): string[] {
        return this.spec().controls.map((c) => c.name);
    }
}
