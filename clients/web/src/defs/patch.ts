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
import { FaustDef } from "./faustdef.ts";
import { GraphDef } from "./graphdef.ts";
import { SynthDef } from "./synthdef.ts";
import { Control, Ugen, ugenInputNames } from "./ugens/index.ts";
import type { Channel, ControlRate, UgenRate } from "./ugens/index.ts";

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

/** The rate a cord runs at — the two a **server bus** has. */
export type PortRate = "audio" | "control";

/**
 * The rate a cord is *drawn* at. Level 2 adds a third weight over the bus
 * rates: **init** (`ir`) — a scalar read once at init time, never a bus, so it
 * exists only inside one def's graph and the widget dashes it.
 */
export type CordRate = PortRate | "init";

/** A port as a caller writes it: a bare name (audio), or `[name, rate]`. */
export type PortSpec = string | readonly [name: string, rate: PortRate];

/** A port in the flat form the cord→bus pass consumes. */
export interface Port {
    name: string;
    dir: "in" | "out";
    rate: CordRate;
}

/** What a level-2 box *is*, which is what {@link DefPatch.toSynthdef} rebuilds from. */
export type BoxKind = "ugen" | "control" | "const" | "faust" | "faust-opaque";

/**
 * One box: the def it instantiates, and its ports. A level-1 box is a whole def
 * and carries nothing else; a level-2 box also says what it is (`kind`) and
 * keeps what it was decoded from, which is what makes the decode reversible.
 */
export interface Box {
    def: string;
    ports: Port[];
    /** The host's layout role; absent means `"object"`. */
    role?: string;
    /** Level 2 only: what this box is. */
    kind?: BoxKind;
    /** A `"ugen"` box's node, as {@link DefPatch.toSynthdef} rebuilds it. */
    ugen?: {
        kind: string;
        rate?: UgenRate;
        op?: string;
        label?: string;
        static?: Record<string, unknown>;
    };
    /** A `"control"` box's control. */
    control?: {
        name: string;
        default: number;
        rate?: ControlRate;
        lag?: number;
        lagDown?: number;
    };
    /** A `"const"` box's literal value. */
    const?: number;
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

// ===================================================================
// Level 2: the Def-view — a SynthDef/FaustDef as its internal graph.
// ===================================================================

/**
 * A UGen calculation rate → the cord type the widget draws. `ir` (init /
 * scalar) is the level-2 third weight (dashed); `dr` (demand) has no bus weight
 * of its own, so it reads as control. An **unset** UGen rate defaults to audio:
 * most UGens are audio-rate, and the exact per-kind default is the server's,
 * not the client's — an honest headless heuristic for a view.
 */
const UGEN_RATE: Record<string, CordRate> = {
    ar: "audio",
    kr: "control",
    ir: "init",
    dr: "control",
};

/** A control **type** → the cord type. A scalar (`ir`) control is an init cord. */
const CONTROL_RATE: Record<string, CordRate> = {
    kr: "control",
    control: "control",
    tr: "control",
    trigger: "control",
    ir: "init",
    scalar: "init",
};

/** Faust signal ops that are controls (a UI label), drawn as source boxes. */
const FAUST_CONTROL_OPS = new Set(["hslider", "vslider", "nentry", "button", "checkbox"]);

/**
 * The cord type of `node`'s output — `"audio"`/`"control"`/`"init"` — for
 * drawing and typing a cord. A `Ugen` maps its calc rate (unset → audio); a
 * `Control` maps its type (unset → control); a bare number is a constant
 * (init).
 */
function rateOf(node: unknown): CordRate {
    if (node instanceof Ugen) return UGEN_RATE[node.rate ?? ""] ?? "audio";
    if (node instanceof Control) return CONTROL_RATE[node.rate ?? ""] ?? "control";
    return "init";
}

/**
 * The box caption for a UGen: the operator name for the generic op UGens (so
 * `a.mul(b)` reads `mul`, not `BinaryOpUGen`), the kind otherwise.
 */
function ugenLabel(u: Ugen): string {
    if (u.op && (u.kind === "BinaryOpUGen" || u.kind === "UnaryOpUGen")) return u.op;
    return u.kind;
}

/**
 * A value box's caption: a compact number (an integer-valued float drops its
 * trailing `.0`, others keep a few significant digits).
 */
function formatConst(value: unknown): string {
    const f = Number(value);
    if (!Number.isFinite(f)) return String(value);
    return Number.isInteger(f) ? String(f) : String(Number(f.toPrecision(6)));
}

/**
 * The flat index of a box's single outlet in its `ports` — the inlets come
 * first, so it is the inlet count.
 */
function outletFlat(box: Box): number {
    return box.ports.filter((p) => p.dir === "in").length;
}

/**
 * Every `Ugen` reachable from `outputs` in the def's topological order (a UGen
 * after its inputs), each once — the same post-order `SynthDef` serialization
 * walks, so in the decode a box's input boxes always precede it.
 */
function topoUgens(outputs: readonly Channel[]): Ugen[] {
    const ordered: Ugen[] = [];
    const seen = new Set<Ugen>();
    const visit = (node: unknown): void => {
        if (!(node instanceof Ugen) || seen.has(node)) return;
        seen.add(node);
        for (const input of node.inputs) visit(input);
        ordered.push(node);
    };
    for (const o of outputs) visit(o);
    return ordered;
}

/**
 * A level-2 patch — the internal graph of a single `SynthDef`/`FaustDef`, its
 * UGen (or Faust op) boxes wired by internal cords. Built as a **read-only
 * view**: {@link DefPatch.fromSynthdef} / {@link DefPatch.fromFaustdef} decode a
 * def's in-memory graph so it draws as its boxes; {@link DefPatch.toWidget}
 * renders it for the `patch` widget exactly as level 1, plus the init (`ir`)
 * cord type; {@link DefPatch.toSynthdef} reconstructs the SynthDef (the decode
 * is faithful — the round trip reproduces the spec).
 *
 * A cord here is an **internal wire**, never an allocated server bus — that is
 * the whole difference from {@link GraphPatch}.
 */
export class DefPatch {
    /**
     * Each box, carrying a `kind` and a layout `role`. A **ugen** box:
     * `{def, kind:"ugen", role:"object", ugen:{…}, ports:[…]}`. A **control**
     * box: `{def, kind:"control", role:"source", control:{…}, ports:[outlet]}`.
     * A **const** value box: `{def, kind:"const", role:"const", const, ports:
     * [outlet]}`. A **faust** box mirrors ugen without the rebuild fields.
     */
    boxes: Box[] = [];
    /**
     * Each cord — flat port indices into each box's `ports` (an outlet → an
     * inlet).
     */
    cords: Cord[] = [];
    /**
     * Box indices of the def's output roots (its `out`/side-effect UGens or the
     * Faust output signals), in order — what {@link DefPatch.toSynthdef}
     * rebuilds.
     */
    roots: number[] = [];

