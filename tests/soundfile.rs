//! **A soundfile's header, read and written by the server's own codecs**:
//! what `clausters_soundfile_info` and `clausters_write_soundfile` (and the
//! page's `probeAudio`) are doors onto.

use clausters::server::nrt::{probe_audio, probe_audio_bytes, read_audio, write_audio};

fn path(name: &str) -> String {
    std::env::temp_dir()
        .join(format!("clausters-soundfile-{}-{name}", std::process::id()))
        .to_string_lossy()
        .into_owned()
}

/// A written file's header says the shape it was written with, and reading
/// it back returns the samples at the format's precision.
#[test]
fn a_written_file_probes_and_reads_back() {
    for (format, tolerance) in [("int16", 1.0 / 32767.0), ("int24", 1e-6), ("float", 0.0)] {
        let p = path(&format!("{format}.wav"));
        let samples: Vec<f32> = (0..300).map(|i| ((i as f32) * 0.01).sin() * 0.5).collect();
        write_audio(&p, samples.clone(), 3, 44_100.0, format).unwrap();
        let info = probe_audio(&p).unwrap();
        assert_eq!(
            (
                info.frames,
                info.channels,
                info.sample_rate,
                info.codec.as_str()
            ),
            (100, 3, 44_100.0, "wav")
        );
        assert_eq!(info.sample_format.as_deref(), Some(format));
        let back = read_audio(&p, 0, 0).unwrap();
        assert_eq!((back.frames(), back.channels()), (100, 3));
        let worst = back
            .to_vec()
            .iter()
            .zip(&samples)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(worst <= tolerance, "{format}: {worst}");
        let bytes = std::fs::read(&p).unwrap();
        assert_eq!(probe_audio_bytes(bytes, "wav", &p).unwrap(), info);
        let _ = std::fs::remove_file(&p);
    }
}

/// An empty file is a file: zero frames of the channels it was given.
#[test]
fn an_empty_file_is_written_and_probed() {
    let p = path("empty.wav");
    write_audio(&p, Vec::new(), 2, 48_000.0, "float").unwrap();
    let info = probe_audio(&p).unwrap();
    assert_eq!((info.frames, info.channels), (0, 2));
    let _ = std::fs::remove_file(&p);
}

/// What cannot be a file is refused before anything is written.
#[test]
fn a_ragged_or_shapeless_write_is_refused() {
    let p = path("bad.wav");
    assert!(write_audio(&p, vec![0.0; 5], 2, 48_000.0, "int16").is_err());
    assert!(write_audio(&p, vec![0.0; 4], 0, 48_000.0, "int16").is_err());
    assert!(write_audio(&p, vec![0.0; 4], 2, 0.0, "int16").is_err());
    assert!(write_audio(&p, vec![0.0; 4], 2, 48_000.0, "mp3").is_err());
    assert!(!std::path::Path::new(&p).exists());
    assert!(probe_audio(&path("missing.wav")).is_err());
}
