// Bulk samples: fetching audio the page decodes, and interleaving it.
//
// The browser's counterpart of the server's file reading. A page has no
// filesystem, so `/b_allocRead`'s path means nothing to it; what it has is
// `fetch` and `decodeAudioData`, which between them turn a URL into decoded
// float samples — every format the browser plays, decoded by the browser.
//
// These are pure functions over a URL and an `AudioBuffer`: nothing here
// knows a server or a carrier. `Server.loadSample` is what puts the result
// into a buffer, and the reverse direction (reading a server buffer back out)
// is `Server.getSamples` — one is `fetch`, the other `/b_getn`, and a
// waveform view does not care which fed it.

/**
 * Interleaves a decoded `AudioBuffer` into the flat `L R L R …` layout every
 * buffer in the system uses (`frame * channels + channel`).
 */
export function interleave(audio: AudioBuffer): Float32Array {
    const channels = audio.numberOfChannels;
    const frames = audio.length;
    const out = new Float32Array(frames * channels);
    for (let ch = 0; ch < channels; ch++) {
        const data = audio.getChannelData(ch);
        for (let f = 0; f < frames; f++) out[f * channels + ch] = data[f];
    }
    return out;
}

/**
 * De-interleaves `samples` into one array per channel — what a per-channel
 * view (a waveform lane, a correlation of a stereo pair) reads.
 */
export function deinterleave(
    samples: ArrayLike<number>,
    channels: number,
): Float32Array[] {
    const n = Math.max(1, Math.trunc(channels));
    const frames = Math.floor(samples.length / n);
    const out = Array.from({ length: n }, () => new Float32Array(frames));
    for (let f = 0; f < frames; f++) {
        for (let ch = 0; ch < n; ch++) out[ch][f] = samples[f * n + ch];
    }
    return out;
}

/**
 * Fetches an audio file and decodes it with the page's own decoder, resolving
 * with the `AudioBuffer`.
 *
 * **The decode resamples to the context's rate**, so which context decodes is
 * not a detail: pass the one whose rate the samples are going to be played at
 * — the engine's `AudioContext` in the page, or a rate matching the server
 * over a socket, which is what `sampleRate` builds a scratch context for.
 */
export async function fetchAudio(
    url: string,
    {
        context,
        sampleRate = 48000,
    }: { context?: BaseAudioContext; sampleRate?: number } = {},
): Promise<AudioBuffer> {
    const response = await fetch(url);
    if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
    const bytes = await response.arrayBuffer();
    // A one-frame offline context decodes without an audio device and without
    // the page's gesture: it is a decoder here, never a renderer.
    const ctx = context ?? new OfflineAudioContext(1, 1, sampleRate);
    return ctx.decodeAudioData(bytes);
}
