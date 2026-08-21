// The directed patcher model: boxes with typed inlets/outlets and cords
// (mirrors the level-1 half of `clausters/defs/patch.py`).
//
// This is the **programmatic** patcher — the `patch` GUI widget is only a visual
// view of it (`toWidget` renders the model the widget draws). A box is a **whole
// def** (a SynthDef/FaustDef the server has) — itself a graph — and the patch
// compiles to a **GraphDef**, whole nodes wired by server buses. A cord *is* a
// bus, but you never number one: `compile` runs the shared cord→bus pass
// (`clausters_core::patch`, through the core's wasm door) that names one bus per
// connected net, its writers summing.
//
// A box has typed **inlets** and **outlets**; a **cord** runs an outlet to an
// inlet, and cords of different rates never connect.
//
//     const p = new GraphPatch();
//     const tone = p.add(toneDef);            // ports read off the SynthDef
//     const dac = p.add(dacDef);              // a terminal sink: an inlet, no outlet
//     p.connect(tone, "out", dac, "in");      // tone -> dac -> speakers
//     await p.toGraphdef("chain").send(server);
//
// Pass a `SynthDef` to `add` and its typed ports are read off the def itself — a
// control feeding an `In` is an inlet, one feeding an `Out` an outlet (the same
// structural fact the server uses to order a graph). Or pass a def **name** and
// list the ports yourself.
//
// The buses are never drawn or named by you, so **the hardware output is not one
// either**: a signal reaches the speakers through a **terminal def** — a `dac`
// with an inlet and no outlet, its `out(0, …)` baked in — a box like any other.
//
// The **rate** of a port is its cord type: an audio port is a plain name, a
// control port the pair `[name, "control"]`.
//
// **Level 2 — `DefPatch`, one def's internal UGen graph — is not ported yet**
// (`clients/web/PLAN.md`). What is here is what the multitrack editor needs to
// draw a logical aggregate, and the gap is named rather than papered over.

import { patchCompile } from "../core/clausters_core_web.js";
import { GraphDef } from "./graphdef.ts";
import { SynthDef } from "./synthdef.ts";
import { Control, Ugen } from "./ugens/index.ts";

/**
 * A UGen that **reads** a bus (an inlet when its bus is a control), and the port
 * rate it implies.
 */
const READERS: Record<string, PortRate> = { In: "audio", InCtl: "control" };
/**
 * A UGen that **writes** a bus (an outlet when its bus is a control), and its
 * rate. `ReplaceOut` overwrites rather than sums, but it is still an outlet.
 */
const WRITERS: Record<string, PortRate> = {
    Out: "audio",
    ReplaceOut: "audio",
    OutCtl: "control",
};

/** The rate a cord runs at. */
export type PortRate = "audio" | "control";

/** A port as a caller writes it: a bare name (audio), or `[name, rate]`. */
export type PortSpec = string | readonly [name: string, rate: PortRate];

/** A port in the flat form the cord→bus pass consumes. */
export interface Port {
    name: string;
    dir: "in" | "out";
    rate: PortRate;
}

/** One box: the def it instantiates, and its ports. */
export interface Box {
    def: string;
    ports: Port[];
    /** The host's layout role, for a level-2 box; absent means `"object"`. */
    role?: string;
}

/** One cord, its endpoints flat indices into each box's `ports`. */
export interface Cord {
    from_box: number;
    from_port: number;
    to_box: number;
    to_port: number;
}

/** What `compile` answers: the private buses, and each member wired to them. */
export interface Compiled {
    buses: { name: string; rate: PortRate }[];
    members: { def: string; controls: { control: string; bus: string }[] }[];
}

/**
 * Derive a `SynthDef`'s patcher ports `[inlets, outlets]` from its graph, the way
 * the directed patcher wants them — **structural, not a guess**: a control that
 * feeds an `In`/`InCtl` is an inlet, one that feeds an `Out`/`OutCtl`/
 * `ReplaceOut` is an outlet, and the reading/writing UGen's family fixes the rate
 * (audio for `In`/`Out`, control for the `Ctl` pair). A control that feeds
 * neither is a plain value, not a port.
 *
 * Each port comes back in the form {@link GraphPatch.add} consumes: a bare name
 * for an audio port, `[name, "control"]` for a control one. Names are
 * de-duplicated keeping first-seen order (a stereo `Out` writing one bus control
 * is one outlet).
 */
