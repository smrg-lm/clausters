//! **A PCM file read at any frame**: the shape of a sound file whose samples
//! sit in one run of bytes, and a reader that lands on a frame by arithmetic.
//!
//! `DiskIn` decodes through symphonia and reaches a start frame by decoding up
//! to it, because a compressed format has no exact frame seek. That is the
//! right reading for a file played once from its top and the wrong one for a
//! take an editor places, cuts and locates into: an hour into a file is an
//! hour of decoding. A PCM file needs none of it -- frame `n` is `n` block
//! alignments past the start of its data -- so a part of a segment list that
//! names a file reads it through this module, and a file a list can name is a
//! PCM one.
//!
//! The parse is over any `Read + Seek`, so the same code reads a header from a
//! file natively and from bytes a page's host handed over. What it finds is a
//! [`Layout`]: where the samples start, how a frame is laid out and how many
//! there are. Nothing here holds the file open; each reader opens its own.
//!
//! # Three containers, one body
//!
//! A RIFF WAV counts its sizes in 32 bits, so it ends at 4 GB -- under seven
//! hours of 48 kHz stereo floats. The two standard ways past that keep the
//! samples exactly as a WAV lays them out and change only the header:
//!
//! - **RF64** (EBU Tech 3306) is a WAV whose `RIFF` says `RF64`, whose 32-bit
//!   sizes are all ones, and whose `ds64` chunk carries the real sizes in 64
//!   bits.
//! - **W64** (Sony Wave64) names every chunk by a 16-byte GUID and sizes it in
//!   64 bits, each size counting its own 24-byte header and each chunk padded
//!   to 8 bytes.
//!
//! A WAV written here keeps an RF64's room and becomes one at its close when
//! its body outgrew 32 bits, under the name it was given ([`header`]): a
//! `.wav` is never refused for its length.
//!
//! So a file is one [`Container`] -- chosen by its extension when one is
//! written, `.w64` or `.rf64`, a WAV otherwise -- and one body, and everything
//! that reads or writes a sample reads or writes it the same way in all
//! three. [`header`] writes the header of each, [`Writer`] streams a body
//! behind one and goes back to fill in the sizes, and [`Layout::parse`] reads
//! any of them.

use std::io::{Read, Seek, SeekFrom, Write};

/// How one sample is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    /// 8-bit unsigned, centred on 128.
    U8,
    I16,
    I24,
    I32,
    F32,
    F64,
}

impl Encoding {
    /// The encoding a writer is asked for by name -- the writing family's
    /// names, `int16`, `int24` and `float`.
    pub fn named(name: &str) -> Result<Self, String> {
        match name {
            "int16" => Ok(Encoding::I16),
            "int24" => Ok(Encoding::I24),
            "float" | "float32" => Ok(Encoding::F32),
            other => Err(format!("unsupported sample format {other:?}")),
        }
    }

    /// The format tag and bit depth a `fmt ` chunk states for it.
    fn tag_bits(self) -> (u16, u16) {
        match self {
            Encoding::U8 => (1, 8),
            Encoding::I16 => (1, 16),
            Encoding::I24 => (1, 24),
            Encoding::I32 => (1, 32),
            Encoding::F32 => (3, 32),
            Encoding::F64 => (3, 64),
        }
    }

    /// Bytes per sample.
    pub fn bytes(self) -> usize {
        match self {
            Encoding::U8 => 1,
            Encoding::I16 => 2,
            Encoding::I24 => 3,
            Encoding::I32 | Encoding::F32 => 4,
            Encoding::F64 => 8,
        }
    }

    /// One sample, decoded from its little-endian bytes.
    #[inline]
    pub fn decode(self, b: &[u8]) -> f32 {
        match self {
            Encoding::U8 => (b[0] as f32 - 128.0) / 128.0,
            Encoding::I16 => i16::from_le_bytes([b[0], b[1]]) as f32 / 32_768.0,
            Encoding::I24 => {
                let v = i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8;
                v as f32 / 8_388_608.0
            }
            Encoding::I32 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
            Encoding::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            Encoding::F64 => {
                f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f32
            }
        }
    }
}

/// How a file's header is laid out around its samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Container {
    /// RIFF WAV: 32-bit sizes, so at most 4 GB; one written here is closed
    /// as an RF64 past that ([`header`]).
    Wav,
    /// Sony Wave64: GUID chunks with 64-bit sizes.
    W64,
    /// EBU RF64: a WAV with its sizes in a `ds64` chunk.
    Rf64,
}

