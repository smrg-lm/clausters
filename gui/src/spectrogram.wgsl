// Spectrogram shader. A single full-screen triangle samples the magnitude
// texture (x = time/frame, y = frequency bin); `u.time` is the visible time
// window so panning/zooming only reshapes the sampled slice - rendering cost is
// constant regardless of zoom. Magnitude is mapped to colour with a viridis
// approximation. The same WGSL runs unchanged under WebGPU.

struct Uniforms {
    // x = start_frac, y = len_frac of the visible time window (zw unused).
    time: vec4<f32>,
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
    let x = clamp(t, 0.0, 1.0);
    let c0 = vec3<f32>(0.2777273272234177, 0.005407344544966578, 0.3340998053353061);
    let c1 = vec3<f32>(0.1050930431085774, 1.404613529898575, 1.384590162594685);
    let c2 = vec3<f32>(-0.3308618287255563, 0.214847559468213, 0.09509516302823659);
    let c3 = vec3<f32>(-4.634230498983486, -5.799100973351585, -19.33244095627987);
    let c4 = vec3<f32>(6.228269936347081, 14.17993336680509, 56.69055260068105);
    let c5 = vec3<f32>(4.776384997670288, -13.74514537774601, -65.35303263337234);
    let c6 = vec3<f32>(-5.435455855934631, 4.645852612178535, 26.3124352495832);
    return c0 + x * (c1 + x * (c2 + x * (c3 + x * (c4 + x * (c5 + x * c6)))));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let t = clamp(u.time.x + in.uv.x * u.time.y, 0.0, 1.0);
    // Flip vertical so bin 0 (low frequency) sits at the bottom of the screen.
    let v = 1.0 - in.uv.y;
    let mag = textureSampleLevel(tex, samp, vec2<f32>(t, v), 0.0).r;
    return vec4<f32>(viridis(mag), 1.0);
}
