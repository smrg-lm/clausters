//! Generates the twiddle table the inverse real FFT (`fft::irfft_into`)
//! post-rotates its odd half by: `e^{+j 2 pi k / 4096}` for `k` in
//! `0..2048`, computed in `f64` and rounded once to `f32`. A smaller size
//! reads it at a stride. Built here rather than at run time because the
//! audio thread may neither compute two thousand sines per transform nor
//! wait on a lazily initialized table.

use std::fmt::Write as _;

const N: usize = 4096;

fn main() {
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("twiddle.rs");
    let mut src = String::new();
    for (name, f) in [("COS", f64::cos as fn(f64) -> f64), ("SIN", f64::sin)] {
        writeln!(src, "pub(crate) static {name}: [f32; {}] = [", N / 2).unwrap();
        for k in 0..N / 2 {
            let x = f(2.0 * std::f64::consts::PI * k as f64 / N as f64) as f32;
            writeln!(src, "    {x:?},").unwrap();
        }
        writeln!(src, "];").unwrap();
    }
    std::fs::write(out, src).unwrap();
    println!("cargo:rerun-if-changed=build.rs");
}
