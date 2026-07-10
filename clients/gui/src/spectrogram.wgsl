// Spectrogram shader. A single full-screen triangle samples the magnitude
// texture (x = time/frame, y = frequency bin); `u.time` is the visible time
// window so panning/zooming only reshapes the sampled slice - rendering cost is
// constant regardless of zoom. Magnitude is mapped to colour with a viridis
// approximation. The same WGSL runs unchanged under WebGPU.

struct Uniforms {
    // x = start_frac, y = len_frac of the visible time window; z = Nyquist Hz
    // (the mel/bark mappings need the absolute frequency axis).
    time: vec4<f32>,
    // x = d0, y = d1 of the visible frequency window in display coords [0,1];
    // z = scale index (0 linear, 1 log, 2 mel, 3 bark); w = normalized
    // log-axis floor (e.g. 20 Hz / Nyquist).
    freq: vec4<f32>,
    // x = lo_frac, y = hi_frac of the display dB window; z = colormap index.
    db: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    let xy = corners[vi];
    var out: VsOut;
    out.pos = vec4<f32>(xy, 0.0, 1.0);
    // uv: (0,0) top-left .. (1,1) bottom-right of the screen.
    out.uv = vec2<f32>((xy.x + 1.0) * 0.5, (1.0 - xy.y) * 0.5);
    return out;
}

// Viridis colormap polynomial fit (Matt Zucker). t in [0, 1].
fn viridis(t: f32) -> vec3<f32> {
    let c0 = vec3<f32>(0.2777273272234177, 0.005407344544966578, 0.3340998053353061);
    let c1 = vec3<f32>(0.1050930431085774, 1.404613529898575, 1.384590162594685);
    let c2 = vec3<f32>(-0.3308618287255563, 0.214847559468213, 0.09509516302823659);
    let c3 = vec3<f32>(-4.634230498983486, -5.799100973351585, -19.33244095627987);
    let c4 = vec3<f32>(6.228269936347081, 14.17993336680509, 56.69055260068105);
    let c5 = vec3<f32>(4.776384997670288, -13.74514537774601, -65.35303263337234);
    let c6 = vec3<f32>(-5.435455855934631, 4.645852612178535, 26.3124352495832);
    return c0 + t * (c1 + t * (c2 + t * (c3 + t * (c4 + t * (c5 + t * c6)))));
}

// Magma colormap polynomial fit (Matt Zucker). t in [0, 1].
fn magma(t: f32) -> vec3<f32> {
    let c0 = vec3<f32>(-0.002136485053939, -0.000749655052795, -0.005386127855323);
    let c1 = vec3<f32>(0.2516605407371642, 0.6775232436837668, 2.494026599312351);
    let c2 = vec3<f32>(8.353717279216625, -3.577719514958484, 0.3144679030132573);
    let c3 = vec3<f32>(-27.66873308576866, 14.26473078096533, -13.64921318813922);
    let c4 = vec3<f32>(52.17613981234068, -27.94360607168351, 12.94416944238394);
    let c5 = vec3<f32>(-50.76852536473588, 29.04658282127291, 4.23415299384598);
    let c6 = vec3<f32>(18.65570506591883, -11.48977351997711, -5.601961508734096);
    return c0 + t * (c1 + t * (c2 + t * (c3 + t * (c4 + t * (c5 + t * c6)))));
}

fn colormap(t: f32, which: f32) -> vec3<f32> {
    let x = clamp(t, 0.0, 1.0);
    if which < 0.5 {
        return viridis(x);
    } else if which < 1.5 {
        return magma(x);
    }
    return vec3<f32>(x, x, x);
}

// Hertz -> mel and its inverse (O'Shaughnessy), the same closed form as
// `clausters_core::scale` so the ruler ticks land on the shader's rows.
fn hz_to_mel(f: f32) -> f32 {
    return 2595.0 * log(1.0 + f / 700.0) / 2.302585092994046;
}
fn mel_to_hz(m: f32) -> f32 {
    return 700.0 * (pow(10.0, m / 2595.0) - 1.0);
}

// Hertz -> bark and its inverse (Traunmuller's closed form; -0.53 at 0 Hz is
// the axis floor the display normalizes against), matching the core.
fn hz_to_bark(f: f32) -> f32 {
    return 26.81 * f / (1960.0 + f) - 0.53;
}
fn bark_to_hz(z: f32) -> f32 {
    let zc = clamp(z, -0.53, 26.279999);
    return 1960.0 * (zc + 0.53) / (26.28 - zc);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let t = clamp(u.time.x + in.uv.x * u.time.y, 0.0, 1.0);

    // screen_y: 0 at the bottom (low frequency) .. 1 at the top. The visible
    // frequency window is a sub-range of display space; map it to a normalized
    // bin over the full axis: linear, geometric (log), or through the
    // perceptual mel/bark forms over the absolute axis 0..Nyquist.
    let screen_y = 1.0 - in.uv.y;
    let d = clamp(mix(u.freq.x, u.freq.y, screen_y), 0.0, 1.0);
    let nyq = max(u.time.z, 1.0);
    var bin_norm: f32;
    if u.freq.z < 0.5 {
        bin_norm = d;
    } else if u.freq.z < 1.5 {
        // Log axis from the floor (freq.w) up to Nyquist (1.0): f_lo^(1 - d).
        bin_norm = pow(u.freq.w, 1.0 - d);
    } else if u.freq.z < 2.5 {
        // Mel axis: even display steps are even mel steps (mel(0) = 0).
        bin_norm = mel_to_hz(d * hz_to_mel(nyq)) / nyq;
    } else {
        // Bark axis, normalized from the formula's own floor at 0 Hz.
        let z0 = hz_to_bark(0.0);
        bin_norm = bark_to_hz(mix(z0, hz_to_bark(nyq), d)) / nyq;
    }
    // Texture row 0 is bin 0 (low frequency), so the bottom shows the lowest.
    let mag = textureSampleLevel(tex, samp, vec2<f32>(t, clamp(bin_norm, 0.0, 1.0)), 0.0).r;

    // Remap the stored magnitude into the display dB window for contrast.
    let c = clamp((mag - u.db.x) / max(u.db.y - u.db.x, 1e-5), 0.0, 1.0);
    return vec4<f32>(colormap(c, u.db.z), 1.0);
}