export function synthdefPorts(sdef: SynthDef): [PortSpec[], PortSpec[]] {
    const inlets = new Map<string, PortRate>();
    const outlets = new Map<string, PortRate>();
    for (const ugen of walk(sdef.roots)) {
        const bus = ugen.inputs.length > 0 ? ugen.inputs[0] : undefined;
        if (!(bus instanceof Control)) continue;
        const reader = READERS[ugen.kind];
        const writer = WRITERS[ugen.kind];
        if (reader !== undefined) {
            if (!inlets.has(bus.name)) inlets.set(bus.name, reader);
        } else if (writer !== undefined) {
            if (!outlets.has(bus.name)) outlets.set(bus.name, writer);
        }
    }
    const spec = (name: string, rate: PortRate): PortSpec =>
        rate === "audio" ? name : [name, rate];
    return [
        [...inlets].map(([name, rate]) => spec(name, rate)),
        [...outlets].map(([name, rate]) => spec(name, rate)),
    ];
}

/**
 * Every `Ugen` reachable from `roots`, each once (a DFS over `inputs`). Controls
 * and constants are inputs, not walked as nodes.
 */
function* walk(roots: readonly unknown[]): Generator<Ugen> {
    const seen = new Set<Ugen>();
    const stack = [...roots];
    while (stack.length > 0) {
        const node = stack.pop();
        if (node instanceof Ugen && !seen.has(node)) {
            seen.add(node);
            yield node;
            stack.push(...node.inputs);
        }
    }
}

/**
 * Normalize a port spec — a bare name (audio) or `[name, "control"]` — into the
 * flat `{name, dir, rate}` the cord→bus pass consumes.
 */
function port(spec: PortSpec, dir: "in" | "out"): Port {
    const [name, rate] = typeof spec === "string" ? [spec, "audio" as PortRate] : spec;
    if (rate !== "audio" && rate !== "control") {
        throw new TypeError(`port rate must be 'audio' or 'control', got ${String(rate)}`);
    }
    return { name: String(name), dir, rate };
}

/**
 * A directed level-1 patch — whole defs wired by buses — that compiles to a
 * `GraphDef`. Its boxes and the cords between their ports.
 */
export class GraphPatch {
    /** Each box, in the schema the cord→bus pass reads. */
    boxes: Box[] = [];
    /** Each cord, its ports flat indices into the box's `ports`. */
    cords: Cord[] = [];

    // ---- building ----

    /**
     * Add a box for a def and answer its index. `defname` is either a `SynthDef`
     * — whose typed ports are then **derived from its graph** (see
     * {@link synthdefPorts}) — or a def **name**, for which you list the
     * `inlets`/`outlets` yourself. Passing explicit ports with a `SynthDef`
     * overrides the derived ones. A **terminal** def (a sink that reaches
     * hardware itself) is simply one with inlets and no outlets.
     */
    add(
        defname: SynthDef | string,
        inlets: readonly PortSpec[] = [],
        outlets: readonly PortSpec[] = [],
    ): number {
        let name: string;
        let ins = inlets;
        let outs = outlets;
        if (defname instanceof SynthDef) {
            name = defname.name;
            if (ins.length === 0 && outs.length === 0) {
                [ins, outs] = synthdefPorts(defname);
            }
        } else {
            name = String(defname);
        }
        const ports = [
            ...ins.map((p) => port(p, "in")),
            ...outs.map((p) => port(p, "out")),
        ];
        this.boxes.push({ def: name, ports });
        return this.boxes.length - 1;
    }

