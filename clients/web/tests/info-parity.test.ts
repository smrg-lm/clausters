// The readable line every server record prints, against the Python client's.
//
// `tests/info.test.ts` asserts the two clients read one *wire* alike; this one
// asserts they print it alike. There is no shared core under a record's line —
// it is presentation, written twice from one description — so the only thing
// that holds the wording together is the reference output frozen in
// `info-vectors.json` (written by `gen-info-vectors.py` from the Python
// records' own `__str__`).
//
// The records are interfaces here and dataclasses there, which is why the line
// is a free `format*` function on this side and a method on that one: an
// interface carries no method. Same fields, same text.

import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";

import {
    formatBufferInfo,
    formatControlInfo,
    formatDefInfo,
    formatNodeInfo,
    formatNodeMap,
    formatUgenInfo,
    formatUgenInput,
} from "../src/defs/info.ts";
import { formatServerInfo } from "../src/defs/server/options.ts";
import { formatWidgetInfo } from "../src/gui/host.ts";
import type {
    BufferInfo, ControlInfo, DefInfo, NodeInfo, NodeMap, UgenInfo, UgenInput,
} from "../src/defs/info.ts";
import type { ServerInfo } from "../src/defs/server/options.ts";
import type { WidgetInfo } from "../src/gui/host.ts";

interface Vector {
    kind: string;
    record: Record<string, unknown>;
    line: string;
}

const vectors: Vector[] = JSON.parse(
    readFileSync(new URL("./info-vectors.json", import.meta.url), "utf8"),
) as Vector[];

// The generator writes the fields in *this* client's spelling, so a case is
// the record itself — nothing is transliterated here, which is the only way a
// renamed field shows up as a failure rather than as a silent cast.
const formatters: Record<string, (record: never) => string> = {
    control: (r: ControlInfo) => formatControlInfo(r),
    def: (r: DefInfo) => formatDefInfo(r),
    buffer: (r: BufferInfo) => formatBufferInfo(r),
    ugenInput: (r: UgenInput) => formatUgenInput(r),
    ugen: (r: UgenInfo) => formatUgenInfo(r),
    nodeMap: (r: NodeMap) => formatNodeMap(r),
    node: (r: NodeInfo) => formatNodeInfo(r),
    server: (r: ServerInfo) => formatServerInfo(r),
    widget: (r: WidgetInfo) => formatWidgetInfo(r),
} as Record<string, (record: never) => string>;

test("every record prints the line the Python client prints", () => {
    assert.ok(vectors.length > 0, "the vectors file is empty");
    for (const { kind, record, line } of vectors) {
        const format = formatters[kind];
        assert.ok(format, `no formatter for a '${kind}' record`);
        assert.equal(format(record as never), line, `${kind}: ${JSON.stringify(record)}`);
    }
});

test("every record kind the vectors carry has a formatter, and the reverse", () => {
    const seen = new Set(vectors.map((v) => v.kind));
    assert.deepEqual([...seen].sort(), Object.keys(formatters).sort());
});
