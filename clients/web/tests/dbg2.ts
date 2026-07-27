import { spawn } from "node:child_process";
import { readFile } from "node:fs/promises";
import { setTimeout as sleep } from "node:timers/promises";

import { WsConnection } from "../src/base/connection.ts";
import { loadOsc } from "../src/base/osc.ts";
import { Server } from "../src/defs/server.ts";
import { SynthDef } from "../src/defs/synthdef.ts";
import { control, out, sine } from "../src/defs/ugens.ts";
import { TempoClock } from "../src/base/clock.ts";
import { Pbind, Pseq } from "../src/seq/index.ts";

const here = new URL(".", import.meta.url);
await loadOsc(await readFile(new URL("../dist/core/clausters_core_web_bg.wasm", here)));
const bin = new URL("../../../target/debug/clausters", here).pathname;
const port = 57992;
const proc = spawn(bin, ["--ws", String(port), "--no-tcp", "--no-persist"], {
    stdio: "ignore",
});
let conn: WsConnection | null = null;
for (let i = 0; i < 50 && !conn; i++) {
    conn = await WsConnection.open(`ws://127.0.0.1:${port}`).catch(() => null);
    if (!conn) await sleep(100);
}
const server = await Server.open(conn!);
server.onReply((m) => console.log("reply:", m.addr, m.args.slice(0, 8)));
await server.addSynthDef(
    new SynthDef("dbg", out(0.0, sine(control("freq", 440.0)).mul(0.05))),
);

const sent: number[] = [];
const raw = conn!.send.bind(conn!);
conn!.send = (packet: Uint8Array) => {
    sent.push(packet.length);
    raw(packet);
};

const clock = new TempoClock(1.0);
clock.start();
console.log("clock started, beats:", clock.beats(), "startTime:", clock.startTime);
new Pbind({
    instrument: "dbg",
    degree: new Pseq([0, 2, 4, 7]),
    dur: 0.25,
    legato: 8.0,
}).play(server, { clock });
console.log("queued:", clock.queued);
for (const t of [200, 300, 300, 300, 1000, 1000]) {
    await sleep(t);
    console.log(
        "at", clock.beats().toFixed(2), "tree:",
        JSON.stringify((await server.queryTree()).children?.map((c) => c.id)),
    );
}
console.log("packets sent:", sent.length, "queued:", clock.queued);
const c = await server.request("/clock", [], { expect: ["/clock.reply"] });
console.log("clock reply:", c.args);
clock.close();
server.close();
conn!.close();
proc.kill();
