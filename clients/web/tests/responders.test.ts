// The responders: `OscFunc` matching, its lifecycle, and the receiving door
// under it.
//
// Everything here runs over a fake carrier — a `Connection` with no server
// behind it that a test pushes packets into — because what is being asserted
// is the matching and the dispatch, not a server's behaviour. The end-to-end
// half (real notifications, both carriers) is `responders-ws` in
// `tests/seq-ws.test.ts` and the page acceptance `tests/responders.html`.
//
// The reference is `clausters/responders.py`: the same constructor arguments,
// the same `(msg, time, src)` callback, the same `enable`/`disable`/`free`/
// `oneShot` lifecycle. Where a difference is deliberate — `src` naming a
// carrier rather than a `(host, port)`, the default receiver being the ambient
// server's — the test says so in its name.

import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";

import { encodeBundle, encodeMessage, loadOsc } from "../src/base/osc.ts";
import type { Connection } from "../src/base/connection.ts";
import { OscReceiver } from "../src/base/receiver.ts";
import { OscFunc, oscfunc } from "../src/responders.ts";
import type { ResponderMessage } from "../src/responders.ts";
import { TempoClock, manualTicker } from "../src/base/clock.ts";
import { ManualTimebase } from "../src/base/timebase.ts";

const here = new URL(".", import.meta.url);
await loadOsc(await readFile(new URL("../dist/core/clausters_core_web_bg.wasm", here)));

/** A carrier that sends nowhere and lets a test push packets in. */
class FakeConnection implements Connection {
    sent: Uint8Array[] = [];
    url?: string;
    private listeners = new Set<(packet: Uint8Array) => void>();

    constructor(url?: string) {
        this.url = url;
    }
    send(packet: Uint8Array): void {
        this.sent.push(packet);
    }
    addReply(listener: (packet: Uint8Array) => void): void {
        this.listeners.add(listener);
    }
    removeReply(listener: (packet: Uint8Array) => void): void {
        this.listeners.delete(listener);
    }
    close(): void {
        this.listeners.clear();
    }
    /** Pushes one bare message, the shape a notification arrives in. */
    push(addr: string, args: [string, unknown][] = []): void {
        const packet = encodeMessage(addr, args as never);
        for (const listener of [...this.listeners]) listener(packet);
    }
    /** Pushes a bundle stamped at `unixSecs`, so a callback sees a `time`. */
    pushBundle(unixSecs: number, addr: string, args: [string, unknown][] = []): void {
        const packet = encodeBundle(unixSecs, [{ addr, args: args as never }]);
        for (const listener of [...this.listeners]) listener(packet);
    }
    /** Pushes raw bytes — what an undecodable packet is made of. */
    pushRaw(bytes: Uint8Array): void {
        for (const listener of [...this.listeners]) listener(bytes);
    }
}

test("a responder fires on its address and passes the message the reference passes", () => {
    const carrier = new FakeConnection();
    const recv = new OscReceiver(carrier);
    const seen: ResponderMessage[] = [];
    const resp = new OscFunc((msg) => seen.push(msg), "/node_start", { recv });

    carrier.push("/node_start", [["i", 1000], ["i", 0]]);
    carrier.push("/node_end", [["i", 1000]]);

    assert.equal(seen.length, 1, "only the matching address fires");
    // The reference's list: the address first, so msg[1] is the first argument
    // in both clients.
    assert.deepEqual(seen[0], ["/node_start", 1000, 0]);
    resp.free();
});

test("a leading slash is added to the path, as in the reference", () => {
    const carrier = new FakeConnection();
    const recv = new OscReceiver(carrier);
    let fired = 0;
    const resp = new OscFunc(() => fired++, "done", { recv });
    assert.equal(resp.path, "/done");
    carrier.push("/done", [["s", "/def_send"]]);
    assert.equal(fired, 1);
    resp.free();
});

test("the argument template matches by position: literal, predicate, and a hole", () => {
    const carrier = new FakeConnection();
    const recv = new OscReceiver(carrier);
    const seen: number[] = [];
    // /tr nodeId triggerId value — only trigger 7, and only above 0.5.
    const resp = new OscFunc(
        (msg) => seen.push(Number(msg[3])),
        "/tr",
        { recv, argTemplate: [null, 7, (v) => Number(v) > 0.5] },
    );

    carrier.push("/tr", [["i", 1000], ["i", 7], ["f", 0.75]]); // matches
    carrier.push("/tr", [["i", 1000], ["i", 8], ["f", 0.75]]); // wrong trigger
    carrier.push("/tr", [["i", 1000], ["i", 7], ["f", 0.25]]); // predicate says no
    carrier.push("/tr", [["i", 1001], ["i", 7], ["f", 0.875]]); // the hole matches

    assert.deepEqual(seen, [0.75, 0.875]); // exact in f32, so the wire is not the subject
    resp.free();
});

test("a template longer than the message checks only the positions that are there", () => {
    const carrier = new FakeConnection();
    const recv = new OscReceiver(carrier);
    let fired = 0;
    const resp = new OscFunc(() => fired++, "/done", { recv, argTemplate: ["/b_alloc", 3] });
    carrier.push("/done", [["s", "/b_alloc"]]);
    assert.equal(fired, 1, "the absent position is not a mismatch");
    resp.free();
});