    // ---- decoding a SynthDef's UGen graph ----

    /**
     * Decode a `SynthDef`'s in-memory UGen graph into a level-2 patch: every
     * UGen a box, every referenced control a **source** box, every constant a
     * **value** box, and every input a cord. Each box carries a layout role, so
     * the host draws it as an inverted tree — controls pinned to the top row,
     * value boxes tucked above the box they feed, sinks at the bottom.
     */
    static fromSynthdef(sdef: SynthDef): DefPatch {
        const patch = new DefPatch();
        const ordered = topoUgens(sdef.roots);
        // Controls first (one box per unique name — the pinned source row), then
        // the UGens in the def's own order (each after the inputs that feed it).
        const controls = new Map<string, number>();
        for (const u of ordered) {
            for (const input of u.inputs) {
                if (input instanceof Control && !controls.has(input.name)) {
                    controls.set(input.name, patch.boxes.length);
                    patch.addControl(input);
                }
            }
        }
        const ugenBox = new Map<Ugen, number>();
        for (const u of ordered) {
            ugenBox.set(u, patch.boxes.length);
            patch.addUgen(u);
        }
        for (const u of ordered) {
            const bi = ugenBox.get(u)!;
            u.inputs.forEach((input, pos) => {
                let src: number;
                if (input instanceof Ugen) src = ugenBox.get(input)!;
                else if (input instanceof Control) src = controls.get(input.name)!;
                else src = patch.addConst(input); // a literal → its own value box
                patch.connect(src, outletFlat(patch.boxes[src]!), bi, pos);
            });
        }
        patch.roots = sdef.roots.map((o) => ugenBox.get(o as Ugen)!);
        return patch;
    }

    private addUgen(u: Ugen): void {
        const names = ugenInputNames(u.kind) ?? [];
        const inlets: Port[] = u.inputs.map((input, pos) => ({
            name: names[pos] ?? String(pos),
            dir: "in",
            rate: rateOf(input),
        }));
        this.boxes.push({
            def: ugenLabel(u),
            kind: "ugen",
            role: "object",
            ugen: {
                kind: u.kind,
                rate: u.rate,
                op: u.op,
                label: u.label,
                static: u.staticFields,
            },
            ports: [...inlets, { name: "", dir: "out", rate: rateOf(u) }],
        });
    }

    private addControl(c: Control): void {
        this.boxes.push({
            def: c.name,
            kind: "control",
            role: "source",
            control: {
                name: c.name,
                default: c.default,
                rate: c.rate,
                lag: c.lag,
                lagDown: c.lagDown,
            },
            ports: [{ name: "", dir: "out", rate: rateOf(c) }],
        });
    }

    /**
     * Add a **value** box for a literal input and answer its index — a source
     * with a single init-rate outlet, captioned with the number.
     */
    private addConst(value: unknown): number {
        this.boxes.push({
            def: formatConst(value),
            kind: "const",
            role: "const",
            const: Number(value),
            ports: [{ name: "", dir: "out", rate: "init" }],
        });
        return this.boxes.length - 1;
    }

    private connect(fb: number, fp: number, tb: number, tp: number): void {
        this.cords.push({ from_box: fb, from_port: fp, to_box: tb, to_port: tp });
    }

    // ---- decoding a FaustDef ----

