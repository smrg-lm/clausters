// The resource records, parsed from wire arguments — no server, no carrier.
//
// The same reply bytes the Python client's `test_parse_query_tree` walks, so
// the two clients are asserted to read one wire the same way, down to the
// drawing a printed tree produces.

import assert from "node:assert/strict";
import test from "node:test";

import { Tree, parseBufferList, parseNodeInfo, parseQueryTree } from "../src/defs/info.ts";

test("a queried tree carries a full node record per entry", () => {
    // detail=2; root 0 -> group 1000 -> synth 1001 (beep, freq mapped to c5)
    const args = [2, 0, 1, 1000, 1, 1001, -1, "beep", 2, "freq", 330.0, "amp", 0.2,
        1, 0, 5, 0, "-", "0"];
    const tree = parseQueryTree(args);
    assert.equal(tree.id, 0);
    assert.ok(tree.info.isGroup);
    assert.equal(tree.info.head, 1000);

    const group = tree.children[0]!;
    assert.ok(group.info.isGroup);
    assert.equal(group.info.parent, 0);
    assert.deepEqual([group.info.head, group.info.tail], [1001, 1001]);

    // Every entry is a full NodeInfo: what the tree adds is the nesting, and
    // the siblings and head/tail follow from it.
    const synth = group.children[0]!.info;
    assert.equal(synth.id, 1001);
    assert.equal(synth.defname, "beep");
    assert.equal(synth.parent, 1000);
    assert.deepEqual(synth.controls, { freq: 330.0, amp: 0.2 });
    assert.deepEqual(synth.maps, [{ control: 0, bus: 5, audio: false }]);
    assert.deepEqual([synth.reads, synth.writes], ["-", "0"]);
    assert.deepEqual([...tree.walk()].map((i) => i.id), [0, 1000, 1001]);
    assert.equal(tree.find(1001)!.info, synth);

    // The object is the data; its string draws it — the split the Python
    // client spells `repr` vs `str`.
    assert.ok(tree instanceof Tree);
    assert.deepEqual(String(tree).split("\n"), [
        "group 0",
        "  group 1000",
        "    1001 beep  freq<-c5 amp=0.2",
    ]);
});

test("siblings and an empty group come out of the nesting", () => {
    // detail=0: no controls on the wire, three children of the root.
    const tree = parseQueryTree([0, 0, 3, 1001, -1, "a", 1002, -1, "b", 100, 0]);
    const [a, b, empty] = tree.children.map((t) => t.info);
    assert.deepEqual([a!.prev, a!.next], [-1, 1002]);
    assert.deepEqual([b!.prev, b!.next], [1001, 100]);
    assert.ok(empty!.isGroup);
    assert.deepEqual([empty!.head, empty!.tail], [-1, -1]);
    assert.equal(String(tree).split("\n").pop(), "  group 100 (empty)");
});

test("a resource that is not there is a record, not a throw", () => {
    // /node_query.reply with isGroup = -1, and /buffer_query.reply with frames = -1.
    const gone = parseNodeInfo([4242, -1, -1, -1, -1]);
    assert.equal(gone.id, 4242);
    assert.equal(gone.exists, false);

    const [buffer] = parseBufferList([7, -1, 0, 0.0]);
    assert.equal(buffer!.bufnum, 7);
    assert.equal(buffer!.exists, false);
    assert.equal(buffer!.frames, 0);

    const [held] = parseBufferList([3, 100, 2, 44100.0]);
    assert.ok(held!.exists);
    assert.deepEqual([held!.frames, held!.channels], [100, 2]);
});
