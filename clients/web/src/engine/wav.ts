// Just enough WAV to *frame* a file: where the samples start, how they are
// laid out, how long they are.
//
// Deliberately no sample conversion. Reading one goes through the server's own
// decoder (`read_audio_bytes`) and writing one through its own encoder
// (`encodeWavFrames`), so a page and a window turn the same bytes into the same
// numbers. What is left here is parsing chunk offsets and re-emitting a
// canonical header — pure framing, where a second implementation cannot make
// two different answers, only a wrong one.
//
// It exists for streaming: reading a whole file to learn where its samples
// begin is exactly what a stream must not do.

/** How a WAV file is laid out. */
export interface WavShape {
    channels: number;
    sampleRate: number;
    bitsPerSample: number;
    /** WAVE_FORMAT_IEEE_FLOAT rather than PCM. */
    float: boolean;
    /** Byte offset of the `data` chunk's payload. */
    dataOffset: number;
    /** Length of that payload in bytes. */
    dataBytes: number;
    /** Bytes per frame, across all channels. */
    blockAlign: number;
    /** Frames the file holds. */
    frames: number;
}

/** How many bytes of the head are enough to find `fmt ` and `data` in any
 *  ordinary file. A WAV with a very large metadata chunk before `data` needs
 *  more, and `parseShape` says so rather than guessing. */
export const HEAD_BYTES = 4096;

/**
 * Reads the shape out of the first bytes of a file. Throws if those bytes do
 * not contain both chunks — read more and call again.
 */
export function parseShape(head: Uint8Array): WavShape {
    const view = new DataView(head.buffer, head.byteOffset, head.byteLength);
    const tag = (at: number) => String.fromCharCode(...head.subarray(at, at + 4));
    if (head.byteLength < 12 || tag(0) !== "RIFF" || tag(8) !== "WAVE") {
        throw new Error("not a RIFF/WAVE file");
    }
    let at = 12;
    let fmt: { channels: number; sampleRate: number; bits: number; float: boolean } | null = null;
    while (at + 8 <= head.byteLength) {
        const id = tag(at);
        const size = view.getUint32(at + 4, true);
        const body = at + 8;
        if (id === "fmt " && body + 16 <= head.byteLength) {
            const code = view.getUint16(body, true);
            fmt = {
                channels: view.getUint16(body + 2, true),
                sampleRate: view.getUint32(body + 4, true),
                bits: view.getUint16(body + 14, true),
                // 3 is IEEE float; 0xfffe is extensible, whose real code sits
                // in the sub-format GUID's first two bytes.
                float:
                    code === 3 ||
                    (code === 0xfffe &&
                        body + 26 <= head.byteLength &&
                        view.getUint16(body + 24, true) === 3),
            };
        } else if (id === "data") {
            if (fmt === null) throw new Error("the data chunk came before fmt");
            const blockAlign = fmt.channels * Math.ceil(fmt.bits / 8);
            return {
                channels: fmt.channels,
                sampleRate: fmt.sampleRate,
                bitsPerSample: fmt.bits,
                float: fmt.float,
                dataOffset: body,
                dataBytes: size,
                blockAlign,
                frames: blockAlign > 0 ? Math.floor(size / blockAlign) : 0,
            };
        }
        // Chunks are word-aligned; an odd size carries a pad byte.
        at = body + size + (size % 2);
    }
    throw new Error(`no data chunk in the first ${head.byteLength} bytes`);
}

/**
 * A whole small WAV: a canonical header the given shape describes, followed by
 * `body`. This is how a *span* of a big file becomes something the decoder can
 * read — the decoder wants a file, and a range of one is not a file until it
 * has a header of its own.
 */
export function wrapSpan(shape: WavShape, body: Uint8Array): Uint8Array<ArrayBuffer> {
    const out = new Uint8Array(44 + body.byteLength);
    const view = new DataView(out.buffer);
    const ascii = (at: number, s: string) => {
        for (let i = 0; i < s.length; i++) view.setUint8(at + i, s.charCodeAt(i));
    };
    ascii(0, "RIFF");
    view.setUint32(4, 36 + body.byteLength, true);
    ascii(8, "WAVEfmt ");
    view.setUint32(16, 16, true);
    view.setUint16(20, shape.float ? 3 : 1, true);
    view.setUint16(22, shape.channels, true);
    view.setUint32(24, shape.sampleRate, true);
    view.setUint32(28, shape.sampleRate * shape.blockAlign, true);
    view.setUint16(32, shape.blockAlign, true);
    view.setUint16(34, shape.bitsPerSample, true);
    ascii(36, "data");
    view.setUint32(40, body.byteLength, true);
    out.set(body, 44);
    return out;
}