    /**
     * Draw a directed cord: box `src`'s `outlet` → box `dst`'s `inlet` (each port
     * by name or flat index). A no-op if it already exists, so applying an edit
     * twice is safe.
     */
    connect(src: number, outlet: string | number, dst: number, inlet: string | number): this {
        const cord: Cord = {
            from_box: Math.trunc(src),
            from_port: this.portIndex(src, outlet, "out"),
            to_box: Math.trunc(dst),
            to_port: this.portIndex(dst, inlet, "in"),
        };
        if (!this.cords.some((c) => sameCord(c, cord))) this.cords.push(cord);
        return this;
    }

    /** Remove the cord `src.outlet → dst.inlet` if present. */
    disconnect(src: number, outlet: string | number, dst: number, inlet: string | number): this {
        const cord: Cord = {
            from_box: Math.trunc(src),
            from_port: this.portIndex(src, outlet, "out"),
            to_box: Math.trunc(dst),
            to_port: this.portIndex(dst, inlet, "in"),
        };
        this.cords = this.cords.filter((c) => !sameCord(c, cord));
        return this;
    }

    // ---- decoding a stored graph back into a patch ----

    /**
     * Decode a `GraphDef` into a directed patch — the inverse of
     * {@link GraphPatch.toGraphdef}. Each member becomes a box; a member control
     * valued an internal-bus **name** (a string other than the hardware sentinel
     * `"OUT"`) becomes a cord from the writing outlet to every reading inlet on
     * that bus.
     *
     * Direction and rate are **not guessed**: a box's typed ports come from its
     * def, so `defs` maps a member's def name to the `SynthDef` it was built
     * from. A member whose def is not resolvable through `defs` draws
     * **port-less** — its wiring cannot be typed, so it grows no cords. The box
     * order is the member order, so a caller maps a box index straight back to
     * the member it came from.
     */
    static fromGraphdef(gdef: GraphDef, defs: Record<string, unknown> = {}): GraphPatch {
        const patch = new GraphPatch();
        const members = gdef.members();
        for (const member of members) {
            const sdef = defs[member.def];
            patch.add(sdef instanceof SynthDef ? sdef : member.def);
        }
        // A cord is a bus: group each box's bus-valued controls into writers and
        // readers by port direction, then wire every writer to every reader
        // sharing a bus name (fan-in and fan-out fall out of the shared name).
        const writers = new Map<string, [number, string][]>();
        const readers = new Map<string, [number, string][]>();
        members.forEach((member, box) => {
            const ports = patch.boxes[box]?.ports ?? [];
            const outNames = new Set(ports.filter((p) => p.dir === "out").map((p) => p.name));
            const inNames = new Set(ports.filter((p) => p.dir === "in").map((p) => p.name));
            for (const [ctl, value] of Object.entries(member.controls ?? {})) {
                // A number is a value; "OUT" reaches hardware, not a cord.
                if (typeof value !== "string" || value === "OUT") continue;
                const into = outNames.has(ctl) ? writers : inNames.has(ctl) ? readers : null;
                if (into === null) continue;
                const list = into.get(value);
                if (list === undefined) into.set(value, [[box, ctl]]);
                else list.push([box, ctl]);
            }
        });
        for (const [bus, sources] of writers) {
            for (const [srcBox, outlet] of sources) {
                for (const [dstBox, inlet] of readers.get(bus) ?? []) {
                    patch.connect(srcBox, outlet, dstBox, inlet);
                }
            }
        }
        return patch;
    }

    // ---- compiling ----

    /** The patch as the cord→bus pass reads it. */
    toJson(): { boxes: Box[]; cords: Cord[] } {
        return { boxes: this.boxes, cords: this.cords };
    }

    /**
     * Run the shared cord→bus pass. Answers `{buses, members}` — one private bus
     * per connected net (writers summing), each member its def and its wired
     * controls. Throws on a bad cord (reversed, rate-mismatched, out of range),
     * naming the offender.
     */
    compile(): Compiled {
        return JSON.parse(patchCompile(JSON.stringify(this.toJson()))) as Compiled;
    }

