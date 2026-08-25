/**
 * **A listener watching somebody else's subscription needs `ANY_PEER`**, and
 * nothing but this file says so.
 *
 * The engine keeps a reply queue **per client** (`docs/ipc.md`): a page holding
 * both a script and a GUI host claims a tag each, and `addReply(fn)` with no
 * peer hears only the default client's replies. So a page that watches the
 * *host's* `/bus_stream` subscription — every bundle page and every
 * component page does, because the meter belongs to the host — must register
 * under `ANY_PEER`, the observer door.
 *
 * Getting it wrong fails **silently and in the worst direction**: the listener
 * is installed, no error is raised, and the callback simply never fires. A
 * readout sits at its initial value; an assertion that waits for movement times
 * out with an empty list and blames the audio; an assertion that waits for
 * *silence* passes for entirely the wrong reason. All four of the pages this
 * test covers were broken that way at once, undetected, because the smokes that
 * would have caught it are not in CI and the readouts are only looked at by
 * hand.
 *
 * The check is deliberately textual — a page is a plain module no type-checker
 * reads, and the mistake is a missing argument, which is exactly what a type
 * cannot catch here (the parameter is optional by design, for the common page
 * that has one client).
 */
import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import test from "node:test";

/** Every `.html` under `examples/` and `tests/`, recursively. */
async function pages(dir: string): Promise<string[]> {
    const out: string[] = [];
    for (const entry of await readdir(dir, { withFileTypes: true })) {
        const path = `${dir}/${entry.name}`;
        if (entry.isDirectory()) out.push(...(await pages(path)));
        else if (entry.name.endsWith(".html")) out.push(path);
    }
    return out;
}

test("a page reading /bus_stream.reply registers as an observer", async () => {
    const offenders: string[] = [];
    for (const path of [...(await pages("examples")), ...(await pages("tests"))]) {
        const source = await readFile(path, "utf8");
        // Each `addReply(` call with its arguments, balanced to the closing
        // paren, so the peer is read from the call that owns it rather than
        // from whatever line happens to follow.
        for (let at = source.indexOf("addReply("); at !== -1; at = source.indexOf("addReply(", at + 1)) {
            const start = source.indexOf("(", at);
            let depth = 0;
            let end = start;
            for (; end < source.length; end++) {
                if (source[end] === "(") depth += 1;
                else if (source[end] === ")" && --depth === 0) break;
            }
            const call = source.slice(start, end + 1);
            // A listener that never mentions the stream is reading its own
            // client's replies and is right without a peer. One that does
            // mention it has a decision to make — the stream belongs to
            // whoever subscribed — so it states which client it means, either
            // way. Declared rather than forbidden: a page watching its own
            // subscription is legitimate, and `DEFAULT_PEER` says it meant to.
            if (!call.includes("/bus_stream.reply")) continue;
            if (!call.includes("ANY_PEER") && !call.includes("DEFAULT_PEER")) {
                offenders.push(path);
            }
        }
    }
    assert.deepEqual(
        offenders,
        [],
        "these pages mention /bus_stream.reply without naming the client they " +
            "mean (ANY_PEER to watch somebody else's subscription, DEFAULT_PEER " +
            "for their own): " + offenders.join(", "),
    );
});
