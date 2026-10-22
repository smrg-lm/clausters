//! **A PCM file read at any frame**: the container parser, the reader and
//! the writer are `clausters_core::pcm`, shared with the GUI host, which
//! reads a list's files with the same code the server's streams do. This
//! module names them where the engine has always found them.

pub use clausters_core::pcm::*;

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
}