    /**
     * Decode a `FaustDef` into a level-2 patch. A **signal-tree** def
     * (`FaustDef.fromSignals`) decodes node for node — every signal op a box,
     * every control (slider/button) a source box, every operand a cord. A
     * **box-tree** or **source** def is opaque (its internals are the Faust
     * compiler's, not reconstructable client-side), so it draws as a single
     * box. Faust cords carry no server-bus rate, so they read as audio; a
     * control's is control.
     */
    static fromFaustdef(fdef: FaustDef): DefPatch {
        const patch = new DefPatch();
        if (fdef.kind === "signals") {
            const memo = new Map<object, number>();
            const payload = fdef.patchPayload as { signals?: unknown[] } | undefined;
            for (const node of payload?.signals ?? []) {
                const root = patch.signalBox(node, memo);
                if (root !== undefined) patch.roots.push(root);
            }
        } else {
            patch.boxes.push({
                def: fdef.name,
                kind: "faust-opaque",
                role: "object",
                ports: [{ name: "", dir: "out", rate: "audio" }],
            });
            patch.roots.push(0);
        }
        return patch;
    }

    /**
     * Build the box for one Faust signal node (post-order, so operands precede
     * it); answers its index, or `undefined` for a bare number (which the caller
     * turns into a value box). Shared nodes dedup by identity.
     */
    private signalBox(node: unknown, memo: Map<object, number>): number | undefined {
        if (node === null || typeof node !== "object") return undefined;
        const key = node as object;
        const seen = memo.get(key);
        if (seen !== undefined) return seen;
        const spec = node as { op?: string; in?: unknown[]; label?: string };
        const op = spec.op ?? "?";
        const isControl = FAUST_CONTROL_OPS.has(op);
        const operands = isControl ? [] : [...(spec.in ?? [])];
        const children = operands.map((o) => this.signalBox(o, memo));
        const bi = this.boxes.length;
        memo.set(key, bi);
        const inlets: Port[] = operands.map((_, i) => ({
            name: String(i),
            dir: "in",
            rate: "audio",
        }));
        this.boxes.push({
            def: isControl ? (spec.label ?? op) : op,
            kind: "faust",
            role: isControl ? "source" : "object",
            ports: [
                ...inlets,
                { name: "", dir: "out", rate: isControl ? "control" : "audio" },
            ],
        });
        operands.forEach((operand, pos) => {
            const child = children[pos];
            const src = child ?? this.addConst(operand);
            this.connect(src, outletFlat(this.boxes[src]!), bi, pos);
        });
        return bi;
    }

    // ---- the GUI view + the SynthDef round trip ----

    /**
     * The patch as the `patch` widget draws it — boxes with split
     * inlets/outlets and flat cord quadruples (see {@link patchToWidget}), the
     * same schema level 1 uses, with init cords dashed.
     */
    toWidget(geometry: Record<number, readonly [number, number]> = {}): PatchWidget {
        return patchToWidget(this.boxes, this.cords, geometry);
    }

    /**
     * Reconstruct the `SynthDef` this patch represents — the inverse of
     * {@link DefPatch.fromSynthdef}. Each box is rebuilt from its cords
     * (following them back to the sources, so a shared box rebuilds once and
     * value boxes resolve to their numbers). Only a UGen-graph patch rebuilds;
     * a Faust patch has no SynthDef.
     */
    toSynthdef(name: string): SynthDef {
        const incoming = new Map<number, Map<number, number>>();
        for (const c of this.cords) {
            const wired = incoming.get(c.to_box) ?? new Map<number, number>();
            wired.set(c.to_port, c.from_box);
            incoming.set(c.to_box, wired);
        }
        const built = new Map<number, Channel>();
        const build = (bi: number): Channel => {
            const done = built.get(bi);
            if (done !== undefined) return done;
            const box = this.boxes[bi]!;
            let node: Channel;
            if (box.kind === "control") {
                const cc = box.control!;
                node = new Control(cc.name, cc.default, {
                    rate: cc.rate,
                    lag: cc.lag,
                    lagDown: cc.lagDown,
                });
            } else if (box.kind === "const") {
                node = box.const!;
            } else if (box.kind === "ugen") {
                const wired = incoming.get(bi) ?? new Map<number, number>();
                const inputs: Channel[] = [];
                for (let pos = 0; pos < outletFlat(box); pos++) {
                    inputs.push(build(wired.get(pos)!));
                }
                const uu = box.ugen!;
                node = new Ugen(uu.kind, inputs, {
                    rate: uu.rate,
                    op: uu.op,
                    label: uu.label,
                    static: uu.static,
                });
            } else {
                throw new Error(
                    "toSynthdef only rebuilds a UGen-graph patch (fromSynthdef)",
                );
            }
            built.set(bi, node);
            return node;
        };
        const roots = this.roots.map((bi) => {
            const node = build(bi);
            // A root is an output UGen by construction; a hand-built patch whose
            // root is a control or a number is not a def, and says so here
            // rather than in the serializer.
            if (!(node instanceof Ugen)) {
                throw new Error(`box ${bi} is not a UGen, so it is no def root`);
            }
            return node;
        });
        return new SynthDef(name, ...roots);
    }
}
