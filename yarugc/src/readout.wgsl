@group(0) @binding(0) var atlas_sampler: sampler;
@group(0) @binding(1) var<storage, read> glyph_ids: array<u32>;

struct Glyph {
    origin: vec2<f32>,
    extent: vec2<f32>,
    offset: vec2<f32>,
    page: u32,
    padding: u32,
}
@group(0) @binding(2) var<storage, read> glyph_table: array<Glyph>;
// ATLAS_BINDINGS

fn sample_atlas(page: u32, texel: vec2<f32>) -> f32 {
    switch page {
        // ATLAS_CASES
        default: { return 0.0; }
    }
}

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) atlas_origin: vec2<f32>,
    @location(2) @interpolate(flat) atlas_extent: vec2<f32>,
    @location(3) @interpolate(flat) atlas_page: u32,
    @location(4) @interpolate(flat) color: vec4<f32>,
}

@vertex
fn vs(@location(0) position: vec2<f32>, @location(1) uv: vec2<f32>,
      @location(2) cell: u32, @location(3) color: vec4<f32>) -> VertexOut {
    let id = glyph_ids[cell];
    let glyph = glyph_table[id];
    if id == 0u || any(glyph.extent <= vec2<f32>(0.0)) {
        // All vertices of an empty cell are outside the viewport.
        return VertexOut(vec4(2.0, 2.0, 0.0, 1.0), vec2(0.0), vec2(0.0), vec2(0.0), 0u, color);
    }
    return VertexOut(vec4(position, 0.0, 1.0), uv - glyph.offset, glyph.origin, glyph.extent, glyph.page, color);
}

@fragment
fn fs(in: VertexOut) -> @location(0) vec4<f32> {
    let texel = in.uv;
    if any(texel < vec2<f32>(0.0)) || any(texel >= in.atlas_extent) {
        discard;
    }
    let coverage = sample_atlas(in.atlas_page, in.atlas_origin + texel);
    let alpha = coverage * in.color.a;
    if alpha == 0.0 { discard; }
    return vec4(in.color.rgb * alpha, alpha);
}
