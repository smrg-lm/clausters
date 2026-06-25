//! Little-endian byte (de)serialization shared by the analysis caches
//! (`peaks::Pyramid`, `spectrogram::Stft`).
//!
//! Both caches are flat sequences of headers and `f32` arrays so a production
//! build can memory-map them. Float arrays are written native-endian via
//! `bytemuck` and read back with `from_ne_bytes` over chunks, which is
//! alignment-independent (the bytes come from a `Vec<u8>`/mmap with no f32
//! alignment guarantee). The format is therefore machine-local.

pub(crate) fn push_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub(crate) fn push_u64(out: &mut Vec<u8>, v: usize) {
    out.extend_from_slice(&(v as u64).to_le_bytes());
}

pub(crate) fn push_f32s(out: &mut Vec<u8>, s: &[f32]) {
    out.extend_from_slice(bytemuck::cast_slice(s));
}

/// Minimal little-endian cursor for the `from_bytes` parsers.
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let slice = self.bytes.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }

    /// Read a 4-byte magic tag and check it matches `expected`.
    pub(crate) fn tag(&mut self, expected: &[u8; 4]) -> Option<()> {
        (self.take(4)? == expected).then_some(())
    }

    pub(crate) fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    pub(crate) fn usize(&mut self) -> Option<usize> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?) as usize)
    }

    pub(crate) fn f32_vec(&mut self, len: usize) -> Option<Vec<f32>> {
        let bytes = self.take(len.checked_mul(4)?)?;
        Some(
            bytes
                .chunks_exact(4)
                .map(|c| f32::from_ne_bytes(c.try_into().unwrap()))
                .collect(),
        )
    }
}
