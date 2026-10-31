// The score editor's handler: which call each member makes through the
// application door, how it reads the answer, and the page's score following the
// window's while it is open (mirrors clients/python/tests/test_gui_score_editor.py).
// What a gesture does on the page is the application's and tested in its crate.
//
// Needs `./build.sh` (the core wasm) and `third_party/build-verovio-wasm.sh`
// (the engraver in `vendor/`); skips itself, loudly, when the engraver is not
// built. Run with `npm test`.

import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { Score, setEngraverUrl } from "../src/gui/notation/index.ts";
import { HostedTransport, ScoreEditor, edit } from "../src/gui/editing/index.ts";
import type { GuiHost } from "../src/gui/host.ts";
import { Event } from "../src/seq/event.ts";
import { EventSequence } from "../src/seq/sequence.ts";

const engraver = new URL("../vendor/verovio/verovio.js", import.meta.url);
const PHRASE = "@clef:G-2\n@timesig:4/4\n@data:4CDEF/ 4GABc'/";

await loadCore();

type Call = Record<string, any>;

/** A host whose window holds a score of its own, which the door's `page` calls read. */
class FakeHost {
    calls: Call[] = [];
    verbs: string[] = [];
    window: Score | null = null;
    sequence: unknown = null;
    closedHandlers = new Map<number, () => void>();
    open = new Set<number>();
    next = 20;
    answers: Record<string, unknown> = {};

    async openApp(app: string, options: Call = {}): Promise<number> {
        this.calls.push({ open: app, options });
        this.window = await Score.open(options.mei as string);
        this.sequence = options.sequence ?? this.sequence;
        this.next += 1;
        this.open.add(this.next);
        return this.next;
    }

    async callApp(window: number | { id: number }, call: Call): Promise<unknown> {
        const id = typeof window === "number" ? window : window.id;
        this.calls.push({ window: id, ...call });
        if (call.query !== undefined) return {};
        if (call.member !== undefined) {
            const verb = call.member.verb;
            if (verb === "value") return { value: [1, 8] };
            if (verb === "input") return { dotted: true };
            if (verb === "unsaved") return { unsaved: false };
            return {};
        }
        const doing = call.do as Call;
        if (doing.verb === "page") {
            if (doing.call.verb === "mei") return { mei: this.window!.mei() };
            if (doing.call.verb === "apply") return this.window!.apply(doing.call.op);
            return null;
        }
        if (doing.verb === "handle") return { events: [] };
        if (doing.verb === "act") return { changed: doing.call.action !== "refused", refused: doing.call.action === "refused" ? "why" : null };
        if (doing.verb === "save") return { destination: doing.path ?? "/kept.mei" };
        if (doing.verb === "load") return true;
        if (doing.verb === "close") {
            this.open.delete(id);
            this.closedHandlers.get(id)?.();
            return false;
        }
        if (doing.verb === "playing") return true;
        return null;
    }

    closedWith(): unknown {
        return { mei: this.window!.mei(), path: "/left.mei" };
    }

    setClosedHandler(id: number, handler: (() => void) | null): void {
        if (handler === null) this.closedHandlers.delete(id);
        else this.closedHandlers.set(id, handler);
    }

    isOpen(id: number): boolean {
        return this.open.has(id);
    }

    onMessage(): () => void {
        return () => {};
    }

    verb(_window: unknown, name: string): void {
        this.verbs.push(name);
    }

    async waitWhile(pred: () => boolean): Promise<boolean> {
        return !pred();
    }

    asHost(): GuiHost {
        return this as unknown as GuiHost;
    }
}

if (!existsSync(engraver)) {
    test("the engraver is built", { skip: "run third_party/build-verovio-wasm.sh" }, () => {});
} else {
    setEngraverUrl(engraver.href);

    test("a score opens in the application, and a verb is a call to it", async () => {
        const host = new FakeHost();
        const score = await Score.open(PHRASE);
        const editor = new ScoreEditor(score, { value: [1, 8], chrome: false, width: 900 });
        await editor.open(host.asHost());
        const opened = host.calls[0]!;
        assert.equal(opened.open, "score");
        assert.ok(String(opened.options.mei).includes("<mei"));
        assert.deepEqual(opened.options.value, [1, 8]);
        assert.equal(opened.options.chrome, false);
        assert.equal(opened.options.w, 900);
        assert.equal(score.window, editor);
        assert.ok(host.calls.some((c) => c.do?.verb === "end" && c.do.end === "contents"), "a pass ends where the music does");
        assert.equal(await editor.articulation("stacc"), true);
        assert.deepEqual(host.calls.find((c) => c.do?.verb === "act")!.do.call, { action: "articulation", name: "stacc" });
        assert.equal(editor.refused, undefined);
        await editor.select(["n1"]);
        assert.deepEqual(host.calls.find((c) => c.do?.call?.verb === "select")!.do.call.elements, ["n1"]);
        assert.deepEqual(await editor.value(), [1, 8]);
        await editor.setValue([1, 2]);
        assert.ok(host.calls.some((c) => c.do?.verb === "set" && String(c.do.call.value) === "1,2"));
        assert.equal(await editor.dotted(), true);
        assert.equal(editor.selectLeft(), true);
        assert.deepEqual(host.verbs, ["select_left"]);
        await editor.close();
    });

    test("a score's change goes to the window, and a close hands it back", async () => {
        const host = new FakeHost();
        const score = await Score.open(PHRASE);
        const editor = new ScoreEditor(score);
        await editor.open(host.asHost());
        const { id } = (score.sheet() as any).staves[0].voices[0].items[0];
        assert.equal(score.apply({ op: "set_marks", id, marks: { articulations: ["stacc"] } } as never), true);
        assert.ok((score.sheet() as any).staves[0].voices[0].items[0].marks, "the copy answers at once");
        await editor.call({ query: "history" });
        assert.ok(host.calls.some((c) => c.do?.verb === "page" && c.do.call.verb === "apply"));
        assert.ok(host.window!.mei().includes("stacc"), "and the window has it");
        await editor.close();
        assert.equal(score.window, null);
        assert.equal(score.path, "/left.mei");
        assert.ok(score.mei().includes("stacc"), "it holds what the window left");
    });

    test("the score's file is the application's", async () => {
        const host = new FakeHost();
        const score = await Score.open(PHRASE);
        const editor = new ScoreEditor(score);
        await editor.open(host.asHost());
        assert.equal(await editor.save("/a.mei"), "/a.mei");
        assert.equal(score.path, "/a.mei");
        assert.equal(await editor.load("/b.mei"), true);
        assert.equal(score.path, "/b.mei");
        assert.equal(await editor.unsaved(), false);
        await editor.close();
    });

    test("a page over a sequence opens beside a roll, and plays on a transport of its own", async () => {
        const host = new FakeHost();
        const take = new EventSequence([[0, new Event({ midinote: 60, dur: 1 })]]);
        const page = await edit(take, { host: host.asHost(), view: "score", division: 8 });
        assert.ok(page instanceof ScoreEditor);
        assert.equal(page.sequence, take);
        assert.equal(page.structure, take);
        assert.ok(host.calls[0]!.options.sequence, "the sequence is handed over");
        assert.equal(host.calls[0]!.options.how.division, 8);
        assert.equal(await edit(take, { host: host.asHost(), view: "score" }), page);
        await assert.rejects(edit(take, { view: "tablature", open: false } as never) as Promise<unknown>, /no view/);
        await page.play(1);
        assert.ok(host.calls.some((c) => c.do?.verb === "play" && c.do.beat === 1));
        assert.ok(page.transport instanceof HostedTransport);
        await page.close();
    });
}
