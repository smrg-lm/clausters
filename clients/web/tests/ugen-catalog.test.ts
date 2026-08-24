// The catalog contrast: the TypeScript builders against the server's own
// registry (`ugen-vectors.json`, generated from `/ugen_query` by
// `gen-ugen-vectors.py`).
//
// Every builder here is a hand-written mirror of a row the server owns, and
// nothing else compares the two: a builder assembles its wire list explicitly,
// so parameters in the wrong order still emit a valid def that compiles, type-
// checks and sounds right. What drifts is the *signature* — the name a caller
// reads, and the label a patcher's Def view puts on an inlet, which it takes
// from the builder's parameter names by position. This is the test that sees
// it, and it is the port of
// `clients/python/tests/test_session.py::test_ugen_catalog_matches_the_python_callables`,
// written for a language that has no `inspect`: parameter names and defaults
// are read out of `Function.prototype.toString()`, which under node's type
// stripping still carries both.
//
// Three declared lists say where a builder legitimately departs from the wire.
// They are asserted **exact** — every entry must still name a live kind with a
// live builder — so a renamed or removed UGen cannot leave a stale excuse
// behind that quietly drops a row from the contrast.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";
import test from "node:test";

import * as U from "../src/defs/ugens/index.ts";

const HERE = fileURLToPath(new URL(".", import.meta.url));

interface WireInput {
    name: string;
    default: number;
}
interface WireKind {
    name: string;
    arity: number;
    inputs: WireInput[];
}

const catalog: WireKind[] = JSON.parse(
    readFileSync(`${HERE}ugen-vectors.json`, "utf8"),
).kinds;

/** Kinds with no builder of their own, because another surface builds them. */
const NO_BUILDER: Record<string, string> = {
    Add: "built by the `+` operator on a graph node",
    Sub: "built by the `-` operator on a graph node",
    Mul: "built by the `*` operator on a graph node",
    Div: "built by the `/` operator on a graph node",
    BinaryOpUGen: "built by every other binary selector on a graph node",
    UnaryOpUGen: "built by every unary selector method on a graph node",
};

/**
 * Kinds whose parameters are the wire's, plus or minus a declared tail. The
 * common prefix is still contrasted — only the tail is excused.
 */
const TRAILING: Record<string, string> = {
    // Static configuration, not a signal: it sizes the private line.
    DelayN: "maxDelay is a static field",
    DelayL: "maxDelay is a static field",
    DelayC: "maxDelay is a static field",
    CombN: "maxDelay is a static field",
    CombL: "maxDelay is a static field",
    CombC: "maxDelay is a static field",
    AllpassN: "maxDelay is a static field",
    AllpassL: "maxDelay is a static field",
    AllpassC: "maxDelay is a static field",
    // The TS idiom for a run of optional inputs or static fields.
    FFT: "the static fields come in a trailing options object",
    Conv: "the static fields come in a trailing options object",
    RecordBuf: "the optional inputs come in a trailing options object",
    // A UGen has one output and a panner has two, so the builder emits one row
    // per channel and fills the trailing `chan` itself. The caller never sees
    // it: the signature is the wire's minus its last input, by construction.
    Pan2: "chan is filled by the builder, once per output channel",
    LinPan2: "chan is filled by the builder, once per output channel",
    Balance2: "chan is filled by the builder, once per output channel",
    Rotate2: "chan is filled by the builder, once per output channel",
    MidSide: "chan is filled by the builder, once per output channel",
    StereoWidth: "chan is filled by the builder, once per output channel",
};

