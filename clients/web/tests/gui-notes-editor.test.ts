// The notes editor's handler: which call each member makes through the
// application door (`GuiHost.openApp`, `GuiHost.callApp`), how it reads the
// answer, and the page's sequence following the window's while it is open
// (mirrors clients/python/tests/test_gui_notes_editor.py). What the application
// does with a call is the host's and is tested there, and in a page by
// tests/app-window.html.

import assert from "node:assert/strict";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { JsEventSequence } from "../src/core/clausters_core_web.js";
import { HostedTransport, NotesEditor, edit } from "../src/gui/editing/index.ts";
import type { GuiHost } from "../src/gui/host.ts";
import { Event } from "../src/seq/event.ts";
import { pseq, pvoice } from "../src/seq/index.ts";
import { EventSequence } from "../src/seq/sequence.ts";

await loadCore();

type Call = Record<string, any>;

/** A host that answers the door, its window holding a sequence of its own, which a `handle` call reads and edits. */
class FakeHost {
    calls: Call[] = [];
    verbs: string[] = [];
    held: JsEventSequence | null = null;
    recorded: [string | undefined, unknown[]][] = [];
    closedHandlers = new Map<number, () => void>();
    listeners: ((msg: { addr: string; args: unknown[] }) => void)[] = [];
    next = 8;
    selected: number[] = [];
    monophonic = false;
    open = new Set<number>();

    async openApp(app: string, options: Call = {}): Promise<number> {
        this.calls.push({ open: app, options });
        if (options.sequence !== undefined) this.held = new JsEventSequence(JSON.stringify(options.sequence));
        this.next += 1;
        this.open.add(this.next);
        return this.next;
    }

    async callApp(window: number | { id: number }, call: Call): Promise<unknown> {
        const id = typeof window === "number" ? window : window.id;
        this.calls.push({ window: id, ...call });
        if (call.query === "selected") return { selected: this.selected };
        if (call.query === "history") return { canUndo: true, canRedo: false, undoLabel: this.recorded.at(-1)?.[0] ?? null, redoLabel: null };
        if (call.query !== undefined) return {};
        if (call.member !== undefined) {
            if (call.member.verb === "editing") {
                return { monophonic: this.monophonic, mode: "notation", scope: { show: "staff", index: 1 }, voice: [1, 0] };
            }
            return call.member.verb === "grid" ? { grid: 0.25, snap: true } : {};
        }
        const doing = call.do as Call;
        if (doing.verb === "handle") {
            const { verb, ...args } = doing.call as Call;
            if (verb === "record") {
                this.recorded.push([doing.label, args.legs]);
                return true;
            }
            return JSON.parse(this.held!.call(JSON.stringify({ verb, ...args })));
        }
        if (doing.verb === "select") {
            this.selected = doing.regions;
            return null;
        }
        if (doing.verb === "close") {
            this.closeWindow(id);
            return false;
        }
        if (doing.verb === "playing") return true;
        return null;
    }

    closeWindow(id: number): void {
        this.open.delete(id);
        this.closedHandlers.get(id)?.();
    }

    closedWith(_id: number): unknown {
        return { sequence: JSON.parse(this.held!.call(JSON.stringify({ verb: "state" }))) };
    }

    setClosedHandler(id: number, handler: (() => void) | null): void {
        if (handler === null) this.closedHandlers.delete(id);
        else this.closedHandlers.set(id, handler);
    }

    isOpen(id: number): boolean {
        return this.open.has(id);
    }