impl Container {
    /// The container a file is written in, by its extension: `.w64` and
    /// `.rf64` name theirs, and anything else is a WAV.
    pub fn for_path(path: &str) -> Self {
        let ext = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        if ext.eq_ignore_ascii_case("w64") {
            Container::W64
        } else if ext.eq_ignore_ascii_case("rf64") {
            Container::Rf64
        } else {
            Container::Wav
        }
    }

    /// The container a `/buffer_write` header argument names: `w64` and
    /// `rf64` name theirs, and `wav` leaves it to the path, so a `.w64` path
    /// written with the default header is still a W64.
    pub fn for_header(header: &str, path: &str) -> Result<Self, String> {
        if header.eq_ignore_ascii_case("w64") {
            Ok(Container::W64)
        } else if header.eq_ignore_ascii_case("rf64") {
            Ok(Container::Rf64)
        } else if header.eq_ignore_ascii_case("wav") || header.eq_ignore_ascii_case("wave") {
            Ok(Container::for_path(path))
        } else {
            Err(format!("unsupported header format {header:?}"))
        }
    }

    /// The name a header argument and a page give it.
    pub fn name(self) -> &'static str {
        match self {
            Container::Wav => "wav",
            Container::W64 => "w64",
            Container::Rf64 => "rf64",
        }
    }

    /// Whether `ext` (without the dot) names a container written here.
    pub fn names(ext: &str) -> bool {
        ["wav", "wave", "w64", "rf64"]
            .iter()
            .any(|e| ext.eq_ignore_ascii_case(e))
    }
}

/// Wave64's chunk GUIDs: the four-letter id, then the same tail for all but
/// `riff`.
const W64_RIFF: [u8; 16] = [
    0x72, 0x69, 0x66, 0x66, 0x2E, 0x91, 0xCF, 0x11, 0xA5, 0xD6, 0x28, 0xDB, 0x04, 0xC1, 0x00, 0x00,
];
const W64_TAIL: [u8; 12] = [
    0xF3, 0xAC, 0xD3, 0x11, 0x8C, 0xD1, 0x00, 0xC0, 0x4F, 0x8E, 0xDB, 0x8A,
];

fn w64_guid(id: &[u8; 4]) -> [u8; 16] {
    let mut g = [0u8; 16];
    g[..4].copy_from_slice(id);
    g[4..].copy_from_slice(&W64_TAIL);
    g
}

/// Where a PCM file keeps its samples and how they are laid out.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    /// The file it describes.
    pub path: String,
    /// The header around the samples.
    pub container: Container,
    /// The byte offset of frame 0.
    pub data_offset: u64,
    pub channels: usize,
    pub sample_rate: f64,
    pub encoding: Encoding,
    /// Whole frames of samples; a trailing partial frame is not one.
    pub frames: u64,
}

impl Layout {
    /// **The shape of a file read somewhere else**: in a page, where the host
    /// reads the page's files and hands the samples over already decoded, so
    /// the offset and the encoding are the host's to know and are not kept.
    pub fn read_elsewhere(path: &str, channels: usize, sample_rate: f64, frames: u64) -> Self {
        Layout {
            path: path.to_string(),
            container: Container::for_path(path),
            data_offset: 0,
            channels,
            sample_rate,
            encoding: Encoding::F32,
            frames,
        }
    }

    /// Bytes per frame.
    pub fn frame_bytes(&self) -> usize {
        self.channels * self.encoding.bytes()
    }

    /// Reads the header of the file at `path`.
    pub fn open(path: &str) -> Result<Self, String> {
        let mut file = crate::files::File::open(path).map_err(|e| format!("{path}: {e}"))?;
        Self::parse(&mut file, path)
    }

    /// Reads a header from `source`, describing the file at `path`: a WAV,
    /// an RF64 or a W64, whatever its name says.
    pub fn parse<R: Read + Seek>(source: &mut R, path: &str) -> Result<Self, String> {
        Self::parse_within(source, path, None)
    }

