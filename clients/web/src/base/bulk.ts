// Bulk samples at the boundary: the little-endian `f32` blob, both ways.
//
// **The rule this module exists to keep in one place.** A payload whose length
// scales with the *audio* — a buffer range, a scope window, a waveform to draw
// — crosses as raw little-endian `f32`; a payload whose length scales with the
// *parameters* stays typed OSC arguments (`docs/schemas.md`). The reason is not
// tidiness: N samples as N float arguments costs N type tags and N encode steps
// at each end, which is thousands of times slower than one byte copy at the
// sizes an editor works with, and wider on the wire besides.
//
// So every path carrying samples — `/buffer_setRange` and
// `/buffer_getRange.reply`, `/bus_tapStream.reply`, a `waveform`'s `blob` prop,
// `/buffer_export`'s file — goes through these two functions rather than looping
// per sample. The loop that stays is the typed array's, which is native.
//
// The one thing worth centralizing beyond speed is **endianness**: a
// `Float32Array` is host-endian, so on a big-endian runtime the naive view over
// its buffer is silently wrong. That check belongs to the convention, not to
// each caller of it.

/**
 * Whether this runtime's typed arrays already match the wire's byte order, so
 * the pack and unpack are a straight view (every browser and node target in
 * practice; the check is what keeps the two correct where they are not).
 */
export const LITTLE_ENDIAN = new Uint8Array(Uint16Array.of(1).buffer)[0] === 1;

/**
 * Samples packed as a little-endian `f32` blob. `samples` may be a
 * `Float32Array`, a plain array, or anything iterable — the conversion is one
 * native call, never a loop over the samples here.
 */
export function samplesToBlob(samples: ArrayLike<number> | Iterable<number>): Uint8Array {
    const floats = samples instanceof Float32Array
        ? samples
        : Float32Array.from(samples as Iterable<number>);
    const bytes = new Uint8Array(floats.buffer, floats.byteOffset, floats.byteLength);
    if (LITTLE_ENDIAN) return bytes;
    const view = new DataView(new ArrayBuffer(floats.byteLength));
    for (let i = 0; i < floats.length; i++) view.setFloat32(i * 4, floats[i]!, true);
    return new Uint8Array(view.buffer);
}

/**
 * A little-endian `f32` blob unpacked into a `Float32Array` — the inverse of
 * `samplesToBlob`, and what every reply carrying samples is read with.
 *
 * Throws when the blob is not a whole number of `f32`s, which is the only way
 * it can be malformed.
 */
export function blobToSamples(blob: Uint8Array): Float32Array {
    if (blob.byteLength % 4) {
        throw new Error(
            `a sample blob is little-endian f32: ${blob.byteLength} bytes ` +
                "is not a multiple of 4",
        );
    }
    // A copy rather than a view: the blob is a slice of the receive buffer, and
    // its alignment is not guaranteed to suit a Float32Array.
    const bytes = blob.slice();
    const floats = new Float32Array(bytes.buffer);
    if (LITTLE_ENDIAN) return floats;
    const view = new DataView(bytes.buffer);
    for (let i = 0; i < floats.length; i++) floats[i] = view.getFloat32(i * 4, true);
    return floats;
}
