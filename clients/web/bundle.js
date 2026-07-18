// Booting a standalone bundle against the page singletons.
//
// The multi-consumer sibling of the gui harness's `clients/gui/web/bundle.js`
// (single-page, single-consumer): here the engine↔host legs are already wired
// by the singletons (`gui.js`), so this module only fetches the bundle's
// persisted files, loads its samples, replays the boot packets and opens the
// GuiDef. The bundle format — the native `--standalone` data directory plus
// the generated `bundle.json` manifest — is documented there and in
// docs/clients.md ("A standalone bundle in a tab").

import { bundle_boot_packets } from "./gui-host/clausters_gui.js";
import { decodePacket } from "./engine/osc.js";
import { server } from "./server.js";
import { guiHost } from "./gui.js";

// Each boot gets its own /sync ids so two bundles on one page cannot
// mistake each other's /synced for their own.
let nextSync = 0xb40;

async function fetchBytes(url) {
    const response = await fetch(url);
    if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
    return new Uint8Array(await response.arrayBuffer());
}

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

/// Boots the bundle at `base` (a URL prefix) against the page singletons,
/// resolving to `{ id, tree }` once the engine confirmed the boot (/synced)
/// and the GuiDef opened. `name` overrides the manifest's GuiDef.
export async function bootBundle({ base, name = null } = {}) {
    const engine = await server();
    const gui = await guiHost();

    const manifest = await (await fetch(`${base}/bundle.json`)).json();
    const guiName = name ?? manifest.gui;
    const [synthdefs, graphdefs, record] = await Promise.all([
        Promise.all((manifest.synthdefs ?? []).map(
            (n) => fetchBytes(`${base}/defs/synthdefs/${n}.json`))),
        Promise.all((manifest.graphdefs ?? []).map(
            (n) => fetchBytes(`${base}/defs/graphdefs/${n}.json`))),
        (await fetch(`${base}/defs/guidefs/${guiName}.json`)).json(),
    ]);
    const bootResponse = await fetch(`${base}/boot.json`);
    const bootJson = bootResponse.ok ? await bootResponse.text() : null;

    // Samples first, so a boot /s_new can already play them.
    for (const [index, url] of Object.entries(manifest.buffers ?? {})) {
        const bytes = await fetchBytes(`${base}/${url}`);
        const decoded = await engine.context.decodeAudioData(bytes.buffer);
        await engine.bLoad(
            Number(index), decoded.numberOfChannels, decoded.sampleRate,
            interleave(decoded),
        );
    }

    const syncId = (nextSync += 2);
    let bootedResolve;
    const booted = new Promise((r) => { bootedResolve = r; });
    const watch = (bytes) => {
        for (const { addr, args } of decodePacket(bytes)) {
            if (addr === "/synced" && args[0] === syncId + 1) bootedResolve();
        }
    };
    engine.addReply(watch);
    try {
        const tree = JSON.stringify(record.gui);
        for (const packet of bundle_boot_packets(
            synthdefs, graphdefs, bootJson, tree, syncId,
        )) {
            engine.send(packet);
        }
        await Promise.race([booted, new Promise((_, reject) =>
            setTimeout(() => reject(
                new Error("bundle boot: no /synced from the engine")), 15000),
        )]);
        gui.bridge.def(record.id, tree);
        return { id: record.id, tree: record.gui };
    } finally {
        engine.removeReply(watch);
    }
}
