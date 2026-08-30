// The MIDI layer, over ports that do not exist.
//
// A page cannot open a MIDI port and headless Chrome has none to offer, so
// every port here is a stand-in — which is exactly why `MidiOutputPort` and
// `MidiInputPort` are named as *shapes* rather than borrowed from the DOM: a
// test satisfies them in three lines, as the OSC suites satisfy `Connection`.
// What that leaves untested is the browser's own plumbing (the grant, the port
// list) and nothing above it.
//
// The frozen vectors (`gen-midi-vectors.py`) carry the reference client's
// answers. Two kinds: the parse and the note mapping are each client's own
// arithmetic, so they are compared; the file bytes are **one** implementation
// reached two ways — `clausters-midi` over the C ABI there, the same writers
// through the core's wasm door here — so comparing them proves the door is
// wired to the shared writer and not to a second one.

import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";

import { loadCore } from "../src/base/core.ts";
import {
    MidiNrtInterface,
    MidiReceiver,
    MidiRtInterface,
    MidiScore,
    MidiServer,
    parseMidi,
} from "../src/base/midi.ts";
import type { MidiInputPort, MidiMessage, MidiOutputPort } from "../src/base/midi.ts";
import { MidiFunc, setDefaultMidiReceiver } from "../src/responders.ts";
import { MidiItem, OscItem } from "../src/seq/timeline.ts";
import { Event } from "../src/seq/event.ts";
import type { EventDestination } from "../src/seq/event.ts";
import { Pbind, Pseq } from "../src/seq/pattern.ts";
import { Moment } from "../src/base/moment.ts";
import { TempoClock, manualTicker } from "../src/base/clock.ts";
import { ManualTimebase } from "../src/base/timebase.ts";
import { Routine } from "../src/base/stream.ts";
import type { ManualTicker } from "../src/base/clock.ts";
import { flush } from "./flush.ts";

/**
 * Advances a manual clock by `seconds`, firing every wake it arms — the same
 * harness `clock.test.ts` uses, so a beat here means what it means there.
 */
function runner(clock: TempoClock, timebase: ManualTimebase, ticker: ManualTicker) {
    void clock;
    return async (seconds: number) => {
        await flush();
        const target = timebase.now() + seconds;
        for (;;) {
            const pending = ticker.pending;
            if (pending === null) break;
            const at = timebase.now() + pending;
            if (at > target) break;
            timebase.set(at);
            ticker.fire();
            await flush();
        }
        timebase.set(target);
    };
}

await loadCore();

const vectors = JSON.parse(
    await readFile(new URL("./midi-vectors.json", import.meta.url), "utf8"),
) as {
    ppq: number;
    parse: { bytes: number[]; parsed: Record<string, number | string> | null }[];
    score: {
        events: [number, number[]][];
        sorted: [number, number[]][];
        smf: number[];
        clip: number[];
    };
    notes: { props: Record<string, unknown>; channel: number; events: [number, number[]][] }[];
};

// ---- the stand-in ports ----

/** An output that records what it was sent, and when it was told to send it. */
class FakeOutput implements MidiOutputPort {
    readonly name = "fake out";
    readonly id = "out-1";
    readonly sent: { data: number[]; timestamp: number | undefined }[] = [];

    send(data: Uint8Array | number[], timestamp?: number): void {
        this.sent.push({ data: [...data], timestamp });
    }
}

/** An input a test pushes bytes into, as a device would. */
class FakeInput implements MidiInputPort {
    readonly name = "fake in";
    readonly id = "in-1";
    onmidimessage: ((event: { data: Uint8Array | null }) => void) | null = null;

    deliver(...bytes: number[]): void {
        this.onmidimessage?.({ data: Uint8Array.from(bytes) });
    }
}

const ports = (input: FakeInput, output: FakeOutput) => ({
    inputs: new Map([[input.id, input]]),
    outputs: new Map([[output.id, output]]),
});

// ---- the parse ----

test("parseMidi answers what the reference client answers", () => {
    for (const { bytes, parsed } of vectors.parse) {
        assert.deepEqual(parseMidi(bytes), parsed, `for ${JSON.stringify(bytes)}`);
    }
});

test("a pitch wheel is one 14-bit value, not two 7-bit ones", () => {
    assert.equal(parseMidi([0xe0, 0x00, 0x40])!.pitch, 8192);
    assert.equal(parseMidi([0xe0, 0x7f, 0x7f])!.pitch, 16383);
});

// ---- the score ----

test("a score sorts by beat and keeps same-beat order", () => {
    const score = new MidiScore();
    for (const [beat, bytes] of vectors.score.events) score.add(beat, bytes);
    assert.deepEqual(
        score.sorted().map(([beat, bytes]) => [beat, [...bytes]]),
        vectors.score.sorted,
    );
});

