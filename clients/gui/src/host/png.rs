//! **A picture as a PNG file**, with no dependency: what a window's capture
//! (`/gui_capture`) is written as.
//!
//! The image data is deflate in **stored** blocks -- no compression at all --
//! which is the part of the format that needs no code worth the name: a block
//! header, the bytes, and the two checksums the container asks for. A capture
//! is read once and thrown away, so its size does not matter, and a codec of
//! our own here would be the kind of thing that is wrong in one file in a
//! thousand. Every viewer and every image library reads a stored PNG.
//!
//! Pure: it compiles to wasm with the rest of the core, since a page writes
//! its captures into its own storage.

/// The eight bytes every PNG starts with.
const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

/// The largest stored deflate block: its length is a 16-bit field.
const STORED_MAX: usize = 0xffff;

/// **Encodes `rgba` as a PNG of `width` by `height`**: eight bits per channel,
/// with alpha, rows top to bottom and `4 * width` bytes each.
///
/// Panics when `rgba` is not exactly that long -- a caller that read back a
/// texture knows its size, so a mismatch is a bug in the caller.
pub fn encode_rgba(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let row = width as usize * 4;
    assert_eq!(
        rgba.len(),
        row * height as usize,
        "the pixels are not {width}x{height} RGBA"
    );
    // Each scanline is preceded by its filter type: 0, none.
    let mut raw = Vec::with_capacity((row + 1) * height as usize);
    for line in rgba.chunks_exact(row.max(1)).take(height as usize) {
        raw.push(0);
        raw.extend_from_slice(line);
    }
    if row == 0 {
        raw.resize(height as usize, 0);
    }

    let mut out = Vec::with_capacity(raw.len() + raw.len() / STORED_MAX * 5 + 64);
    out.extend_from_slice(&SIGNATURE);
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    // Bit depth 8, color type 6 (RGBA), deflate, adaptive filtering, no
    // interlace.
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &zlib_stored(&raw));
    chunk(&mut out, b"IEND", &[]);
    out
}

/// One chunk: its length, its type, its data and the CRC of type and data.
fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// `data` as a zlib stream of stored deflate blocks.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / STORED_MAX * 5 + 11);
    // CMF: deflate with a 32 KiB window; FLG: no dictionary, and the check
    // bits that make the pair a multiple of 31.
    out.extend_from_slice(&[0x78, 0x01]);
    let mut blocks = data.chunks(STORED_MAX).peekable();
    if blocks.peek().is_none() {
        // An empty stream is still one (final, empty) block.
        out.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    while let Some(block) = blocks.next() {
        let last = blocks.peek().is_none();
        out.push(u8::from(last));
        let len = block.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(block);
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

/// The CRC-32 the PNG container checks each chunk with (the IEEE polynomial,
/// reflected).
fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for (n, slot) in table.iter_mut().enumerate() {
            let mut c = n as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *slot = c;
        }
        table
    });
    let mut c = 0xffff_ffffu32;
    for &b in data {
        c = table[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8);
    }
    c ^ 0xffff_ffff
}

/// The Adler-32 a zlib stream ends with.
fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    let (mut a, mut b) = (1u32, 0u32);
    // 5552 bytes is the most that can be summed before `b` could overflow.
    for block in data.chunks(5552) {
        for &x in block {
            a += u32::from(x);
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two checksums against their published check values.
    #[test]
    fn the_checksums_match_their_check_values() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
        assert_eq!(adler32(b""), 1);
    }

    /// Reads the chunks of a PNG back: their types, and whether each CRC
    /// holds.
    fn chunks(png: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
        assert_eq!(&png[..8], &SIGNATURE);
        let mut at = 8;
        let mut out = Vec::new();
        while at < png.len() {
            let len = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
            let kind: [u8; 4] = png[at + 4..at + 8].try_into().unwrap();
            let data = png[at + 8..at + 8 + len].to_vec();
            let crc = u32::from_be_bytes(png[at + 8 + len..at + 12 + len].try_into().unwrap());
            assert_eq!(
                crc,
                crc32(&png[at + 4..at + 8 + len]),
                "the CRC of {kind:?}"
            );
            out.push((kind, data));
            at += 12 + len;
        }
        out
    }

    /// Inflates a stream of stored blocks, checking each block's length
    /// against its complement and the stream's Adler-32.
    fn inflate_stored(z: &[u8]) -> Vec<u8> {
        assert_eq!((u16::from(z[0]) << 8 | u16::from(z[1])) % 31, 0);
        let mut at = 2;
        let mut out = Vec::new();
        loop {
            let last = z[at] & 1 == 1;
            assert_eq!(z[at] >> 1, 0, "a stored block");
            let len = u16::from_le_bytes([z[at + 1], z[at + 2]]);
            let nlen = u16::from_le_bytes([z[at + 3], z[at + 4]]);
            assert_eq!(len, !nlen);
            out.extend_from_slice(&z[at + 5..at + 5 + len as usize]);
            at += 5 + len as usize;
            if last {
                break;
            }
        }
        assert_eq!(
            u32::from_be_bytes(z[at..at + 4].try_into().unwrap()),
            adler32(&out)
        );
        out
    }

    /// **A picture is written whole and reads back as itself**: the header
    /// says its size and kind, and the scanlines are the pixels, each after
    /// its filter byte.
    #[test]
    fn a_picture_reads_back_as_itself() {
        let (w, h) = (3u32, 2u32);
        let rgba: Vec<u8> = (0..w * h * 4).map(|n| n as u8).collect();
        let png = encode_rgba(w, h, &rgba);
        let chunks = chunks(&png);
        let kinds: Vec<&[u8; 4]> = chunks.iter().map(|(k, _)| k).collect();
        assert_eq!(kinds, [b"IHDR", b"IDAT", b"IEND"]);
        let header = &chunks[0].1;
        assert_eq!(&header[..4], &w.to_be_bytes());
        assert_eq!(&header[4..8], &h.to_be_bytes());
        assert_eq!(&header[8..], &[8, 6, 0, 0, 0]);
        let raw = inflate_stored(&chunks[1].1);
        let mut expected = Vec::new();
        for line in rgba.chunks(12) {
            expected.push(0);
            expected.extend_from_slice(line);
        }
        assert_eq!(raw, expected);
    }

    /// **A picture larger than a stored block is split across several**, and
    /// only the last says it is.
    #[test]
    fn a_large_picture_spans_several_blocks() {
        let (w, h) = (300u32, 200u32);
        let rgba = vec![7u8; (w * h * 4) as usize];
        let png = encode_rgba(w, h, &rgba);
        let chunks = chunks(&png);
        let raw = inflate_stored(&chunks[1].1);
        assert_eq!(raw.len(), ((w * 4 + 1) * h) as usize);
        assert!(raw.len() > STORED_MAX);
    }
}
