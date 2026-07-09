// Waveform shader. The CPU builds, per frame, one quad per pixel column
// spanning [min, max] of the audio in that column (see `waveform.rs`), one
// vertex range per channel. The vertices already arrive in clip space and
// carry their channel's trace color, so both stages are passthroughs and
// zoom/pan cost nothing on the GPU: they only change which samples the CPU
// maps onto each column. This same WGSL runs unchanged under WebGPU in a
// browser, which is the whole point of prototyping the renderer in wgpu.

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(@location(0) pos: vec2<f32>, @location(1) color: vec4<f32>) -> VsOut {
    var out: VsOut;
    out.pos = vec4<f32>(pos, 0.0, 1.0);
    out.color = color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
