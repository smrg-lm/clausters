// Numbers, printed as the Python client prints them.
//
// The reference is the language: `gen-format-vectors.py` asks Python what
// `f"{x:g}"` and `f"{x:.Nf}"` say for a spread chosen to find a wrong rule --
// exact ties in both parities, values that only look like ties once scaled,
// the edges of `%g`, and a seeded run of ordinary values as doubles and as the
// float32 a figure off the wire is. `src/base/format.ts` has to say the same,
// to the character.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { formatFixed, formatGeneral } from "../src/base/format.ts";

interface Case {
    value: number | "nan" | "inf" | "-inf";
    negativeZero: boolean;
    general: string;
    fixed: Record<string, string>;
}

const cases: Case[] = JSON.parse(
    readFileSync(new URL("./format-vectors.json", import.meta.url), "utf8"),
) as Case[];

/** The float a case names, the three JSON cannot say included. */
function valueOf(entry: Case): number {
    if (entry.value === "nan") return Number.NaN;
    if (entry.value === "inf") return Number.POSITIVE_INFINITY;
    if (entry.value === "-inf") return Number.NEGATIVE_INFINITY;
    return entry.negativeZero ? -0 : entry.value;
}

test("the vectors are there, ties and all", () => {
    assert.ok(cases.length > 700, `only ${cases.length} cases`);
    assert.ok(cases.some((c) => c.value === 4.25), "the tie that started this");
    assert.ok(cases.some((c) => c.negativeZero), "a negative zero");
});

test("a number at fixed decimals is the line Python writes", () => {
    for (const entry of cases) {
        const value = valueOf(entry);
        for (const [digits, line] of Object.entries(entry.fixed)) {
            assert.equal(
                formatFixed(value, Number(digits)), line,
                `${String(entry.value)} at ${digits} decimals`,
            );
        }
    }
});

test("a number in the general format is the line Python writes", () => {
    for (const entry of cases) {
        assert.equal(formatGeneral(valueOf(entry)), entry.general, String(entry.value));
    }
});

test("an exact tie goes to the even digit, which toFixed does not do", () => {
    // The finding, said once in the open: the host language rounds a tie up,
    // the reference client rounds it to even, and the formatters follow the
    // reference.
    assert.equal((4.25).toFixed(1), "4.3");
    assert.equal(formatFixed(4.25, 1), "4.2");
    assert.equal(formatFixed(4.75, 1), "4.8");
    assert.equal((100000.5).toPrecision(6), "100001");
    assert.equal(formatGeneral(100000.5), "100000");
    assert.equal(formatGeneral(100001.5), "100002");
    // And what only looks like a tie is not treated as one.
    assert.equal(formatFixed(0.05, 1), "0.1");
    assert.equal(formatFixed(0.15, 1), "0.1");
});

test("the general format writes an exponent where %g does", () => {
    assert.equal(formatGeneral(1e6), "1e+06");
    assert.equal(formatGeneral(999999.5), "1e+06", "a rounding that carries");
    assert.equal(formatGeneral(999999.0), "999999");
    assert.equal(formatGeneral(0.0001), "0.0001");
    assert.equal(formatGeneral(0.00001), "1e-05");
    assert.equal(formatGeneral(48000), "48000");
    assert.equal(formatGeneral(-0), "-0");
});