/** Kinds whose parameter list cannot line up at all, and why. */
const SIGNATURE_DIFFERS: Record<string, string> = {
    EnvGen: "the Env leads in TS, its flattened array trails on the wire",
    SendReply: "the value list leads; replyId and cmd are an options object",
    Dseq: "repeats leads on the wire; the value list leads here",
    Drand: "repeats leads on the wire; the value list leads here",
    Dxrand: "repeats leads on the wire; the value list leads here",
    Dshuf: "repeats leads on the wire; the value list leads here",
    Select: "the value list is a variadic tail",
    SelectX: "the value list is a variadic tail",
    Dswitch1: "the value list is a variadic tail",
    Poll: "the static label sits between two wire inputs",
    DiskIn: "path/loop are static fields; only chan is an input",
    DiskOut: "path/format are static fields; only signal is an input",
    PV_Kernel: "mag/phase are static fields; the wire takes chain + params",
    PanAz: "numchans leads here and trails on the wire, beside the filled chan",
};

/**
 * Wire input → the parameter that carries it, where the client spells it
 * differently on purpose. The resonant filters take `rq` and `q` as one
 * options object, so the caller may give either; Python takes the same pair as
 * `rq=` plus a keyword-only `q=`.
 */
const ALIASES: Record<string, Record<string, string>> = {
    RLPF: { rq: "res" },
    RHPF: { rq: "res" },
    BPF: { rq: "res" },
    BRF: { rq: "res" },
    Resonz: { rq: "res" },
    Svf: { rq: "res" },
};

const camel = (s: string) => s.replace(/_([a-z])/g, (_m, c) => c.toUpperCase());

interface Param {
    name: string;
    default?: string;
    /** An options object: everything it carries is named, not positional. */
    options?: boolean;
}

/**
 * The parameter list of an arrow-function builder, read from its source.
 * Node strips type annotations to whitespace, so the names and the default
 * expressions survive; commas inside a default (an object, a call) are skipped
 * by tracking bracket depth.
 */
function params(fn: Function): Param[] {
    const src = fn.toString();
    const arrow = src.indexOf("=>");
    const open = src.indexOf("(");
    // A single parameter may have lost its parentheses (`bufnum=>new Ugen(…)`):
    // the transpiler that strips the types is free to drop them, and then the
    // first `(` in the source belongs to the **body**, whose arguments would be
    // read as a signature. Everything with parentheses falls through below.
    if (arrow >= 0 && (open < 0 || open > arrow)) {
        const head = src.slice(0, arrow).trim();
        return head ? [{ name: head }] : [];
    }
    assert.ok(open >= 0, `no parameter list in ${src.slice(0, 60)}`);
    let depth = 0;
    let end = -1;
    for (let i = open; i < src.length; i++) {
        const c = src[i];
        if ("([{".includes(c)) depth++;
        else if (")]}".includes(c)) {
            depth--;
            if (depth === 0) {
                end = i;
                break;
            }
        }
    }
    assert.ok(end > open, "unbalanced parameter list");
    const inner = src.slice(open + 1, end);
    const pieces: string[] = [];
    let d = 0;
    let start = 0;
    for (let i = 0; i <= inner.length; i++) {
        const c = inner[i];
        if (i === inner.length || (c === "," && d === 0)) {
            const piece = inner.slice(start, i).trim().replace(/\s+/g, " ");
            if (piece) pieces.push(piece);
            start = i + 1;
        } else if ("([{".includes(c)) d++;
        else if (")]}".includes(c)) d--;
    }
    return pieces.map((p) => {
        // A destructured parameter is an options object whatever is inside it.
        if (p.startsWith("{")) return { name: "{…}", options: true };
        const eq = p.indexOf("=");
        if (eq < 0) return { name: p.trim() };
        const def = p.slice(eq + 1).trim();
        return { name: p.slice(0, eq).trim(), default: def, options: def === "{}" };
    });
}

