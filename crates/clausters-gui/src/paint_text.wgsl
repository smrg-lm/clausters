// The `font-atlas` feature's second pipeline: the same 2D geometry as
// `paint.wgsl`, with a coverage sample from the window's glyph atlas.
//
// One texture and one bind group per window, never per widget and never per
// batch -- which is what keeps a document's text one draw call a batch, the
// property the flat batch was built for. The atlas is a single-channel coverage sheet: the red channel is the
// glyph's alpha, the vertex color is the ink.

struct VsIn {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@group(0) @binding(0) var atlas_tex: texture_2d<f32>;
@group(0) @binding(1) var atlas_smp: sampler;

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = vec4<f32>(in.pos, 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    return out;
}

// **Coverage as a browser blends it.** A glyph's coverage is exact area, and
// blended as it is over the window's colors (no shader here corrects gamma)
// a light letter on a dark ground reads thin and a dark one on a light
// ground reads grey. A browser's text corrects each coverage for the ink's
// luminance before the blend, against a ground guessed as the ink's opposite:
// the blend is done as if in a space of this gamma, with this contrast added
// to a coverage over a light ground. The formula is the browser's; the two
// numbers are the ones that drew the same ink as its captures, light on dark
// and dark on light, at 1x and 2x.
const GAMMA: f32 = 1.3;
const CONTRAST: f32 = 0.4;

fn to_luma(x: f32) -> f32 {
    return pow(max(x, 0.0001), GAMMA);
}

fn from_luma(x: f32) -> f32 {
    return pow(max(x, 0.0001), 1.0 / GAMMA);
}

fn corrected(coverage: f32, ink: vec3<f32>) -> f32 {
    let src = from_luma(dot(
        vec3<f32>(to_luma(ink.r), to_luma(ink.g), to_luma(ink.b)),
        vec3<f32>(0.2126, 0.7152, 0.0722),
    ));
    let dst = 1.0 - src;
    let lin_src = to_luma(src);
    let lin_dst = to_luma(dst);
    // the contrast fades out as the ink goes light
    let a = coverage + (1.0 - coverage) * CONTRAST * lin_dst * coverage;
    if abs(src - dst) < 1.0 / 256.0 {
        return a;
    }
    // what the blend should give, undone through the blend it will get
    let out = from_luma(lin_src * a + (1.0 - a) * lin_dst);
    return clamp((out - dst) / (src - dst), 0.0, 1.0);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // The coordinates are texels of the sheet, which grows: a fraction of it
    // would name another place after every growth, and the vertices a window
    // has already uploaded would with it.
    let size = vec2<f32>(textureDimensions(atlas_tex));
    let coverage = textureSample(atlas_tex, atlas_smp, in.uv / size).r;
    return vec4<f32>(in.color.rgb, in.color.a * corrected(coverage, in.color.rgb));
}
