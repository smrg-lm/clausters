// Minimal OSC 1.0 codec for the harness pages — just enough to talk to the
// worklet engine: encode a message with explicit type tags, decode a reply
// packet (message or bundle). A real JS client arrives with the W track; this
// stays a page-side utility.

const PAD = (n) => (n + 4) & ~3;  // strings: at least one NUL, 4-aligned
const PAD4 = (n) => (n + 3) & ~3; // blobs: 4-aligned, no terminator

function writeString(bytes, offset, s) {
    for (let i = 0; i < s.length; i++) bytes[offset + i] = s.charCodeAt(i);
    return offset + PAD(s.length); // >= 1 NUL terminator, zero-filled pad
}

/// encodeMessage("/s_new", [["s","default"],["i",1000],["f",440]]) -> Uint8Array
export function encodeMessage(addr, args = []) {
    let size = PAD(addr.length) + PAD(1 + args.length);
    for (const [tag, value] of args) {
        if (tag === "i" || tag === "f") size += 4;
        else if (tag === "d" || tag === "h") size += 8;
        else if (tag === "s") size += PAD(String(value).length);
        else if (tag === "b") size += 4 + PAD4(value.length);
        else throw new Error(`osc encode: unsupported tag ${tag}`);
    }
    const bytes = new Uint8Array(size);
    const view = new DataView(bytes.buffer);
    let o = writeString(bytes, 0, addr);
    o = writeString(bytes, o, "," + args.map(([t]) => t).join(""));
    for (const [tag, value] of args) {
        if (tag === "i") { view.setInt32(o, value); o += 4; }
        else if (tag === "f") { view.setFloat32(o, value); o += 4; }
        else if (tag === "d") { view.setFloat64(o, value); o += 8; }
        else if (tag === "h") { view.setBigInt64(o, BigInt(value)); o += 8; }
        else if (tag === "s") { o = writeString(bytes, o, String(value)); }
        else { // b
            view.setInt32(o, value.length); o += 4;
            bytes.set(value, o); o += PAD4(value.length);
        }
    }
    return bytes;
}

function readString(bytes, offset) {
    let end = offset;
    while (bytes[end] !== 0) end++;
    let s = "";
    for (let i = offset; i < end; i++) s += String.fromCharCode(bytes[i]);
    return [s, offset + PAD(end - offset)];
}

function decodeMessage(bytes, view, base) {
    let [addr, o] = readString(bytes, base);
    let tags;
    [tags, o] = readString(bytes, o);
    const args = [];
    for (const tag of tags.slice(1)) {
        if (tag === "i") { args.push(view.getInt32(o)); o += 4; }
        else if (tag === "f") { args.push(view.getFloat32(o)); o += 4; }
        else if (tag === "d") { args.push(view.getFloat64(o)); o += 8; }
        else if (tag === "h") { args.push(Number(view.getBigInt64(o))); o += 8; }
        else if (tag === "s") { let s; [s, o] = readString(bytes, o); args.push(s); }
        else if (tag === "b") {
            const len = view.getInt32(o); o += 4;
            args.push(bytes.slice(o, o + len));
            o += PAD4(len);
        } else if (tag === "T") args.push(true);
        else if (tag === "F") args.push(false);
        else if (tag === "N") args.push(null);
        else throw new Error(`osc decode: unsupported tag ${tag} in ${addr}`);
    }
    return { addr, args };
}

/// decodePacket(Uint8Array) -> [{addr, args}, ...] (a bundle is flattened;
/// its timetag is ignored — replies are immediate).
export function decodePacket(data) {
    const bytes = data instanceof Uint8Array ? data : new Uint8Array(data);
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const messages = [];
    const walk = (start, end) => {
        if (bytes[start] === 0x23) { // '#bundle'
            let o = start + 8 /* "#bundle\0" */ + 8 /* timetag */;
            while (o < end) {
                const size = view.getInt32(o); o += 4;
                walk(o, o + size); o += size;
            }
        } else {
            messages.push(decodeMessage(bytes, view, start));
        }
    };
    walk(0, bytes.byteLength);
    return messages;
}