    /// [`Layout::parse`] over the **head** of a file whose whole length is
    /// `len`: what a page reads to learn where a file's samples are without
    /// reading them. `None` takes the source's own end.
    pub fn parse_within<R: Read + Seek>(
        source: &mut R,
        path: &str,
        len: Option<u64>,
    ) -> Result<Self, String> {
        let err = |e: &str| format!("{path}: {e}");
        let io = |e: std::io::Error| format!("{path}: {e}");
        let mut head = [0u8; 16];
        source.read_exact(&mut head[..12]).map_err(io)?;
        let end = match len {
            Some(len) => len,
            None => source.seek(SeekFrom::End(0)).map_err(io)?,
        };
        let (container, found) = match (&head[0..4], &head[8..12]) {
            (b"RIFF", b"WAVE") => (Container::Wav, riff_chunks(source, end, false)),
            (b"RF64", b"WAVE") => (Container::Rf64, riff_chunks(source, end, true)),
            _ => {
                source.seek(SeekFrom::Start(0)).map_err(io)?;
                source.read_exact(&mut head).map_err(io)?;
                if head != W64_RIFF {
                    return Err(err("not a PCM WAV, RF64 or W64 file"));
                }
                (Container::W64, w64_chunks(source, end))
            }
        };
        let (fmt, data_offset, size) = found.map_err(|e| err(&e))?;
        let (encoding, channels, sample_rate) = parse_fmt(&fmt).map_err(|e| err(&e))?;
        let frame = (channels * encoding.bytes()) as u64;
        // A data chunk still being written, or one whose size a writer never
        // went back to fill in, says less than the file holds; the file's own
        // length is the bound.
        let held = end.saturating_sub(data_offset);
        let bytes = match size {
            Some(size) if size > 0 => size.min(held),
            _ => held,
        };
        Ok(Layout {
            path: path.to_string(),
            container,
            data_offset,
            channels,
            sample_rate,
            encoding,
            frames: bytes / frame,
        })
    }
}

/// A RIFF or RF64 body's `fmt ` chunk and where its samples are: the bytes
/// of the format, the data's offset and its size (`None` when the header
/// does not say). An RF64 states the data's size in its `ds64` chunk, and the
/// 32-bit size of its `data` chunk is all ones.
fn riff_chunks<R: Read + Seek>(
    source: &mut R,
    end: u64,
    rf64: bool,
) -> Result<(Vec<u8>, u64, Option<u64>), String> {
    let io = |e: std::io::Error| e.to_string();
    let mut fmt = None;
    let mut ds64_data = None;
    let mut at = 12u64;
    source.seek(SeekFrom::Start(at)).map_err(io)?;
    while at + 8 <= end {
        let mut chunk = [0u8; 8];
        source.read_exact(&mut chunk).map_err(io)?;
        let size = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]) as u64;
        let body = at + 8;
        match &chunk[0..4] {
            b"fmt " => {
                let mut bytes = vec![0u8; size.min(64) as usize];
                source.read_exact(&mut bytes).map_err(io)?;
                fmt = Some(bytes);
            }
            b"ds64" if rf64 => {
                let mut bytes = [0u8; 24];
                source.read_exact(&mut bytes).map_err(io)?;
                ds64_data = Some(u64::from_le_bytes(bytes[8..16].try_into().unwrap()));
            }
            b"data" => {
                let fmt = fmt.ok_or("the data chunk comes before the format")?;
                let size = if size == u32::MAX as u64 {
                    ds64_data
                } else {
                    Some(size)
                };
                return Ok((fmt, body, size));
            }
            _ => {}
        }
        // Chunks are padded to an even length.
        at = body + size + (size & 1);
        source.seek(SeekFrom::Start(at)).map_err(io)?;
    }
    Err("no data chunk".into())
}

/// A W64 body's `fmt ` chunk and where its samples are. Each chunk is a GUID
/// and a 64-bit size that counts its own 24 bytes, padded to 8.
fn w64_chunks<R: Read + Seek>(
    source: &mut R,
    end: u64,
) -> Result<(Vec<u8>, u64, Option<u64>), String> {
    let io = |e: std::io::Error| e.to_string();
    let mut fmt = None;
    // The riff GUID, the file size and the wave GUID.
    let mut at = 40u64;
    while at + 24 <= end {
        source.seek(SeekFrom::Start(at)).map_err(io)?;
        let mut chunk = [0u8; 24];
        source.read_exact(&mut chunk).map_err(io)?;
        let size = u64::from_le_bytes(chunk[16..24].try_into().unwrap());
        if size < 24 {
            return Err("a chunk smaller than its own header".into());
        }
        let body = at + 24;
        if chunk[..16] == w64_guid(b"fmt ") {
            let mut bytes = vec![0u8; (size - 24).min(64) as usize];
            source.read_exact(&mut bytes).map_err(io)?;
            fmt = Some(bytes);
        } else if chunk[..16] == w64_guid(b"data") {
            let fmt = fmt.ok_or("the data chunk comes before the format")?;
            return Ok((fmt, body, Some(size - 24)));
        }
        at += size.div_ceil(8) * 8;
    }
    Err("no data chunk".into())
}

