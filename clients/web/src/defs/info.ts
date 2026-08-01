// What the server reports about its resources: the records and their parsers
// (mirrors `clausters/defs/info.py`).
//
// One rule holds for every resource the server owns — a node, a buffer, a def:
//
// * an **Info** is a frozen-in-time record of **one** instance, identified by
//   itself (`id`, `bufnum`, `name`), carrying no server and no commands,
// * the **instance** asks about itself (`Node.info`, `Buffer.info`) and gets
//   exactly that record,
// * the **server** asks about every instance of a type (`Server.queryBuffers`,
//   `Server.queryDefs`, `Server.queryTree`) and answers with a structure of
//   those same records — an array, or a `Tree`,
// * and a resource that is **not there** is a state, not an error: the record
//   comes back with `exists = false` rather than throwing, so one dead id
//   never aborts a query about the others.
//
// The records live here rather than next to one resource because both ends
// need them: `Server` builds them from the catalog replies, and `Node`/
// `Buffer` from their own. A bus has no record at all — the server does not
// model one (its index and width are the client allocator's invention), which
// is why there is no `BusInfo`.

/** Any decoded OSC argument — what a reply parser walks. */
export type ReplyArgs = readonly (number | string | boolean | null | Uint8Array)[];

/**
 * A control identifier in a reply is a name string, or an int index when the
 * server could not resolve a name.
 */
const controlKey = (key: ReplyArgs[number]): string =>
    typeof key === "string" ? key : String(Number(key));

/** One inner target of a graph def's surface port, as `/def_query.reply` reports it. */
export interface PortTargetInfo {
    member: number;
    control: string;
    mul: number;
    add: number;
}

/** One entry of a def's control surface, as `queryDefs` reports it. */
export interface ControlInfo {
    name: string;
    default: number;
    /** The control type the def declared: `"kr"`, `"tr"` or `"ir"`. */
    rate: string;
    /** A Faust parameter's declared range (its UI widget's). */
    min?: number;
    max?: number;
    step?: number;
    /** A graph def's port: the member controls it drives. */
    targets?: PortTargetInfo[];
}

/**
 * A def the server holds: its name, its family and its control surface. A def
 * it does not hold comes back with `exists` false and an empty family.
 */
export interface DefInfo {
    name: string;
    /** `"synth"`, `"faust"` or `"graph"` — empty when the name is unknown. */
    family: string;
    controls: ControlInfo[];
    exists: boolean;
}

/**
 * A buffer the server holds: its slot and its shape. `sampleRate` is 0 while
 * unknown; a slot with nothing in it comes back with `exists` false.
 */
export interface BufferInfo {
    bufnum: number;
    frames: number;
    channels: number;
    sampleRate: number;
    exists: boolean;
}

/**
 * One named input slot of a UGen, in **wire order**.
 *
 * The wire is positional — a def lists input values, it never names them — so
 * this is what a palette labels an inlet with, and `default` is what to offer
 * when the user leaves the slot alone.
 */
export interface UgenInput {
    name: string;
    default: number;
}

/**
 * A UGen kind as `queryUgens` reports it, straight from the server's catalog.
 *
 * This is a **type**, not an instantiated resource: there is no handle for it
 * and so no `exists`. `arity` is the input count, or `-1` for a variadic kind
 * — whose `inputs` then name only the fixed head (`EnvGen`'s five before the
 * envelope array). `rates` are the rates the kind may be instantiated at and
 * `defaultRate` the one a def gets by omitting `rate`. `exec`, `bus`,
 * `opFamily` and `spectral` expose the compiler's own classification; the ones
 * that do not apply are empty strings.
 */
export interface UgenInfo {
    name: string;
    arity: number;
    defaultRate: string;
    rates: string[];
    exec: string;
    bus: string;
    needsPath: boolean;
    opFamily: string;
    spectral: string;
    inputs: UgenInput[];
}

/** One live `/node_map`/`/node_mapAudio` binding: the control follows the bus. */
export interface NodeMap {
    control: number;
    bus: number;
    audio: boolean;
}

/**
 * A node the server holds — a synth or a group — at one moment.
 *
 * Unlike a buffer's, this record goes stale on its own: an envelope runs, a
 * mapped control follows its bus, a `doneAction` frees the node. It is a
 * photograph, which is why no handle keeps one.
 *
 * A **group** carries `head`/`tail` (`-1` when empty) and its children are the
 * `Tree`'s business; a **synth** carries `defname`, its `controls` by name,
 * its `maps` and the `reads`/`writes` bus lists the server infers (`"-"` when
 * none). A node that is gone comes back with `exists` false and nothing else
 * filled in.
 */
export interface NodeInfo {
    id: number;
    parent: number;
    prev: number;
    next: number;
    isGroup: boolean;
    exists: boolean;
    head: number;
    tail: number;
    /** A group's `/group_name`, `""` when it has none. Never a synth's. */
    name: string;
    defname: string;
    controls: Record<string, number>;
    maps: NodeMap[];
    reads: string;
    writes: string;
}