    /**
     * Compile to a ready-to-send `GraphDef`: the private buses declared and each
     * member wired to them.
     */
    toGraphdef(name: string): GraphDef {
        const compiled = this.compile();
        const gdef = new GraphDef(name);
        const refs = new Map(
            compiled.buses.map((b) => [b.name, gdef.bus(b.name, { rate: b.rate })]),
        );
        for (const member of compiled.members) {
            const controls: Record<string, ReturnType<GraphDef["bus"]>> = {};
            for (const w of member.controls) {
                const ref = refs.get(w.bus);
                if (ref !== undefined) controls[w.control] = ref;
            }
            gdef.add(member.def, controls);
        }
        return gdef;
    }

    // ---- the GUI view (the `patch` widget's split schema) ----

    /**
     * The patch as the `patch` widget draws it: boxes with **split**
     * inlets/outlets and cords as `[fromBox, outlet, toBox, inlet]` quadruples
     * (the indices are within each box's inlet/outlet lists). Pass `geometry`
     * (`{boxIndex: [x, y]}`) to place boxes; the rest auto-stack. The GUI edits
     * the same model — a `"wire"` event names its ports, which
     * {@link GraphPatch.connect} resolves, so the round trip needs no index
     * bookkeeping.
     */
    toWidget(geometry: Record<number, readonly [number, number]> = {}): PatchWidget {
        return patchToWidget(this.boxes, this.cords, geometry);
    }

    // ---- helpers ----

    private portIndex(box: number, name: string | number, dir: "in" | "out"): number {
        if (typeof name === "number") return name;
        const ports = this.boxes[Math.trunc(box)]?.ports ?? [];
        const found = ports.findIndex((p) => p.name === name && p.dir === dir);
        if (found < 0) throw new Error(`box ${box} has no ${dir}let named ${name}`);
        return found;
    }
}

/** What the `patch` widget draws: split boxes, and cords as flat quadruples. */
export interface PatchWidget {
    boxes: Record<string, unknown>[];
    cords: number[];
}

const sameCord = (a: Cord, b: Cord): boolean =>
    a.from_box === b.from_box &&
    a.from_port === b.from_port &&
    a.to_box === b.to_box &&
    a.to_port === b.to_port;

/**
 * A port for the widget schema: a bare name for audio, `{name, rate}` for
 * control or init (so the widget draws the cord's weight, dashing init).
 */
const widgetPort = (p: Port): string | { name: string; rate: string } =>
    p.rate === "audio" ? p.name : { name: p.name, rate: p.rate };

/**
 * The within-side index of the flat port `flat` on `box`: the widget draws inlets
 * and outlets as separate lists, so a cord endpoint (a flat index into the box's
 * combined `ports`) is remapped to its position among the ports on its own side.
 */
function splitIndex(boxes: Box[], box: number, flat: number, dir: "in" | "out"): number {
    const ports = boxes[Math.trunc(box)]?.ports ?? [];
    const same = ports.map((p, i) => [p, i] as const).filter(([p]) => p.dir === dir);
    return same.findIndex(([, i]) => i === flat);
}

/**
 * Render the shared `{boxes, cords}` model into the `patch` widget schema — boxes
 * with split inlet/outlet lists and cords as flat `[fromBox, outlet, toBox,
 * inlet]` quadruples.
 */
export function patchToWidget(
    boxes: Box[],
    cords: Cord[],
    geometry: Record<number, readonly [number, number]> = {},
): PatchWidget {
    const drawn = boxes.map((box, i) => {
        const d: Record<string, unknown> = {
            def: box.def,
            inlets: box.ports.filter((p) => p.dir === "in").map(widgetPort),
            outlets: box.ports.filter((p) => p.dir === "out").map(widgetPort),
        };
        // The layout role (the host's inverted tree pins sources, tucks
        // constants); absent / "object" is the default, so level-1 boxes need
        // not carry it.
        if (box.role !== undefined && box.role !== "object") d.role = box.role;
        const at = geometry[i];
        if (at !== undefined) {
            d.x = at[0];
            d.y = at[1];
        }
        return d;
    });
    const flat: number[] = [];
    for (const c of cords) {
        flat.push(
            c.from_box,
            splitIndex(boxes, c.from_box, c.from_port, "out"),
            c.to_box,
            splitIndex(boxes, c.to_box, c.to_port, "in"),
        );
    }
    return { boxes: drawn, cords: flat };
}
