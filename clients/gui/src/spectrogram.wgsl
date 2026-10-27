// Spectrogram shader. A single full-screen triangle samples the magnitude
// texture (x = time/frame, y = frequency bin); `u.time` is the visible time
// window over **what the texture holds** -- the whole ring of a live view, or
// the stretch of a stored transform its view put on the card, at the level of
// its time pyramid the zoom asks for -- so panning/zooming only reshapes the
// sampled slice and rendering cost is constant regardless of zoom. Magnitude is mapped to colour with a viridis
// approximation. The same WGSL runs unchanged under WebGPU.

struct Uniforms {
    // x = start_frac, y = len_frac of the visible time window; z = Nyquist Hz
    // (the mel/bark mappings need the absolute frequency axis).
    time: vec4<f32>,
    // x = d0, y = d1 of the visible frequency window in display coords [0,1];
    // z = scale index (0 linear, 1 log, 2 mel, 3 bark); w = normalized
    // log-axis floor (e.g. 20 Hz / Nyquist).
    freq: vec4<f32>,
    // x = lo_frac, y = hi_frac of the display dB window; z = colormap index;
    // w = the layer's own alpha (1 where it is the only picture on the body).
    db: vec4<f32>,
    // xy = scale, zw = offset placing the picture inside its viewport. A
    // viewport positions but does not cut, so a view hanging off the window
    // edge is drawn for its full rectangle and the part outside clip space is
    // rasterized away. The identity (1, 1, 0, 0) for a view shown whole.
    rect: vec4<f32>,
    // **Where this picture is drawn**, across the element (0 left .. 1
    // right): x..y is what its columns cover, z..w a hole where a finer
    // picture of the same take is drawn over it (empty when z > w). A
    // transform of a span draws nothing past its columns, where it used to
    // stretch the first and the last over the rest of the view.
    cover: vec4<f32>,
    // x = 1 where each frequency band is drawn at its own scale: lifted so
    // the loudest it is (the rows texture) is the top of the range.
    shape: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var rows: texture_2d<f32>;

// **The stored magnitude at texture place (x, y)**, a band lifted to its own
// scale where the view asks for it. A row nothing has measured (a live ring)
// is left as it is.
fn level(x: f32, y: f32) -> f32 {
    var m = textureSampleLevel(tex, samp, vec2<f32>(x, y), 0.0).r;
    if u.shape.x > 0.5 {
        let top = textureSampleLevel(rows, samp, vec2<f32>(0.5, y), 0.0).r;
        if top > 0.0 {
            m = min(1.0, m + 1.0 - top);
        }
    }
    return m;
}

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
    out.pos = vec4<f32>(xy * u.rect.xy + u.rect.zw, 0.0, 1.0);
    // uv: (0,0) top-left .. (1,1) bottom-right of the *element*, taken from the
    // untransformed corner - so the framing moves the picture without moving
    // what each of its pixels samples.
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
    // Row k is centred on (k + 0.5) / rows and bin k measures k / rows of
    // Nyquist, so a frequency is read half a row up: each bin is drawn
    // centred on its own frequency, not from it up to the next one's.
    let rows_n = f32(textureDimensions(tex).y);
    let v = clamp(bin_norm + 0.5 / rows_n, 0.0, 1.0);
    // **Every pixel shows a magnitude the analysis measured.** Where a pixel
    // covers more than one texel, along either axis, it shows the largest of
    // them: the bins a pixel row spans on a log axis's upper octaves or in a
    // short row, and the columns a pixel spans when the texture is finer in
    // time than the screen. A sampler would draw the two texels nearest the
    // pixel's middle, and a narrow line between them would not be there at
    // all. It is the reading a waveform's pixel makes of its samples, made of
    // the magnitudes. Where a texel is larger than a pixel, the pixel shows
    // that texel (the sampler is nearest): a bin taller than a pixel is a
    // block as tall as its band, a column wider than one a block as wide as
    // its hop. A curve or a ramp between two texels would draw magnitudes
    // nothing measured, and the block's edges are where the analysis's own
    // resolution ends.
    let dims = vec2<f32>(textureDimensions(tex));
    let bins = dims.y;
    let half_t = abs(dpdx(t)) * 0.5;
    let half_v = abs(dpdy(v)) * 0.5;
    let cols_per_px = 2.0 * half_t * dims.x;
    let rows_per_px = 2.0 * half_v * bins;
    var mag: f32;
    if cols_per_px > 1.0 || rows_per_px > 1.0 {
        // The texels under the pixel, a few dozen at most: past that a
        // stride, which the level the view holds keeps from being needed in
        // time.
        let x0 = clamp(floor((t - half_t) * dims.x), 0.0, dims.x - 1.0);
        let x1 = clamp(floor((t + half_t) * dims.x), x0, dims.x - 1.0);
        let y0 = clamp(floor((v - half_v) * bins), 0.0, bins - 1.0);
        let y1 = clamp(floor((v + half_v) * bins), y0, bins - 1.0);
        var nx = 1u;
        if cols_per_px > 1.0 {
            nx = min(u32(x1 - x0) + 1u, 8u);
        }
        var ny = 1u;
        if rows_per_px > 1.0 {
            ny = min(u32(y1 - y0) + 1u, 64u);
        }
        mag = 0.0;
        for (var i = 0u; i < nx; i++) {
            // A column's middle, or the pixel's own place where a column is
            // wider than it, which the sampler reads as that column.
            var x = t;
            if nx > 1u {
                x = (x0 + floor(f32(i) * (x1 - x0 + 1.0) / f32(nx)) + 0.5) / dims.x;
            }
            for (var j = 0u; j < ny; j++) {
                var y = v;
                if ny > 1u {
                    y = (y0 + floor(f32(j) * (y1 - y0 + 1.0) / f32(ny)) + 0.5) / bins;
                }
                mag = max(mag, level(x, y));
            }
        }
    } else {
        mag = level(t, v);
    }

    // Remap the stored magnitude into the display dB window for contrast.
    let c = clamp((mag - u.db.x) / max(u.db.y - u.db.x, 1e-5), 0.0, 1.0);
    // Last, so the derivatives above are taken in uniform control flow.
    if in.uv.x < u.cover.x || in.uv.x > u.cover.y
        || (in.uv.x >= u.cover.z && in.uv.x < u.cover.w) {
        discard;
    }
    return vec4<f32>(colormap(c, u.db.z), u.db.w);
}
