// Waveform shader. The CPU builds, per frame, one quad per pixel column
// spanning [min, max] of the audio in that column (see `waveform.rs`). The
// vertices already arrive in clip space, so the vertex stage is a passthrough
// and zoom/pan cost nothing on the GPU: they only change which samples the CPU
// maps onto each column. This same WGSL runs unchanged under WebGPU in a
// browser, which is the whole point of prototyping the renderer in wgpu.

struct Uniforms {
    // RGBA fill color for the waveform body.
    color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

@vertex
fn vs_main(@location(0) pos: vec2<f32>) -> @builtin(position) vec4<f32> {
    return vec4<f32>(pos, 0.0, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return u.color;
}
