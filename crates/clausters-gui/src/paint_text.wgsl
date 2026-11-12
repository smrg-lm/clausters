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

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // The coordinates are texels of the sheet, which grows: a fraction of it
    // would name another place after every growth, and the vertices a window
    // has already uploaded would with it.
    let size = vec2<f32>(textureDimensions(atlas_tex));
    let coverage = textureSample(atlas_tex, atlas_smp, in.uv / size).r;
    return vec4<f32>(in.color.rgb, in.color.a * coverage);
}
