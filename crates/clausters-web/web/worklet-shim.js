// TextDecoder shim for the AudioWorkletGlobalScope.
//
// The wasm-bindgen glue (clausters_web.js) instantiates a TextDecoder at
// module-evaluation time, and the AudioWorkletGlobalScope ships neither
// TextDecoder nor TextEncoder. worklet.js imports this module *before* the
// glue — ES modules evaluate dependencies in import order — so the shim is in
// place when the glue's top level runs. Only the decoder is needed: the
// WebServer surface passes no strings into wasm (numbers and byte arrays
// only); strings come *out* (error messages), through decode().
//
// UTF-8 only, which is all wasm-bindgen ever asks for.

class TextDecoderShim {
    constructor(label = "utf-8", options = {}) {
        if (!/^utf-?8$/i.test(label)) {
            throw new RangeError(`TextDecoderShim: unsupported encoding ${label}`);
        }
        this.encoding = "utf-8";
        this.fatal = !!options.fatal;
        this.ignoreBOM = !!options.ignoreBOM;
    }

    decode(input) {
        if (input === undefined) return "";
        const bytes = input instanceof Uint8Array
            ? input
            : ArrayBuffer.isView(input)
                ? new Uint8Array(input.buffer, input.byteOffset, input.byteLength)
                : new Uint8Array(input);
        // Decode into code points, buffering fromCharCode in chunks so huge
        // strings do not overflow the argument list.
        const units = [];
        const parts = [];
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
            const b0 = bytes[i++];
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
                    cp < [0, 0x80, 0x800, 0x10000][need]) {
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
    globalThis.TextDecoder = TextDecoderShim;
}
