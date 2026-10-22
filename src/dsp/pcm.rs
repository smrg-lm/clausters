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

use std::io::{Read, Seek, SeekFrom};

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
    fn decode(self, b: &[u8]) -> f32 {
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

/// Where a PCM file keeps its samples and how they are laid out.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    /// The file it describes.
    pub path: String,
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
        let mut file = std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
        Self::parse(&mut file, path)
    }

    /// Reads a header from `source`, describing the file at `path`.
    pub fn parse<R: Read + Seek>(source: &mut R, path: &str) -> Result<Self, String> {
        let err = |e: &str| format!("{path}: {e}");
        let io = |e: std::io::Error| format!("{path}: {e}");
        let mut head = [0u8; 12];
        source.read_exact(&mut head).map_err(io)?;
        if &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
            return Err(err("not a PCM WAV file"));
        }
        let mut format: Option<(Encoding, usize, f64)> = None;
        let mut at = 12u64;
        loop {
            let mut chunk = [0u8; 8];
            if source.read_exact(&mut chunk).is_err() {
                return Err(err("no data chunk"));
            }
            let size = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]) as u64;
            let body = at + 8;
            match &chunk[0..4] {
                b"fmt " => {
                    let mut fmt = vec![0u8; size.min(64) as usize];
                    source.read_exact(&mut fmt).map_err(io)?;
                    format = Some(parse_fmt(&fmt).map_err(|e| err(&e))?);
                }
                b"data" => {
                    let Some((encoding, channels, sample_rate)) = format else {
                        return Err(err("the data chunk comes before the format"));
                    };
                    let frame = (channels * encoding.bytes()) as u64;
                    // A data chunk still being written, or one whose size a
                    // writer never went back to fill in, says less than the
                    // file holds; the file's own length is the bound.
                    let end = source.seek(SeekFrom::End(0)).map_err(io)?;
                    let held = end.saturating_sub(body);
                    let bytes = if size == 0 || size == u32::MAX as u64 {
                        held
                    } else {
                        size.min(held)
                    };
                    return Ok(Layout {
                        path: path.to_string(),
                        data_offset: body,
                        channels,
                        sample_rate,
                        encoding,
                        frames: bytes / frame,
                    });
                }
                _ => {}
            }
            // Chunks are padded to an even length.
            at = body + size + (size & 1);
            source.seek(SeekFrom::Start(at)).map_err(io)?;
        }
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
    file: std::fs::File,
    bytes: Vec<u8>,
}

impl Reader {
    /// Opens the file `layout` describes.
    pub fn open(layout: &Layout) -> Result<Self, String> {
        let file =
            std::fs::File::open(&layout.path).map_err(|e| format!("{}: {e}", layout.path))?;
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
        out.clear();
        let available = self.layout.frames.saturating_sub(frame);
        let frames = (frames as u64).min(available) as usize;
        if frames == 0 {
            return Ok(0);
        }
        let width = self.layout.frame_bytes();
        self.bytes.resize(frames * width, 0);
        let offset = self.layout.data_offset + frame * width as u64;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(&mut self.bytes)?;
        let encoding = self.layout.encoding;
        out.extend(
            self.bytes
                .chunks_exact(encoding.bytes())
                .map(|b| encoding.decode(b)),
        );
        Ok(frames)
    }
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

    fn wav(dir: &std::path::Path, name: &str, spec: hound::WavSpec, values: &[f32]) -> String {
        let path = dir.join(name);
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for &v in values {
            match (spec.sample_format, spec.bits_per_sample) {
                (hound::SampleFormat::Float, _) => w.write_sample(v).unwrap(),
                (_, 16) => w.write_sample((v * 32_767.0) as i16).unwrap(),
                (_, 24) => w.write_sample((v * 8_388_607.0) as i32).unwrap(),
                _ => unreachable!(),
            }
        }
        w.finalize().unwrap();
        path.to_string_lossy().into_owned()
    }

    /// **A frame is reached by arithmetic**: a read from the middle of a file
    /// is the frames there, whatever the encoding.
    #[test]
    fn a_read_lands_on_the_frame_it_names() {
        let dir = scratch("frame");
        let values: Vec<f32> = (0..200).map(|i| (i as f32 - 100.0) / 128.0).collect();
        for (bits, format) in [
            (16, hound::SampleFormat::Int),
            (24, hound::SampleFormat::Int),
            (32, hound::SampleFormat::Float),
        ] {
            let spec = hound::WavSpec {
                channels: 2,
                sample_rate: 44_100,
                bits_per_sample: bits,
                sample_format: format,
            };
            let path = wav(&dir, &format!("t{bits}.wav"), spec, &values);
            let layout = Layout::open(&path).unwrap();
            assert_eq!(layout.channels, 2);
            assert_eq!(layout.frames, 100);
            assert_eq!(layout.sample_rate, 44_100.0);
            let mut reader = Reader::open(&layout).unwrap();
            let mut out = Vec::new();
            assert_eq!(reader.read(40, 3, &mut out).unwrap(), 3);
            for (i, v) in out.iter().enumerate() {
                assert!((v - values[80 + i]).abs() < 1e-4, "{bits} bits: {out:?}");
            }
            assert_eq!(
                reader.read(98, 10, &mut out).unwrap(),
                2,
                "the end is short"
            );
            assert_eq!(
                reader.read(100, 10, &mut out).unwrap(),
                0,
                "past it, nothing"
            );
        }
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
    }
}
