// GraphDef: a named node-graph "program" ready for `/def_send graph` (mirrors
// `clausters/defs/graphdef.py`, emitting the same JSON `GraphDefSpec`).
//
// Where `SynthDef` and `FaustDef` each describe a *single* synthesis node, a
// `GraphDef` describes a whole **configuration of member nodes wired by
// buses** -- an effect chain, a mixer, a layered instrument -- that the server
// stores and instantiates as one unit. It exposes a **named parameter
// surface**: ports that map to inner member controls, so the running instance
// is driven through the port names, never the private member node ids.
//
// ```ts
// const g = new GraphDef("chain");
// const mix = g.bus("mix");                          // a private audio bus
// const src = g.add("gsrc", { out: mix, level: 1.0 });
// g.add("gsink", { in: mix, out: "OUT" });           // `out` -> hardware
// g.port("gain", [src.control("level")], 0.5);       // port -> the level
// await g.send(server);                       // /def_send graph
//
// const inst = Group.graph("chain", { gain: 0.8 });        // /graph_new
// inst.set({ gain: 0.3 });                   // resolves on the surface
// inst.free();                                 // group + private buses
// ```
//
// The reserved control name `"OUT"` wires a member's output to hardware bus
// 0; any other string value of a member control is the name of an internal
// bus.

/**
 * A reference to an internal GraphDef bus, returned by `GraphDef.bus`. Used
 * as a member control value (it serializes to the bus name).
 */
import type { MsgArg } from "../base/osc.ts";
import type { Server } from "./server/index.ts";
import { resolveServer } from "./wire.ts";
import type { PatchViewOptions, PatchWindow } from "../plot.ts";
import type { ControlInfo } from "./info.ts";

export class GraphBusRef {
    readonly name: string;

    constructor(name: string) {
        this.name = String(name);
    }
}

/**
 * One inner target of a surface port: a member's control with optional
 * linear scaling (`mul`·v + `add`).
 */
export class PortTarget {
    readonly member: number;
    readonly control: string;
    readonly mul: number;
    readonly add: number;

    /** One of the member's own surface ports, when the member is a graph. */
    readonly port?: string;

    constructor(member: number, control: string, mul = 1.0, add = 0.0, port?: string) {
        this.member = member;
        this.control = control;
        this.mul = mul;
        this.add = add;
        this.port = port;
    }

    /**
     * A copy of this target with linear scaling applied to incoming values,
     * e.g. `filt.control("cutoff").scaled(7800, 200)` maps a 0..1 port to
     * 200..8000 Hz.
     */
    scaled(mul = 1.0, add = 0.0): PortTarget {
        return new PortTarget(this.member, this.control, Number(mul), Number(add), this.port);
    }

    /** @internal -- the serialized form inside a surface entry. */
    asSpec(): Record<string, unknown> {
        const d: Record<string, unknown> = { member: this.member };
        if (this.port !== undefined) d.port = this.port;
        else d.control = this.control;
        if (this.mul !== 1.0) d.mul = this.mul;
        if (this.add !== 0.0) d.add = this.add;
        return d;
    }
}

/**
 * A handle to a member added with `GraphDef.add`. Name something on it to get
 * a surface `PortTarget`.
 *
 * **What that name is follows from what the member is**, which is the point: on
 * an ordinary member it is one of its def's controls, and on a nested graph
 * (`kind: "graph"`) it is one of *its* surface ports -- re-exporting a child's
 * interface, written the same way whichever it turns out to be.
 */
export class MemberRef {
    readonly index: number;
    readonly kind: MemberKind;

    constructor(index: number, kind: MemberKind = "def") {
        this.index = index;
        this.kind = kind;
    }

    /** The member's `name` control -- or, for a graph member, its `name` port. */
    control(name: string): PortTarget {
        return this.kind === "graph"
            ? new PortTarget(this.index, "", 1.0, 0.0, String(name))
            : new PortTarget(this.index, String(name));
    }
}

/**
 * What a member is an instance **of**: one node (`"def"`) or a whole nested
 * GraphDef (`"graph"`).
 */
export type MemberKind = "def" | "graph";

/**
 * What a member control may be set to: a number, an internal bus, or a
 * string naming one (`"OUT"` is hardware bus 0).
 */
export type MemberControlValue = number | GraphBusRef | string;

function controlValue(v: MemberControlValue): number | string {
    if (v instanceof GraphBusRef) return v.name;
    if (typeof v === "string") return v;
    return Number(v);
}

