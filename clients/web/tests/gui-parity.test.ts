// The GuiDef builders against the Python client's, on the shared vectors.
//
// `gen-gui-vectors.py` freezes the JSON document the Python builders emit for
// a set of trees; each case here rebuilds the same tree with the TS builders
// and asserts the emitted document is identical. The two surfaces are written
// independently (TypeScript takes camelCase options where Python takes
// snake_case keywords) — what has to match is only the wire, which is the
// whole point of a shared GuiDef format.
//
// The comparison is on the **parsed** document, not the JSON text: JavaScript
// has one number type, so `480` and `480.0` serialize the same. The host reads
// every continuous prop as a float and every id/index prop as an integer, so
// the two documents mean the same thing to it.
//
// (Not to be confused with `gui-parity.html`, which is the *host's* rendering
// parity pass over the raw binding surface — a B-track page, no client in it.)
//
// Needs the core wasm staged (`./build.sh`); run with `npm test`.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { Env } from "../src/defs/ugens/index.ts";
import { BASE_ID, GuiIdAllocator } from "../src/gui/ids.ts";
import {
    bpf,
    canvas,
    clip,
    field,
    envToPoints,
    label,
    knob,
    layout,
    menu,
    meter,
    node,
    nodetree,
    number,
    panel,
    patch,
    phasescope,
    plane,
    piano,
    pianoroll,
    plot,
    pointsToEnv,
    samplesToBlob,
    scope,
    score,
    scroll,
    signal,
    slider,
    stack,
    spectrogram,
    spectrum,
    text,
    timeruler,
    toggle,
    toJson,
    track,
    waveform,
    window,
    button,
} from "../src/gui/guidef.ts";
import type { GuiNode } from "../src/gui/guidef.ts";
import * as guidef from "../src/gui/guidef.ts";

const here = new URL(".", import.meta.url);

// The id allocator is core-backed (the same occupancy map the server and the
// Python client use), so the wasm has to be in before it is exercised.
await loadCore();

interface Vector {
    name: string;
    tree: unknown;
}

const vectors: Vector[] = JSON.parse(
    await readFile(new URL("gui-vectors.json", here), "utf8"),
) as Vector[];

const find = (name: string): unknown => {
    const row = vectors.find((v) => v.name === name);
    assert.ok(row, `no vector named '${name}' — regenerate gui-vectors.json`);
    return row.tree;
};

