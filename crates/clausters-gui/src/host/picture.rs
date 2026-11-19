//! **A picture's file, decoded**: the PNG or JPEG bytes a documentation page
//! names, as the RGBA texels the sheet of pictures holds
//! (`clausters_gui_draw::image`). Pure Rust, so a page decodes them as a
//! window does.

/// A picture: its width, its height and its RGBA texels, rows top first.
pub type Decoded = (u32, u32, Vec<u8>);

/// **`bytes` as a picture**, read as a PNG or as a JPEG by what they start
/// with; why not, for anything else or for a file it could not read.
pub fn decode(bytes: &[u8]) -> Result<Decoded, String> {
    if bytes.starts_with(b"\x89PNG") {
        png(bytes)
    } else if bytes.starts_with(&[0xff, 0xd8]) {
        jpeg(bytes)
    } else {
        Err("not a PNG or a JPEG".into())
    }
}

fn png(bytes: &[u8]) -> Result<Decoded, String> {
    let mut decoder = ::png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(::png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let size = reader
        .output_buffer_size()
        .ok_or("the picture is too large")?;
    let mut buf = vec![0; size];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width, info.height);
    let rgba = match info.color_type {
        ::png::ColorType::Rgba => buf,
        ::png::ColorType::Rgb => buf
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        ::png::ColorType::GrayscaleAlpha => buf
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        ::png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        ::png::ColorType::Indexed => return Err("an indexed PNG not expanded".into()),
    };
    Ok((w, h, rgba))
}

fn jpeg(bytes: &[u8]) -> Result<Decoded, String> {
    use zune_core::bytestream::ZCursor;
    use zune_core::colorspace::ColorSpace;
    use zune_core::options::DecoderOptions;

    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    let rgba = decoder.decode().map_err(|e| format!("{e:?}"))?;
    let (w, h) = decoder.dimensions().ok_or("a JPEG with no size")?;
    Ok((w as u32, h as u32, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PNG the host writes (a capture) reads back as the same texels.
    #[test]
    fn a_png_reads_back_as_its_texels() {
        let texels: Vec<u8> = (0..2 * 3 * 4).map(|i| (i * 9) as u8).collect();
        let file = crate::host::png::encode_rgba(2, 3, &texels);
        let (w, h, rgba) = decode(&file).unwrap();
        assert_eq!((w, h), (2, 3));
        assert_eq!(rgba, texels);
        assert!(decode(b"GIF89a").is_err());
    }
}