export interface MemberSpec {
    def: string;
    kind?: MemberKind;
    controls?: Record<string, number | string>;
    maps?: Record<string, string>;
    voice?: boolean;
    slot?: string;
}

/** The `GraphDefSpec` the server's `/def_send graph` validates. */
export interface GraphDefSpec {
    name: string;
    members: MemberSpec[];
    buses?: { name: string; rate: string; channels: number; external?: boolean }[];
    surface?: Record<string, Record<string, unknown>[]>;
    defaults?: Record<string, number>;
}

/**
 * A named node graph. Build it with `bus`, `add` and `port`, then send it
 * with `server.addGraphDef`.
 */
export class GraphDef {
    readonly name: string;
    private readonly buses_: {
        name: string;
        rate: string;
        channels: number;
        external?: boolean;
    }[] = [];
    private readonly members_: MemberSpec[] = [];
    private readonly surface_: Record<string, Record<string, unknown>[]> = {};
    private readonly defaults_: Record<string, number> = {};

    constructor(name: string) {
        this.name = String(name);
    }

    /**
     * Declares a private internal bus. Each instance allocates its own, so
     * two instances never collide.
     *
     * `external: true` declares a bus **whoever instantiates the graph
     * provides** -- how a nested graph says it does not decide where it goes.
     * The parent names which of its own buses that is when it adds the member
     * (`add`, `kind: "graph"`). A graph instantiated on its own is handed
     * nothing and allocates everything, so one def works standalone and nested.
     */
    bus(
        name: string,
        {
            rate = "audio",
            channels = 1,
            external = false,
        }: { rate?: "audio" | "control"; channels?: number; external?: boolean } = {},
    ): GraphBusRef {
        if (rate !== "audio" && rate !== "control") {
            throw new TypeError("bus rate must be 'audio' or 'control'");
        }
        const spec: { name: string; rate: string; channels: number; external?: boolean } = {
            name: String(name),
            rate,
            channels: Math.trunc(channels),
        };
        if (external) spec.external = true;
        this.buses_.push(spec);
        return new GraphBusRef(name);
    }

    /**
     * Adds a member: an instance of the SynthDef/FaustDef `defname`. Control
     * values may be numbers, a `GraphBusRef` (to wire the control to an
     * internal bus), or `"OUT"` (hardware bus 0). `maps` binds controls to
     * internal *control* buses via `/node_map`.
     *
     * `kind: "graph"` makes the member **another GraphDef** rather than one
     * node: it is instantiated as a subgroup with private buses of its own and
     * freed with its parent, and its `controls` name which of *this* graph's
     * buses each of its external buses is. That is what lets a track hold clips
     * and a clip hold an effect chain without either being a second mechanism --
     * and an effect can itself be a GraphDef.
     *
     * `slot: "name"` makes it a member there is a **changing number of**:
     * instantiated on demand by `Group.addSlot`, once per thing there is one of
     * -- a clip on a track, an effect in a chain, a voice of an instrument.
     * `voice: true` is the slot named `"voice"`, spelled the way it was before
     * slots had names, and it is what a MIDI note spawns.
     */
    add(
        defname: string,
        controls: Record<string, MemberControlValue> = {},
        {
            maps,
            voice = false,
            slot,
            kind = "def",
        }: {
            maps?: Record<string, GraphBusRef | string>;
            voice?: boolean;
            slot?: string;
            kind?: MemberKind;
        } = {},
    ): MemberRef {
        if (kind !== "def" && kind !== "graph") {
            throw new TypeError("member kind must be 'def' or 'graph'");
        }
        const member: MemberSpec = { def: String(defname) };
        if (kind !== "def") member.kind = kind;
        const entries = Object.entries(controls);
        if (entries.length > 0) {
            member.controls = Object.fromEntries(
                entries.map(([k, v]) => [k, controlValue(v)]),
            );
        }
        if (maps && Object.keys(maps).length > 0) {
            member.maps = Object.fromEntries(
                Object.entries(maps).map(([k, v]) => [
                    k,
                    v instanceof GraphBusRef ? v.name : String(v),
                ]),
            );
        }
        if (voice) member.voice = true;
        if (slot !== undefined) member.slot = String(slot);
        const index = this.members_.length;
        this.members_.push(member);
        return new MemberRef(index, kind);
    }