/// **The header of a file whose body is `data_bytes` of samples**, in its
/// container: what a writer puts before the samples, and writes again once it
/// knows how many there were.
///
/// **A WAV keeps room to become an RF64**, the way EBU Tech 3306 says to
/// write one: a `JUNK` chunk the size of a `ds64` before its format, so the
/// header is as long as an RF64's. A body past what 32 bits count is then the
/// same header in place, `RIFF` turned to `RF64` and `JUNK` to `ds64` with
/// the real sizes: a writer that only learns the length at its close (a
/// render, a recording, a page's writer in pieces) never has to move the
/// samples, and a `.wav` of hours is written rather than refused at 4 GB.
pub fn header(
    container: Container,
    channels: usize,
    sample_rate: f64,
    encoding: Encoding,
    data_bytes: u64,
) -> Result<Vec<u8>, String> {
    let (tag, bits) = encoding.tag_bits();
    let channels16 = u16::try_from(channels).map_err(|_| format!("{channels} channels"))?;
    let rate = sample_rate.round() as u32;
    let block = channels16 * (bits / 8);
    let mut fmt = Vec::with_capacity(16);
    fmt.extend_from_slice(&tag.to_le_bytes());
    fmt.extend_from_slice(&channels16.to_le_bytes());
    fmt.extend_from_slice(&rate.to_le_bytes());
    fmt.extend_from_slice(&(rate * u32::from(block)).to_le_bytes());
    fmt.extend_from_slice(&block.to_le_bytes());
    fmt.extend_from_slice(&bits.to_le_bytes());
    let mut out = Vec::with_capacity(104);
    match container {
        Container::Wav => {
            match u32::try_from(72 + data_bytes) {
                // what 32 bits count: a WAV, the ds64's room left as JUNK
                Ok(riff) => {
                    out.extend_from_slice(b"RIFF");
                    out.extend_from_slice(&riff.to_le_bytes());
                    out.extend_from_slice(b"WAVEJUNK");
                    out.extend_from_slice(&28u32.to_le_bytes());
                    out.extend_from_slice(&[0u8; 28]);
                    out.extend_from_slice(b"fmt ");
                    out.extend_from_slice(&16u32.to_le_bytes());
                    out.extend_from_slice(&fmt);
                    out.extend_from_slice(b"data");
                    out.extend_from_slice(&(data_bytes as u32).to_le_bytes());
                }
                Err(_) => {
                    return header(Container::Rf64, channels, sample_rate, encoding, data_bytes);
                }
            }
        }
        Container::Rf64 => {
            let frames = data_bytes / u64::from(block.max(1));
            out.extend_from_slice(b"RF64");
            out.extend_from_slice(&u32::MAX.to_le_bytes());
            out.extend_from_slice(b"WAVEds64");
            out.extend_from_slice(&28u32.to_le_bytes());
            // The RIFF size counts everything after its own eight bytes.
            out.extend_from_slice(&(72 + data_bytes).to_le_bytes());
            out.extend_from_slice(&data_bytes.to_le_bytes());
            out.extend_from_slice(&frames.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(b"fmt ");
            out.extend_from_slice(&16u32.to_le_bytes());
            out.extend_from_slice(&fmt);
            out.extend_from_slice(b"data");
            out.extend_from_slice(&u32::MAX.to_le_bytes());
        }
        Container::W64 => {
            out.extend_from_slice(&W64_RIFF);
            // The whole file: this header's 104 bytes and the body, which
            // is the last chunk and so is not padded.
            out.extend_from_slice(&(104 + data_bytes).to_le_bytes());
            out.extend_from_slice(&w64_guid(b"wave"));
            out.extend_from_slice(&w64_guid(b"fmt "));
            out.extend_from_slice(&40u64.to_le_bytes());
            out.extend_from_slice(&fmt);
            out.extend_from_slice(&w64_guid(b"data"));
            out.extend_from_slice(&(24 + data_bytes).to_le_bytes());
        }
    }
    Ok(out)
}

/// The integer a `bits`-bit file stores for the sample `s`: clamped to
/// `[-1, 1]`, scaled to the positive full scale and rounded. Every writer
/// quantizes through it, so a file and a page's piece of one hold the same
/// bytes for the same samples.
pub fn quantize(s: f32, bits: u16) -> i32 {
    let scale = ((1u64 << (bits - 1)) - 1) as f32;
    (s.clamp(-1.0, 1.0) * scale).round() as i32
}

/// Appends `samples`, encoded, to `out`: the body every container shares.
pub fn encode(samples: &[f32], encoding: Encoding, out: &mut Vec<u8>) {
    out.reserve(samples.len() * encoding.bytes());
    match encoding {
        Encoding::F32 => samples
            .iter()
            .for_each(|s| out.extend_from_slice(&s.to_le_bytes())),
        Encoding::F64 => samples
            .iter()
            .for_each(|&s| out.extend_from_slice(&(s as f64).to_le_bytes())),
        Encoding::U8 => samples
            .iter()
            .for_each(|&s| out.push((quantize(s, 8) + 128) as u8)),
        Encoding::I16 | Encoding::I24 | Encoding::I32 => {
            let bits = encoding.tag_bits().1;
            let width = encoding.bytes();
            for &s in samples {
                out.extend_from_slice(&quantize(s, bits).to_le_bytes()[..width]);
            }
        }
    }
}

/// **A file written as it goes**: a header with no sizes yet, the body
/// appended as it arrives, and the header written again with the sizes at
/// [`Writer::finalize`]. The container is the path's ([`Container::for_path`]).
pub struct Writer {
    path: String,
    file: std::io::BufWriter<crate::files::File>,
    container: Container,
    channels: usize,
    sample_rate: f64,
    encoding: Encoding,
    data_bytes: u64,
    bytes: Vec<u8>,
}

impl Writer {
    /// Creates the file at `path` for `channels` channels at `sample_rate`,
    /// in the encoding named `sample_format` (`int16`, `int24`, `float`).
    pub fn create(
        path: &str,
        channels: usize,
        sample_rate: f64,
        sample_format: &str,
    ) -> Result<Self, String> {
        Self::create_in(
            path,
            Container::for_path(path),
            channels,
            sample_rate,
            sample_format,
        )
    }

    /// [`Writer::create`] in a container a caller names rather than the
    /// path's.
    pub fn create_in(
        path: &str,
        container: Container,
        channels: usize,
        sample_rate: f64,
        sample_format: &str,
    ) -> Result<Self, String> {
        let encoding = Encoding::named(sample_format)?;
        let head = header(container, channels, sample_rate, encoding, 0)
            .map_err(|e| format!("{path}: {e}"))?;
        let file = crate::files::File::create(path).map_err(|e| format!("{path}: {e}"))?;
        let mut file = std::io::BufWriter::with_capacity(1 << 16, file);
        file.write_all(&head).map_err(|e| format!("{path}: {e}"))?;
        Ok(Self {
            path: path.to_string(),
            file,
            container,
            channels,
            sample_rate,
            encoding,
            data_bytes: 0,
            bytes: Vec::new(),
        })
    }

    /// Appends interleaved samples. A WAV that grows past 4 GB is closed as
    /// an RF64 ([`header`]).
    pub fn write(&mut self, samples: &[f32]) -> Result<(), String> {
        let add = (samples.len() * self.encoding.bytes()) as u64;
        self.bytes.clear();
        encode(samples, self.encoding, &mut self.bytes);
        self.file
            .write_all(&self.bytes)
            .map_err(|e| format!("{}: {e}", self.path))?;
        self.data_bytes += add;
        Ok(())
    }

    /// Pads the body as the container wants, and writes the header again
    /// with the sizes.
    pub fn finalize(mut self) -> Result<(), String> {
        let err = |e: std::io::Error| format!("{}: {e}", self.path);
        // A RIFF chunk is padded to an even length. W64 pads to 8, but the
        // data is the last chunk, and a reader that sizes a W64 by its file's
        // length would read the padding as one more frame.
        let pad = match self.container {
            Container::Wav | Container::Rf64 => (self.data_bytes & 1) as usize,
            Container::W64 => 0,
        };
        self.file.write_all(&[0u8; 8][..pad]).map_err(err)?;
        let head = header(
            self.container,
            self.channels,
            self.sample_rate,
            self.encoding,
            self.data_bytes,
        )
        .map_err(|e| format!("{}: {e}", self.path))?;
        self.file.seek(SeekFrom::Start(0)).map_err(err)?;
        self.file.write_all(&head).map_err(err)?;
        self.file.flush().map_err(err)?;
        Ok(())
    }
}

/// The `fmt ` chunk: the encoding, the channel count and the rate.
fn parse_fmt(fmt: &[u8]) -> Result<(Encoding, usize, f64), String> {
    if fmt.len() < 16 {
        return Err("a format chunk too short to read".into());
    }
    let u16_at = |i: usize| u16::from_le_bytes([fmt[i], fmt[i + 1]]);
    let mut tag = u16_at(0);
    let channels = u16_at(2) as usize;
    let sample_rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]) as f64;
    let bits = u16_at(14);
    // WAVE_FORMAT_EXTENSIBLE names the real format in the first two bytes of
    // its sub-format GUID.
    if tag == 0xFFFE {
        if fmt.len() < 26 {
            return Err("an extensible format chunk too short to read".into());
        }
        tag = u16_at(24);
    }
    let encoding = match (tag, bits) {
        (1, 8) => Encoding::U8,
        (1, 16) => Encoding::I16,
        (1, 24) => Encoding::I24,
        (1, 32) => Encoding::I32,
        (3, 32) => Encoding::F32,
        (3, 64) => Encoding::F64,
        _ => return Err(format!("format {tag} at {bits} bits is not PCM")),
    };
    if channels == 0 || sample_rate <= 0.0 {
        return Err("a format with no channels or no rate".into());
    }
    Ok((encoding, channels, sample_rate))
}