/** The TS side of each named vector, built independently of the Python one. */
const trees: Record<string, () => GuiNode> = {
    panel_controls: () =>
        window(
            { title: "panel", w: 480, h: 420, layout: "col", margin: 8.0 },
            label("clausters", { id: 1, textSize: 3.0, align: "center", h: 24 }),
            panel(
                { id: 2, layout: "row", gap: 10.0 },
                knob({
                    id: 3, label: "freq", min: 50.0, max: 2000.0, value: 220.0,
                    name: "freq",
                }),
                slider({
                    id: 4, label: "cutoff", min: 20.0, max: 20000.0,
                    value: 800.0, vertical: true,
                }),
                number({ id: 5, label: "amp", min: 0.0, max: 1.0, value: 0.2 }),
            ),
            panel(
                { id: 6, layout: "row", h: 40 },
                button({ id: 7, label: "ping" }),
                toggle({ id: 8, label: "gate", value: true }),
                menu(["sine", "saw", "pulse"], { id: 9, index: 1, label: "wave" }),
                text({ id: 10, value: "/node_set 1000 freq 440", multiline: false }),
            ),
        ),

    containers: () =>
        window(
            { title: "workspace", layout: "col", theme: { window_fill: "#0d0d12" } },
            scroll(
                {
                    id: 1, axis: "y", zoom: false, contentH: 1200.0, viewY: 40.0,
                    viewZoom: 1.0, layout: "free",
                },
                panel({
                    id: 2, x: 0.0, y: 0.0, w: 200.0, h: 1200.0,
                    theme: { panel_fill: "#101018", accent: "#40c0a0" },
                }),
            ),
            menu(["one", "two"], { id: 3, index: 1, bind: ["widget", 4, "index"] }),
            stack(
                { id: 4, index: 1, margin: 4.0 },
                panel({ id: 5 }),
                panel({ id: 6 }),
            ),
        ),

    heavy_views: () =>
        window(
            { title: "views", layout: "col" },
            waveform({
                id: 1, path: "take.f32", channels: 2, baseBucket: 512,
                ruler: "beats", rulerY: "db", tempo: 2.0, beatAt: 0.0,
                quant: 4.0, selStart: 1000.0, selLen: 4000.0,
                selMin: -0.5, selMax: 0.75,
                playheadAt: 48000.0, playhead: 32000.0,
                playheadLoopStart: 1000.0, playheadLoopLen: 8000.0,
                yStart: 0.25, yLen: 0.5, link: 7,
                overlay: true, measure: "rms", fills: true,
            }),
            spectrogram({
                id: 2, cache: "take.stft", windowSize: 2048, hop: 512,
                sampleRate: 48000.0, dbFloor: -90.0, dbCeil: 0.0,
                freqScale: "mel", colormap: 1, ruler: "time", rulerY: "hz",
                link: 7,
            }),
            plot({
                id: 3, data: [0.0, 0.5, -0.5, 1.0], view: "spectrum",
                fftSize: 1024, dbFloor: -100.0, freqScale: "log",
                ruler: "samples", rulerY: "off", label: "render",
            }),
        ),

    live_views: () =>
        window(
            { title: "live", layout: "col" },
            meter(10, { id: 1, rate: "control", min: -1.0, max: 1.0, label: "bus" }),
            scope(10, { id: 2, rate: "control", min: -1.0, max: 1.0 }),
            scope(0, {
                id: 3, channels: 2, windowMs: 20.0, trigger: 0.0,
                overlay: true, ruler: false, rulerY: "off",
            }),
            phasescope(0, { id: 4, windowMs: 30.0, hold: false }),
            spectrum(0, {
                id: 5, channels: 2, fftSize: 2048, dbFloor: -100.0,
                dbCeil: 0.0, freqScale: "bark", averaging: 0.5, peakHold: true,
            }),
            nodetree({ id: 6, group: 0, controls: true }),
        ),

    bpf_points: () =>
        window(
            { title: "envelopes", layout: "col" },
            bpf({
                id: 1,
                points: [[0.0, 0.0], [0.5, 1.0, "exp"], [1.0, 0.0, -4.0]],
                min: 0.0, max: 1.0, duration: 1.0, exp: false, label: "env",
            }),
            bpf({ id: 2, points: envToPoints(Env.adsr(0.01, 0.2, 0.6, 0.4)) }),
            bpf({ id: 3, points: [0.0, 0.0, 1, 0.0, 2.0, 1.0, 8, 0.0] }),
        ),

    timeline_editors: () =>
        window(
            { title: "arrangement", layout: "col" },
            pianoroll({
                id: 1,
                notes: [[0.0, 4800.0, 60], [4800.0, 4800.0, 67, 90, 1]],
                osc: [[0.0, "start"], 9600.0],
                min: 48, max: 84, snap: 1200.0, velocity: true, oscLane: true,
                ruler: "beats", tempo: 2.0, playheadAt: -1.0,
                playhead: 2400.0, playheadLoopStart: 0.0,
                playheadLoopLen: 9600.0,
            }),
            piano({
                id: 2, min: 36, max: 96, activeMin: 48, activeMax: 84,
                velocity: 100, channel: 0, voice: "piano_voice",
                voiceArgs: [["amp", 0.3]], overview: true, pan: true,
            }),
            track(
                {
                    id: 3, label: "drums", height: 2.0, snap: 1200.0,
                    ruler: "time", sampleRate: 48000.0, playheadAt: 0.0,
                    playhead: 12000.0, playheadLoopStart: 0.0,
                    playheadLoopLen: 96000.0, link: 7,
                },
                clip({
                    id: 4, offset: 0.0, dur: 48000.0, path: "take.f32",
                    channels: 1, baseBucket: 256, label: "take",
                }),
                clip({
                    id: 5, offset: 48000.0, dur: 24000.0,
                    notes: [[0.0, 12000.0, 64]], min: 48.0, max: 84.0,
                }),
                clip({
                    id: 6, offset: 72000.0, dur: 24000.0,
                    points: [[0.0, 0.0], [24000.0, 1.0, "sin"]], exp: false,
                }),
            ),
        ),

    ruler_score_legacy: () =>
        window(
            { title: "ruler and score", layout: "col" },
            timeruler({
                id: 1, h: 24.0, link: 7, ruler: "beats", tempo: 2.0,
                beatAt: 0.0, quant: 4.0,
            }),
            score({
                id: 2,
                displayList: {
                    vb: [100.0, 50.0],
                    glyphs: { e0a4: "M0 0 L1 1 Z" },
                    prims: [{ id: "n1", kind: "glyph", cp: "e0a4", x: 10.0, y: 20.0 }],
                    cursors: [{ t: 0.0, x: 10.0, y0: 0.0, y1: 40.0 }],
                    step: 2.5,
                },
                playhead: 250.0, playheadAt: 48000.0,
                playheadLoopStart: 0.0, playheadLoopLen: 4000.0,
                sampleRate: 48000.0, selected: "n1", editable: true,
            }),
            spectrogram({ id: 3, data: [0.0, 0.5, -0.5, 1.0], logFreq: true }),
            spectrum(0, { id: 4, logFreq: false, label: "legacy" }),
        ),

    patch_canvas: () =>
        window(
            { title: "patch", layout: "row" },
            patch({
                id: 1,
                boxes: [
                    { def: "gsrc", inlets: [], outlets: ["out"], x: 0.0, y: 0.0 },
                    {
                        def: "gsink",
                        inlets: ["in", { name: "gain", rate: "control" }],
                        outlets: [], x: 0.0, y: 120.0,
                    },
                ],
                cords: [0, 0, 1, 0],
                label: "graph",
            }),
            canvas("return vec4<f32>(uv, u.params.x, 1.0);", {
                id: 2, params: [0.5, 0.0, 0.0, 0.0], buses: [10, -1, -1, -1],
            }),
        ),

    model_vocabulary: () =>
        window(
            { title: "the model", flow: "col" },
            layout(
                { id: 1, flow: "row", gap: 4.0 },
                signal({
                    id: 2, view: "trace", path: "take.f32", channels: 2,
                    navigable: true, selectable: true, baseBucket: 512,
                    axes: {
                        x: { unit: "beats", tempo: 2.0, link: 1 },
                        y: { unit: "db", min: -1.0, max: 1.0 },
                    },
                }),
                signal({
                    id: 3, view: "spectrum", bus: 0, rate: "audio", channels: 2,
                    fft_size: 2048, label: "live",
                }),
            ),
            plane(
                { id: 5, axis: "both", zoom: true, viewZoom: 1.0 },
                label("placed in content units", { id: 6, x: 0.0, y: 0.0 }),
            ),
            field(
                {
                    id: 7, label: "drums", height: 2.0,
                    axes: { x: { unit: "time", sample_rate: 48000.0, link: 1 } },
                },
                field({ id: 8, offset: 0.0, dur: 48000.0, label: "take" }),
            ),
            field({ id: 9, h: 24.0, axes: { x: { unit: "beats", link: 1 } } }),
        ),

    generic_node: () =>
        window(
            { title: "generic" },
            node("gizmo", { id: 1, spin: 2.5, mode: "loose", w: 64 }),
        ),
};