/** kind → builder, read from the `new Ugen("Kind"` literal in its body. */
function buildersByKind(): Map<string, [string, Function]> {
    const out = new Map<string, [string, Function]>();
    for (const [name, value] of Object.entries(U)) {
        if (typeof value !== "function") continue;
        const src = value.toString();
        if (/^class\s/.test(src)) continue;
        const m = /new Ugen\(\s*"([A-Za-z_0-9]+)"/.exec(src);
        if (m && !out.has(m[1])) out.set(m[1], [name, value as Function]);
    }
    return out;
}

test("the TS builders match the server's UGen catalog", () => {
    assert.ok(catalog.length > 100, "the vector must hold a whole catalog");
    const byKind = buildersByKind();
    let contrasted = 0;

    for (const kind of catalog) {
        const hit = byKind.get(kind.name);
        if (!hit) {
            assert.ok(
                kind.name in NO_BUILDER,
                `${kind.name} has no TypeScript builder and is not declared as ` +
                    `built another way — the packages move together, so a kind ` +
                    `the server grew needs one here too`,
            );
            continue;
        }
        if (kind.name in SIGNATURE_DIFFERS) continue;

        const [fname, fn] = hit;
        const ps = params(fn);
        const alias = ALIASES[kind.name] ?? {};
        const want = kind.inputs.map((i) => alias[i.name] ?? camel(i.name));
        // A parameter that shares its name with something else in its module
        // reaches here **renamed with a numeric suffix** (`trig` -> `trig2`):
        // these signatures are read off the transpiled function, and keeping
        // names unique is the transpiler's business, not a drift. Only that
        // exact shape is forgiven, and only against the slot it lines up with.
        const got = ps.map((p, k) => {
            const w = want[k];
            return w !== undefined && new RegExp(`^${w}\\d+$`).test(p.name)
                ? w
                : p.name;
        });

        const declared = kind.name in TRAILING;
        // An options object carries named slots, so the positional contrast
        // ends where one begins — but only for a kind that declares it, so an
        // undeclared `{}` cannot quietly truncate the check.
        const optsAt = ps.findIndex((p) => p.options);
        const cut = declared && optsAt >= 0 ? optsAt : Infinity;
        const n = Math.min(want.length, got.length, cut);
        if (want.length !== got.length || n < want.length) {
            assert.ok(
                declared,
                `${kind.name} (${fname}): ${got.length} parameters against ` +
                    `${want.length} wire inputs, and no declared tail`,
            );
        }
        assert.deepEqual(
            got.slice(0, n),
            want.slice(0, n),
            `${kind.name} (${fname}): parameters ${JSON.stringify(got)} against ` +
                `wire inputs ${JSON.stringify(want)}`,
        );

        // A numeric default must be the server's own, at f32 — the value the
        // client sends when the caller leaves the slot alone.
        for (let k = 0; k < n; k++) {
            const lit = ps[k].default;
            if (lit === undefined) continue;
            const num = Number(lit);
            if (!Number.isFinite(num)) continue; // `{}`, an enum member, a call
            assert.equal(
                Math.fround(num),
                kind.inputs[k].default,
                `${kind.name}.${kind.inputs[k].name}: TS default ${lit} against ` +
                    `server default ${kind.inputs[k].default}`,
            );
        }
        contrasted++;
    }

    assert.ok(contrasted > 100, `only contrasted ${contrasted} kinds`);
});

test("every declared exception still names a live kind and builder", () => {
    const byKind = buildersByKind();
    const kinds = new Set(catalog.map((k) => k.name));
    for (const [name, why] of Object.entries({ ...TRAILING, ...SIGNATURE_DIFFERS })) {
        assert.ok(kinds.has(name), `${name} is no longer in the catalog (${why})`);
        assert.ok(byKind.has(name), `${name} has no builder any more (${why})`);
    }
    for (const name of Object.keys(NO_BUILDER)) {
        assert.ok(kinds.has(name), `${name} is no longer in the catalog`);
        assert.ok(!byKind.has(name), `${name} has a builder now — undeclare it`);
    }
    for (const name of Object.keys(ALIASES)) {
        assert.ok(kinds.has(name), `${name} is no longer in the catalog`);
        assert.ok(byKind.has(name), `${name} has no builder any more`);
    }
});