    /**
     * The member specs in add order (read-only copies): each a def name and
     * its control wiring.
     */
    members(): MemberSpec[] {
        return this.members_.map((m) => ({ ...m }));
    }

    /**
     * Defines a surface port mapping `name` to one or more member controls
     * (each a `PortTarget`, optionally `.scaled(...)`). `defaultValue` is
     * applied at instantiation unless overridden.
     */
    port(name: string, targets: readonly PortTarget[], defaultValue?: number): void {
        if (targets.length === 0) {
            throw new TypeError(`surface port '${name}' needs at least one target`);
        }
        this.surface_[String(name)] = targets.map((t) => t.asSpec());
        if (defaultValue !== undefined) {
            this.defaults_[String(name)] = Number(defaultValue);
        }
    }

    /**
     * This def's surface ports as `ControlInfo` entries, in declaration order --
     * the shape all three def families answer with, so a GUI reads one of them
     * the same way. `targets` names what each port drives inside. A port
     * declares **no range**: like a control it is a name the server takes any
     * float for, and the range a knob is drawn over belongs to the knob.
     */
    controls(): ControlInfo[] {
        const out: ControlInfo[] = [];
        for (const [name, targets] of Object.entries(this.surface_)) {
            const info: ControlInfo = {
                name,
                default: this.defaults_[name] ?? 0.0,
                rate: "kr",
                targets: targets.map((t) => ({
                    member: t.member as number,
                    control: t.control as string,
                    mul: (t.mul as number | undefined) ?? 1.0,
                    add: (t.add as number | undefined) ?? 0.0,
                })),
            };
            out.push(info);
        }
        return out;
    }

    /** One surface port by name, as a `ControlInfo` -- `gd.control("mix")`. */
    control(name: string): ControlInfo {
        for (const info of this.controls()) {
            if (info.name === name) return info;
        }
        const has = Object.keys(this.surface_).join(", ") || "none";
        throw new Error(`'${this.name}' declares no surface port '${name}' (it has: ${has})`);
    }

    /** The `GraphDefSpec` object the server's `/def_send graph` validates. */
    spec(): GraphDefSpec {
        if (this.members_.length === 0) {
            throw new TypeError("a GraphDef needs at least one member");
        }
        const spec: GraphDefSpec = { name: this.name, members: this.members_ };
        if (this.buses_.length > 0) spec.buses = this.buses_;
        if (Object.keys(this.surface_).length > 0) spec.surface = this.surface_;
        if (Object.keys(this.defaults_).length > 0) spec.defaults = this.defaults_;
        return spec;
    }

    /**
     * Sends this def to the server via `/def_send graph` and returns its name.
     *
     * Loading a GraphDef is cheap on the server (no JIT -- it only validates and
     * references the member defs), but it is still asynchronous, so the same
     * barrier discipline applies.
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
            target.sendMsg("/def_send", "graph", ...payload);
            return this.name;
        }
        await target.command("/def_send", ["graph", ...payload], timeout);
        return this.name;
    }

    /**
     * Open this def's **structure** as a directed `patch` view in its own
     * window on the ambient GUI host -- the level-1 patcher drawn from the def
     * itself (the inverse of building it), the host laying the boxes out as an
     * inverted tree. One window per call, the `plot` posture; this shows the
     * def's *structure*, where `plot(this)` renders its *sound*.
     *
     * `defs` maps a member's def name to the `SynthDef` it was built from, so a
     * box's ports are typed (a control feeding an `In` is an inlet, one feeding
     * an `Out` an outlet); a member whose def is not resolvable draws port-less
     * (no cords). `label` captions the patch panel (defaults to `"graphdef"` --
     * the panel names *what* is drawn, not the def's name); `host` is an
     * explicit `GuiHost`, absent resolves the ambient one. Resolves with a
     * `PatchWindow` (`close()`).
     */
    async plotDef(
        defs: Record<string, unknown> = {},
        options: PatchViewOptions = {},
    ): Promise<PatchWindow> {
        const { GraphPatch } = await import("./patch.ts");
        const { openPatchView } = await import("../plot.ts");
        return openPatchView(GraphPatch.fromGraphdef(this, defs), {
            label: "graphdef",
            title: this.name,
            ...options,
        });
    }

    /**
     * The def serialized to text -- the `/def_send graph` wire payload. Useful to
     * inspect the composition before sending it.
     */
    dumpDef(): string {
        return JSON.stringify(this.spec());
    }
}
