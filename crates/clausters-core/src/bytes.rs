//! Little-endian byte (de)serialization for the analysis caches.
//!
//! An analysis cache (a [`peaks::Pyramid`](crate::peaks::Pyramid), and the
//! GUI's spectrogram STFT) is a flat sequence of small headers and `f32`
//! arrays, so a build can **memory-map** it instead of reading it into RAM —
//! which is exactly what a local shared-resource cache wants. Integers are
//! written little-endian; `f32` arrays are written native-endian and read back
//! with `from_ne_bytes` over 4-byte chunks, which is alignment-independent (the
//! bytes come from a `Vec<u8>`/mmap with no `f32` alignment guarantee). The
//! format is therefore machine-local — fine for a same-machine cache.
//!
//! Kept here in the shared core (rather than re-implemented per client) so the
//! cache a client writes and the host reads use one layout, byte for byte.

pub fn push_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub fn push_u64(out: &mut Vec<u8>, v: usize) {
    out.extend_from_slice(&(v as u64).to_le_bytes());
}

pub fn push_f32s(out: &mut Vec<u8>, s: &[f32]) {
    out.reserve(s.len() * 4);
    for &x in s {
        out.extend_from_slice(&x.to_ne_bytes());
    }
}

/// Minimal little-endian cursor for the `from_bytes` parsers.
pub struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let slice = self.bytes.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }

    /// Read a 4-byte magic tag and check it matches `expected`.
    pub fn tag(&mut self, expected: &[u8; 4]) -> Option<()> {
        (self.take(4)? == expected).then_some(())
    }

    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    pub fn usize(&mut self) -> Option<usize> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?) as usize)
    }

    pub fn f32_vec(&mut self, len: usize) -> Option<Vec<f32>> {
        let bytes = self.take(len.checked_mul(4)?)?;
        Some(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_ne_bytes(*c))
                .collect(),
        )
    }
}
