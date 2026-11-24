struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) straight_alpha: f32,
}

@vertex
fn vs(@location(0) position: vec2<f32>, @location(1) uv: vec2<f32>,
      @location(2) straight_alpha: f32) -> VertexOut {
    return VertexOut(vec4(position, 0.0, 1.0), uv, straight_alpha);
}

@group(0) @binding(0) var layer: texture_2d<f32>;
@group(0) @binding(1) var layer_sampler: sampler;

fn premultiplied_texel(position: vec2<i32>, limit: vec2<i32>) -> vec4<f32> {
    let c = textureLoad(layer, clamp(position, vec2(0), limit), 0);
    return vec4(c.rgb * c.a, c.a);
}

@fragment
fn fs(in: VertexOut) -> @location(0) vec4<f32> {
    if in.straight_alpha < 0.5 {
        return textureSampleLevel(layer, layer_sampler, in.uv, 0.0);
    }
    // Vello writes straight alpha. Premultiply *before* interpolation to avoid
    // dark fringes and leaking RGB from fully transparent texels during movement.
    let size = textureDimensions(layer);
    let xy = in.uv * vec2<f32>(size) - vec2(0.5);
    let base = vec2<i32>(floor(xy));
    let t = fract(xy);
    let limit = vec2<i32>(size) - vec2(1);
    let a = mix(premultiplied_texel(base, limit),
                premultiplied_texel(base + vec2(1, 0), limit), t.x);
    let b = mix(premultiplied_texel(base + vec2(0, 1), limit),
                premultiplied_texel(base + vec2(1, 1), limit), t.x);
    return mix(a, b, t.y);
}
