// TextDecoder and TextEncoder shims for the AudioWorkletGlobalScope.
//
// The wasm-bindgen glue (clausters_web.js) instantiates both at
// module-evaluation time, and the AudioWorkletGlobalScope ships neither.
// worklet.ts imports this module *before* the glue — ES modules evaluate
// dependencies in import order — so the shims are in place when the glue's top
// level runs.
//
// **The encoder was added when the surface first needed it, and its absence
// was invisible until then.** This file used to carry the decoder alone, on
// the grounds that the `WebServer` surface passed no strings *into* wasm —
// true at the time, and a note saying so sat right here. The delegation door
// (`finishDelegated`, which carries the host's error message) made it false,
// and the failure is not a message about a missing encoder: the glue's top
// level throws, the module never calls `registerProcessor`, and the browser
// says the processor name is not defined. Anything reached from the worklet
// that grows a string argument lands here again, so both shims stay whether or
// not today's surface uses them.
//
// UTF-8 only, which is all wasm-bindgen ever asks for.

class TextDecoderShim {
    encoding: string;
    fatal: boolean;
    ignoreBOM: boolean;

    constructor(label = "utf-8", options: { fatal?: boolean; ignoreBOM?: boolean } = {}) {
        if (!/^utf-?8$/i.test(label)) {
            throw new RangeError(`TextDecoderShim: unsupported encoding ${label}`);
        }
        this.encoding = "utf-8";
        this.fatal = !!options.fatal;
        this.ignoreBOM = !!options.ignoreBOM;
    }

    decode(input?: ArrayBuffer | ArrayBufferView): string {
        if (input === undefined) return "";
        const bytes = input instanceof Uint8Array
            ? input
            : ArrayBuffer.isView(input)
                ? new Uint8Array(input.buffer, input.byteOffset, input.byteLength)
                : new Uint8Array(input);
        // Decode into code points, buffering fromCharCode in chunks so huge
        // strings do not overflow the argument list.
        const units: number[] = [];
        const parts: string[] = [];
        const flush = () => {
            if (units.length) {
                parts.push(String.fromCharCode(...units));
                units.length = 0;
            }
        };
        let i = 0;
        if (!this.ignoreBOM && bytes.length >= 3 &&
            bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf) {
            i = 3;
        }
        const bad = () => {
            if (this.fatal) throw new TypeError("TextDecoderShim: invalid UTF-8");
            return 0xfffd;
        };
        while (i < bytes.length) {
            const b0 = bytes[i++]!;
            let cp;
            if (b0 < 0x80) {
                cp = b0;
            } else if (b0 < 0xc2 || b0 > 0xf4) {
                cp = bad();
            } else {
                const need = b0 < 0xe0 ? 1 : b0 < 0xf0 ? 2 : 3;
                cp = b0 & (0x3f >> need);
                let ok = true;
                for (let k = 0; k < need; k++) {
                    const b = bytes[i];
                    if (b === undefined || (b & 0xc0) !== 0x80) { ok = false; break; }
                    cp = (cp << 6) | (b & 0x3f);
                    i++;
                }
                if (!ok || cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff) ||
                    cp < [0, 0x80, 0x800, 0x10000][need]!) {
                    cp = bad();
                }
            }
            if (cp > 0xffff) {
                cp -= 0x10000;
                units.push(0xd800 + (cp >> 10), 0xdc00 + (cp & 0x3ff));
            } else {
                units.push(cp);
            }
            if (units.length >= 4096) flush();
        }
        flush();
        return parts.join("");
    }
}

if (typeof globalThis.TextDecoder === "undefined") {
    (globalThis as { TextDecoder: unknown }).TextDecoder = TextDecoderShim;
}

class TextEncoderShim {
    readonly encoding = "utf-8";

    encode(input = ""): Uint8Array {
        const out: number[] = [];
        for (let i = 0; i < input.length; i++) {
            let cp = input.charCodeAt(i);
            // A surrogate pair is one code point; a lone surrogate is U+FFFD,
            // which is what a real TextEncoder substitutes.
            if (cp >= 0xd800 && cp <= 0xdbff) {
                const low = input.charCodeAt(i + 1);
                if (low >= 0xdc00 && low <= 0xdfff) {
                    cp = 0x10000 + ((cp - 0xd800) << 10) + (low - 0xdc00);
                    i++;
                } else {
                    cp = 0xfffd;
                }
            } else if (cp >= 0xdc00 && cp <= 0xdfff) {
                cp = 0xfffd;
            }
            if (cp < 0x80) {
                out.push(cp);
            } else if (cp < 0x800) {
                out.push(0xc0 | (cp >> 6), 0x80 | (cp & 0x3f));
            } else if (cp < 0x10000) {
                out.push(0xe0 | (cp >> 12), 0x80 | ((cp >> 6) & 0x3f), 0x80 | (cp & 0x3f));
            } else {
                out.push(
                    0xf0 | (cp >> 18),
                    0x80 | ((cp >> 12) & 0x3f),
                    0x80 | ((cp >> 6) & 0x3f),
                    0x80 | (cp & 0x3f),
                );
            }
        }
        return Uint8Array.from(out);
    }

    /** wasm-bindgen's fast path: encode straight into the wasm heap. Returns
     *  what it wrote, and only ever writes whole code points. */
    encodeInto(input: string, view: Uint8Array): { read: number; written: number } {
        let read = 0;
        let written = 0;
        for (let i = 0; i < input.length; ) {
            const start = i;
            let cp = input.charCodeAt(i);
            let units = 1;
            if (cp >= 0xd800 && cp <= 0xdbff) {
                const low = input.charCodeAt(i + 1);
                if (low >= 0xdc00 && low <= 0xdfff) {
                    cp = 0x10000 + ((cp - 0xd800) << 10) + (low - 0xdc00);
                    units = 2;
                } else {
                    cp = 0xfffd;
                }
            } else if (cp >= 0xdc00 && cp <= 0xdfff) {
                cp = 0xfffd;
            }
            const width = cp < 0x80 ? 1 : cp < 0x800 ? 2 : cp < 0x10000 ? 3 : 4;
            if (written + width > view.length) break;
            if (width === 1) {
                view[written++] = cp;
            } else if (width === 2) {
                view[written++] = 0xc0 | (cp >> 6);
                view[written++] = 0x80 | (cp & 0x3f);
            } else if (width === 3) {
                view[written++] = 0xe0 | (cp >> 12);
                view[written++] = 0x80 | ((cp >> 6) & 0x3f);
                view[written++] = 0x80 | (cp & 0x3f);
            } else {
                view[written++] = 0xf0 | (cp >> 18);
                view[written++] = 0x80 | ((cp >> 12) & 0x3f);
                view[written++] = 0x80 | ((cp >> 6) & 0x3f);
                view[written++] = 0x80 | (cp & 0x3f);
            }
            i = start + units;
            read = i;
        }
        return { read, written };
    }
}

if (typeof globalThis.TextEncoder === "undefined") {
    (globalThis as { TextEncoder: unknown }).TextEncoder = TextEncoderShim;
}
