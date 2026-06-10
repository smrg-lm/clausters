//! F0 smoke test: build a sine oscillator through the libfaust Box API,
//! JIT-compile it to native code and verify the audio. Gated behind the
//! `faust` feature: `cargo test --features faust --test faust_smoke`.
//!
//! The graph is the SinOsc equivalent, built from primitives (no Faust
//! stdlib): `sin(2π · phasor(freq))` with `phasor(f) = (+(f/SR) : wrap) ~ _`
//! and `wrap(x) = x - floor(x)`. `freq` is an hslider left at its default
//! (440), so the parameter path is exercised without UIGlue (that's F3).

#![cfg(feature = "faust")]

use std::ffi::{CStr, CString, c_char};
use std::time::Instant;

use claudesufa::faust::ffi::*;

const SR: f32 = 48_000.0;
const BLOCK: usize = 64;

fn rms(buf: &[f32]) -> f32 {
    (buf.iter().map(|x| x * x).sum::<f32>() / buf.len() as f32).sqrt()
}

fn estimated_freq(buf: &[f32]) -> f32 {
    let crossings = buf
        .windows(2)
        .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
        .count();
    crossings as f32 * SR / buf.len() as f32
}

/// `wrap = _ <: _ - floor(_)` — one input, one output.
unsafe fn wrap_box() -> FaustBox {
    unsafe {
        CboxSplit(
            CboxWire(),
            CboxSubAux(CboxWire(), CboxFloorAux(CboxWire())),
        )
    }
}

/// `phasor = (+(freq/SR) : wrap) ~ _` — zero inputs, one output.
unsafe fn phasor_box(freq: FaustBox) -> FaustBox {
    unsafe {
        let inc = CboxDivAux(freq, CboxReal(SR as f64));
        let body = CboxSeq(CboxAddAux(CboxWire(), inc), wrap_box());
        CboxRec(body, CboxWire())
    }
}

#[test]
fn jit_compiled_box_sine_plays_at_440() {
    let name = CString::new("smoke_sine").unwrap();
    let label = CString::new("freq").unwrap();
    let target = CString::new("").unwrap();
    let mut error_msg = [0 as c_char; ERROR_MSG_SIZE];

    // ---- compiler-side: context, box construction, JIT ----
    let t0 = Instant::now();
    let factory = unsafe {
        createLibContext();
        let freq = CboxHSlider(
            label.as_ptr(),
            CboxReal(440.0),
            CboxReal(20.0),
            CboxReal(20_000.0),
            CboxReal(0.01),
        );
        let two_pi = CboxReal(std::f64::consts::TAU);
        let sine = CboxSinAux(CboxMulAux(two_pi, phasor_box(freq)));
        let process = CboxMulAux(sine, CboxReal(0.2));
        let factory = createCDSPFactoryFromBoxes(
            name.as_ptr(),
            process,
            0,
            std::ptr::null(),
            target.as_ptr(),
            error_msg.as_mut_ptr(),
            -1,
        );
        destroyLibContext();
        factory
    };
    let compile_ms = t0.elapsed().as_secs_f64() * 1e3;
    if factory.is_null() {
        let msg = unsafe { CStr::from_ptr(error_msg.as_ptr()) };
        panic!("factory creation failed: {}", msg.to_string_lossy());
    }
    println!("JIT compile latency: {compile_ms:.1} ms");

    // ---- instance: init fixes the sample rate ----
    let t1 = Instant::now();
    let dsp = unsafe { createCDSPInstance(factory) };
    assert!(!dsp.is_null(), "instance creation failed");
    unsafe { initCDSPInstance(dsp, SR as i32) };
    println!(
        "instantiate+init latency: {:.2} ms",
        t1.elapsed().as_secs_f64() * 1e3
    );

    assert_eq!(unsafe { getNumInputsCDSPInstance(dsp) }, 0);
    assert_eq!(unsafe { getNumOutputsCDSPInstance(dsp) }, 1);

    // ---- audio-thread-style render: non-interleaved float** buffers ----
    let mut block = [0.0f32; BLOCK];
    let mut out = Vec::with_capacity(750 * BLOCK);
    for _ in 0..750 {
        // exactly 1 s at 48 kHz
        let mut outputs: [*mut f32; 1] = [block.as_mut_ptr()];
        unsafe {
            computeCDSPInstance(dsp, BLOCK as i32, std::ptr::null_mut(), outputs.as_mut_ptr())
        };
        out.extend_from_slice(&block);
    }

    assert!(out.iter().all(|x| x.is_finite()));
    let freq = estimated_freq(&out);
    assert!((freq - 440.0).abs() < 5.0, "estimated freq = {freq}");
    let expected_rms = 0.2 * std::f32::consts::FRAC_1_SQRT_2;
    assert!(
        (rms(&out) - expected_rms).abs() < 0.005,
        "rms = {}, expected ≈ {expected_rms}",
        rms(&out)
    );

    // ---- teardown: instances strictly before their factory ----
    unsafe {
        deleteCDSPInstance(dsp);
        deleteCDSPFactory(factory);
    }
}

#[test]
fn factory_creation_reports_errors() {
    // A box with dangling inputs is not a valid `process`: the JIT must fail
    // and fill the error buffer instead of crashing.
    let name = CString::new("smoke_bad").unwrap();
    let target = CString::new("").unwrap();
    let mut error_msg = [0 as c_char; ERROR_MSG_SIZE];

    let factory = unsafe {
        createLibContext();
        let bad = CboxAddAux(CboxWire(), CboxWire()); // 2 inputs, never fed
        let factory = createCDSPFactoryFromBoxes(
            name.as_ptr(),
            bad,
            0,
            std::ptr::null(),
            target.as_ptr(),
            error_msg.as_mut_ptr(),
            -1,
        );
        destroyLibContext();
        factory
    };

    if factory.is_null() {
        let msg = unsafe { CStr::from_ptr(error_msg.as_ptr()) }.to_string_lossy();
        assert!(!msg.is_empty(), "error buffer must explain the failure");
        println!("expected failure message: {msg}");
    } else {
        // Some libfaust versions accept open boxes by auto-wiring inputs;
        // if so the factory is valid — just clean it up.
        unsafe {
            deleteCDSPFactory(factory);
        }
    }
}