for (const [name, build] of Object.entries(trees)) {
    test(`GuiDef parity: ${name}`, () => {
        assert.deepEqual(JSON.parse(toJson(build())), find(name));
    });
}

// ---- the sweep: every builder, every option, one at a time ----
//
// The trees above are a sample of what a script writes, and a sample cannot
// see the prop nobody put in one. `gen-gui-vectors.py` also freezes the
// exhaustive reading — each builder crossed with each option its Python
// signature declares — and this rebuilds every one of them here.
//
// It is the reading `docs/gui-props.md` cannot make: that manifest compares
// the two surfaces by **wire type**, unioning every builder of a type
// (`waveform`, `plot` and `scope` all build a `signal`), so an option missing
// from one builder of a type reads as present. Here the key is the builder,
// and the comparison is of what it built rather than of what it declares.

/** A swept option's Python name as the option this client takes. */
const camel = (name: string): string =>
    name.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase());

/**
 * The builders whose first argument is the widget's subject rather than its
 * option bag — the value the sentence is about (`label(text)`, `meter(bus)`),
 * which Python takes as its first keyword. The rest of the options follow in
 * the bag, so a sweep of one of these passes the subject and an empty bag, or
 * the subject itself when that is what is being swept.
 */
const SUBJECT: Record<string, { option: string; blank: unknown }> = {
    label: { option: "text", blank: "" },
    menu: { option: "options", blank: [] },
    meter: { option: "bus", blank: 0 },
    scope: { option: "bus", blank: 0 },
    phasescope: { option: "bus", blank: 0 },
    spectrum: { option: "bus", blank: 0 },
    canvas: { option: "shader", blank: undefined },
};