test("the file bytes are the shared writer's, not a second one", () => {
    const score = new MidiScore();
    for (const [beat, bytes] of vectors.score.events) score.add(beat, bytes);
    assert.deepEqual([...score.toSmf(vectors.ppq)], vectors.score.smf);
    assert.deepEqual([...score.toClip(vectors.ppq)], vectors.score.clip);
});

test("an SMF starts with the header chunk a reader looks for", () => {
    const smf = new MidiScore();
    smf.add(0, [0x90, 60, 100]);
    assert.deepEqual([...smf.toSmf(480).slice(0, 4)], [...Buffer.from("MThd")]);
});

// ---- the event mapping ----

test("an Event renders as the note pair the reference client renders", () => {
    for (const { props, channel, events } of vectors.notes) {
        const server = new MidiServer({ channel });
        server.playEvent(new Event(props));
        assert.deepEqual(
            server.score!.sorted().map(([beat, bytes]) => [beat, [...bytes]]),
            events,
            `for ${JSON.stringify(props)}`,
        );
    }
});

test("a real-time server keeps no score", () => {
    const output = new FakeOutput();
    const server = new MidiServer({ interface: new MidiRtInterface(output) });
    assert.equal(server.score, null);
});

test("a MidiServer answers no OSC, loudly", () => {
    const server = new MidiServer();
    assert.throws(() => server.sendMsg("/synth_new"), /carries no OSC/);
    assert.throws(() => new OscItem("/x", 1).play(server), /carries no OSC/);
});

// ---- the live interface ----

test("a message at the current beat goes out now, a later one with a deadline", async () => {
    const output = new FakeOutput();
    const iface = new MidiRtInterface(output);
    const clock = new TempoClock(1.0, {
        timebase: new ManualTimebase(1000),
        ticker: manualTicker(),
    }).start();
    clock.play(new Routine(function* () {
        iface.emit(0, [0x90, 60, 100]);       // now
        iface.emit(2, [0x80, 60, 0]);         // two beats out
    }));
    await flush();

    assert.equal(output.sent.length, 2);
    assert.equal(output.sent[0].timestamp, undefined);
    assert.ok(
        output.sent[1].timestamp !== undefined && output.sent[1].timestamp > 0,
        "the future note-off carries a performance.now() deadline",
    );
});

test("closing a live interface panics every channel", () => {
    const output = new FakeOutput();
    new MidiRtInterface(output).close();
    assert.equal(output.sent.length, 16);
    assert.deepEqual(output.sent[0].data, [0xb0, 0x7b, 0]);
    assert.deepEqual(output.sent[15].data, [0xbf, 0x7b, 0]);
});

// ---- the receiver and the responder ----

test("a receiver picks its port by name and decodes what arrives", async () => {
    const input = new FakeInput();
    const recv = await new MidiReceiver({
        port: "fake",
        access: ports(input, new FakeOutput()),
    }).start();
    assert.equal(recv.port, "fake in");

    const seen: MidiMessage[] = [];
    recv.add((message) => seen.push(message));
    input.deliver(0x90, 60, 100);
    input.deliver(0xf8);                        // clock: decodes to nothing
    assert.equal(seen.length, 1);
    assert.equal(seen[0].type, "note_on");
    recv.stop();
    input.deliver(0x90, 62, 100);               // after stop: nothing
    assert.equal(seen.length, 1);
});

test("a port that matches nothing says what there was", async () => {
    const recv = new MidiReceiver({
        port: "Novation",
        access: ports(new FakeInput(), new FakeOutput()),
    });
    await assert.rejects(() => recv.start(), /fake in/);
});

test("a MidiFunc matches by type, by channel and by field", async () => {
    const input = new FakeInput();
    const recv = await new MidiReceiver({
        port: input,
        access: ports(input, new FakeOutput()),
    }).start();

    const byType: number[] = [];
    const byChannel: number[] = [];
    const byField: number[] = [];
    new MidiFunc((m) => byType.push(m.note as number), "note_on", { recv });
    new MidiFunc((m) => byChannel.push(m.note as number), ["note_on", "note_off"], {
        chan: 2,
        recv,
    });
    new MidiFunc((m) => byField.push(m.value as number), "control_change", {
        argTemplate: { control: 7, value: (v) => (v as number) > 64 },
        recv,
    });

    input.deliver(0x90, 60, 100);   // ch 0 note on
    input.deliver(0x92, 62, 100);   // ch 2 note on
    input.deliver(0x82, 62, 0);     // ch 2 note off
    input.deliver(0xb0, 7, 96);     // cc 7, above the threshold
    input.deliver(0xb0, 7, 20);     // cc 7, below it
    input.deliver(0xb0, 1, 96);     // cc 1, wrong controller

    assert.deepEqual(byType, [60, 62]);
    assert.deepEqual(byChannel, [62, 62]);
    assert.deepEqual(byField, [96]);
});