/**
 * The node tree from one group down: a `NodeInfo` plus its children.
 *
 * The structure is the only thing the tree adds — every entry is the same
 * record `Node.info` returns, so reading a tree needs no follow-up query. The
 * queried group is the root, and its own `parent`/`prev`/`next` are unknown
 * (`-1`): the reply starts at it, so it has no siblings to report.
 *
 * Where the Python client splits `repr` from `str`, this one splits the
 * object from its `toString()`: logging a `Tree` shows the data, `String(tree)`
 * (or a template literal) draws it indented.
 */
export class Tree {
    readonly info: NodeInfo;
    readonly children: Tree[];

    constructor(info: NodeInfo, children: Tree[] = []) {
        this.info = info;
        this.children = children;
    }

    /** The node this subtree is rooted at. */
    get id(): number {
        return this.info.id;
    }

    /** Yields every `NodeInfo` in the tree, depth-first, this one first. */
    *walk(): Generator<NodeInfo> {
        yield this.info;
        for (const child of this.children) yield* child.walk();
    }

    /** The subtree rooted at `id`, or `undefined`. */
    find(node: number | { id: number }): Tree | undefined {
        const wanted = typeof node === "number" ? node : node.id;
        for (const sub of this.subtrees()) {
            if (sub.info.id === wanted) return sub;
        }
        return undefined;
    }

    private *subtrees(): Generator<Tree> {
        yield this;
        for (const child of this.children) yield* child.subtrees();
    }

    /** The tree drawn indented, one line per node. */
    toString(): string {
        return this.lines(0).join("\n");
    }

    private lines(depth: number): string[] {
        const pad = "  ".repeat(depth);
        const info = this.info;
        if (info.isGroup) {
            const named = info.name ? ` "${info.name}"` : "";
            const head = `${pad}group ${info.id}${named}${this.children.length ? "" : " (empty)"}`;
            return [head, ...this.children.flatMap((c) => c.lines(depth + 1))];
        }
        const mapped = new Map(info.maps.map((m) => [m.control, m]));
        const parts = Object.entries(info.controls).map(([name, value], i) => {
            const m = mapped.get(i);
            return m ? `${name}<-${m.audio ? "a" : "c"}${m.bus}` : `${name}=${format(value)}`;
        });
        return [`${pad}${info.id} ${info.defname}${parts.length ? "  " + parts.join(" ") : ""}`];
    }
}

/** Python's `%g` for the control values a drawn tree shows. */
function format(value: number): string {
    return Number(value.toPrecision(6)).toString();
}

/** An empty record, which the parsers fill in. */
function emptyNode(id: number): NodeInfo {
    return {
        id,
        parent: -1,
        prev: -1,
        next: -1,
        isGroup: false,
        exists: true,
        head: -1,
        tail: -1,
        name: "",
        defname: "",
        controls: {},
        maps: [],
        reads: "-",
        writes: "-",
    };
}

/** `numControls` then (name|index, value) pairs. */
function parseControls(args: ReplyArgs, i: number): [Record<string, number>, number] {
    const count = Number(args[i++]);
    const controls: Record<string, number> = {};
    for (let c = 0; c < count; c++) {
        controls[controlKey(args[i++])] = Number(args[i++]);
    }
    return [controls, i];
}

/** `numMaps` then (control, bus, audio) triples. */
function parseMaps(args: ReplyArgs, i: number): [NodeMap[], number] {
    const count = Number(args[i++]);
    const maps: NodeMap[] = [];
    for (let m = 0; m < count; m++) {
        maps.push({
            control: Number(args[i++]),
            bus: Number(args[i++]),
            audio: Number(args[i++]) !== 0,
        });
    }
    return [maps, i];
}

/**
 * One `/node_query.reply` reply. `isGroup` −1 is how the server says the node is not
 * there.
 */
export function parseNodeInfo(args: ReplyArgs): NodeInfo {
    const info = emptyNode(Number(args[0]));
    const kind = Number(args[4]);
    if (kind < 0) {
        info.exists = false;
        return info;
    }
    info.parent = Number(args[1]);
    info.prev = Number(args[2]);
    info.next = Number(args[3]);
    if (kind === 1) {
        info.isGroup = true;
        info.head = Number(args[5]);
        info.tail = Number(args[6]);
        info.name = String(args[7]);
        return info;
    }
    info.defname = String(args[5]);
    let i: number;
    [info.controls, i] = parseControls(args, 6);
    [info.maps, i] = parseMaps(args, i);
    info.reads = String(args[i]);
    info.writes = String(args[i + 1]);
    return info;
}

/**
 * Recursively parses `count` entries of a `/group_queryTree.reply` starting at
 * `i`; returns the subtrees and the next index. A synth has child-count −1.
 * Every entry is `id, childCount, name` — the group's `/group_name` or the
 * synth's def name.
 *
 * The wire gives the nesting; the siblings and a group's head/tail follow from
 * it, so each entry comes out as complete as `Node.info` would.
 */
