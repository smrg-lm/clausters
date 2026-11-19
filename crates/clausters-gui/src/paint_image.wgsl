// The pictures' pipeline: the same 2D geometry as `paint_text.wgsl`, with a
// color sample from the window's copy of the sheet of pictures
// (`clausters_gui_draw::image`). One texture and one bind group per window,
// as the glyphs' sheet; the vertex alpha is the ink's opacity.

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

@group(0) @binding(0) var sheet_tex: texture_2d<f32>;
@group(0) @binding(1) var sheet_smp: sampler;

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
    // texels of the sheet, which grows, as the glyphs' coordinates are
    let size = vec2<f32>(textureDimensions(sheet_tex));
    let texel = textureSample(sheet_tex, sheet_smp, in.uv / size);
    return vec4<f32>(texel.rgb, texel.a * in.color.a);
}