test("a responder is disabled, enabled again, and freed after one match", async () => {
    const input = new FakeInput();
    const recv = await new MidiReceiver({ port: input, access: ports(input, new FakeOutput()) })
        .start();

    const seen: number[] = [];
    const resp = new MidiFunc((m) => seen.push(m.note as number), "note_on", { recv });
    input.deliver(0x90, 60, 1);
    resp.disable();
    input.deliver(0x90, 61, 1);
    resp.enable();
    input.deliver(0x90, 62, 1);
    resp.free();
    input.deliver(0x90, 63, 1);
    assert.deepEqual(seen, [60, 62]);

    const once: number[] = [];
    new MidiFunc((m) => once.push(m.note as number), "note_on", { recv }).oneShot();
    input.deliver(0x90, 70, 1);
    input.deliver(0x90, 71, 1);
    assert.deepEqual(once, [70]);
});

test("the module default is pinned, never opened behind the page's back", async () => {
    assert.throws(
        () => new MidiFunc(() => {}, "note_on"),
        /user grant/,
        "with nothing pinned, the error says what to do",
    );
    const input = new FakeInput();
    const recv = await new MidiReceiver({ port: input, access: ports(input, new FakeOutput()) })
        .start();
    setDefaultMidiReceiver(recv);
    const seen: number[] = [];
    const resp = new MidiFunc((m) => seen.push(m.note as number), "note_on");
    input.deliver(0x90, 64, 1);
    assert.deepEqual(seen, [64]);
    resp.free();
});

// ---- the milestone's own acceptance ----

test("a pattern plays to MIDI on the beat grid it plays to a server", async () => {
    // The same `Pbind`, twice: once into a MidiServer, once into a stand-in
    // that records what an OSC destination would have been asked for. What has
    // to agree is not the messages -- they are different protocols -- but
    // *when*: MIDI carries no timetags, so its only timing is the beat the
    // clock was on when the event was rendered, and that beat has to be the
    // one the OSC leg stamped its bundle with.
    const pattern = () =>
        new Pbind({
            midinote: new Pseq([60, 62, 64, 65]),
            dur: new Pseq([0.5, 0.5, 1, 1]),
            amp: 0.5,
        });

    const beatsSeen: number[] = [];
    const oscLeg: EventDestination = {
        playEvent: () => {
            beatsSeen.push(Moment.current().beat);
            return null;
        },
        sendMsg: () => {},
    };

    const midi = new MidiServer({ channel: 0 });
    const timebase = new ManualTimebase(1000);
    const ticker = manualTicker();
    const clock = new TempoClock(1.0, { timebase, ticker }).start();
    const run = runner(clock, timebase, ticker);

    pattern().play(oscLeg, { clock });
    pattern().play(midi, { clock });
    await run(4.0);

    // Four notes, at 0, 0.5, 1 and 2.
    assert.deepEqual(beatsSeen, [0, 0.5, 1, 2]);
    const onsets = midi
        .score!.sorted()
        .filter(([, bytes]) => (bytes[0] & 0xf0) === 0x90)
        .map(([beat]) => beat);
    assert.deepEqual(onsets, beatsSeen, "the MIDI leg is on the OSC leg's grid");

    // And each note is released at its own sustain, not at the next onset.
    const offs = midi
        .score!.sorted()
        .filter(([, bytes]) => (bytes[0] & 0xf0) === 0x80)
        .map(([beat]) => beat);
    assert.deepEqual(offs, [0.4, 0.9, 1.8, 2.8]);
});

// ---- the timeline item ----

test("a MidiItem renders through a MidiServer and refuses an OSC one", async () => {
    const server = new MidiServer();
    const clock = new TempoClock(1.0, {
        timebase: new ManualTimebase(1000),
        ticker: manualTicker(),
    }).start();
    clock.play(new Routine(function* () {
        new MidiItem([0xb0, 74, 40]).play(server);
    }));
    await flush();

    assert.deepEqual(
        server.score!.sorted().map(([beat, bytes]) => [beat, [...bytes]]),
        [[0, [0xb0, 74, 40]]],
    );
    assert.throws(
        () => new MidiItem([0x90, 60, 1]).play({} as never),
        /needs a MIDI destination/,
    );
});
