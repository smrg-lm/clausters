// Booting a native-saved standalone bundle in a tab — the page half.
//
// A *bundle* is the same data directory the native `--standalone` mode reads
// (defs/synthdefs, defs/graphdefs, defs/guidefs, boot.json), served over HTTP
// plus one addition: a `bundle.json` manifest at its root, because HTTP cannot
// list directories (generate it with web/bundle-manifest.py):
//
//   { "gui": "drone",                    // the GuiDef name (defs/guidefs/)
//     "synthdefs": ["drone_def"],        // file stems under defs/synthdefs/
//     "graphdefs": [],                   // file stems under defs/graphdefs/
//     "buffers": { "0": "kick.wav" } }   // optional: server buffer index ->
//                                        // audio URL (relative to the bundle)
//
// The split of labor: this module fetches (a page concern) and pumps; the
// boot's ordering/encoding is the gui host's platform-agnostic
// `host::bundle::boot_packets`, reached through the wasm export
// `bundle_boot_packets`. Samples load with fetch + decodeAudioData into the
// engine's `bLoad` (the browser's /b_allocRead), before the defs' /sync so a
// boot /s_new can already play them.
//
// `bootBundle` wires the two wasm worlds of the page together: the engine in
// the AudioWorklet (crates/clausters-web's loader handle) and the GUI host on
// the main thread (the GuiBridge) — bridge outbound -> engine.send, engine
// replies -> bridge.server_reply — then replays the bundle and opens its
// GuiDef. Resolves when the engine confirms the boot (/synced), so meters and
// scopes are already streaming.

import { bundle_boot_packets } from "./clausters_gui.js";
import { decodePacket } from "./engine/osc.js";

const SYNC_ID = 0xb3;

async function fetchBytes(url) {
    const response = await fetch(url);
    if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
    return new Uint8Array(await response.arrayBuffer());
}

/// Interleaves a decoded AudioBuffer's channels for the engine's bLoad.
function interleave(audioBuffer) {
    const channels = audioBuffer.numberOfChannels;
    const frames = audioBuffer.length;
    const out = new Float32Array(frames * channels);
    for (let ch = 0; ch < channels; ch++) {
        const data = audioBuffer.getChannelData(ch);
        for (let f = 0; f < frames; f++) out[f * channels + ch] = data[f];
    }
    return out;
}

/// Boots the bundle at `base` (a URL prefix) against the in-page engine and
/// GUI host. `onReply(addr, args)` observes every engine reply, after the
/// bridge got it. Returns `{ id, tree }` — the opened GuiDef record.
export async function bootBundle({ bridge, engine, base, name = null, onReply = null }) {
    const manifest = await (await fetch(`${base}/bundle.json`)).json();
    const guiName = name ?? manifest.gui;
    const [synthdefs, graphdefs, record] = await Promise.all([
        Promise.all((manifest.synthdefs ?? []).map(
            (n) => fetchBytes(`${base}/defs/synthdefs/${n}.json`))),
        Promise.all((manifest.graphdefs ?? []).map(
            (n) => fetchBytes(`${base}/defs/graphdefs/${n}.json`))),
        (await fetch(`${base}/defs/guidefs/${guiName}.json`)).json(),
    ]);
    // boot.json is optional — a missing file is an empty preset, as natively.
    const bootResponse = await fetch(`${base}/boot.json`);
    const bootJson = bootResponse.ok ? await bootResponse.text() : null;

    // Wire the legs before anything flows: engine replies feed the host (and
    // resolve our /synced), host outbound feeds the engine.
    let bootedResolve;
    const booted = new Promise((r) => { bootedResolve = r; });
    engine.onReply = (bytes) => {
        bridge.server_reply(bytes);
        for (const { addr, args } of decodePacket(bytes)) {
            if (addr === "/synced" && args[0] === SYNC_ID + 1) bootedResolve();
            onReply?.(addr, args);
        }
    };
    bridge.connect_page((bytes) => engine.send(bytes));

    // Samples first (before the defs' boot messages can /s_new over them).
    for (const [index, url] of Object.entries(manifest.buffers ?? {})) {
        const bytes = await fetchBytes(`${base}/${url}`);
        const decoded = await engine.context.decodeAudioData(bytes.buffer);
        await engine.bLoad(
            Number(index), decoded.numberOfChannels, decoded.sampleRate,
            interleave(decoded),
        );
    }

    const tree = JSON.stringify(record.gui);
    for (const packet of bundle_boot_packets(
        synthdefs, graphdefs, bootJson, tree, SYNC_ID,
    )) {
        engine.send(packet);
    }
    await Promise.race([booted, new Promise((_, reject) =>
        setTimeout(() => reject(new Error("bundle boot: no /synced from the engine")), 15000),
    )]);

    bridge.def(record.id, tree);
    return { id: record.id, tree: record.gui };
}
