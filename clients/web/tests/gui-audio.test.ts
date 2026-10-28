// `AudioEditor`: the handler of the audio editor the GUI host holds, with
// the same cases as the Python client's `tests/test_gui_audio.py`.
//
// What is checked is this client's half: which call each member makes
// through the application door (`GuiHost.openApp`, `GuiHost.callApp`), and
// how it reads the answer. What the application does with a call is the
// host's and is tested there (`clients/gui/src/host/audio_editor.rs`).

import assert from "node:assert/strict";
import test from "node:test";

import { AudioEditor, edit, readsInPlace } from "../src/gui/editing/index.ts";
import type { GuiHost } from "../src/gui/host.ts";
import { Segment } from "../src/segments.ts";
import { SoundFile } from "../src/soundfile.ts";

const SR = 48_000;

type Call = Record<string, any>;

/** A host that answers the door from a table, and records the calls. */
class FakeHost {
    calls: (["open", string, Record<string, unknown>] | ["call", number, Call])[] = [];
    verbs: [number, string][] = [];
    open = new Set<number>();
    answers: Record<string, unknown>;

    constructor(answers: Record<string, unknown> = {}) {
        this.answers = {
            binding: { rate: SR, cursor: 4_800, dirty: false, layers: ["waveform"] },
            history: { version: 3, canUndo: true, canRedo: false, undoLabel: "cut", redoLabel: null },
            transport: { transport: 2, span: [0.5, 1.0], looping: false },
            ...answers,
        };
    }

    async openApp(app: string, options: Record<string, unknown> = {}): Promise<number> {
        this.calls.push(["open", app, { ...options }]);
        this.open.add(7);
        return 7;
    }

