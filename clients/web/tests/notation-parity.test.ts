// The engraved page against the Python client's, on the shared vectors.
//
// This is the check the notation layer exists for. A window and a page engrave
// with **one verovio** — the same pinned sources and the same importer options,
// one built natively and one by Emscripten — configure it through one shared
// `engraveOptions`, walk the SVG with one shared core, and edit through one
// shared state machine. So the drawing has to come out identical, and this says
// whether it does: `gen-notation-vectors.py` freezes what the Python client
// engraves, and each case here engraves the same MEI through the browser stack
// and asserts the same page — before an edit and after one.
//
// Ids are normalized away on both sides: verovio mints fresh `xml:id`s per load,
// so each is replaced by the index of its first appearance, which still checks
// that the same primitives belong to the same element.
//
// Needs `./build.sh` (the core wasm) and `third_party/build-verovio-wasm.sh`
// (the engraver in `vendor/`); skips itself, loudly, when the engraver is not
// built. Run with `npm test`.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { Score, setEngraverUrl } from "../src/gui/notation/index.ts";

const here = new URL(".", import.meta.url);
const engraver = new URL("../vendor/verovio/verovio.js", here);

await loadCore(
    await readFile(new URL("../dist/core/clausters_core_web_bg.wasm", here)),
);

interface Case {
    mei: string;
    options: { scale?: number; page_width?: number };
    page: unknown;
    edited: unknown;
}

const vectors = JSON.parse(
    await readFile(new URL("./notation-vectors.json", here), "utf8"),
) as { cases: Record<string, Case> };

/**
 * The page with every engraver-minted id replaced by the order it first appears
 * in — the same normalization the generator applies.
 */
function normalized(value: unknown, ids = new Map<string, number>()): unknown {
    if (Array.isArray(value)) return value.map((v) => normalized(v, ids));
    if (value !== null && typeof value === "object") {
        const out: Record<string, unknown> = {};
        for (const [key, v] of Object.entries(value as Record<string, unknown>)) {
            if (key === "id" && typeof v === "string") {
                if (!ids.has(v)) ids.set(v, ids.size);
                out[key] = ids.get(v);
            } else {
                out[key] = normalized(v, ids);
            }
        }
        return out;
    }
    return value;
}

if (!existsSync(engraver)) {
    test("the engraver is built", { skip: "run third_party/build-verovio-wasm.sh" }, () => {});
} else {
    setEngraverUrl(engraver.href);

    for (const [name, vector] of Object.entries(vectors.cases)) {
        test(`'${name}' engraves to the same page`, async () => {
            const score = await Score.open(vector.mei, {
                scale: vector.options.scale,
                pageWidth: vector.options.page_width,
            });
            try {
                assert.deepEqual(normalized(score.displayList(1)), vector.page);

                // And the same after an edit: the round trip is what the
                // shared state machine is for, so it is what is pinned.
                const first = score.displayList(1).notes[0]?.id as string;
                assert.equal(score.transpose(first, 1), true);
                assert.deepEqual(normalized(score.displayList(1)), vector.edited);
            } finally {
                score.free();
            }
        });
    }

    test("the engraver is the version both clients are pinned to", async () => {
        const score = await Score.open(
            (Object.values(vectors.cases)[0] as Case).mei,
        );
        try {
            // `third_party/verovio.pin` is the tag; the build appends its commit.
            assert.match(score.engraverVersion(), /^6\.3\.0/);
        } finally {
            score.free();
        }
    });
}