/// An open PCM file, read by frame.
pub struct Reader {
    layout: Layout,
    file: crate::files::File,
    bytes: Vec<u8>,
}

impl Reader {
    /// Opens the file `layout` describes.
    pub fn open(layout: &Layout) -> Result<Self, String> {
        let file =
            crate::files::File::open(&layout.path).map_err(|e| format!("{}: {e}", layout.path))?;
        Ok(Self {
            layout: layout.clone(),
            file,
            bytes: Vec::new(),
        })
    }

    /// The layout it reads.
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// Reads up to `frames` frames from `frame` on into `out`, interleaved,
    /// replacing what it held. Returns how many frames were read: fewer at the
    /// end of the file, none past it.
    pub fn read(
        &mut self,
        frame: u64,
        frames: usize,
        out: &mut Vec<f32>,
    ) -> std::io::Result<usize> {
        read_frames(
            &mut self.file,
            &self.layout,
            frame,
            frames,
            &mut self.bytes,
            out,
        )
    }
}

/// [`Reader::read`] over any source: up to `frames` frames from `frame` on,
/// decoded into `out` through the scratch `bytes`.
pub fn read_frames<R: Read + Seek>(
    source: &mut R,
    layout: &Layout,
    frame: u64,
    frames: usize,
    bytes: &mut Vec<u8>,
    out: &mut Vec<f32>,
) -> std::io::Result<usize> {
    out.clear();
    let available = layout.frames.saturating_sub(frame);
    let frames = (frames as u64).min(available) as usize;
    if frames == 0 {
        return Ok(0);
    }
    let width = layout.frame_bytes();
    bytes.resize(frames * width, 0);
    source.seek(SeekFrom::Start(layout.data_offset + frame * width as u64))?;
    source.read_exact(bytes)?;
    let encoding = layout.encoding;
    out.extend(
        bytes
            .chunks_exact(encoding.bytes())
            .map(|b| encoding.decode(b)),
    );
    Ok(frames)
}
#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of this test's own, emptied first.
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("clausters-pcm-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// **Three containers, one body**: what a writer writes in each reads
    /// back frame for frame, from anywhere in it, and the header says what
    /// was written.
    #[test]
    fn every_container_reads_back_what_was_written() {
        let dir = scratch("containers");
        let values: Vec<f32> = (0..2_001 * 2)
            .map(|i| ((i % 97) as f32 - 48.0) / 64.0)
            .collect();
        for (name, container) in [
            ("t.wav", Container::Wav),
            ("t.w64", Container::W64),
            ("t.rf64", Container::Rf64),
        ] {
            for (format, step) in [("float", 0.0), ("int24", 2e-7), ("int16", 6e-5)] {
                let path = dir.join(format!("{format}-{name}"));
                let path = path.to_str().unwrap();
                let mut w = Writer::create(path, 2, 44_100.0, format).unwrap();
                // In two runs, as a stream arrives.
                w.write(&values[..1_000]).unwrap();
                w.write(&values[1_000..]).unwrap();
                w.finalize().unwrap();
                let layout = Layout::open(path).unwrap();
                assert_eq!(layout.container, container, "{path}");
                assert_eq!((layout.channels, layout.frames), (2, 2_001), "{path}");
                assert_eq!(layout.sample_rate, 44_100.0);
                let mut reader = Reader::open(&layout).unwrap();
                let mut out = Vec::new();
                assert_eq!(reader.read(1_500, 501, &mut out).unwrap(), 501);
                for (i, v) in out.iter().enumerate() {
                    assert!((v - values[3_000 + i]).abs() <= step, "{path}: {i}");
                }
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A WAV keeps an RF64's room and becomes one past 4 GB**, in place:
    /// the two headers are one length, so a writer that learns at its close
    /// that the body outgrew 32 bits rewrites the header and nothing else.
    #[test]
    fn a_wav_becomes_an_rf64_in_place_past_four_gigabytes() {
        let wav = header(Container::Wav, 1, 48_000.0, Encoding::I16, 100).unwrap();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[12..16], b"JUNK", "the ds64's room, kept");
        let past = header(Container::Wav, 1, 48_000.0, Encoding::I16, 5 << 30).unwrap();
        assert_eq!(&past[0..4], b"RF64");
        assert_eq!(&past[12..16], b"ds64");
        assert_eq!(wav.len(), past.len(), "one length, so nothing moves");
        let rf64 = header(Container::Rf64, 1, 48_000.0, Encoding::I16, 5 << 30).unwrap();
        assert_eq!(past, rf64);
        let w64 = header(Container::W64, 1, 48_000.0, Encoding::I16, 5 << 30).unwrap();
        assert_eq!(w64.len(), 104);
    }

    /// **A `.wav` that grew past 4 GB reads back as the RF64 it became**:
    /// the header a writer rewrites at its close over a body it never moved.
    /// Written as the header of a body past 32 bits over a short one, since
    /// a real one is gigabytes; what is checked is that the reader takes
    /// the header the writer left.
    #[test]
    fn a_wav_promoted_at_its_close_reads_as_an_rf64() {
        let dir = scratch("promoted");
        let path = dir.join("long.wav");
        let path = path.to_str().unwrap();
        let mut w = Writer::create(path, 1, 48_000.0, "float").unwrap();
        w.write(&[0.25, -0.5, 0.75]).unwrap();
        w.finalize().unwrap();
        let mut bytes = std::fs::read(path).unwrap();
        let head = header(Container::Wav, 1, 48_000.0, Encoding::F32, 12).unwrap();
        assert_eq!(&bytes[..head.len()], &head[..], "a WAV while it fits");
        // the same body under the header it would get past 4 GB, its ds64
        // stating the body's real size
        let mut rf64 = header(Container::Wav, 1, 48_000.0, Encoding::F32, 5 << 30).unwrap();
        rf64[28..36].copy_from_slice(&12u64.to_le_bytes());
        bytes[..rf64.len()].copy_from_slice(&rf64);
        std::fs::write(path, &bytes).unwrap();
        let layout = Layout::open(path).unwrap();
        assert_eq!(layout.container, Container::Rf64);
        assert_eq!(layout.frames, 3);
        let mut out = Vec::new();
        Reader::open(&layout).unwrap().read(0, 3, &mut out).unwrap();
        assert_eq!(out, [0.25, -0.5, 0.75]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A compressed or foreign file is refused by name rather than read as
    /// noise.
    #[test]
    fn a_file_that_is_not_pcm_wav_is_refused() {
        let dir = scratch("foreign");
        let path = dir.join("x.wav");
        std::fs::write(&path, b"OggS not a wave file at all").unwrap();
        let e = Layout::open(path.to_str().unwrap()).unwrap_err();
        assert!(e.contains("not a PCM WAV"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