    async callApp(window: number | { id: number }, call: Call): Promise<unknown> {
        const id = typeof window === "number" ? window : window.id;
        this.calls.push(["call", id, call]);
        const verb = call.member?.verb ?? call.query ?? call.do?.verb;
        const answer = this.answers[verb];
        if (answer instanceof Error) throw answer;
        return typeof answer === "function" ? answer(call) : answer;
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

    /** The last call's object. */
    last(): Call {
        const call = this.calls[this.calls.length - 1]!;
        return call[2] as Call;
    }
}

const aFile = (path = "/takes/take.wav", codec = "wav") => new SoundFile(path, 48_000, 1, SR, codec, "float");

async function opened(answers: Record<string, unknown> = {}): Promise<[AudioEditor, FakeHost]> {
    const host = new FakeHost(answers);
    const editor = await edit(aFile(), { host: host as unknown as GuiHost });
    return [editor, host];
}

test("edit opens the host's application on the file", async () => {
    const [editor, host] = await opened();
    assert.deepEqual(host.calls[0], ["open", "audio", { title: "Audio", path: "/takes/take.wav" }]);
    assert.equal(editor.id, 7);
    assert.equal(editor.closed, false);
    // a second edit of the same file is the editor already open
    assert.equal(await edit(aFile(), { host: host as unknown as GuiHost }), editor);
    assert.equal(host.calls.filter((c) => c[0] === "open").length, 1);
});

test("a title and layers are the open's", async () => {
    const host = new FakeHost();
    await edit(aFile("/takes/other.wav"), {
        host: host as unknown as GuiHost, title: "other", layers: ["spectrogram"],
    });
    assert.equal((host.calls[0]![2] as Record<string, unknown>).title, "other");
    assert.deepEqual(host.last(), {
        do: { verb: "verb", call: { verb: "layers", version: 3, stack: ["spectrogram"] } },
    });
});

test("a buffer is not edited", async () => {
    const buffer = { bufnum: 3, setSamples() {} };
    await assert.rejects(edit(buffer, { host: new FakeHost() as unknown as GuiHost }), /sound files/);
});

test("a file not on disk is refused and a compressed one is not", async () => {
    await assert.rejects(
        edit(new SoundFile(null, 100, 1, SR), { host: new FakeHost() as unknown as GuiHost }),
        /not a file on disk/,
    );
    assert.equal(readsInPlace(aFile("/takes/take.mp3", "mp3")), false);
    await edit(aFile("/takes/take.mp3", "mp3"), { host: new FakeHost() as unknown as GuiHost, open: false });
});

test("what the editor holds is read from the application", async () => {
    const [editor] = await opened({
        file: { path: "/takes/take.wav", frames: 24_000, channels: 1, rate: SR, codec: "wav", sample_format: "float" },
        segments: { parts: [{ path: "/takes/take.wav", start: 0, frames: 24_000 }] },
    });
    const file = (await editor.file())!;
    assert.deepEqual([file.path, file.frames, file.sampleRate], ["/takes/take.wav", 24_000, SR]);
    assert.deepEqual(await editor.parts(), [{ path: "/takes/take.wav", start: 0, frames: 24_000 }]);
    assert.ok(Math.abs((await editor.cursor())! - 0.1) < 1e-9);
    assert.equal(await editor.dirty(), false);
    assert.deepEqual(await editor.layers(), ["waveform"]);
    assert.deepEqual(
        [await editor.canUndo(), await editor.canRedo(), await editor.undoLabel()],
        [true, false, "cut"],
    );
});

test("a verb that needs a file says none is open", async () => {
    const [editor] = await opened({ file: { error: "no file is open" } });
    await assert.rejects(editor.file(), RangeError);
});

test("a save waits for the application and answers the path", async () => {
    const [editor, host] = await opened({
        save: (call: Call) => ({ destination: { path: call.do.request.path } }),
    });
    assert.equal(await editor.save({ path: "/takes/out.wav", sampleFormat: "int16" }), "/takes/out.wav");
    assert.deepEqual(host.last(), { do: { verb: "save", request: { format: "int16", path: "/takes/out.wav" } } });
});

test("what the application raises is raised", async () => {
    const [editor] = await opened({ save: new Error("the window closed") });
    await assert.rejects(editor.save(), /window closed/);
});

test("the transport is the application's", async () => {
    const [editor, host] = await opened({ playing: true });
    const t = editor.transport;
    await t.play(1.5);
    await t.locate(0.25);
    await t.loop(0.5, 1.0);
    assert.deepEqual(await t.span(), [0.5, 1.0]);
    assert.equal(await t.looping(), false);
    assert.equal(await t.playing(), true);
    const done = host.calls.filter((c) => c[0] === "call" && "do" in (c[2] as Call)).map((c) => (c[2] as Call).do);
    assert.deepEqual(done.slice(0, 5), [
        { verb: "play", at: 1.5 },
        { verb: "locate", at: 0.25 },
        { verb: "span", show: true, span: [0.5, 1.0] },
        { verb: "looping", on: true },
        { verb: "playing" },
    ]);
});

test("what is marked is the span over the file", async () => {
    const [editor, host] = await opened({
        file: { path: "/takes/take.wav", frames: 48_000, channels: 1, rate: SR, codec: "wav", sample_format: "float" },
    });
    const marked = (await editor.selected())!;
    assert.equal(marked.start, 24_000);
    assert.ok(Math.abs(marked.duration - 0.5) < 1e-12);
    assert.equal(marked.source!.path, "/takes/take.wav");
    await editor.select(new Segment(marked.source, 4_800, 0.2));
    assert.deepEqual(host.last(), { do: { verb: "span", show: true, span: [0.1, 0.30000000000000004] } });
    await editor.unselect();
    assert.deepEqual(host.last(), { do: { verb: "span", show: true, span: null } });
});

test("an undo is the window's event against the version now", async () => {
    const [editor, host] = await opened();
    assert.equal(await editor.undo(), true);
    assert.deepEqual(host.last(), { do: { verb: "event", addr: "/gui_event", args: [7, 0, 3, "undo"] } });
    // nothing to redo asks nothing of the take
    const before = host.calls.length;
    assert.equal(await editor.redo(), false);
    assert.ok(host.calls.slice(before).every((c) => !("do" in (c[2] as Call))));
});

test("the window's verbs go to its window", async () => {
    const [editor, host] = await opened();
    editor.cut();
    editor.reverse();
    assert.deepEqual(host.verbs, [[7, "cut"], [7, "reverse"]]);
});

test("a channel refused is raised", async () => {
    const [editor] = await opened({ verb: { reason: "the take has 1 channel" } });
    await assert.rejects(editor.foldChannel(3), /1 channel/);
});

test("the envelope is one action", async () => {
    const [editor, host] = await opened({ act: { changed: true } });
    assert.equal(await editor.setEnvelope([0, 1, 1, 0]), true);
    assert.deepEqual(host.last(), { do: { verb: "act", call: { verb: "setautomation", points: [0, 1, 1, 0] } } });
});

test("closing asks the application and the window goes when it says", async () => {
    const [editor, host] = await opened();
    await editor.close();
    assert.deepEqual(host.last(), { do: { verb: "close" } });
    assert.equal(editor.closed, false, "the window goes when the host says so");
    host.open.delete(7);
    assert.equal(editor.closed, true);
    const before = host.calls.length;
    await editor.close();
    assert.equal(host.calls.length, before, "a closed editor asks nothing");
});

test("an editor not opened says so", async () => {
    const editor = new AudioEditor(aFile());
    assert.equal(editor.closed, true);
    assert.equal(editor.window, null);
    await assert.rejects(editor.file(), /open/);
});
