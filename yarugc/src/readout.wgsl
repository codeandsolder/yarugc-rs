@group(0) @binding(0) var atlas: texture_2d_array<f32>;
@group(0) @binding(1) var atlas_sampler: sampler;
@group(0) @binding(2) var<storage, read> glyph_ids: array<u32>;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) glyph: u32,
    @location(2) @interpolate(flat) color: vec4<f32>,
}

@vertex
fn vs(@location(0) position: vec2<f32>, @location(1) uv: vec2<f32>,
      @location(2) cell: u32, @location(3) color: vec4<f32>) -> VertexOut {
    return VertexOut(vec4(position, 0.0, 1.0), uv, glyph_ids[cell], color);
}

@fragment
fn fs(in: VertexOut) -> @location(0) vec4<f32> {
    let coverage = textureSample(atlas, atlas_sampler, in.uv, i32(in.glyph)).r;
    let alpha = coverage * in.color.a;
    return vec4(in.color.rgb * alpha, alpha);
}