/** What a builder needs before it builds anything — `gen-gui-vectors.py`'s. */
const REQUIRED: Record<string, Record<string, unknown>> = {
    clip: { dur: 4.0 },
};

interface SweptCase {
    builder: string;
    option: string;
    value: unknown;
    tree: unknown;
}

const swept: SweptCase[] = JSON.parse(
    await readFile(new URL("gui-sweep-vectors.json", here), "utf8"),
) as SweptCase[];

const builders = guidef as unknown as Record<
    string,
    (...args: unknown[]) => GuiNode
>;

for (const one of swept) {
    test(`GuiDef parity: ${one.builder}(${one.option})`, () => {
        const build = builders[one.builder];
        assert.ok(build, `the web client has no ${one.builder} builder`);
        const bag = { ...REQUIRED[one.builder], [camel(one.option)]: one.value };
        const subject = SUBJECT[one.builder];
        let node: GuiNode;
        if (subject === undefined) {
            node = build(bag);
        } else if (subject.option === one.option) {
            node = build(one.value, { ...REQUIRED[one.builder] });
        } else {
            node = build(subject.blank, bag);
        }
        assert.deepEqual(JSON.parse(toJson(node)), one.tree);
    });
}

// ---- what no vector can show ----

test("a name is client-only: kept in the tree, stripped from the wire", () => {
    const tree = window({}, knob({ id: 1, name: "cutoff", value: 0.5 }));
    assert.equal(tree.children?.[0]?.name, "cutoff");
    const wire = JSON.parse(toJson(tree)) as GuiNode;
    assert.equal((wire.children?.[0] as GuiNode).name, undefined);
    assert.equal((wire.children?.[0] as GuiNode).value, 0.5);
});

test("an id must be an integer", () => {
    assert.throws(() => knob({ id: 1.5 }), TypeError);
});

test("a flat points list must be whole quads", () => {
    assert.throws(() => bpf({ points: [0.0, 1.0, 1] }), TypeError);
});

test("points round-trip through an Env", () => {
    const env = Env.adsr(0.01, 0.2, 0.6, 0.4);
    const back = pointsToEnv(envToPoints(env));
    assert.deepEqual(back.levels, env.levels);
    for (const [i, t] of back.times.entries()) {
        assert.ok(Math.abs(t - env.times[i]!) < 1e-9, `segment ${i}: ${t}`);
    }
});

test("widget ids come out of one bounded, recycling window", () => {
    const alloc = new GuiIdAllocator();
    const ids = [alloc.alloc(), alloc.alloc(), alloc.alloc()];
    assert.ok(ids.every((id) => id >= BASE_ID), `${ids} must start at the base`);
    assert.equal(new Set(ids).size, 3, "every id is distinct");
    assert.equal(alloc.inUse, 3);
    for (const id of ids) alloc.free(id);
    assert.equal(alloc.inUse, 0, "a freed subtree's ids leave the occupancy map");
    // Freeing an id the allocator never handed out (a hand-picked one below
    // the base) is a no-op, never an error.
    alloc.free(7);
    assert.equal(alloc.inUse, 0);
});

test("samples pack as a little-endian f32 blob", () => {
    const blob = samplesToBlob([1.0, -1.0]);
    assert.equal(blob.length, 8);
    const view = new DataView(blob.buffer, blob.byteOffset, blob.byteLength);
    assert.equal(view.getFloat32(0, true), 1.0);
    assert.equal(view.getFloat32(4, true), -1.0);
});
