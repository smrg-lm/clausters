//! Preparing a Faust-emitted wasm module so it can be linked into the engine.
//!
//! Faust's wasm backend can be told to take its memory from outside
//! (`-lang wasm-e`), which is what lets a compiled def live in the engine's own
//! linear memory and be called from Rust rather than run beside it. One thing
//! stands in the way, and it is invisible until it corrupts something: the
//! backend writes the DSP's JSON into a **data segment at absolute offset 0**,
//! unconditionally, external memory included. Instantiating such a module
//! against the engine's memory therefore writes over the engine's own first
//! bytes — which, on a `wasm32-unknown-unknown` link, is the bottom of the
//! stack (rustc passes `--stack-first`, so the stack occupies the low
//! megabyte).
//!
//! Moving the engine's data out of the way is the obvious fix and is not
//! available: `--global-base` must be at least the stack size when
//! `--stack-first` is used, so it cannot open a gap *below* the stack, which is
//! exactly where the segment lands.
//!
//! So the segment goes instead. Nothing reads it: the JSON we use is the one
//! the compiler hands back beside the binary, so the copy inside the module is
//! dead weight. [`strip_data_section`] removes it, and refuses rather than
//! guesses if the module does not look like what this reasoning assumed.

/// Removes the module's data section, returning the shortened module.
///
/// Refuses (with a message) if there is more than one data segment, or if the
/// one there is does not start at offset 0 — either would mean the backend
/// grew a use for that memory that this has not accounted for, and dropping it
/// blind would be the kind of corruption nobody traces back here.
///
/// Sections are the wasm binary's own top-level framing (an id byte and a
/// LEB128 length), stable since the MVP, so this walks them rather than
/// parsing anything inside.
pub fn strip_data_section(module: &[u8]) -> Result<Vec<u8>, String> {
    const DATA: u8 = 11;
    const DATA_COUNT: u8 = 12;
    if module.len() < 8 || &module[..4] != b"\0asm" {
        return Err("not a wasm module".into());
    }
    let mut out = Vec::with_capacity(module.len());
    out.extend_from_slice(&module[..8]);
    let mut at = 8;
    let mut stripped = false;
    while at < module.len() {
        let id = module[at];
        let (len, header) = leb128(&module[at + 1..])?;
        let body = at + 1 + header;
        let end = body
            .checked_add(len)
            .filter(|e| *e <= module.len())
            .ok_or_else(|| format!("section {id} runs past the end of the module"))?;
        match id {
            DATA => {
                check_one_segment_at_zero(&module[body..end])?;
                stripped = true;
            }
            // The data-count section only exists to declare how many segments
            // follow; with none, it must go too or validation fails.
            DATA_COUNT => {}
            _ => out.extend_from_slice(&module[at..end]),
        }
        at = end;
    }
    if !stripped {
        return Err("the module carries no data section; the backend changed".into());
    }
    Ok(out)
}

/// The assumption this rests on: one segment, active, at a constant offset of
/// zero, carrying the JSON. Anything else and the caller is told.
fn check_one_segment_at_zero(body: &[u8]) -> Result<(), String> {
    let (count, n) = leb128(body)?;
    if count != 1 {
        return Err(format!(
            "the module has {count} data segments; this expects the one the \
             Faust backend writes"
        ));
    }
    let rest = &body[n..];
    // Segment 0: a flags byte (0 = active, memory 0), then an i32.const
    // offset, then `end`.
    match rest {
        [0x00, 0x41, 0x00, 0x0b, ..] => Ok(()),
        [0x00, 0x41, ..] => Err("the data segment does not start at offset 0".into()),
        _ => Err("the data segment is not the active, memory-0 kind".into()),
    }
}

/// One unsigned LEB128, returning the value and how many bytes it took.
fn leb128(bytes: &[u8]) -> Result<(usize, usize), String> {
    let mut value: usize = 0;
    let mut shift = 0;
    for (i, byte) in bytes.iter().enumerate() {
        if shift >= usize::BITS as usize {
            return Err("a LEB128 length is too long to be real".into());
        }
        value |= usize::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok((value, i + 1));
        }
        shift += 7;
    }
    Err("a LEB128 length ran off the end".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A module with the shape the Faust backend emits: a type section, then a
    /// data section holding one segment at offset 0.
    fn module_with_data(payload: &[u8]) -> Vec<u8> {
        let mut segment = vec![0x00, 0x41, 0x00, 0x0b];
        segment.push(payload.len() as u8);
        segment.extend_from_slice(payload);
        let mut data = vec![0x01];
        data.extend_from_slice(&segment);

        let mut out = b"\0asm\x01\0\0\0".to_vec();
        out.extend_from_slice(&[1, 4, 1, 0x60, 0x00, 0x00]); // one type: ()->()
        out.push(11);
        out.push(data.len() as u8);
        out.extend_from_slice(&data);
        out
    }

    #[test]
    fn the_data_section_goes_and_the_rest_stays() {
        let module = module_with_data(b"{\"name\":\"x\"}");
        let stripped = strip_data_section(&module).unwrap();
        assert_eq!(&stripped[..8], b"\0asm\x01\0\0\0");
        assert_eq!(&stripped[8..], &[1, 4, 1, 0x60, 0x00, 0x00]);
        assert!(stripped.len() < module.len());
    }

    #[test]
    fn a_module_without_one_is_refused_rather_than_passed_through() {
        // Silently accepting it would hide exactly the change this guards:
        // a backend that stopped writing the JSON there, or wrote it twice.
        let mut module = b"\0asm\x01\0\0\0".to_vec();
        module.extend_from_slice(&[1, 4, 1, 0x60, 0x00, 0x00]);
        assert!(strip_data_section(&module).is_err());
    }

    #[test]
    fn a_segment_somewhere_other_than_zero_is_refused() {
        let mut module = b"\0asm\x01\0\0\0".to_vec();
        // One segment at a non-zero constant offset.
        let data = [0x01u8, 0x00, 0x41, 0x80, 0x01, 0x0b, 0x01, 0xff];
        module.push(11);
        module.push(data.len() as u8);
        module.extend_from_slice(&data);
        let err = strip_data_section(&module).unwrap_err();
        assert!(err.contains("offset 0"), "{err}");
    }

    #[test]
    fn two_segments_are_refused() {
        let mut module = b"\0asm\x01\0\0\0".to_vec();
        let data = [
            0x02u8, 0x00, 0x41, 0x00, 0x0b, 0x00, 0x00, 0x41, 0x00, 0x0b, 0x00,
        ];
        module.push(11);
        module.push(data.len() as u8);
        module.extend_from_slice(&data);
        assert!(
            strip_data_section(&module)
                .unwrap_err()
                .contains("segments")
        );
    }

    #[test]
    fn a_thing_that_is_not_a_module_is_refused() {
        assert!(strip_data_section(b"not wasm at all").is_err());
    }
}
