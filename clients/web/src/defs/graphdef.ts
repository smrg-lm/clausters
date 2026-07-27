// GraphDef: a named node-graph "program" ready for `/d_graph` (mirrors
// `clausters/defs/graphdef.py`, emitting the same JSON `GraphDefSpec`).
//
// Where `SynthDef` and `FaustDef` each describe a *single* synthesis node, a
// `GraphDef` describes a whole **configuration of member nodes wired by
// buses** — an effect chain, a mixer, a layered instrument — that the server
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
// await server.addGraphDef(g);                       // /d_graph
//
// const inst = server.graph("chain", { gain: 0.8 }); // /graph_new
// server.set(inst, { gain: 0.3 });                   // resolves on the surface
// server.free(inst);                                 // group + private buses
// ```
//
// The reserved control name `"OUT"` wires a member's output to hardware bus
// 0; any other string value of a member control is the name of an internal
// bus.

/// A reference to an internal GraphDef bus, returned by `GraphDef.bus`. Used
/// as a member control value (it serializes to the bus name).
export class GraphBusRef {
    readonly name: string;

    constructor(name: string) {
        this.name = String(name);
    }
}

/// One inner target of a surface port: a member's control with optional
/// linear scaling (`mul`·v + `add`).
export class PortTarget {
    readonly member: number;
    readonly control: string;
    readonly mul: number;
    readonly add: number;

    constructor(member: number, control: string, mul = 1.0, add = 0.0) {
        this.member = member;
        this.control = control;
        this.mul = mul;
        this.add = add;
    }

    /// A copy of this target with linear scaling applied to incoming values,
    /// e.g. `filt.control("cutoff").scaled(7800, 200)` maps a 0..1 port to
    /// 200..8000 Hz.
    scaled(mul = 1.0, add = 0.0): PortTarget {
        return new PortTarget(this.member, this.control, Number(mul), Number(add));
    }

    /// @internal — the serialized form inside a surface entry.
    asSpec(): Record<string, unknown> {
        const d: Record<string, unknown> = { member: this.member, control: this.control };
        if (this.mul !== 1.0) d.mul = this.mul;
        if (this.add !== 0.0) d.add = this.add;
        return d;
    }
}

/// A handle to a member added with `GraphDef.add`. Name a control on it to
/// get a surface `PortTarget`.
export class MemberRef {
    readonly index: number;

    constructor(index: number) {
        this.index = index;
    }

    /// The member's `name` control, as a port target.
    control(name: string): PortTarget {
        return new PortTarget(this.index, String(name));
    }
}

/// What a member control may be set to: a number, an internal bus, or a
/// string naming one (`"OUT"` is hardware bus 0).
export type MemberControlValue = number | GraphBusRef | string;

function controlValue(v: MemberControlValue): number | string {
    if (v instanceof GraphBusRef) return v.name;
    if (typeof v === "string") return v;
    return Number(v);
}

export interface MemberSpec {
    def: string;
    controls?: Record<string, number | string>;
    maps?: Record<string, string>;
    voice?: boolean;
}

/// The `GraphDefSpec` the server's `/d_graph` validates.
export interface GraphDefSpec {
    name: string;
    members: MemberSpec[];
    buses?: { name: string; rate: string; channels: number }[];
    surface?: Record<string, Record<string, unknown>[]>;
    defaults?: Record<string, number>;
}

/// A named node graph. Build it with `bus`, `add` and `port`, then send it
/// with `server.addGraphDef`.
export class GraphDef {
    readonly name: string;
    private readonly buses_: { name: string; rate: string; channels: number }[] = [];
    private readonly members_: MemberSpec[] = [];
    private readonly surface_: Record<string, Record<string, unknown>[]> = {};
    private readonly defaults_: Record<string, number> = {};

    constructor(name: string) {
        this.name = String(name);
    }

    /// Declares a private internal bus. Each instance allocates its own, so
    /// two instances never collide.
    bus(
        name: string,
        { rate = "audio", channels = 1 }: { rate?: "audio" | "control"; channels?: number } = {},
    ): GraphBusRef {
        if (rate !== "audio" && rate !== "control") {
            throw new TypeError("bus rate must be 'audio' or 'control'");
        }
        this.buses_.push({ name: String(name), rate, channels: Math.trunc(channels) });
        return new GraphBusRef(name);
    }

    /// Adds a member: an instance of the SynthDef/FaustDef `defname`. Control
    /// values may be numbers, a `GraphBusRef` (to wire the control to an
    /// internal bus), or `"OUT"` (hardware bus 0). `maps` binds controls to
    /// internal *control* buses via `/n_map`. `voice: true` marks a
    /// **per-voice** member: instantiated once per `Server.graphVoice` (or
    /// MIDI note) instead of at instantiation — the per-note part of a
    /// polyphonic instrument.
    add(
        defname: string,
        controls: Record<string, MemberControlValue> = {},
        {
            maps,
            voice = false,
        }: { maps?: Record<string, GraphBusRef | string>; voice?: boolean } = {},
    ): MemberRef {
        const member: MemberSpec = { def: String(defname) };
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
        const index = this.members_.length;
        this.members_.push(member);
        return new MemberRef(index);
    }

    /// The member specs in add order (read-only copies): each a def name and
    /// its control wiring.
    members(): MemberSpec[] {
        return this.members_.map((m) => ({ ...m }));
    }

    /// Defines a surface port mapping `name` to one or more member controls
    /// (each a `PortTarget`, optionally `.scaled(...)`). `defaultValue` is
    /// applied at instantiation unless overridden.
    port(name: string, targets: readonly PortTarget[], defaultValue?: number): void {
        if (targets.length === 0) {
            throw new TypeError(`surface port '${name}' needs at least one target`);
        }
        this.surface_[String(name)] = targets.map((t) => t.asSpec());
        if (defaultValue !== undefined) {
            this.defaults_[String(name)] = Number(defaultValue);
        }
    }

    /// The `GraphDefSpec` object the server's `/d_graph` validates.
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

    /// The def serialized to text — the `/d_graph` wire payload. Useful to
    /// inspect the composition before sending it.
    dumpDef(): string {
        return JSON.stringify(this.spec());
    }
}
