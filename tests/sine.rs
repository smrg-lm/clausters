//! Verifies the M0 engine signal offline, without an audio device:
//! frequency via zero crossings and amplitude via RMS.

use claudesufa::server::engine::{BLOCK_SIZE, Engine};

#[test]
fn sine_440hz_amp_02() {
    let sr = 48_000.0;
    let channels = 2;
    let mut engine = Engine::new(sr, channels);

    let mut out = vec![0.0f32; BLOCK_SIZE * channels];
    let mut left: Vec<f32> = Vec::new();
    for _ in 0..750 {
        // 750 blocks * 64 frames = exactly 1 s at 48 kHz
        engine.process_block(&mut out);
        left.extend(out.iter().step_by(channels).copied());
    }

    assert!(left.iter().all(|x| x.is_finite()));

    let rms = (left.iter().map(|x| x * x).sum::<f32>() / left.len() as f32).sqrt();
    let expected_rms = 0.2 * std::f32::consts::FRAC_1_SQRT_2;
    assert!(
        (rms - expected_rms).abs() < 0.002,
        "rms = {rms}, expected ≈ {expected_rms}"
    );

    let crossings = left
        .windows(2)
        .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
        .count();
    let freq = crossings as f32 * sr / left.len() as f32;
    assert!((freq - 440.0).abs() < 5.0, "estimated freq = {freq}");
}

#[test]
fn interleaved_channels_match() {
    let mut engine = Engine::new(44_100.0, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    engine.process_block(&mut out);
    for frame in out.chunks_exact(2) {
        assert_eq!(frame[0], frame[1]);
    }
    // The signal is not silence
    assert!(out.iter().any(|x| x.abs() > 0.01));
}