function parseTreeNodes(
    args: ReplyArgs,
    i: number,
    count: number,
    detail: number,
    parent: number,
): [Tree[], number] {
    const out: Tree[] = [];
    for (let n = 0; n < count; n++) {
        const id = Number(args[i++]);
        const childCount = Number(args[i++]);
        if (childCount === -1) {
            const info = emptyNode(id);
            info.parent = parent;
            info.defname = String(args[i++]);
            if (detail >= 1) [info.controls, i] = parseControls(args, i);
            if (detail >= 2) {
                [info.maps, i] = parseMaps(args, i);
                info.reads = String(args[i++]);
                info.writes = String(args[i++]);
            }
            out.push(new Tree(info));
        } else {
            const name = String(args[i++]);
            const [kids, next] = parseTreeNodes(args, i, childCount, detail, id);
            i = next;
            const info = emptyNode(id);
            info.parent = parent;
            info.isGroup = true;
            info.name = name;
            info.head = kids.length ? kids[0]!.info.id : -1;
            info.tail = kids.length ? kids[kids.length - 1]!.info.id : -1;
            out.push(new Tree(info, kids));
        }
    }
    out.forEach((sub, pos) => {
        sub.info.prev = pos ? out[pos - 1]!.info.id : -1;
        sub.info.next = pos + 1 < out.length ? out[pos + 1]!.info.id : -1;
    });
    return [out, i];
}

/** `/group_queryTree.reply` → a `Tree` of `NodeInfo`. */
export function parseQueryTree(args: ReplyArgs): Tree {
    const detail = Number(args[0]);
    const rootId = Number(args[1]);
    const [children] = parseTreeNodes(args, 4, Number(args[2]), detail, rootId);
    const root = emptyNode(rootId);
    root.isGroup = true;
    root.name = String(args[3]);
    root.head = children.length ? children[0]!.info.id : -1;
    root.tail = children.length ? children[children.length - 1]!.info.id : -1;
    return new Tree(root, children);
}

/**
 * One `/def_query.reply` reply: `name, family, numControls` then per control `name,
 * default, rate` — plus `min, max, step` for a Faust parameter, or
 * `numTargets` and the target tuples for a graph port.
 */
export function parseDefInfo(args: ReplyArgs): DefInfo {
    const name = String(args[0]);
    const family = String(args[1]);
    const count = Number(args[2]);
    const controls: ControlInfo[] = [];
    let i = 3;
    for (let c = 0; c < count; c++) {
        const info: ControlInfo = {
            name: String(args[i]),
            default: Number(args[i + 1]),
            rate: String(args[i + 2]),
        };
        i += 3;
        if (family === "faust") {
            info.min = Number(args[i]);
            info.max = Number(args[i + 1]);
            info.step = Number(args[i + 2]);
            i += 3;
        } else if (family === "graph") {
            const numTargets = Number(args[i++]);
            const targets: PortTargetInfo[] = [];
            for (let t = 0; t < numTargets; t++) {
                targets.push({
                    member: Number(args[i]),
                    control: String(args[i + 1]),
                    mul: Number(args[i + 2]),
                    add: Number(args[i + 3]),
                });
                i += 4;
            }
            info.targets = targets;
        }
        controls.push(info);
    }
    return { name, family, controls, exists: family !== "" };
}

/**
 * A `/buffer_query.reply` reply, four args per buffer. `frames` −1 marks a slot with
 * nothing in it (the argument-less listing form never reports one).
 */
export function parseBufferList(args: ReplyArgs): BufferInfo[] {
    const out: BufferInfo[] = [];
    for (let i = 0; i + 3 < args.length; i += 4) {
        const frames = Number(args[i + 1]);
        out.push({
            bufnum: Number(args[i]),
            frames: Math.max(frames, 0),
            channels: Number(args[i + 2]),
            sampleRate: Number(args[i + 3]),
            exists: frames >= 0,
        });
    }
    return out;
}

/**
 * One `/ugen_query.reply` reply: ten fixed fields then `(name, default)` per named
 * input.
 */
export function parseUgenInfo(args: ReplyArgs): UgenInfo {
    const count = Number(args[9]);
    const inputs: UgenInput[] = [];
    for (let k = 0; k < count; k++) {
        inputs.push({
            name: String(args[10 + 2 * k]),
            default: Number(args[11 + 2 * k]),
        });
    }
    return {
        name: String(args[0]),
        arity: Number(args[1]),
        defaultRate: String(args[2]),
        rates: String(args[3]).split(",").filter((r) => r),
        exec: String(args[4]),
        bus: String(args[5]),
        needsPath: Number(args[6]) !== 0,
        opFamily: String(args[7]),
        spectral: String(args[8]),
        inputs,
    };
}
