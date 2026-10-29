// `SessionEditor`: the handler of the multitrack the GUI host holds, over a
// saved session, with the same cases as the Python client's
// `tests/test_gui_session.py`.
//
// What is checked is this client's half: which call each member makes
// through the application door (`GuiHost.openApp`, `GuiHost.callApp`), and
// how it reads the answer. What the application does with a call is the
// host's and is tested there (`clients/gui/src/host/audio_editor.rs`).

import assert from "node:assert/strict";
import test from "node:test";

import { SessionEditor, edit } from "../src/gui/editing/index.ts";
import type { GuiHost } from "../src/gui/host.ts";
import { Session as SavedSession } from "../src/multitrack.ts";
import { loadCore } from "../src/base/core.ts";

await loadCore();

type Call = Record<string, any>;

/** A host that answers the door from a table, and records the calls. */
class FakeHost {
    calls: (["open", string, Record<string, unknown>] | ["call", number, Call])[] = [];
    verbs: [number, string][] = [];
    open = new Set<number>();
    answers: Record<string, unknown> = {
        playing: true,
        save: { destination: "/songs/song.json" },
        output: { tracks: { "1": [0.5] }, starved: 0 },
    };

    async openApp(app: string, options: Record<string, unknown> = {}): Promise<number> {
        this.calls.push(["open", app, { ...options }]);
        this.open.add(9);
        return 9;
    }

    async callApp(window: number | { id: number }, call: Call): Promise<unknown> {
        const id = typeof window === "number" ? window : window.id;
        this.calls.push(["call", id, call]);
        return this.answers[call.query ?? call.do?.verb];
    }

    verb(window: number | { id: number }, verb: string): void {
        this.verbs.push([typeof window === "number" ? window : window.id, verb]);
    }

    isOpen(id: number): boolean {
        return this.open.has(id);
    }

    async waitWhile(pred: () => boolean): Promise<boolean> {
        return !pred();
    }

    setClosedHandler(): void {}

    last(): Call {
        return this.calls[this.calls.length - 1]![2] as Call;
    }
}

function aSession(path = "/songs/song.json"): SavedSession {
    const session = new SavedSession();
    session.path = path;
    return session;
}

test("edit opens a saved session in the multitrack application", async () => {
    const host = new FakeHost();
    const editor = await edit(aSession(), { host: host as unknown as GuiHost });
    assert.ok(editor instanceof SessionEditor);
    assert.deepEqual(host.calls[0], ["open", "multitrack", { title: "song.json", path: "/songs/song.json" }]);
    assert.ok(editor.window !== null && !editor.closed);
    assert.equal(await edit(aSession(), { host: host as unknown as GuiHost }), editor, "a second edit finds it");
});

test("a session with no file is refused", () => {
    assert.throws(() => new SessionEditor(new SavedSession()), /save it first/);
});

test("the transport and the save are calls to the application", async () => {
    const host = new FakeHost();
    const editor = new SessionEditor(aSession("/songs/other.json"), { title: "mix" });
    await editor.open(host as unknown as GuiHost);
    await editor.transport.play(1.0, 2.0, { looping: true });
    assert.deepEqual(host.last(), { do: { verb: "play", looping: true, range: [1.0, 2.0] } });
    await editor.transport.stop(0.5);
    assert.deepEqual(host.last(), { do: { verb: "stop", mark: 0.5 } });
    await editor.transport.locate(3.0);
    assert.deepEqual(host.last(), { do: { verb: "locate", at: 3.0 } });
    assert.ok(await editor.transport.playing());
    assert.equal((await editor.output()).starved, 0);
    assert.equal(await editor.save("/songs/song.json"), "/songs/song.json");
    assert.deepEqual(host.last(), { do: { verb: "save", path: "/songs/song.json" } });
    assert.ok(editor.undo());
    assert.deepEqual(host.verbs[host.verbs.length - 1], [9, "undo"]);
    assert.ok(editor.split());
    assert.deepEqual(host.verbs[host.verbs.length - 1], [9, "split"]);
});