    onMessage(handler: (msg: { addr: string; args: unknown[] }) => void): () => void {
        this.listeners.push(handler);
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

const take = () =>
    new EventSequence([
        [0, new Event({ midinote: 60, dur: 1 })],
        [1, new Event({ midinote: 64, dur: 1 })],
    ]);

const settled = async (editor: NotesEditor) => {
    await editor.query("history");
};

test("a roll opens in the application over the sequence it hands over", async () => {
    const host = new FakeHost();
    const sequence = take();
    const editor = new NotesEditor(sequence, { title: "notes", yAxis: "hz", midiIn: true, width: 800 });
    await editor.open(host.asHost());
    const opened = host.calls[0];
    assert.equal(opened.open, "notes");
    assert.equal(opened.options.domain, "hz");
    assert.equal(opened.options.midi_in, true);
    assert.equal(opened.options.w, 800);
    assert.equal(opened.options.sequence.events.length, 2);
    assert.equal(sequence.window, editor);
    await editor.close();
    assert.equal(sequence.window, null);
});

test("a second roll opens beside the first, and the sequence stays the window's until the last goes", async () => {
    const host = new FakeHost();
    const sequence = take();
    const keys = new NotesEditor(sequence);
    const hertz = new NotesEditor(sequence, { yAxis: "hz" });
    await keys.open(host.asHost());
    await hertz.open(host.asHost());
    assert.equal(host.calls.find((c) => c.options?.over !== undefined)?.options.over, keys.id);
    assert.equal(host.calls.filter((c) => c.options?.sequence !== undefined).length, 1);
    await keys.close();
    assert.equal(sequence.window, hertz, "the other answers for it now");
    await hertz.close();
    assert.equal(sequence.window, null);
});

test("edit hands back the roll open over a sequence", async () => {
    const host = new FakeHost();
    const sequence = take();
    const keys = await edit(sequence, { host: host.asHost() });
    assert.equal(await edit(sequence, { host: host.asHost() }), keys);
    const hertz = await edit(sequence, { host: host.asHost(), yAxis: "hz" });
    assert.notEqual(hertz, keys);
    await keys.close();
    await hertz.close();
});

test("a page's change is an edit of the window's, under its label, and one entry in a block", async () => {
    const host = new FakeHost();
    const sequence = take();
    const editor = new NotesEditor(sequence);
    await editor.open(host.asHost());
    const first = [...sequence.events][0]!;
    first.set("midinote", 62);
    assert.equal(first.get("midinote"), 62, "the copy answers at once");
    await settled(editor);
    assert.equal(host.recorded.length, 1);
    const held = JSON.parse(host.held!.call(JSON.stringify({ verb: "state" })));
    assert.equal(held.events[0].data.midinote, 62, "and the window has it");
    const history = sequence.history as NotesEditor["history"];
    history.entry("both", () => {
        first.at = 0.5;
        [...sequence.events][1]!.at = 1.5;
    });
    await settled(editor);
    assert.equal(host.recorded.length, 2);
    assert.equal(host.recorded[1]![0], "both");
    assert.equal((host.recorded[1]![1] as unknown[]).length, 2);
    await editor.close();
    assert.equal(first.get("midinote"), 62, "it holds what the window left");
});

test("what is marked is the window's, as the events themselves", async () => {
    const host = new FakeHost();
    const sequence = take();
    const editor = new NotesEditor(sequence);
    await editor.open(host.asHost());
    const first = [...sequence.events][0]!;
    await editor.select([first]);
    assert.deepEqual(await editor.selected(), [first]);
    assert.equal((await editor.selected())[0], first);
    await editor.unselect();
    assert.deepEqual(await editor.selected(), []);
    await editor.close();
    assert.deepEqual(await editor.selected(), [], "empty with no window open");
});

test("the transport, the grid and the verbs are calls to the application", async () => {
    const host = new FakeHost();
    const editor = new NotesEditor(take());
    await editor.open(host.asHost());
    assert.equal(await editor.grid(), 0.25);
    await editor.setGrid(1);
    assert.ok(host.calls.some((c) => c.do?.verb === "set" && c.do.call.grid === 1));
    await editor.play(0.5, undefined, { range: [0, 1], looping: true });
    const played = host.calls.find((c) => c.do?.verb === "play")!.do;
    assert.deepEqual([played.beat, played.range, played.looping], [0.5, [0, 1], true]);
    assert.equal(await editor.playing(), true);
    await editor.setEnd("contents");
    assert.ok(host.calls.some((c) => c.do?.verb === "end" && c.do.end === "contents"));
    assert.ok(editor.transport instanceof HostedTransport);
    await editor.transport.setSpan([0.5, 1.5]);
    assert.ok(host.calls.some((c) => c.do?.call?.verb === "span"));
    assert.equal(await editor.monophonic(), false);
    await editor.setMonophonic(true);
    assert.ok(host.calls.some((c) => c.do?.verb === "set" && c.do.call.monophonic === true));
    assert.equal(editor.split(), true);
    assert.equal(editor.unjoin(), true);
    assert.equal(editor.stepUp() && editor.stepDown() && editor.octaveUp() && editor.octaveDown(), true);
    assert.deepEqual(host.verbs, ["split", "unjoin", "step_up", "step_down", "octave_up", "octave_down"]);
    await editor.close();
    assert.equal(editor.split(), false, "no window, no verb");
    assert.equal(await editor.playing(), false);
});

test("a voice from a pattern is left as it was written", async () => {
    const voice = await pvoice(
        "default",
        { midinote: pseq([60, 62, 64, 64, 67]), dur: 0.5, legato: pseq([1, 1, 1, 0.6, 0.8]) },
        { artic: true },
    ).renderEvents();
    const before = JSON.stringify(voice.data());
    const host = new FakeHost();
    const editor = new NotesEditor(voice);
    await editor.open(host.asHost());
    await editor.setMonophonic(true);
    await editor.close();
    const after = voice.data() as { events: { data: Record<string, unknown> }[] };
    assert.equal(JSON.stringify(after), before, "opened and closed, the roll wrote nothing");
    assert.ok(after.events.every((e) => !("tie" in e.data)) && !("notation" in after));
});

test("the roll's modes, what it shows and the voice it edits are calls to the application", async () => {
    const host = new FakeHost();
    const sequence = take();
    const editor = new NotesEditor(sequence);
    await editor.open(host.asHost());
    assert.equal(await editor.mode(), "notation");
    assert.deepEqual(await editor.scope(), ["staff", 1]);
    assert.deepEqual(await editor.voice(), [1, 0]);
    await editor.setMode("notation");
    await editor.setScope("system");
    await editor.setVoice([0, 1]);
    assert.ok(host.calls.some((c) => c.do?.verb === "set" && c.do.call.scope?.show === "system"));
    assert.ok(host.calls.some((c) => c.do?.verb === "set" && c.do.call.voice?.[1] === 1));
    assert.equal(editor.writing(), "voices");
    editor.setWriting("ties");
    assert.equal(editor.slur() && editor.slurJoin() && editor.tie() && editor.staccato() && editor.pedal(), true);
    assert.deepEqual(host.verbs, ["notation_mode", "write_ties", "slur", "slur_join", "tie", "staccato", "pedal"]);
    await assert.rejects(() => editor.setMode("score" as "midi"), /midi/);
    await editor.close();
});