test("src narrows to one carrier, which is what a page has instead of a host and port", () => {
    const page = new FakeConnection();
    const socket = new FakeConnection("ws://127.0.0.1:57120");
    const fromPage = new OscReceiver(page);
    const fromSocket = new OscReceiver(socket);
    assert.equal(fromPage.src, "page");
    assert.equal(fromSocket.src, "ws://127.0.0.1:57120");

    const seen: string[] = [];
    const a = new OscFunc((_m, _t, src) => seen.push(src), "/done", { recv: fromPage });
    const b = new OscFunc((_m, _t, src) => seen.push(src), "/done", {
        recv: fromSocket,
        src: "ws://127.0.0.1:57120",
    });
    const c = new OscFunc(() => seen.push("never"), "/done", {
        recv: fromSocket,
        src: "ws://127.0.0.1:9999",
    });

    page.push("/done", [["s", "/def_send"]]);
    socket.push("/done", [["s", "/def_send"]]);
    assert.deepEqual(seen, ["page", "ws://127.0.0.1:57120"]);
    a.free();
    b.free();
    c.free();
});

test("time is the containing bundle's, null for a bare message", () => {
    const carrier = new FakeConnection();
    const recv = new OscReceiver(carrier);
    const times: (number | null)[] = [];
    const resp = new OscFunc((_m, time) => times.push(time), "/done", { recv });

    carrier.push("/done", [["s", "/def_send"]]);
    carrier.pushBundle(1_700_000_000.25, "/done", [["s", "/def_send"]]);

    assert.equal(times[0], null, "a bare message means now, not an instant");
    assert.ok(Math.abs(Number(times[1]) - 1_700_000_000.25) < 1e-6);
    resp.free();
});

test("disable stops it, enable puts it back, free is permanent", () => {
    const carrier = new FakeConnection();
    const recv = new OscReceiver(carrier);
    let fired = 0;
    const resp = new OscFunc(() => fired++, "/done", { recv });
    assert.equal(resp.enabled, true, "a responder is enabled on creation");

    carrier.push("/done");
    resp.disable();
    carrier.push("/done");
    assert.equal(fired, 1);
    assert.equal(resp.enabled, false);

    resp.enable();
    carrier.push("/done");
    assert.equal(fired, 2);

    resp.free();
    carrier.push("/done");
    assert.equal(fired, 2);
});

test("oneShot frees after the first match", () => {
    const carrier = new FakeConnection();
    const recv = new OscReceiver(carrier);
    let fired = 0;
    const resp = new OscFunc(() => fired++, "/node_end", { recv }).oneShot();
    carrier.push("/node_end", [["i", 1000]]);
    carrier.push("/node_end", [["i", 1001]]);
    assert.equal(fired, 1);
    assert.equal(resp.enabled, false);
});

test("the oscfunc builder is the decorator form, curried", () => {
    const carrier = new FakeConnection();
    const recv = new OscReceiver(carrier);
    const seen: ResponderMessage[] = [];
    const resp = oscfunc("/play", { recv })((msg) => seen.push(msg));
    assert.ok(resp instanceof OscFunc);
    carrier.push("/play", [["f", 440]]);
    assert.deepEqual(seen, [["/play", 440]]);
    resp.free();
    assert.throws(() => oscfunc(7 as never), TypeError);
});

test("a receiver with a clock hands its handlers to the clock, not to the socket", () => {
    const carrier = new FakeConnection();
    const timebase = new ManualTimebase(1000);
    const ticker = manualTicker();
    const clock = new TempoClock(1, { timebase, ticker });
    const recv = new OscReceiver(carrier, { clock });
    const beats: number[] = [];
    const resp = new OscFunc(() => beats.push(clock.beats()), "/done", { recv });

    // A stopped clock runs nothing: the packet's arrival queues the handler
    // rather than calling it, which is the property the reference client gets
    // from a thread and this one from the queue.
    carrier.push("/done");
    assert.deepEqual(beats, []);

    clock.start();
    timebase.advance(2); // 2 s at 1 beat/s
    ticker.fire();
    assert.equal(beats.length, 1, "it ran once the clock was running");
    resp.free();
    clock.stop();
});

test("a handler freeing its own responder does not disturb the ones beside it", () => {
    const carrier = new FakeConnection();
    const recv = new OscReceiver(carrier);
    const order: string[] = [];
    const first = new OscFunc(() => {
        order.push("first");
        first.free();
    }, "/done", { recv });
    const second = new OscFunc(() => order.push("second"), "/done", { recv });

    carrier.push("/done");
    carrier.push("/done");
    assert.deepEqual(order, ["first", "second", "second"]);
    second.free();
});

test("an undecodable packet is dropped, not thrown", () => {
    const carrier = new FakeConnection();
    const recv = new OscReceiver(carrier);
    let fired = 0;
    const resp = new OscFunc(() => fired++, "/done", { recv });
    assert.doesNotThrow(() => carrier.pushRaw(new Uint8Array([1, 2, 3])));
    carrier.push("/done");
    assert.equal(fired, 1, "the door stayed open after the bad bytes");
    resp.free();
});

test("stop and start move the receiver off and back onto its carrier", () => {
    const carrier = new FakeConnection();
    const recv = new OscReceiver(carrier);
    let fired = 0;
    const resp = new OscFunc(() => fired++, "/done", { recv });
    assert.equal(recv.listening, true);

    recv.stop();
    carrier.push("/done");
    assert.equal(fired, 0);

    recv.start();
    carrier.push("/done");
    assert.equal(fired, 1);
    resp.free();
});

test("a responder can answer on the carrier it heard, through the receiver", () => {
    const carrier = new FakeConnection();
    const recv = new OscReceiver(carrier);
    const resp = new OscFunc(() => recv.send("/node_free", 1000), "/node_start", { recv });
    carrier.push("/node_start", [["i", 1000], ["i", 0]]);
    assert.equal(carrier.sent.length, 1, "the reply went out the same carrier");
    resp.free();
});
